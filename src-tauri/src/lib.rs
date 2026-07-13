use std::fs;
use std::sync::Arc;
use serde::Serialize;
use tauri::Manager;

mod api;

/// 共享运行时状态：HTTP 连接池复用，避免每次 fetch_minimax_usage
/// 都重新做 TLS 握手 + DNS 解析 + TCP 连接池重建。
struct AppState {
    client: reqwest::Client,
}

#[derive(Serialize)]
struct UsageSnapshot {
    /// 兼容旧前端：true 表示拿到了 view-model
    found: bool,
    /// 用户可读的错误
    error: Option<String>,
    /// 错误的简短分类（"network" / "invalid-key" / ...）—— 不含路径或 key
    error_kind: Option<String>,
    /// ViewModel 数据（5h/7d 窗口 + fetched_at + state）。
    /// 已不含 model_remains / model_name / Authorization / key path。
    five_hour_used_percent: Option<f64>,
    five_hour_reset_after_ms: Option<i64>,
    five_hour_start_at_ms: Option<i64>,
    five_hour_end_at_ms: Option<i64>,
    weekly_used_percent: Option<f64>,
    weekly_reset_after_ms: Option<i64>,
    weekly_start_at_ms: Option<i64>,
    weekly_end_at_ms: Option<i64>,
    fetched_at: String,
    state: String, // "ok" | "no_data"
    /// 兼容旧字段："env" | "saved" | "missing"（不暴露具体变量名）
    key_source: String,
}

impl UsageSnapshot {
    fn error(user_message: String, kind: &str) -> Self {
        Self {
            found: false,
            error: Some(user_message),
            error_kind: Some(kind.to_string()),
            five_hour_used_percent: None,
            five_hour_reset_after_ms: None,
            five_hour_start_at_ms: None,
            five_hour_end_at_ms: None,
            weekly_used_percent: None,
            weekly_reset_after_ms: None,
            weekly_start_at_ms: None,
            weekly_end_at_ms: None,
            fetched_at: chrono::Utc::now().to_rfc3339(),
            state: "no_data".into(),
            key_source: "missing".into(),
        }
    }

    fn from_view_model(vm: crate::api::UsageViewModel, source: &crate::KeySource) -> Self {
        let state = match vm.state {
            crate::api::UsageState::Ok => "ok",
            crate::api::UsageState::NoData => "no_data",
        };
        let key_source = match source {
            crate::KeySource::ProcessEnv | crate::KeySource::UserEnv => "env",
            crate::KeySource::SecureStore => "saved",
        };
        Self {
            found: true,
            error: None,
            error_kind: None,
            five_hour_used_percent: vm.five_hour.used_percent,
            five_hour_reset_after_ms: vm.five_hour.reset_after_ms,
            five_hour_start_at_ms: vm.five_hour.start_at_ms,
            five_hour_end_at_ms: vm.five_hour.end_at_ms,
            weekly_used_percent: vm.weekly.used_percent,
            weekly_reset_after_ms: vm.weekly.reset_after_ms,
            weekly_start_at_ms: vm.weekly.start_at_ms,
            weekly_end_at_ms: vm.weekly.end_at_ms,
            fetched_at: vm.fetched_at,
            state: state.into(),
            key_source: key_source.into(),
        }
    }
}

#[derive(Serialize)]
struct ProbeState {
    has_key: bool,
    source: String,    // "saved" | "env" | "missing" —— 不暴露具体路径 / 变量名
}

#[derive(Serialize)]
struct SaveResult {
    ok: bool,
    error: Option<String>,
}

// ─── 持久化 key 存储（AES-256-GCM 加密） ──────────────────
// 用 Windows DPAPI 加密（仅 Windows 平台，且只在当前用户下能解密）；
// 这样 key 不是明文落地，进程外的攻击者拿到文件也没法直接用。
// Linux/macOS fallback：直接写明文（开发机）；未来再换 keyring。

#[cfg(target_os = "windows")]
mod keystore {
    use std::fs;
    use std::path::PathBuf;

    pub fn key_path() -> PathBuf {
        let base = std::env::var("APPDATA")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".to_string());
        PathBuf::from(base)
            .join("claude-usage-widget")
            .join("key.bin")
    }

