mod providers;

use providers::{
    build_http_client, is_valid_api_key, provider_from_base_url, provider_from_model,
    request_usage, Provider, UsageMetric,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tauri::Manager;
use zeroize::{Zeroize, Zeroizing};

#[derive(Serialize)]
struct UsageSnapshot {
    found: bool,
    error: Option<String>,
    metrics: Vec<UsageMetric>,
    fetched_at: String,
    key_source: String,
    provider: String,
}

#[derive(Serialize)]
struct ProbeState {
    has_key: bool,
    has_saved_key: bool,
    source: String,
    provider: String,
    provider_name: String,
}

#[derive(Serialize)]
struct SaveResult {
    ok: bool,
    error: Option<String>,
}

#[derive(Serialize)]
struct ActiveModelInfo {
    model: Option<String>,
    source: String,
}

struct AppState {
    client: reqwest::Client,
    selected_provider: Arc<Mutex<Provider>>,
    onboarding: Arc<AtomicBool>,
    onboarding_dismissed: Arc<AtomicBool>,
}

impl AppState {
    fn new(client: reqwest::Client, provider: Provider, onboarding: bool) -> Self {
        Self {
            client,
            selected_provider: Arc::new(Mutex::new(provider)),
            onboarding: Arc::new(AtomicBool::new(onboarding)),
            onboarding_dismissed: Arc::new(AtomicBool::new(false)),
        }
    }
}

struct ApiKey {
    value: Zeroizing<String>,
    source: &'static str,
}

fn normalize_api_key(provider: Provider, value: &str) -> Option<Zeroizing<String>> {
    let trimmed = value.trim();
    is_valid_api_key(provider, trimmed).then(|| Zeroizing::new(trimmed.to_string()))
}

fn user_home() -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())
        .map(std::path::PathBuf::from)
}

fn claude_settings_env_value(name: &str) -> Option<String> {
    let home = user_home()?;
    for filename in ["settings.local.json", "settings.json"] {
        let Ok(contents) = fs::read_to_string(home.join(".claude").join(filename)) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) else {
            continue;
        };
        if let Some(found) = value
            .get("env")
            .and_then(|env| env.get(name))
            .and_then(serde_json::Value::as_str)
        {
            if !found.trim().is_empty() {
                return Some(found.to_string());
            }
        }
    }
    None
}

fn configuration_value(name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(name) {
        if !value.trim().is_empty() {
            return Some(value);
        }
    }

    #[cfg(target_os = "windows")]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;

        if let Ok(environment) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Environment") {
            if let Ok(value) = environment.get_value::<String, _>(name) {
                if !value.trim().is_empty() {
                    return Some(value);
                }
            }
        }
    }

    claude_settings_env_value(name)
}

fn configuration_secret(name: &str) -> Option<Zeroizing<String>> {
    configuration_value(name).map(Zeroizing::new)
}

fn configured_base_url() -> Option<String> {
    configuration_value("ANTHROPIC_BASE_URL")
}

const MODEL_CONFIGURATION_NAMES: [&str; 7] = [
    "MINIMAX_MODEL",
    "DEEPSEEK_MODEL",
    "ZAI_MODEL",
    "ZHIPUAI_MODEL",
    "ANTHROPIC_MODEL",
    "CLAUDE_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
];

fn active_model_info() -> ActiveModelInfo {
    for name in MODEL_CONFIGURATION_NAMES {
        if let Some(value) = configuration_value(name) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return ActiveModelInfo {
                    model: Some(trimmed.to_string()),
                    source: "env".to_string(),
                };
            }
        }
    }

    if let Some(home) = user_home() {
        for filename in ["settings.local.json", "settings.json"] {
            let path = home.join(".claude").join(filename);
            if let Ok(contents) = fs::read_to_string(path) {
                if let Some(model) = extract_model_from_claude_settings(&contents) {
                    return ActiveModelInfo {
                        model: Some(model),
                        source: "config".to_string(),
                    };
                }
            }
        }
    }

    ActiveModelInfo {
        model: None,
        source: "none".to_string(),
    }
}

