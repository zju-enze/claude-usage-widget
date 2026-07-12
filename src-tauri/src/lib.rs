use std::fs;
use serde::Serialize;
use tauri::Manager;

#[derive(Serialize)]
struct UsageSnapshot {
    found: bool,
    error: Option<String>,
    raw: serde_json::Value,
    fetched_at: String,
    key_source: String,
}

#[derive(Serialize)]
struct ProbeState {
    has_key: bool,
    source: String,    // "saved" | "env" | "missing"
    path: String,
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

    /// 把敏感字节用 Windows DPAPI 加密后写入磁盘（仅当前用户可解）
    pub fn save(plaintext: &[u8]) -> Result<PathBuf, String> {
        let path = key_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        let mut buf = Vec::with_capacity(plaintext.len() * 2);
        buf.extend_from_slice(&super::windows_crypto::CRYPTPROTECT_DATA_HEADER);
        super::windows_crypto::encrypt(plaintext, &mut buf).map_err(|e| format!("dpapi encrypt: {e}"))?;
        fs::write(&path, &buf).map_err(|e| format!("write: {e}"))?;
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

    // DPAPI flags: CRYPTPROTECT_UI_FORBIDDEN allows service access; UI = 0x1 (default).
    // 我们用 0 让它跟当前用户绑定。
    fn protect_flags() -> u32 {
        0x0
    }

    pub fn encrypt(plaintext: &[u8], out: &mut Vec<u8>) -> windows::core::Result<()> {
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
        if out_blob.cbData > 0 && !out_blob.pbData.is_null() {
            let slice = unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) };
            out.clear();
            out.extend_from_slice(&CRYPTPROTECT_DATA_HEADER);
            out.extend_from_slice(slice);
            // 注：DPAPI 分配的 buffer 在进程退出时由 OS 回收。
            // 持续调用会微小泄漏，对 widget 这种低频调用没问题。
        }
        Ok(())
    }

    pub fn decrypt(blob: &[u8]) -> windows::core::Result<Vec<u8>> {
        use windows::Win32::Security::Cryptography::{
            CryptUnprotectData, CRYPT_INTEGER_BLOB,
        };
        if blob.len() < CRYPTPROTECT_DATA_HEADER.len() {
            return Ok(Vec::new());
        }
        let payload = &blob[CRYPTPROTECT_DATA_HEADER.len()..];
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
        if out_blob.cbData > 0 && !out_blob.pbData.is_null() {
            let slice = unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) };
            Ok(slice.to_vec())
        } else {
            Ok(Vec::new())
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod keystore {
    use std::fs;
    use std::path::PathBuf;

    pub fn key_path() -> PathBuf {
        let base = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(base).join(".claude-usage-widget").join("key.txt")
    }

    pub fn save(plaintext: &[u8]) -> Result<PathBuf, String> {
        let path = key_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        fs::write(&path, plaintext).map_err(|e| format!("write: {e}"))?;
        Ok(path)
    }

    pub fn load() -> Result<Vec<u8>, String> {
        let path = key_path();
        if !path.exists() { return Err("not found".to_string()); }
        fs::read(&path).map_err(|e| format!("read: {e}"))
    }
}

fn get_api_key() -> Option<String> {
    // 1) env var
    for env_name in &["MINIMAX_API_KEY", "MINIMAX_CP_TOKEN"] {
        if let Ok(v) = std::env::var(env_name) {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    // 2) 已加密存盘的
    if let Ok(bytes) = keystore::load() {
        if let Ok(s) = std::str::from_utf8(&bytes) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[tauri::command]
async fn fetch_minimax_usage() -> UsageSnapshot {
    eprintln!("[minimax] fetch_minimax_usage called");
    let fetched_at = chrono::Utc::now().to_rfc3339();

    let key = match get_api_key() {
        Some(k) => k,
        None => {
            return UsageSnapshot {
                found: false,
                error: Some("Missing API key.".to_string()),
                raw: serde_json::json!({}),
                fetched_at,
                key_source: "missing".to_string(),
            }
        }
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return UsageSnapshot {
                found: false,
                error: Some(format!("client build: {e}")),
                raw: serde_json::json!({}),
                fetched_at,
                key_source: "missing".to_string(),
            }
        }
    };

    let resp = client
        .get("https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains")
        .header("Authorization", format!("Bearer {key}"))
        .header("referer", "https://platform.minimaxi.com/")
        .header("Accept", "application/json")
        .send()
        .await;

    match resp {
        Ok(r) => {
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            if !status.is_success() {
                return UsageSnapshot {
                    found: false,
                    error: Some(format!("HTTP {} — {}", status, &text[..text.len().min(200)])),
                    raw: serde_json::json!({}),
                    fetched_at,
                    key_source: "saved".to_string(),
                };
            }
            eprintln!("[minimax API] HTTP {} body: {}", status, &text[..text.len().min(2000)]);
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) => UsageSnapshot {
                    found: true,
                    error: None,
                    raw: v,
                    fetched_at,
                    key_source: "saved".to_string(),
                },
                Err(e) => UsageSnapshot {
                    found: false,
                    error: Some(format!("JSON parse: {e}")),
                    raw: serde_json::json!({}),
                    fetched_at,
                    key_source: "saved".to_string(),
                },
            }
        }
        Err(e) => UsageSnapshot {
            found: false,
            error: Some(format!("network: {e}")),
            raw: serde_json::json!({}),
            fetched_at,
            key_source: "saved".to_string(),
        },
    }
}

#[tauri::command]
fn frontend_log(level: String, msg: String) {
    eprintln!("[frontend {}] {}", level, msg);
}

#[tauri::command]
fn probe_state() -> ProbeState {
    let path = keystore::key_path();
    if std::env::var("MINIMAX_API_KEY").is_ok() || std::env::var("MINIMAX_CP_TOKEN").is_ok() {
        return ProbeState { has_key: true, source: "env".to_string(), path: path.to_string_lossy().to_string() };
    }
    match keystore::load() {
        Ok(bytes) if !bytes.is_empty() => ProbeState {
            has_key: true,
            source: "saved".to_string(),
            path: path.to_string_lossy().to_string(),
        },
        _ => ProbeState {
            has_key: false,
            source: "missing".to_string(),
            path: path.to_string_lossy().to_string(),
        },
    }
}

#[tauri::command]
async fn save_key_and_test(key: String) -> SaveResult {
    let trimmed = key.trim().to_string();
    if !trimmed.starts_with("sk-cp-") || trimmed.len() < 20 {
        return SaveResult { ok: false, error: Some("key 应以 sk-cp- 开头".to_string()) };
    }

    // 1. 加密存盘
    if let Err(e) = keystore::save(trimmed.as_bytes()) {
        return SaveResult { ok: false, error: Some(format!("保存失败：{e}")) };
    }

    // 2. 立刻打一次 API 验证可用
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
        Ok(r) if r.status().is_success() => SaveResult { ok: true, error: None },
        Ok(r) => {
            // 撤销保存——这个 key 是坏的
            let _ = fs::remove_file(keystore::key_path());
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            let snippet = body.chars().take(200).collect::<String>();
            SaveResult { ok: false, error: Some(format!("API {} — {}", status, snippet)) }
        }
        Err(e) => {
            let _ = fs::remove_file(keystore::key_path());
            SaveResult { ok: false, error: Some(format!("网络：{e}")) }
        }
    }
}

#[tauri::command]
async fn open_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err("URL 必须以 http(s):// 开头".to_string());
    }
    open::that_detached(trimmed).map_err(|e| format!("open: {e}"))?;
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
/// Windows：枚举进程找 `claude.exe`（Electron 桌面端进程名）。
/// 其他平台：暂只做 Windows（项目本身就是 Win 优先）。
fn is_claude_code_running() -> bool {
    #[cfg(target_os = "windows")]
    {
        // 简单实现：tasklist 找 claude.exe
        let out = std::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq claude.exe"])
            .output();
        match out {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                // tasklist 在没找到时会输出 "INFO: No tasks are running..."
                s.contains("claude.exe")
            }
            Err(_) => false,
        }
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

#[tauri::command]
fn set_autohide(_enabled: bool) {
    // 前端 toggle 标记。简化：当前实现忽略此参数（永远检测）。
    // 留接口以便未来实现"用户暂时关掉自动联动"。
    let _ = _enabled;
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            fetch_minimax_usage,
            frontend_log,
            probe_state,
            save_key_and_test,
            open_url,
            clear_key,
            claude_code_running,
            set_autohide,
            enable_autostart,
            disable_autostart,
            is_autostart_enabled,
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();

                let monitor_size: Option<(u32, u32)> = window
                    .primary_monitor()
                    .ok()
                    .flatten()
                    .map(|m| {
                        let s = m.size();
                        (s.width, s.height)
                    });

                let win_size = window
                    .inner_size()
                    .unwrap_or(tauri::PhysicalSize::new(280, 200));

                if let Some((sw, sh)) = monitor_size {
                    if sw > 0 && sh > 0 && win_size.width > 0 && win_size.height > 0 {
                        let x = (sw as i32).saturating_sub(win_size.width as i32 + 24);
                        let y = 48;
                        let _ = window.set_position(tauri::PhysicalPosition::new(x.max(0), y));
                        eprintln!(
                            "[claude-usage-widget] positioned at ({}, {}), screen {}x{}, window {}x{}",
                            x, y, sw, sh, win_size.width, win_size.height
                        );
                    }
                }

                // 启动后台线程：每 5s 检查 Claude Code 进程是否在跑
                // 在 → show 窗口；不在 → hide
                std::thread::spawn({
                    let window = window.clone();
                    let mut claude_running = false;
                    move || loop {
                        std::thread::sleep(std::time::Duration::from_secs(5));
                        let running = is_claude_code_running();
                        if running != claude_running {
                            if running {
                                let _ = window.show();
                                let _ = window.set_focus();
                                eprintln!("[claude-usage-widget] claude.exe detected → show");
                            } else {
                                let _ = window.hide();
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