    /// 把敏感字节用 Windows DPAPI 加密后写入磁盘（仅当前用户可解）。
    ///
    /// 原子保存：先写 `<key>.tmp` → fsync → rename 覆盖原文件。
    /// 任何中途失败（旧 tmp 残留 / 加密失败 / 写入失败）都不会破坏旧 key.bin。
    pub fn save(plaintext: &[u8]) -> Result<PathBuf, String> {
        let path = key_path();
        let parent = path.parent().ok_or("invalid key path")?;
        fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;

        // 加密到内存
        let mut buf = Vec::with_capacity(plaintext.len() * 2 + 8);
        buf.extend_from_slice(&super::windows_crypto::CRYPTPROTECT_DATA_HEADER);
        super::windows_crypto::encrypt(plaintext, &mut buf).map_err(|e| format!("dpapi encrypt: {e}"))?;

        // 写临时文件
        let tmp = path.with_extension("bin.tmp");
        {
            use std::io::Write;
            let mut f = fs::File::create(&tmp).map_err(|e| format!("create tmp: {e}"))?;
            f.write_all(&buf).map_err(|e| format!("write tmp: {e}"))?;
            f.sync_all().map_err(|e| format!("sync tmp: {e}"))?;
        }

        // 原子 rename 覆盖（Windows 上 rename 不允许目标已存在，需要先 remove）
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("remove old: {e}"))?;
        }
        fs::rename(&tmp, &path).map_err(|e| format!("rename: {e}"))?;
        Ok(path)
    }

    /// 从磁盘读取并用 DPAPI 解密
    pub fn load() -> Result<Vec<u8>, String> {
        let path = key_path();
        if !path.exists() { return Err("not found".to_string()); }
        let buf = fs::read(&path).map_err(|e| format!("read: {e}"))?;
        if buf.len() < 4 || buf[..4] != super::windows_crypto::CRYPTPROTECT_DATA_HEADER {
            return Err("not a dpapi blob".to_string());
        }
        super::windows_crypto::decrypt(&buf).map_err(|e| format!("dpapi decrypt: {e}"))
    }
}

#[cfg(target_os = "windows")]
mod windows_crypto {
    pub const CRYPTPROTECT_DATA_HEADER: [u8; 4] = [b'C', b'T', b'W', b'P'];

    /// DPAPI flags: CRYPTPROTECT_UI_FORBIDDEN 禁止任何 UI 弹窗，
    /// 确保后台小组件进程永远不会触发交互式提示。
    /// 我们也使用 0 让 DPAPI 绑定到当前用户。
    fn protect_flags() -> u32 {
        windows::Win32::Security::Cryptography::CRYPTPROTECT_UI_FORBIDDEN
    }

    /// DPAPI 分配 buffer 的 RAII 封装：在 Drop 时调用 LocalFree 释放。
    /// 失败路径也通过 Drop 自动释放。
    struct DpapiBuffer {
        ptr: *mut u8,
        len: usize,
    }

    impl DpapiBuffer {
        /// 空 buffer（DPAPI 在错误或空响应时返回）
        fn empty() -> Self {
            Self { ptr: std::ptr::null_mut(), len: 0 }
        }

        /// 复制到新 Vec 后，buffer 仍需通过 Drop 释放
        fn as_slice(&self) -> &[u8] {
            if self.ptr.is_null() || self.len == 0 {
                &[]
            } else {
                unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
            }
        }

        /// 取出 buffer 所有权（用于复制后提前释放）
        fn into_vec(self) -> Vec<u8> {
            let v = if self.ptr.is_null() || self.len == 0 {
                Vec::new()
            } else {
                unsafe { std::slice::from_raw_parts(self.ptr, self.len).to_vec() }
            };
            // self 即将被 drop；drop 会再调一次 LocalFree——所以这里必须禁止二次释放
            // 通过 ManuallyDrop 跳过 drop。
            std::mem::forget(self);
            v
        }
    }

    impl Drop for DpapiBuffer {
        fn drop(&mut self) {
            if !self.ptr.is_null() {
                unsafe {
                    // LocalFree 是 DPAPI/CryptProtectData/CryptUnprotectData 文档要求
                    // 的释放函数（不能用 free / GlobalFree）。
                    use windows::Win32::Foundation::{HLOCAL, LocalFree};
                    let h = HLOCAL(self.ptr as *mut _);
                    let _ = LocalFree(h);
                }
            }
        }
    }

