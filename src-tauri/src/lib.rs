use serde::{Deserialize, Serialize};
use std::fs;
use std::time::Duration;
use tauri::Manager;
use zeroize::{Zeroize, Zeroizing};

const API_URL: &str = "https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains";
const HELP_URL: &str = "https://platform.minimaxi.com/user-center/basic-information/interface-key";
const MAX_RESPONSE_BYTES: usize = 512 * 1024;

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
    has_saved_key: bool,
    source: String,
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

struct ApiClient(reqwest::Client);

struct ApiKey {
    value: Zeroizing<String>,
    source: &'static str,
}

#[derive(Debug)]
enum ServiceError {
    Network,
    Unauthorized,
    RateLimited,
    Unavailable,
    RequestFailed,
    ResponseTooLarge,
    InvalidResponse,
}

impl ServiceError {
    fn code(&self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Unauthorized => "unauthorized",
            Self::RateLimited => "rate_limited",
            Self::Unavailable => "service_unavailable",
            Self::RequestFailed => "request_failed",
            Self::ResponseTooLarge => "response_too_large",
            Self::InvalidResponse => "invalid_response",
        }
    }
}

fn service_error_for_status(status: reqwest::StatusCode) -> ServiceError {
    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            ServiceError::Unauthorized
        }
        reqwest::StatusCode::TOO_MANY_REQUESTS => ServiceError::RateLimited,
        status if status.is_server_error() => ServiceError::Unavailable,
        _ => ServiceError::RequestFailed,
    }
}

fn build_http_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .user_agent(concat!("claude-usage-widget/", env!("CARGO_PKG_VERSION")))
        .build()
}

async fn request_usage(
    client: &reqwest::Client,
    key: &str,
) -> Result<serde_json::Value, ServiceError> {
    let mut response = client
        .get(API_URL)
        .bearer_auth(key)
        .header(reqwest::header::REFERER, "https://platform.minimaxi.com/")
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| ServiceError::Network)?;

    let status = response.status();
    if !status.is_success() {
        eprintln!("[minimax] request failed with HTTP {status}");
        return Err(service_error_for_status(status));
    }

    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(ServiceError::ResponseTooLarge);
    }

    let initial_capacity = response
        .content_length()
        .unwrap_or(0)
        .min(MAX_RESPONSE_BYTES as u64) as usize;
    let mut body = Vec::with_capacity(initial_capacity);
    while let Some(chunk) = response.chunk().await.map_err(|_| ServiceError::Network)? {
        let next_len = body
            .len()
            .checked_add(chunk.len())
            .ok_or(ServiceError::ResponseTooLarge)?;
        if next_len > MAX_RESPONSE_BYTES {
            return Err(ServiceError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }

    let value = serde_json::from_slice::<serde_json::Value>(&body)
        .map_err(|_| ServiceError::InvalidResponse)?;
    if value
        .get("model_remains")
        .and_then(|item| item.as_array())
        .is_none()
    {
        return Err(ServiceError::InvalidResponse);
    }
    Ok(value)
}

fn is_valid_api_key(value: &str) -> bool {
    let length = value.len();
    (20..=512).contains(&length)
        && value.starts_with("sk-cp-")
        && value.is_ascii()
        && !value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
}

fn normalize_api_key(value: String) -> Option<Zeroizing<String>> {
    let value = Zeroizing::new(value);
    let trimmed = value.trim();
    is_valid_api_key(trimmed).then(|| Zeroizing::new(trimmed.to_string()))
}

fn environment_api_key() -> Option<ApiKey> {
    for env_name in ["MINIMAX_API_KEY", "MINIMAX_CP_TOKEN"] {
        if let Ok(value) = std::env::var(env_name) {
            if let Some(value) = normalize_api_key(value) {
                return Some(ApiKey {
                    value,
                    source: "env",
                });
            }
        }

        #[cfg(target_os = "windows")]
        {
            use winreg::enums::HKEY_CURRENT_USER;
            use winreg::RegKey;

            if let Ok(environment) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Environment") {
                if let Ok(value) = environment.get_value::<String, _>(env_name) {
                    if let Some(value) = normalize_api_key(value) {
                        return Some(ApiKey {
                            value,
                            source: "env",
                        });
                    }
                }
            }
        }
    }
    None
}

