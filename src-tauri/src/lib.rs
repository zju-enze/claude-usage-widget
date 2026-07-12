use serde::Serialize;
use tauri::Manager;

#[derive(Serialize)]
struct UsageSnapshot {
    found: bool,
    error: Option<String>,
    raw: serde_json::Value,
    fetched_at: String,
    key_source: String, // "env" / "missing"
}

fn get_api_key() -> Option<(String, String)> {
    for env_name in &["MINIMAX_API_KEY", "MINIMAX_CP_TOKEN"] {
        if let Ok(v) = std::env::var(env_name) {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return Some((trimmed.to_string(), env_name.to_string()));
            }
        }
    }
    None
}

#[tauri::command]
async fn fetch_minimax_usage() -> UsageSnapshot {
    let fetched_at = chrono::Utc::now().to_rfc3339();

    let (key, key_source) = match get_api_key() {
        Some(t) => t,
        None => {
            return UsageSnapshot {
                found: false,
                error: Some("Missing API key. Set MINIMAX_API_KEY (or MINIMAX_CP_TOKEN) env var.".to_string()),
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
                key_source,
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
                    key_source,
                };
            }
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) => UsageSnapshot {
                    found: true,
                    error: None,
                    raw: v,
                    fetched_at,
                    key_source,
                },
                Err(e) => UsageSnapshot {
                    found: false,
                    error: Some(format!("JSON parse: {e}")),
                    raw: serde_json::json!({}),
                    fetched_at,
                    key_source,
                },
            }
        }
        Err(e) => UsageSnapshot {
            found: false,
            error: Some(format!("network: {e}")),
            raw: serde_json::json!({}),
            fetched_at,
            key_source,
        },
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![fetch_minimax_usage])
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
                            x,
                            y,
                            sw,
                            sh,
                            win_size.width,
                            win_size.height
                        );
                    }
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