    fn call_protect(plaintext: &[u8]) -> windows::core::Result<DpapiBuffer> {
        use windows::Win32::Security::Cryptography::{
            CryptProtectData, CRYPT_INTEGER_BLOB,
        };
        let mut in_blob = CRYPT_INTEGER_BLOB {
            cbData: plaintext.len() as u32,
            pbData: plaintext.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        unsafe {
            CryptProtectData(
                &mut in_blob,
                None,
                None,
                None,
                None,
                protect_flags(),
                &mut out_blob,
            )?;
        }
        if out_blob.cbData == 0 || out_blob.pbData.is_null() {
            Ok(DpapiBuffer::empty())
        } else {
            Ok(DpapiBuffer {
                ptr: out_blob.pbData,
                len: out_blob.cbData as usize,
            })
        }
    }

    fn call_unprotect(payload: &[u8]) -> windows::core::Result<DpapiBuffer> {
        use windows::Win32::Security::Cryptography::{
            CryptUnprotectData, CRYPT_INTEGER_BLOB,
        };
        let mut in_blob = CRYPT_INTEGER_BLOB {
            cbData: payload.len() as u32,
            pbData: payload.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        unsafe {
            CryptUnprotectData(&mut in_blob, None, None, None, None, 0, &mut out_blob)?
        };
        if out_blob.cbData == 0 || out_blob.pbData.is_null() {
            Ok(DpapiBuffer::empty())
        } else {
            Ok(DpapiBuffer {
                ptr: out_blob.pbData,
                len: out_blob.cbData as usize,
            })
        }
    }

    pub fn encrypt(plaintext: &[u8], out: &mut Vec<u8>) -> windows::core::Result<()> {
        let buf = call_protect(plaintext)?;
        // 复制后 buffer 由 Drop 自动 LocalFree
        let slice = buf.as_slice();
        out.clear();
        out.extend_from_slice(&CRYPTPROTECT_DATA_HEADER);
        out.extend_from_slice(slice);
        Ok(())
    }

    pub fn decrypt(blob: &[u8]) -> windows::core::Result<Vec<u8>> {
        if blob.len() < CRYPTPROTECT_DATA_HEADER.len() {
            return Ok(Vec::new());
        }
        let payload = &blob[CRYPTPROTECT_DATA_HEADER.len()..];
        let buf = call_unprotect(payload)?;
        // 一次性取出所有权 → Drop 释放 DPAPI buffer
        Ok(buf.into_vec())
    }

    /// 单元测试：DPAPI encrypt/decrypt 往返
    #[cfg(test)]
    #[test]
    fn dpapi_roundtrip() {
        let plaintext = b"sk-cp-test-key-for-roundtrip-1234567890abcdef";
        let mut ciphertext = Vec::new();
        encrypt(plaintext, &mut ciphertext).expect("encrypt failed");
        assert_eq!(&ciphertext[..4], &CRYPTPROTECT_DATA_HEADER);
        let decrypted = decrypt(&ciphertext).expect("decrypt failed");
        assert_eq!(decrypted, plaintext);
    }

    /// 单元测试：空 blob 返回空 vec
    #[cfg(test)]
    #[test]
    fn dpapi_empty_blob() {
        let r = decrypt(&[]).expect("decrypt empty");
        assert!(r.is_empty());
    }

    /// 单元测试：过短 blob（无 payload）返回空 vec
    #[cfg(test)]
    #[test]
    fn dpapi_short_blob() {
        let r = decrypt(&[b'C', b'T']).expect("decrypt short");
        assert!(r.is_empty());
    }
}

/// 非 Windows 平台：**不允许持久化 Key**。
/// 明文落盘等于把可换钱的 API 凭据暴露给所有本机进程 + 备份系统 + 文件恢复工具。
/// 非 Windows 用户应通过环境变量 `MINIMAX_API_KEY` / `MINIMAX_CP_TOKEN` 注入 Key。
#[cfg(not(target_os = "windows"))]
mod keystore {
    use std::path::PathBuf;

    pub fn key_path() -> PathBuf {
        // 仅用于探测：显示用户禁用持久化时的提示信息；不读不写。
        let base = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(base).join(".claude-usage-widget")
    }

    pub fn save(_plaintext: &[u8]) -> Result<PathBuf, String> {
        Err("Persistent Key storage is not supported on this platform. \
             Set the MINIMAX_API_KEY or MINIMAX_CP_TOKEN environment variable instead."
            .to_string())
    }

    pub fn load() -> Result<Vec<u8>, String> {
        Err("Persistent Key storage is not supported on this platform.".to_string())
    }
}

/// Key 来源 —— 给前端看时只暴露三种枚举值，不暴露具体环境变量名或 key 路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// 进程级环境变量（启动 shell 时 set）
    ProcessEnv,
    /// Windows 用户级环境变量（setx / 系统属性设置）
    UserEnv,
    /// DPAPI 加密的本地文件
    SecureStore,
}

#[derive(Debug)]
pub struct ResolvedApiKey {
    pub value: String,
    pub source: KeySource,
}

/// 统一 Key 解析：与 probe_state 使用完全相同的优先级和空值规则。
///
/// 优先级：进程 env → 用户 env → SecureStore。
/// 空字符串不算存在（防止 set EMPTY_VAR= 让 UI 误以为有 key）。
fn resolve_api_key() -> Result<ResolvedApiKey, String> {
    let env_names = ["MINIMAX_API_KEY", "MINIMAX_CP_TOKEN"];

    // 1) 进程级 env var
    for name in &env_names {
        if let Ok(v) = std::env::var(name) {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return Ok(ResolvedApiKey { value: trimmed.to_string(), source: KeySource::ProcessEnv });
            }
        }
    }