fn get_api_key() -> Option<ApiKey> {
    if let Some(key) = environment_api_key() {
        return Some(key);
    }

    if let Ok(bytes) = keystore::load() {
        if let Ok(value) = std::str::from_utf8(bytes.as_slice()) {
            if let Some(value) = normalize_api_key(value.to_string()) {
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

    pub fn key_path() -> PathBuf {
        let base = std::env::var("APPDATA")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".to_string());
        PathBuf::from(base)
            .join("claude-usage-widget")
            .join("key.bin")
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

    pub fn save(plaintext: &[u8]) -> Result<(), String> {
        let encrypted = super::windows_crypto::encrypt(plaintext)?;
        write_atomically(&key_path(), &encrypted)
    }

    pub fn load() -> Result<Zeroizing<Vec<u8>>, String> {
        let bytes = fs::read(key_path()).map_err(|error| format!("read key: {error}"))?;
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

    pub fn key_path() -> PathBuf {
        let base = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(base)
            .join(".claude-usage-widget")
            .join("key.txt")
    }

    pub fn load() -> Result<Zeroizing<Vec<u8>>, String> {
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

    pub const CRYPTPROTECT_DATA_HEADER: [u8; 4] = [b'C', b'T', b'W', b'P'];
    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

    pub fn encrypt(plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let mut input = CRYPT_INTEGER_BLOB {
            cbData: plaintext.len() as u32,
            pbData: plaintext.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        unsafe {
            CryptProtectData(
                &mut input,
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
        let mut input = CRYPT_INTEGER_BLOB {
            cbData: payload.len() as u32,
            pbData: payload.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        unsafe {
            CryptUnprotectData(
                &mut input,
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
async fn fetch_minimax_usage(client: tauri::State<'_, ApiClient>) -> Result<UsageSnapshot, String> {
    let fetched_at = chrono::Utc::now().to_rfc3339();
    let Some(key) = get_api_key() else {
        return Ok(UsageSnapshot {
            found: false,
            error: Some("missing_key".to_string()),
            raw: serde_json::json!({}),
            fetched_at,
            key_source: "missing".to_string(),
        });
    };

    let key_source = key.source.to_string();
    Ok(match request_usage(&client.0, key.value.as_str()).await {
        Ok(raw) => UsageSnapshot {
            found: true,
            error: None,
            raw,
            fetched_at,
            key_source,
        },
        Err(error) => UsageSnapshot {
            found: false,
            error: Some(error.code().to_string()),
            raw: serde_json::json!({}),
            fetched_at,
            key_source,
        },
    })
}

#[tauri::command]
fn probe_state() -> ProbeState {
    let has_saved_key = saved_key_exists();
    match get_api_key() {
        Some(key) => ProbeState {
            has_key: true,
            has_saved_key,
            source: key.source.to_string(),
        },
        None => ProbeState {
            has_key: false,
            has_saved_key,
            source: "missing".to_string(),
        },
    }
}

#[cfg(target_os = "windows")]
fn saved_key_exists() -> bool {
    keystore::key_path().is_file()
}

#[cfg(not(target_os = "windows"))]
fn saved_key_exists() -> bool {
    false
}

#[tauri::command]
async fn save_key_and_test(
    client: tauri::State<'_, ApiClient>,
    key: String,
) -> Result<SaveResult, String> {
    let mut key = Zeroizing::new(key);
    let candidate = Zeroizing::new(key.trim().to_string());
    key.zeroize();
    if !is_valid_api_key(candidate.as_str()) {
        return Ok(SaveResult {
            ok: false,
            error: Some("invalid_key_format".to_string()),
        });
    }

    if let Err(error) = request_usage(&client.0, candidate.as_str()).await {
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
        if keystore::save(candidate.as_bytes()).is_err() {
            return Ok(SaveResult {
                ok: false,
                error: Some("storage_error".to_string()),
            });
        }

        Ok(SaveResult {
            ok: true,
            error: None,
        })
    }
}

#[tauri::command]
fn clear_key() -> Result<(), String> {
    let path = keystore::key_path();
    if path.exists() {
        fs::remove_file(path).map_err(|_| "remove_failed".to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn open_help_page() -> Result<(), String> {
    open::that_detached(HELP_URL).map_err(|_| "open_failed".to_string())
}

#[tauri::command]
fn hide_main_window(app: tauri::AppHandle) -> Result<(), String> {
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
fn set_window_mode(app: tauri::AppHandle, mode: WindowMode) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "window_unavailable".to_string())?;
    let (width, height) = match mode {
        WindowMode::Expanded => (360.0, 244.0),
        WindowMode::Collapsed => (360.0, 52.0),
        WindowMode::Setup => (360.0, 328.0),
    };
    window
        .set_size(tauri::LogicalSize::new(width, height))
        .map_err(|_| "resize_failed".to_string())
}

#[tauri::command]
fn read_active_model() -> ActiveModelInfo {
    for name in ["MINIMAX_MODEL", "CLAUDE_MODEL", "ANTHROPIC_MODEL"] {
        if let Ok(value) = std::env::var(name) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return ActiveModelInfo {
                    model: Some(trimmed.to_string()),
                    source: "env".to_string(),
                };
            }
        }
    }

    if let Some(home) = std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())
    {
        for filename in ["settings.json", "settings.local.json"] {
            let path = std::path::PathBuf::from(&home)
                .join(".claude")
                .join(filename);
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
        for name in ["MINIMAX_MODEL", "CLAUDE_MODEL", "ANTHROPIC_MODEL"] {
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

#[cfg(target_os = "windows")]
fn system_transparency_enabled() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|personalize| personalize.get_value::<u32, _>("EnableTransparency"))
        .is_ok_and(|value| value != 0)
}

#[tauri::command]
fn window_effect_mode() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        if system_transparency_enabled() {
            "acrylic"
        } else {
            "transparent"
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        "transparent"
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let client = build_http_client().expect("failed to build the HTTPS client");
    tauri::Builder::default()
        .manage(ApiClient(client))
        .invoke_handler(tauri::generate_handler![
            fetch_minimax_usage,
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
                let _ = window.hide();
                let _ = window.unminimize();

                #[cfg(target_os = "windows")]
                if system_transparency_enabled() {
                    use tauri::window::{Effect, EffectsBuilder};

                    let _ =
                        window.set_effects(EffectsBuilder::new().effect(Effect::Acrylic).build());
                }

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

                std::thread::spawn({
                    let window = window.clone();
                    let mut was_running = false;
                    move || loop {
                        std::thread::sleep(Duration::from_secs(5));
                        let is_running = is_claude_code_running();
                        if is_running != was_running {
                            if is_running {
                                let _ = window.show();
                            } else {
                                let _ = window.hide();
                            }
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
    fn validates_api_key_shape_and_bounds() {
        assert!(is_valid_api_key("sk-cp-12345678901234"));
        assert!(!is_valid_api_key("sk-ant-12345678901234"));
        assert!(!is_valid_api_key("sk-cp-short"));
        assert!(!is_valid_api_key("sk-cp-12345678901234\n"));
        assert!(!is_valid_api_key(&format!("sk-cp-{}", "x".repeat(600))));
    }

    #[test]
    fn maps_remote_statuses_to_stable_error_codes() {
        assert_eq!(
            service_error_for_status(reqwest::StatusCode::UNAUTHORIZED).code(),
            "unauthorized"
        );
        assert_eq!(
            service_error_for_status(reqwest::StatusCode::TOO_MANY_REQUESTS).code(),
            "rate_limited"
        );
        assert_eq!(
            service_error_for_status(reqwest::StatusCode::BAD_GATEWAY).code(),
            "service_unavailable"
        );
    }

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