fn active_provider() -> Provider {
    if let Some(provider) = configured_base_url()
        .as_deref()
        .and_then(provider_from_base_url)
    {
        return provider;
    }

    if let Some(provider) = active_model_info()
        .model
        .as_deref()
        .and_then(provider_from_model)
    {
        return provider;
    }

    // Keep MiniMax first for backward compatibility if several unrelated provider keys coexist.
    for provider in [Provider::Minimax, Provider::Deepseek, Provider::Zhipu] {
        if provider.direct_env_names().iter().any(|name| {
            configuration_secret(name)
                .as_deref()
                .is_some_and(|value| is_valid_api_key(provider, value))
        }) {
            return provider;
        }
    }

    Provider::Minimax
}

fn resolve_provider(provider: Option<&str>) -> Result<Provider, String> {
    match provider {
        Some(id) => Provider::from_id(id).ok_or_else(|| "invalid_provider".to_string()),
        None => Ok(active_provider()),
    }
}

fn select_provider(state: &AppState, provider: Provider) {
    if let Ok(mut selected) = state.selected_provider.lock() {
        *selected = provider;
    }
}

fn selected_provider(state: &AppState) -> Provider {
    state
        .selected_provider
        .lock()
        .map(|selected| *selected)
        .unwrap_or_else(|_| active_provider())
}

fn environment_api_key(provider: Provider) -> Option<ApiKey> {
    for name in provider.direct_env_names() {
        if let Some(value) =
            configuration_secret(name).and_then(|value| normalize_api_key(provider, value.as_str()))
        {
            return Some(ApiKey {
                value,
                source: "env",
            });
        }
    }

    let matches_anthropic_base = configured_base_url()
        .as_deref()
        .and_then(provider_from_base_url)
        == Some(provider);
    if matches_anthropic_base {
        for name in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"] {
            if let Some(value) = configuration_secret(name)
                .and_then(|value| normalize_api_key(provider, value.as_str()))
            {
                return Some(ApiKey {
                    value,
                    source: "env",
                });
            }
        }
    }
    None
}