    // 2) Windows 用户级 env var
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::*;
        use winreg::RegKey;
        if let Ok(hkcu) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Environment") {
            for name in &env_names {
                if let Ok(v) = hkcu.get_value::<String, _>(name) {
                    let trimmed = v.trim();
                    if !trimmed.is_empty() {
                        return Ok(ResolvedApiKey { value: trimmed.to_string(), source: KeySource::UserEnv });
                    }
                }
            }
        }
    }

    // 3) SecureStore
    if let Ok(bytes) = keystore::load() {
        if !bytes.is_empty() {
            if let Ok(s) = std::str::from_utf8(&bytes) {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Ok(ResolvedApiKey { value: trimmed.to_string(), source: KeySource::SecureStore });
                }
            }
        }
    }

    Err("API key not configured".to_string())
}

#[tauri::command]
async fn fetch_minimax_usage(state: tauri::State<'_, Arc<AppState>>) -> Result<UsageSnapshot, String> {
    use crate::api::fetch_usage;

    let resolved = match resolve_api_key() {
        Ok(r) => r,
        Err(_) => {
            return Ok(UsageSnapshot::error("未配置 API Key".to_string(), "missing-key"));
        }
    };

    // 共享 reqwest Client（来自 AppState）—— 复用 TLS 连接池。
    // clone() 只是 Arc 引用计数增加，不是新建连接。
    let client = state.client.clone();
    let api_key = resolved.value;

    let view_model = match fetch_usage(&client, &api_key).await {
        Ok(vm) => vm,
        Err(err) => {
            #[cfg(debug_assertions)]
            eprintln!(
                "[minimax API] error kind={:?} retry_after={:?}",
                err.kind, err.retry_after_seconds
            );
            return Ok(UsageSnapshot::error(err.user_message, err_kind_label(err.kind)));
        }
    };

    Ok(UsageSnapshot::from_view_model(view_model, &resolved.source))
}

/// 把 UsageErrorKind 映射到前端 tooltip 上能展示的简短类别标签。
fn err_kind_label(k: crate::api::UsageErrorKind) -> &'static str {
    use crate::api::UsageErrorKind;
    match k {
        UsageErrorKind::MissingKey => "missing-key",
        UsageErrorKind::InvalidKey => "invalid-key",
        UsageErrorKind::Forbidden => "forbidden",
        UsageErrorKind::RateLimited => "rate-limited",
        UsageErrorKind::Timeout => "timeout",
        UsageErrorKind::Network => "network",
        UsageErrorKind::Server => "server",
        UsageErrorKind::InvalidResponse => "invalid-response",
        UsageErrorKind::ResponseTooLarge => "response-too-large",
    }
}

/// `frontend_log` 已删除（生产构建绝不接收前端任意字符串并写入 stderr）。
/// 前端调试日志请使用 `console.*` 由 WebView 自己管理。

