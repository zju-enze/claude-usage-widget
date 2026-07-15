mod providers;

use providers::{
    build_http_client, is_valid_api_key, provider_from_base_url, provider_from_model,
    request_usage, Provider,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::time::Duration;
use tauri::Manager;
use zeroize::{Zeroize, Zeroizing};

#[derive(Serialize)]
struct UsageSnapshot {
    found: bool,
    error: Option<String>,
    raw: serde_json::Value,
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

struct ApiClient(reqwest::Client);

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

fn active_provider() -> Provider {
    if let Some(provider) = configured_base_url()
        .as_deref()
        .and_then(provider_from_base_url)
    {
        return provider;
    }

    for name in [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "MINIMAX_MODEL",
        "DEEPSEEK_MODEL",
        "ZAI_MODEL",
        "ZHIPUAI_MODEL",
    ] {
        if let Some(provider) = configuration_value(name)
            .as_deref()
            .and_then(provider_from_model)
        {
            return provider;
        }
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
async fn fetch_usage(client: tauri::State<'_, ApiClient>) -> Result<UsageSnapshot, String> {
    let provider = active_provider();
    let fetched_at = chrono::Utc::now().to_rfc3339();
    let Some(key) = get_api_key(provider) else {
        return Ok(UsageSnapshot {
            found: false,
            error: Some("missing_key".to_string()),
            raw: serde_json::json!({}),
            fetched_at,
            key_source: "missing".to_string(),
            provider: provider.id().to_string(),
        });
    };

    let key_source = key.source.to_string();
    let base_url = configured_base_url();
    Ok(
        match request_usage(&client.0, provider, key.value.as_str(), base_url.as_deref()).await {
            Ok(raw) => UsageSnapshot {
                found: true,
                error: None,
                raw,
                fetched_at,
                key_source,
                provider: provider.id().to_string(),
            },
            Err(error) => UsageSnapshot {
                found: false,
                error: Some(error.code().to_string()),
                raw: serde_json::json!({}),
                fetched_at,
                key_source,
                provider: provider.id().to_string(),
            },
        },
    )
}

#[tauri::command]
fn probe_state() -> ProbeState {
    let provider = active_provider();
    let has_saved_key = saved_key_exists(provider);
    match get_api_key(provider) {
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
    }
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
    client: tauri::State<'_, ApiClient>,
    key: String,
) -> Result<SaveResult, String> {
    let provider = active_provider();
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
    if let Err(error) =
        request_usage(&client.0, provider, candidate.as_str(), base_url.as_deref()).await
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

        Ok(SaveResult {
            ok: true,
            error: None,
        })
    }
}

#[tauri::command]
fn clear_key() -> Result<(), String> {
    let path = keystore::key_path(active_provider().key_filename());
    if path.exists() {
        fs::remove_file(path).map_err(|_| "remove_failed".to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn open_help_page() -> Result<(), String> {
    open::that_detached(active_provider().help_url()).map_err(|_| "open_failed".to_string())
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
        .map_err(|_| "resize_failed".to_string())?;

    #[cfg(target_os = "windows")]
    apply_rounded_window_region(&window, width, height)?;

    Ok(())
}

#[tauri::command]
fn read_active_model() -> ActiveModelInfo {
    for name in [
        "MINIMAX_MODEL",
        "DEEPSEEK_MODEL",
        "ZAI_MODEL",
        "ZHIPUAI_MODEL",
        "ANTHROPIC_MODEL",
        "CLAUDE_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
    ] {
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
        for name in [
            "MINIMAX_MODEL",
            "DEEPSEEK_MODEL",
            "ZAI_MODEL",
            "ZHIPUAI_MODEL",
            "ANTHROPIC_MODEL",
            "CLAUDE_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
        ] {
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

#[cfg(target_os = "windows")]
const WINDOW_RADIUS_LOGICAL: f64 = 24.0;

#[cfg(target_os = "windows")]
fn apply_rounded_window_region(
    window: &tauri::WebviewWindow,
    logical_width: f64,
    logical_height: f64,
) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, HWND_TOP, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_CAPTION, WS_MAXIMIZEBOX,
        WS_MINIMIZEBOX, WS_SYSMENU, WS_THICKFRAME,
    };

    let scale = window
        .scale_factor()
        .map_err(|_| "window_scale_unavailable".to_string())?;
    let width = (logical_width * scale).round().max(1.0) as i32;
    let height = (logical_height * scale).round().max(1.0) as i32;
    let diameter = (WINDOW_RADIUS_LOGICAL * 2.0 * scale).round().max(2.0) as i32;
    let native_hwnd = window
        .hwnd()
        .map_err(|_| "window_handle_unavailable".to_string())?;
    let hwnd = HWND(native_hwnd.0);

    unsafe {
        // Tao keeps caption-related style bits on undecorated top-level windows
        // and normally suppresses their non-client area through message handling.
        // A custom region plus blur can make Windows briefly paint that caption
        // when the window activates, so remove the unused frame bits explicitly.
        let frame_bits =
            (WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_THICKFRAME).0 as isize;
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        if style & frame_bits != 0 {
            SetWindowLongPtrW(hwnd, GWL_STYLE, style & !frame_bits);
            SetWindowPos(
                hwnd,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
            )
            .map_err(|_| "window_frame_failed".to_string())?;
        }

        // Native Windows backdrops paint the complete HWND. A window region is
        // therefore required so the blur itself follows the same 24px contour
        // as the WebView instead of leaking into four rectangular corners.
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter);
        if region.0.is_null() {
            return Err("window_shape_failed".to_string());
        }
        if SetWindowRgn(hwnd, region, true) == 0 {
            let _ = DeleteObject(region);
            return Err("window_shape_failed".to_string());
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn apply_current_window_shape(window: &tauri::WebviewWindow) -> Result<(), String> {
    let scale = window
        .scale_factor()
        .map_err(|_| "window_scale_unavailable".to_string())?;
    let size = window
        .inner_size()
        .map_err(|_| "window_size_unavailable".to_string())?;
    apply_rounded_window_region(
        window,
        size.width as f64 / scale,
        size.height as f64 / scale,
    )
}

#[tauri::command]
fn window_effect_mode() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        if system_transparency_enabled() {
            "blur"
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
                let _ = window.hide();
                let _ = window.unminimize();

                #[cfg(target_os = "windows")]
                if system_transparency_enabled() {
                    use tauri::window::{Effect, EffectsBuilder};

                    let _ = window.set_effects(EffectsBuilder::new().effect(Effect::Blur).build());
                }

                #[cfg(target_os = "windows")]
                let _ = apply_rounded_window_region(&window, 360.0, 244.0);

                #[cfg(target_os = "windows")]
                {
                    let shape_window = window.clone();
                    window.on_window_event(move |event| match event {
                        tauri::WindowEvent::ScaleFactorChanged {
                            scale_factor,
                            new_inner_size,
                            ..
                        } => {
                            let logical_width = new_inner_size.width as f64 / scale_factor;
                            let logical_height = new_inner_size.height as f64 / scale_factor;
                            let _ = apply_rounded_window_region(
                                &shape_window,
                                logical_width,
                                logical_height,
                            );
                        }
                        tauri::WindowEvent::Focused(true) => {
                            let _ = apply_current_window_shape(&shape_window);
                        }
                        _ => {}
                    });
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
                                #[cfg(target_os = "windows")]
                                let _ = apply_current_window_shape(&window);
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