fn get_api_key(provider: Provider) -> Option<ApiKey> {
    if let Some(key) = environment_api_key(provider) {
        return Some(key);
    }

    if let Ok(bytes) = keystore::load(provider.key_filename()) {
        if let Ok(value) = std::str::from_utf8(bytes.as_slice()) {
            if let Some(value) = normalize_api_key(provider, value) {
                return Some(ApiKey {
                    value,
                    source: "saved",
                });
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
mod keystore {
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use tempfile::NamedTempFile;
    use zeroize::Zeroizing;

    pub fn key_path(filename: &str) -> PathBuf {
        let base = std::env::var("APPDATA")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".to_string());
        PathBuf::from(base)
            .join("claude-usage-widget")
            .join(filename)
    }

    pub(crate) fn write_atomically(path: &Path, contents: &[u8]) -> Result<(), String> {
        let parent = path
            .parent()
            .ok_or_else(|| "invalid key path".to_string())?;
        fs::create_dir_all(parent).map_err(|error| format!("create key directory: {error}"))?;

        let mut temporary =
            NamedTempFile::new_in(parent).map_err(|error| format!("create temp key: {error}"))?;
        temporary
            .write_all(contents)
            .map_err(|error| format!("write temp key: {error}"))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| format!("sync temp key: {error}"))?;
        temporary
            .persist(path)
            .map_err(|error| format!("replace key: {}", error.error))?;
        Ok(())
    }

    pub fn save(filename: &str, plaintext: &[u8]) -> Result<(), String> {
        let encrypted = super::windows_crypto::encrypt(plaintext)?;
        write_atomically(&key_path(filename), &encrypted)
    }

    pub fn load(filename: &str) -> Result<Zeroizing<Vec<u8>>, String> {
        let bytes = fs::read(key_path(filename)).map_err(|error| format!("read key: {error}"))?;
        if !bytes.starts_with(&super::windows_crypto::CRYPTPROTECT_DATA_HEADER) {
            return Err("invalid DPAPI key blob".to_string());
        }
        super::windows_crypto::decrypt(&bytes)
    }
}

#[cfg(not(target_os = "windows"))]
mod keystore {
    use std::path::PathBuf;
    use zeroize::Zeroizing;

    pub fn key_path(filename: &str) -> PathBuf {
        let base = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(base)
            .join(".claude-usage-widget")
            .join(filename)
    }

    pub fn load(_filename: &str) -> Result<Zeroizing<Vec<u8>>, String> {
        Err("secure persistence is only available on Windows".to_string())
    }
}

#[cfg(target_os = "windows")]
mod windows_crypto {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };
    use zeroize::{Zeroize, Zeroizing};

    pub const CRYPTPROTECT_DATA_HEADER: [u8; 4] = *b"CTWP";
    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

    pub fn encrypt(plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: plaintext.len() as u32,
            pbData: plaintext.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        unsafe {
            CryptProtectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
        .map_err(|error| format!("DPAPI encrypt: {error}"))?;

        if output.pbData.is_null() {
            return Err("DPAPI returned an empty encrypted value".to_string());
        }
        if output.cbData == 0 {
            unsafe {
                let _ = LocalFree(HLOCAL(output.pbData.cast()));
            }
            return Err("DPAPI returned an empty encrypted value".to_string());
        }

        let encrypted =
            unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
        unsafe {
            let _ = LocalFree(HLOCAL(output.pbData.cast()));
        }

        let mut result = Vec::with_capacity(CRYPTPROTECT_DATA_HEADER.len() + encrypted.len());
        result.extend_from_slice(&CRYPTPROTECT_DATA_HEADER);
        result.extend_from_slice(&encrypted);
        Ok(result)
    }

    pub fn decrypt(blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
        let payload = blob
            .strip_prefix(&CRYPTPROTECT_DATA_HEADER)
            .ok_or_else(|| "invalid DPAPI header".to_string())?;
        let input = CRYPT_INTEGER_BLOB {
            cbData: payload.len() as u32,
            pbData: payload.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        unsafe {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
        .map_err(|error| format!("DPAPI decrypt: {error}"))?;

        if output.pbData.is_null() {
            return Err("DPAPI returned an empty decrypted value".to_string());
        }
        if output.cbData == 0 {
            unsafe {
                let _ = LocalFree(HLOCAL(output.pbData.cast()));
            }
            return Err("DPAPI returned an empty decrypted value".to_string());
        }

        let plaintext = unsafe {
            let slice = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
            let plaintext = Zeroizing::new(slice.to_vec());
            slice.zeroize();
            let _ = LocalFree(HLOCAL(output.pbData.cast()));
            plaintext
        };
        Ok(plaintext)
    }
}

#[tauri::command]
async fn fetch_usage(
    state: tauri::State<'_, AppState>,
    provider: Option<String>,
) -> Result<UsageSnapshot, String> {
    let provider = resolve_provider(provider.as_deref())?;
    select_provider(&state, provider);
    let fetched_at = chrono::Utc::now().to_rfc3339();
    let Some(key) = get_api_key(provider) else {
        state.onboarding.store(true, Ordering::Release);
        state.onboarding_dismissed.store(false, Ordering::Release);
        return Ok(UsageSnapshot {
            found: false,
            error: Some("missing_key".to_string()),
            metrics: Vec::new(),
            fetched_at,
            key_source: "missing".to_string(),
            provider: provider.id().to_string(),
        });
    };

    let key_source = key.source.to_string();
    let base_url = configured_base_url();
    Ok(
        match request_usage(
            &state.client,
            provider,
            key.value.as_str(),
            base_url.as_deref(),
        )
        .await
        {
            Ok(data) => UsageSnapshot {
                found: true,
                error: None,
                metrics: data.metrics.into_iter().collect(),
                fetched_at,
                key_source,
                provider: provider.id().to_string(),
            },
            Err(error) => UsageSnapshot {
                found: false,
                error: Some(error.code().to_string()),
                metrics: Vec::new(),
                fetched_at,
                key_source,
                provider: provider.id().to_string(),
            },
        },
    )
}

#[tauri::command]
fn probe_state(
    state: tauri::State<'_, AppState>,
    provider: Option<String>,
) -> Result<ProbeState, String> {
    let provider = resolve_provider(provider.as_deref())?;
    select_provider(&state, provider);
    let has_saved_key = saved_key_exists(provider);
    let probe = match get_api_key(provider) {
        Some(key) => ProbeState {
            has_key: true,
            has_saved_key,
            source: key.source.to_string(),
            provider: provider.id().to_string(),
            provider_name: provider.display_name().to_string(),
        },
        None => ProbeState {
            has_key: false,
            has_saved_key,
            source: "missing".to_string(),
            provider: provider.id().to_string(),
            provider_name: provider.display_name().to_string(),
        },
    };
    state.onboarding.store(!probe.has_key, Ordering::Release);
    state.onboarding_dismissed.store(false, Ordering::Release);
    Ok(probe)
}

#[cfg(target_os = "windows")]
fn saved_key_exists(provider: Provider) -> bool {
    keystore::key_path(provider.key_filename()).is_file()
}

#[cfg(not(target_os = "windows"))]
fn saved_key_exists(_provider: Provider) -> bool {
    false
}

#[tauri::command]
async fn save_key_and_test(
    state: tauri::State<'_, AppState>,
    provider: String,
    key: String,
) -> Result<SaveResult, String> {
    let provider = resolve_provider(Some(&provider))?;
    select_provider(&state, provider);
    state
        .onboarding
        .store(get_api_key(provider).is_none(), Ordering::Release);
    let mut key = Zeroizing::new(key);
    let candidate = Zeroizing::new(key.trim().to_string());
    key.zeroize();
    if !is_valid_api_key(provider, candidate.as_str()) {
        return Ok(SaveResult {
            ok: false,
            error: Some("invalid_key_format".to_string()),
        });
    }

    let base_url = configured_base_url();
    if let Err(error) = request_usage(
        &state.client,
        provider,
        candidate.as_str(),
        base_url.as_deref(),
    )
    .await
    {
        return Ok(SaveResult {
            ok: false,
            error: Some(error.code().to_string()),
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        return Ok(SaveResult {
            ok: false,
            error: Some("storage_unsupported".to_string()),
        });
    }

    #[cfg(target_os = "windows")]
    {
        if keystore::save(provider.key_filename(), candidate.as_bytes()).is_err() {
            return Ok(SaveResult {
                ok: false,
                error: Some("storage_error".to_string()),
            });
        }

        state.onboarding.store(false, Ordering::Release);
        state.onboarding_dismissed.store(false, Ordering::Release);
        Ok(SaveResult {
            ok: true,
            error: None,
        })
    }
}

#[tauri::command]
fn clear_key(state: tauri::State<'_, AppState>, provider: String) -> Result<(), String> {
    let provider = resolve_provider(Some(&provider))?;
    select_provider(&state, provider);
    let path = keystore::key_path(provider.key_filename());
    if path.exists() {
        fs::remove_file(path).map_err(|_| "remove_failed".to_string())?;
    }
    state.onboarding.store(true, Ordering::Release);
    state.onboarding_dismissed.store(false, Ordering::Release);
    Ok(())
}

#[tauri::command]
fn open_help_page(state: tauri::State<'_, AppState>, provider: String) -> Result<(), String> {
    let provider = resolve_provider(Some(&provider))?;
    select_provider(&state, provider);
    open::that_detached(provider.help_url()).map_err(|_| "open_failed".to_string())
}

#[tauri::command]
fn hide_main_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let provider = selected_provider(&state);
    if get_api_key(provider).is_none() {
        state.onboarding_dismissed.store(true, Ordering::Release);
    }
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "window_unavailable".to_string())?;
    window.hide().map_err(|_| "hide_failed".to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum WindowMode {
    Expanded,
    Collapsed,
    Setup,
}

#[tauri::command]
fn set_window_mode(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    mode: WindowMode,
) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "window_unavailable".to_string())?;
    let (width, height) = match mode {
        WindowMode::Expanded => {
            state.onboarding.store(false, Ordering::Release);
            (360.0, 244.0)
        }
        WindowMode::Collapsed => {
            state.onboarding.store(false, Ordering::Release);
            (360.0, 52.0)
        }
        WindowMode::Setup => {
            state.onboarding.store(true, Ordering::Release);
            state.onboarding_dismissed.store(false, Ordering::Release);
            (360.0, 372.0)
        }
    };
    window
        .set_size(tauri::LogicalSize::new(width, height))
        .map_err(|_| "resize_failed".to_string())?;

    Ok(())
}

#[tauri::command]
fn read_active_model() -> ActiveModelInfo {
    active_model_info()
}

fn extract_model_from_claude_settings(contents: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(contents).ok()?;

    if let Some(model) = value.get("model").and_then(|item| item.as_str()) {
        let trimmed = model.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Some(model) = value
        .get("model")
        .and_then(|item| item.get("id"))
        .and_then(|item| item.as_str())
    {
        let trimmed = model.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Some(environment) = value.get("env").and_then(|item| item.as_object()) {
        for name in MODEL_CONFIGURATION_NAMES {
            if let Some(model) = environment.get(name).and_then(|item| item.as_str()) {
                let trimmed = model.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn is_claude_code_running() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::ProcessStatus::{EnumProcesses, K32GetModuleFileNameExW};
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

        const MAX_PIDS: usize = 4096;
        let mut process_ids = vec![0_u32; MAX_PIDS];
        let mut bytes_returned = 0_u32;
        if unsafe {
            EnumProcesses(
                process_ids.as_mut_ptr(),
                std::mem::size_of_val(process_ids.as_slice()) as u32,
                &mut bytes_returned,
            )
        }
        .is_err()
        {
            return false;
        }

        let process_count = bytes_returned as usize / std::mem::size_of::<u32>();
        for process_id in process_ids.into_iter().take(process_count) {
            if process_id == 0 {
                continue;
            }
            let Ok(handle) =
                (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) })
            else {
                continue;
            };
            let mut path_buffer = [0_u16; 32_768];
            let path_length = unsafe { K32GetModuleFileNameExW(handle, None, &mut path_buffer) };
            unsafe {
                let _ = CloseHandle(handle);
            }
            if path_length == 0 {
                continue;
            }

            let path = String::from_utf16_lossy(&path_buffer[..path_length as usize]);
            if std::path::Path::new(&path)
                .file_name()
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("claude.exe"))
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
fn window_effect_mode() -> &'static str {
    "transparent"
}

fn should_auto_show_window(
    has_key: bool,
    claude_running: bool,
    onboarding: bool,
    onboarding_dismissed: bool,
) -> bool {
    if onboarding || !has_key {
        !onboarding_dismissed
    } else {
        claude_running
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let client = build_http_client().expect("failed to build the HTTPS client");
    let initial_provider = active_provider();
    let initial_onboarding = get_api_key(initial_provider).is_none();
    tauri::Builder::default()
        .manage(AppState::new(client, initial_provider, initial_onboarding))
        .invoke_handler(tauri::generate_handler![
            fetch_usage,
            probe_state,
            save_key_and_test,
            clear_key,
            open_help_page,
            hide_main_window,
            set_window_mode,
            window_effect_mode,
            read_active_model,
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let state = app.state::<AppState>();
                let provider = selected_provider(&state);
                let has_key = get_api_key(provider).is_some();
                state.onboarding.store(!has_key, Ordering::Release);

                let _ = window.hide();
                let _ = window.unminimize();

                let (initial_width, initial_height) = if has_key {
                    (360.0, 244.0)
                } else {
                    (360.0, 372.0)
                };
                let _ = window.set_size(tauri::LogicalSize::new(initial_width, initial_height));

                let monitor_size = window.primary_monitor().ok().flatten().map(|monitor| {
                    let size = monitor.size();
                    (size.width, size.height)
                });
                let window_size = window
                    .inner_size()
                    .unwrap_or(tauri::PhysicalSize::new(360, 244));

                if let Some((screen_width, screen_height)) = monitor_size {
                    if screen_width > 0
                        && screen_height > 0
                        && window_size.width > 0
                        && window_size.height > 0
                    {
                        let x = (screen_width as i32).saturating_sub(window_size.width as i32 + 24);
                        let _ = window.set_position(tauri::PhysicalPosition::new(x.max(0), 48));
                    }
                }

                let claude_running = is_claude_code_running();
                if should_auto_show_window(has_key, claude_running, !has_key, false) {
                    let _ = window.show();
                }

                std::thread::spawn({
                    let window = window.clone();
                    let selected_provider = Arc::clone(&state.selected_provider);
                    let onboarding = Arc::clone(&state.onboarding);
                    let onboarding_dismissed = Arc::clone(&state.onboarding_dismissed);
                    let mut was_running = claude_running;
                    let mut was_onboarding = !has_key;
                    move || loop {
                        std::thread::sleep(Duration::from_secs(10));
                        let provider = selected_provider
                            .lock()
                            .map(|selected| *selected)
                            .unwrap_or_else(|_| active_provider());
                        let has_key = get_api_key(provider).is_some();
                        let requires_onboarding = onboarding.load(Ordering::Acquire) || !has_key;
                        let onboarding_was_dismissed = onboarding_dismissed.load(Ordering::Acquire);
                        let is_running = is_claude_code_running();

                        if requires_onboarding {
                            if should_auto_show_window(
                                has_key,
                                is_running,
                                true,
                                onboarding_was_dismissed,
                            ) {
                                let _ = window.show();
                            }
                            was_onboarding = true;
                            was_running = is_running;
                            continue;
                        }

                        if was_onboarding || is_running != was_running {
                            if is_running {
                                let _ = window.show();
                            } else {
                                let _ = window.hide();
                            }
                            was_onboarding = false;
                            was_running = is_running;
                        }
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_model_from_supported_claude_settings_shapes() {
        assert_eq!(
            extract_model_from_claude_settings(r#"{"model":"MiniMax-M2.5"}"#).as_deref(),
            Some("MiniMax-M2.5")
        );
        assert_eq!(
            extract_model_from_claude_settings(r#"{"model":{"id":"claude-sonnet"}}"#).as_deref(),
            Some("claude-sonnet")
        );
        assert_eq!(
            extract_model_from_claude_settings(r#"{"env":{"ANTHROPIC_MODEL":"claude-opus"}}"#)
                .as_deref(),
            Some("claude-opus")
        );
        assert_eq!(
            extract_model_from_claude_settings(r#"{"env":{"CLAUDE_MODEL":"deepseek-chat"}}"#)
                .as_deref(),
            Some("deepseek-chat")
        );
    }

    #[test]
    fn explicit_provider_ids_are_strict() {
        assert_eq!(
            resolve_provider(Some("deepseek")).expect("known provider"),
            Provider::Deepseek
        );
        assert_eq!(
            resolve_provider(Some("unsupported")),
            Err("invalid_provider".to_string())
        );
    }

    #[test]
    fn onboarding_hide_is_respected_until_a_key_exists() {
        assert!(should_auto_show_window(false, false, true, false));
        assert!(!should_auto_show_window(false, true, true, true));
        assert!(!should_auto_show_window(true, false, false, true));
        assert!(should_auto_show_window(true, true, false, true));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn dpapi_round_trip_releases_native_buffers() {
        let plaintext = b"sk-cp-test-only-1234567890";
        let encrypted = windows_crypto::encrypt(plaintext).expect("DPAPI encryption should work");
        let decrypted = windows_crypto::decrypt(&encrypted).expect("DPAPI decryption should work");
        assert_eq!(decrypted.as_slice(), plaintext);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn atomic_key_write_replaces_existing_contents() {
        let directory = tempfile::tempdir().expect("temp dir should be created");
        let path = directory.path().join("key.bin");
        keystore::write_atomically(&path, b"first").expect("first write should work");
        keystore::write_atomically(&path, b"second").expect("replacement should work");
        assert_eq!(fs::read(path).expect("key should be readable"), b"second");
    }
}