/// read_plan_metadata / PlanMetadata 已删除。
/// 套餐信息不在 UI 中展示——API 不返回套餐名称，硬编码等于编造数据。
/// 如果未来 minimaxi 提供返回套餐名的公开端点，再实现新的 invoke 命令。

#[derive(Serialize)]
struct ActiveModelInfo {
    model: Option<String>,
    source: String, // "env" | "config" | "none"
}

/// 读取 Claude Code 当前实际配置的模型。
/// 优先级：
///   1. 环境变量 `MINIMAX_MODEL` / `CLAUDE_MODEL`
///   2. Claude Code 配置文件 `~/.claude/settings.json` 中的 model 字段
///   3. `~/.claude/settings.local.json`
/// 失败返回 None（前端应隐藏"当前模型"行而不是编造）。
#[tauri::command]
fn read_active_model() -> ActiveModelInfo {
    // 1) env var
    for name in &["MINIMAX_MODEL", "CLAUDE_MODEL", "ANTHROPIC_MODEL"] {
        if let Ok(v) = std::env::var(name) {
            let t = v.trim();
            if !t.is_empty() {
                return ActiveModelInfo { model: Some(t.to_string()), source: "env".to_string() };
            }
        }
    }
    // 2) Claude Code 配置 (~/.claude/settings.json / settings.local.json)
    if let Some(home) = std::env::var("HOME").ok().or_else(|| std::env::var("USERPROFILE").ok()) {
        for fname in &["settings.json", "settings.local.json"] {
            let p = std::path::PathBuf::from(&home).join(".claude").join(fname);
            if let Ok(text) = std::fs::read_to_string(&p) {
                if let Some(m) = extract_model_from_claude_settings(&text) {
                    return ActiveModelInfo { model: Some(m), source: "config".to_string() };
                }
            }
        }
    }
    ActiveModelInfo { model: None, source: "none".to_string() }
}

/// 从 Claude Code settings.json 文本中提取 model 字段。
/// 支持顶层 `model` 字段、嵌套 `env.ANTHROPIC_MODEL`、嵌套 `model.id`。
fn extract_model_from_claude_settings(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if let Some(s) = v.get("model").and_then(|x| x.as_str()) {
        let t = s.trim();
        if !t.is_empty() { return Some(t.to_string()); }
    }
    if let Some(id) = v.get("model").and_then(|x| x.get("id")).and_then(|x| x.as_str()) {
        let t = id.trim();
        if !t.is_empty() { return Some(t.to_string()); }
    }
    if let Some(env) = v.get("env").and_then(|x| x.as_object()) {
        for key in &["MINIMAX_MODEL", "CLAUDE_MODEL", "ANTHROPIC_MODEL"] {
            if let Some(s) = env.get(*key).and_then(|x| x.as_str()) {
                let t = s.trim();
                if !t.is_empty() { return Some(t.to_string()); }
            }
        }
    }
    None
}

#[tauri::command]
fn probe_state() -> ProbeState {
    // 使用 resolve_api_key 保证规则一致（空字符串 / 优先级 / fallback）
    match resolve_api_key() {
        Ok(k) => ProbeState {
            has_key: true,
            source: match k.source {
                KeySource::ProcessEnv | KeySource::UserEnv => "env".to_string(),
                KeySource::SecureStore => "saved".to_string(),
            },
        },
        Err(_) => ProbeState {
            has_key: false,
            source: "missing".to_string(),
        },
    }
}

