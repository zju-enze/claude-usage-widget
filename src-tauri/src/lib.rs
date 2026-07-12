use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use tauri::Manager;

#[derive(Serialize)]
struct MonitorState {
    found: bool,
    path: String,
    raw: serde_json::Value,
    fetched_at: String,
}

fn default_state_path() -> PathBuf {
    // 优先 USERPROFILE / HOME，再 fallback 到当前目录
    if let Ok(home) = std::env::var("USERPROFILE") {
        return PathBuf::from(home)
            .join(".claude-monitor")
            .join("state")
            .join("latest.json");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".claude-monitor")
            .join("state")
            .join("latest.json");
    }
    PathBuf::from(".claude-monitor/state/latest.json")
}

#[tauri::command]
fn read_monitor_state() -> MonitorState {
    let path = default_state_path();
    let fetched_at = chrono::Utc::now().to_rfc3339();

    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => MonitorState {
                found: true,
                path: path.to_string_lossy().to_string(),
                raw: v,
                fetched_at,
            },
            Err(e) => MonitorState {
                found: false,
                path: format!("{} (parse error: {})", path.display(), e),
                raw: serde_json::json!({}),
                fetched_at,
            },
        },
        Err(_) => MonitorState {
            found: false,
            path: path.to_string_lossy().to_string(),
            raw: serde_json::json!({}),
            fetched_at,
        },
    }
}

#[tauri::command]
fn state_file_path() -> String {
    default_state_path().to_string_lossy().to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![read_monitor_state, state_file_path])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                // 先 show，避免 hidden 状态导致 outer_size 返回 0
                let _ = window.show();
                let _ = window.unminimize();

                // 拿到屏幕大小（拷贝出来避免借用问题）
                let monitor_size: Option<(u32, u32)> = window
                    .primary_monitor()
                    .ok()
                    .flatten()
                    .map(|m| {
                        let s = m.size();
                        (s.width, s.height)
                    });

                // 用内尺寸兜底（更可能在 setup 阶段就有效）
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