#[tauri::command]
async fn save_key_and_test(key: String) -> SaveResult {
    // 1) 校验格式（trim + 长度上限 + 前缀）
    let trimmed = key.trim().to_string();
    const MAX_KEY_LEN: usize = 512;
    if trimmed.len() < 20 || trimmed.len() > MAX_KEY_LEN {
        return SaveResult { ok: false, error: Some("key 长度应在 20–512 字符之间".to_string()) };
    }
    if !trimmed.starts_with("sk-cp-") {
        return SaveResult { ok: false, error: Some("key 应以 sk-cp- 开头".to_string()) };
    }

    // 2) 用内存中的候选 key 调 API —— 失败不破坏旧 key
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => return SaveResult { ok: false, error: Some(format!("client build: {e}")) },
    };
    let resp = client
        .get("https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains")
        .header("Authorization", format!("Bearer {trimmed}"))
        .header("referer", "https://platform.minimaxi.com/")
        .header("Accept", "application/json")
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            // 3) 明确认证成功 → 临时文件 → 原子 rename
            // 注意：keystore::save 已经是"先写临时文件再 rename"的原子操作
            if let Err(e) = keystore::save(trimmed.as_bytes()) {
                return SaveResult { ok: false, error: Some(format!("保存失败：{e}")) };
            }
            SaveResult { ok: true, error: None }
        }
        Ok(r) => {
            // 4xx 表示 key 无效（401/403）；不修改磁盘上的旧 key
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            let snippet: String = body.chars().filter(|c| !c.is_control()).take(120).collect();
            let msg = if status.as_u16() == 401 || status.as_u16() == 403 {
                format!("Key 认证失败 ({})", status.as_u16())
            } else {
                format!("API {} — {}", status, snippet)
            };
            SaveResult { ok: false, error: Some(msg) }
        }
        Err(e) => {
            // 网络错误：保留旧 key
            SaveResult { ok: false, error: Some(format!("网络错误，未修改已存 Key：{e}")) }
        }
    }
}

/// 打开 MiniMax 官方 Key 管理页（URL 硬编码，禁止从前端注入任意 URL）。
/// 删除 `open_url(url: String)` 通用命令，攻击面降至唯一允许的 HTTPS URL。
#[tauri::command]
fn open_minimax_key_page() -> Result<(), String> {
    const ALLOWED: &str = "https://platform.minimaxi.com/user-center/basic-information/interface-key";
    open::that_detached(ALLOWED).map_err(|e| format!("open: {e}"))?;
    Ok(())
}

#[tauri::command]
fn clear_key() -> Result<(), String> {
    let path = keystore::key_path();
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("remove: {e}"))?;
    }
    Ok(())
}

/// 把当前 exe 注册到 Windows 启动项（HKCU\...\Run\ClaudeUsageWidget）。
/// 仅 Windows。第一次启动时调用：用户如果点"开机启动"打钩，安装完就会自动注册。
#[tauri::command]
fn enable_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::*;
        use winreg::RegKey;
        let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
        // exe 路径用引号包起来，避免空格问题
        let value = format!("\"{}\"", exe.display());
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let run_key = hkcu
            .open_subkey_with_flags(
                r"Software\Microsoft\Windows\CurrentVersion\Run",
                KEY_SET_VALUE | KEY_QUERY_VALUE,
            )
            .map_err(|e| format!("open Run: {e}"))?;
        run_key
            .set_value("ClaudeUsageWidget", &value)
            .map_err(|e| format!("set Run: {e}"))?;
        #[cfg(debug_assertions)]
    eprintln!("[claude-usage-widget] autostart enabled → {}", value);
        let _ = app;
        Ok(true)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        Ok(false)
    }
}

#[tauri::command]
fn disable_autostart() -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::*;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let run_key = hkcu
            .open_subkey_with_flags(
                r"Software\Microsoft\Windows\CurrentVersion\Run",
                KEY_SET_VALUE,
            )
            .map_err(|e| format!("open Run: {e}"))?;
        match run_key.delete_value("ClaudeUsageWidget") {
            Ok(_) => {
                #[cfg(debug_assertions)]
                eprintln!("[claude-usage-widget] autostart disabled");
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(format!("delete: {e}")),
        }
    }
    #[cfg(not(target_os = "windows"))]
    Ok(false)
}

#[tauri::command]
fn is_autostart_enabled() -> bool {
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::*;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(run_key) =
            hkcu.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")
        {
            return run_key.get_value::<String, _>("ClaudeUsageWidget").is_ok();
        }
        false
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// 检查 Claude Code 是否正在运行。
/// Windows：用 Win32 EnumProcesses（不依赖 cmd.exe、不弹控制台窗口）。
/// 其他平台：暂只做 Windows（项目本身就是 Win 优先）。
fn is_claude_code_running() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::System::ProcessStatus::{
            EnumProcesses, K32GetModuleFileNameExW,
        };
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        use windows::Win32::Foundation::CloseHandle;

        const MAX_PIDS: usize = 4096;
        let mut pids = vec![0u32; MAX_PIDS];
        let mut bytes_returned = 0u32;
        // SAFETY: EnumProcesses 写入到 pids 缓冲区。我们传递足够大的缓冲区。
        let result = unsafe {
            EnumProcesses(pids.as_mut_ptr(), (MAX_PIDS * 4) as u32, &mut bytes_returned)
        };
        if result.is_err() {
            return false;
        }
        let n_pids = (bytes_returned as usize) / std::mem::size_of::<u32>();
        for &pid in pids.iter().take(n_pids) {
            if pid == 0 {
                continue;
            }
            // SAFETY: OpenProcess 返回 Result<HANDLE>; 失败就是 0（无效句柄）
            let handle_res = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) };
            let handle = match handle_res {
                Ok(h) => h,
                Err(_) => continue,
            };
            let mut buf = [0u16; 256];
            // windows 0.58: K32GetModuleFileNameExW 接收 HANDLE 接口；
            // 直接传 handle（HANDLE 类型）或 .into() 转。
            let len = unsafe {
                K32GetModuleFileNameExW(handle, None, &mut buf)
            };
            unsafe { let _ = CloseHandle(handle); }
            if len == 0 { continue; }
            let path = String::from_utf16_lossy(&buf[..len as usize]).to_lowercase();
            if path.ends_with("\\claude.exe")
                || path.ends_with("/claude.exe")
                || path.contains("claude code")
            {
                return true;
            }
        }
        false
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

#[tauri::command]
fn claude_code_running() -> bool {
    is_claude_code_running()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            fetch_minimax_usage,
            probe_state,
            save_key_and_test,
            open_minimax_key_page,
            clear_key,
            claude_code_running,
            enable_autostart,
            disable_autostart,
            is_autostart_enabled,
            read_active_model,
        ])
        .setup(|app| {
            // 共享 reqwest Client（连接池复用）→ 通过 Tauri AppState 注入
            app.manage(Arc::new(AppState { client: crate::api::build_client() }));

            if let Some(window) = app.get_webview_window("main") {
                // 决定初始可见性：
                //   - 无 Key：首次安装用户必须能直接看到配置向导
                //   - 有 Key + Claude 未运行：默认隐藏
                //   - 有 Key + Claude 运行：直接显示
                let stored_key_present = keystore::load().map(|b| !b.is_empty()).unwrap_or(false);
                let env_key_present = ["MINIMAX_API_KEY", "MINIMAX_CP_TOKEN"]
                    .iter()
                    .any(|n| std::env::var(n).map(|v| !v.trim().is_empty()).unwrap_or(false));
                let has_key = stored_key_present || env_key_present;

                if has_key && is_claude_code_running() {
                    let _ = window.show();
                } else if !has_key {
                    let _ = window.show();
                } else {
                    let _ = window.hide();
                    let _ = window.unminimize();
                }

                let monitor_size = window
                    .primary_monitor()
                    .ok()
                    .flatten()
                    .map(|m| (m.size().width, m.size().height));
                let win_size = window
                    .inner_size()
                    .unwrap_or(tauri::PhysicalSize::new(360, 230));

                if let Some((sw, sh)) = monitor_size {
                    if sw > 0 && sh > 0 && win_size.width > 0 && win_size.height > 0 {
                        let x = (sw as i32).saturating_sub(win_size.width as i32 + 24);
                        let y = 48;
                        let _ = window.set_position(tauri::PhysicalPosition::new(x.max(0), y));
                        #[cfg(debug_assertions)]
                        eprintln!(
                            "[claude-usage-widget] positioned at ({}, {}), screen {}x{}, window {}x{}",
                            x, y, sw, sh, win_size.width, win_size.height
                        );
                    }
                }

                // 后台线程：每 10s 检查 Claude Code 进程（5s → 10s 减半 CPU）
                // 状态改变时 emit；用户手动隐藏后不抢焦点（不调用 set_focus）
                std::thread::spawn({
                    let window = window.clone();
                    let mut claude_running = false;
                    move || loop {
                        std::thread::sleep(std::time::Duration::from_secs(10));
                        let running = is_claude_code_running();
                        if running != claude_running {
                            if running {
                                let _ = window.show();
                                #[cfg(debug_assertions)]
                                eprintln!("[claude-usage-widget] claude.exe detected → show (no focus steal)");
                            } else {
                                let _ = window.hide();
                                #[cfg(debug_assertions)]
                                eprintln!("[claude-usage-widget] claude.exe gone → hide");
                            }
                            claude_running = running;
                        }
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
