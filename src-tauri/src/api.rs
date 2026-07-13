//! Type-safe MiniMax API client + view-model boundary.
//!
//! Frontend only ever sees [`UsageViewModel`] — never the raw JSON body,
//! never the auth path, never a server error body. Anything we cannot
//! validate is collapsed into a structured [`UsageErrorKind`].

use serde::{Deserialize, Serialize};
use std::time::Duration;

const API_BASE: &str = "https://www.minimaxi.com";
const REMAINS_PATH: &str = "/v1/api/openplatform/coding_plan/remains";
/// Hard cap on response body we are willing to allocate. Anything larger
/// is rejected before we even read it into memory.
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;
/// Per-request timeout. Outer retry budget = 3 attempts × this value.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const USER_AGENT: &str = concat!("claude-usage-widget/", env!("CARGO_PKG_VERSION"));

// ─── Error taxonomy ────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // MissingKey may be reported via string label from lib.rs
pub enum UsageErrorKind {
    MissingKey,
    InvalidKey,
    Forbidden,
    RateLimited,
    Timeout,
    Network,
    Server,
    InvalidResponse,
    ResponseTooLarge,
}

/// What the WebView actually receives for any failure.
#[derive(Debug, Clone, Serialize)]
pub struct UsageError {
    pub kind: UsageErrorKind,
    /// User-safe message (no internal paths, no raw server body).
    pub user_message: String,
    /// When the server told us how long to wait.
    pub retry_after_seconds: Option<u64>,
}

// ─── Wire format (private DTOs) ────────────────────────────────

/// Top-level server response for /remains.
#[derive(Debug, Deserialize)]
pub(super) struct ApiRemainsResponse {
    #[serde(default)]
    pub model_remains: Vec<ApiModelRemain>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ApiModelRemain {
    /// 1 = main quota in use, 3 = gifted quota, other values tolerated.
    /// Unknown values fall through; we don't crash on them.
    #[serde(default)]
    pub current_interval_status: Option<i32>,
    /// Remaining % for the 5-hour window (0..=100). Optional / may be null.
    #[serde(default)]
    pub current_interval_remaining_percent: Option<serde_json::Value>,
    /// Remaining % for the 7-day window (0..=100).
    #[serde(default)]
    pub current_weekly_remaining_percent: Option<serde_json::Value>,
    /// Milliseconds until 5h reset.
    #[serde(default)]
    pub remains_time: Option<serde_json::Value>,
    /// Milliseconds until weekly reset.
    #[serde(default)]
    pub weekly_remains_time: Option<serde_json::Value>,
    /// Window start timestamp (ms since epoch).
    #[serde(default)]
    pub start_time: Option<serde_json::Value>,
    /// Window end timestamp (ms since epoch).
    #[serde(default)]
    pub end_time: Option<serde_json::Value>,
}

// ─── View-model boundary (public, sent to WebView) ────────────

#[derive(Debug, Clone, Serialize)]
pub struct UsageViewModel {
    pub five_hour: UsageWindowView,
    pub weekly: UsageWindowView,
    pub fetched_at: String,
    pub state: UsageState,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageWindowView {
    /// 0..=100, None when no signal from server.
    pub used_percent: Option<f64>,
    pub reset_after_ms: Option<i64>,
    pub start_at_ms: Option<i64>,
    pub end_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageState {
    Ok,
    NoData,
}

// ─── Parsing helpers (pure functions, exhaustively unit-tested) ──

/// Pull a finite 0..=100 number out of arbitrary JSON.
/// Returns None on null / wrong type / out-of-range / NaN / Infinity.
pub(super) fn parse_percent(v: Option<&serde_json::Value>) -> Option<f64> {
    let n = match v? {
        serde_json::Value::Number(num) => num.as_f64()?,
        serde_json::Value::String(s) => s.parse::<f64>().ok()?,
        _ => return None,
    };
    if !n.is_finite() {
        return None;
    }
    if n < 0.0 || n > 100.0 {
        return None;
    }
    Some(n)
}

/// Pull a non-negative finite number (durations / timestamps).
pub(super) fn parse_nonneg(v: Option<&serde_json::Value>) -> Option<i64> {
    let n = match v? {
        serde_json::Value::Number(num) => num.as_i64()?,
        serde_json::Value::String(s) => s.parse::<i64>().ok()?,
        _ => return None,
    };
    if n < 0 {
        return None;
    }
    Some(n)
}

/// Used percent = 100 - remaining percent, clamped 0..=100.
pub(super) fn remaining_to_used(remaining: Option<f64>) -> Option<f64> {
    let r = remaining?;
    Some((100.0 - r).clamp(0.0, 100.0))
}

/// Pick the primary window: status == 1 if any, otherwise the first entry.
/// Unknown / missing status still yields a safe fallback (first entry).
pub(super) fn pick_primary<'a>(rows: &'a [ApiModelRemain]) -> Option<&'a ApiModelRemain> {
    rows.iter()
        .find(|r| r.current_interval_status == Some(1))
        .or(rows.first())
}

/// Translate wire → view-model. Pure, no I/O.
pub(super) fn to_view_model(
    primary: Option<&ApiModelRemain>,
    fetched_at: String,
) -> UsageViewModel {
    let (five_hour, weekly, state) = match primary {
        Some(p) => {
            let five_rem = parse_percent(p.current_interval_remaining_percent.as_ref());
            let week_rem = parse_percent(p.current_weekly_remaining_percent.as_ref());
            let five_hour = UsageWindowView {
                used_percent: remaining_to_used(five_rem),
                reset_after_ms: parse_nonneg(p.remains_time.as_ref()),
                start_at_ms: parse_nonneg(p.start_time.as_ref()),
                end_at_ms: parse_nonneg(p.end_time.as_ref()),
            };
            let weekly = UsageWindowView {
                used_percent: remaining_to_used(week_rem),
                reset_after_ms: parse_nonneg(p.weekly_remains_time.as_ref()),
                start_at_ms: None,
                end_at_ms: None,
            };
            let state = if five_hour.used_percent.is_some() || weekly.used_percent.is_some() {
                UsageState::Ok
            } else {
                UsageState::NoData
            };
            (five_hour, weekly, state)
        }
        None => (
            UsageWindowView {
                used_percent: None,
                reset_after_ms: None,
                start_at_ms: None,
                end_at_ms: None,
            },
            UsageWindowView {
                used_percent: None,
                reset_after_ms: None,
                start_at_ms: None,
                end_at_ms: None,
            },
            UsageState::NoData,
        ),
    };
    UsageViewModel {
        five_hour,
        weekly,
        fetched_at,
        state,
    }
}

// ─── HTTP client + retry policy ────────────────────────────────

/// Build a shared reqwest Client. HTTPS-only via rustls.
pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(Duration::from_secs(5))
        .user_agent(USER_AGENT)
        .https_only(true)
        .build()
        .expect("reqwest client builder cannot fail with these options")
}

/// Classify an HTTP status into our error taxonomy.
fn classify_status(status: reqwest::StatusCode) -> UsageErrorKind {
    match status.as_u16() {
        401 => UsageErrorKind::InvalidKey,
        403 => UsageErrorKind::Forbidden,
        429 => UsageErrorKind::RateLimited,
        500..=599 => UsageErrorKind::Server,
        _ => UsageErrorKind::Server,
    }
}

/// User-safe error messages. No server body, no internal context.
fn user_message(kind: UsageErrorKind) -> String {
    match kind {
        UsageErrorKind::MissingKey => "未配置 API Key".into(),
        UsageErrorKind::InvalidKey => "API Key 认证失败（401）".into(),
        UsageErrorKind::Forbidden => "API Key 无权限（403）".into(),
        UsageErrorKind::RateLimited => "请求过于频繁，请稍后重试".into(),
        UsageErrorKind::Timeout => "请求超时".into(),
        UsageErrorKind::Network => "网络错误".into(),
        UsageErrorKind::Server => "服务暂时不可用".into(),
        UsageErrorKind::InvalidResponse => "响应格式异常".into(),
        UsageErrorKind::ResponseTooLarge => "响应过大".into(),
    }
}

/// Result of one fetch attempt including all intermediate context.
#[derive(Debug)]
enum FetchAttempt {
    Ok(ApiRemainsResponse),
    /// Caller should retry if appropriate.
    Retry(UsageError),
    /// Caller must not retry.
    GiveUp(UsageError),
}

/// Single HTTP attempt. Includes content-length preflight and body cap.
async fn try_once(client: &reqwest::Client, api_key: &str) -> FetchAttempt {
    let url = format!("{}{}", API_BASE, REMAINS_PATH);
    let resp = match client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("referer", "https://platform.minimaxi.com/")
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return if e.is_timeout() {
                FetchAttempt::Retry(UsageError {
                    kind: UsageErrorKind::Timeout,
                    user_message: user_message(UsageErrorKind::Timeout),
                    retry_after_seconds: None,
                })
            } else if e.is_connect() || e.is_request() {
                FetchAttempt::Retry(UsageError {
                    kind: UsageErrorKind::Network,
                    user_message: user_message(UsageErrorKind::Network),
                    retry_after_seconds: None,
                })
            } else {
                FetchAttempt::GiveUp(UsageError {
                    kind: UsageErrorKind::Network,
                    user_message: user_message(UsageErrorKind::Network),
                    retry_after_seconds: None,
                })
            }
        }
    };

    let status = resp.status();
    if !status.is_success() {
        let kind = classify_status(status);
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        let err = UsageError {
            kind,
            user_message: user_message(kind),
            retry_after_seconds: retry_after,
        };
        // 401/403 give up; 429 + 5xx are retryable
        return match kind {
            UsageErrorKind::InvalidKey | UsageErrorKind::Forbidden => FetchAttempt::GiveUp(err),
            _ => FetchAttempt::Retry(err),
        };
    }

    // Pre-flight: refuse responses advertising a too-large body.
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_RESPONSE_BYTES {
            return FetchAttempt::GiveUp(UsageError {
                kind: UsageErrorKind::ResponseTooLarge,
                user_message: user_message(UsageErrorKind::ResponseTooLarge),
                retry_after_seconds: None,
            });
        }
    }

    // Read body with hard cap. Use reqwest's chunk() loop (we own the
    // Response value, so we move it back and forth via let mut).
    let mut total: usize = 0;
    let mut buf: Vec<u8> = Vec::new();
    let mut resp = resp;
    loop {
        match resp.chunk().await {
            Ok(Some(bytes)) => {
                total = total.saturating_add(bytes.len());
                if total > MAX_RESPONSE_BYTES {
                    return FetchAttempt::GiveUp(UsageError {
                        kind: UsageErrorKind::ResponseTooLarge,
                        user_message: user_message(UsageErrorKind::ResponseTooLarge),
                        retry_after_seconds: None,
                    });
                }
                buf.extend_from_slice(&bytes);
            }
            Ok(None) => break,
            Err(_) => {
                return FetchAttempt::Retry(UsageError {
                    kind: UsageErrorKind::Network,
                    user_message: user_message(UsageErrorKind::Network),
                    retry_after_seconds: None,
                });
            }
        }
    }

    let text = match std::str::from_utf8(&buf) {
        Ok(s) => s,
        Err(_) => {
            return FetchAttempt::GiveUp(UsageError {
                kind: UsageErrorKind::InvalidResponse,
                user_message: user_message(UsageErrorKind::InvalidResponse),
                retry_after_seconds: None,
            });
        }
    };

    match serde_json::from_str::<ApiRemainsResponse>(text) {
        Ok(v) => FetchAttempt::Ok(v),
        Err(_) => FetchAttempt::GiveUp(UsageError {
            kind: UsageErrorKind::InvalidResponse,
            user_message: user_message(UsageErrorKind::InvalidResponse),
            retry_after_seconds: None,
        }),
    }
}

/// Public entry: fetch with bounded retries (2 retries → 3 attempts total).
/// Exponential backoff with light jitter, honors Retry-After when present.
pub async fn fetch_usage(
    client: &reqwest::Client,
    api_key: &str,
) -> Result<UsageViewModel, UsageError> {
    const MAX_ATTEMPTS: u32 = 3;
    let mut attempt_idx: u32 = 0;
    loop {
        let outcome = try_once(client, api_key).await;
        match outcome {
            FetchAttempt::Ok(resp) => {
                let primary = pick_primary(&resp.model_remains);
                let fetched_at = chrono::Utc::now().to_rfc3339();
                return Ok(to_view_model(primary, fetched_at));
            }
            FetchAttempt::GiveUp(err) => return Err(err),
            FetchAttempt::Retry(err) => {
                attempt_idx += 1;
                if attempt_idx >= MAX_ATTEMPTS {
                    return Err(err);
                }
                // backoff: 250ms, 750ms with ±20% jitter; honor Retry-After if larger
                let base_ms: u64 = match attempt_idx {
                    1 => 250,
                    _ => 750,
                };
                let jitter: f64 = 0.8 + (attempt_idx as f64 * 0.137) % 0.4;
                let mut delay_ms = (base_ms as f64 * jitter) as u64;
                if let Some(s) = err.retry_after_seconds {
                    delay_ms = delay_ms.max(s * 1000);
                }
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
}

// ─── Unit tests ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_percent_accepts_finite_in_range() {
        assert_eq!(parse_percent(Some(&serde_json::json!(42))), Some(42.0));
        assert_eq!(parse_percent(Some(&serde_json::json!(0))), Some(0.0));
        assert_eq!(parse_percent(Some(&serde_json::json!(100))), Some(100.0));
        assert_eq!(parse_percent(Some(&serde_json::json!(3.14))), Some(3.14));
    }

    #[test]
    fn parse_percent_rejects_garbage() {
        assert_eq!(parse_percent(None), None);
        assert_eq!(parse_percent(Some(&serde_json::json!(null))), None);
        assert_eq!(parse_percent(Some(&serde_json::json!("abc"))), None);
        assert_eq!(parse_percent(Some(&serde_json::json!({}))), None);
        assert_eq!(parse_percent(Some(&serde_json::json!([]))), None);
    }

    #[test]
    fn parse_percent_rejects_out_of_range() {
        assert_eq!(parse_percent(Some(&serde_json::json!(-1))), None);
        assert_eq!(parse_percent(Some(&serde_json::json!(100.01))), None);
        assert_eq!(parse_percent(Some(&serde_json::json!(1e9))), None);
    }

    #[test]
    fn parse_percent_accepts_numeric_string() {
        assert_eq!(parse_percent(Some(&serde_json::json!("50"))), Some(50.0));
    }

    #[test]
    fn remaining_to_used_clamps() {
        assert_eq!(remaining_to_used(Some(0.0)), Some(100.0));
        assert_eq!(remaining_to_used(Some(100.0)), Some(0.0));
        assert_eq!(remaining_to_used(Some(40.0)), Some(60.0));
        assert_eq!(remaining_to_used(None), None);
    }

    #[test]
    fn parse_nonneg_rejects_negative() {
        assert_eq!(parse_nonneg(Some(&serde_json::json!(-1))), None);
        assert_eq!(parse_nonneg(Some(&serde_json::json!(0))), Some(0));
        assert_eq!(parse_nonneg(Some(&serde_json::json!(1234))), Some(1234));
    }

    #[test]
    fn pick_primary_prefers_status_one() {
        let rows = vec![
            ApiModelRemain {
                current_interval_status: Some(3),
                current_interval_remaining_percent: None,
                current_weekly_remaining_percent: None,
                remains_time: None,
                weekly_remains_time: None,
                start_time: None,
                end_time: None,
            },
            ApiModelRemain {
                current_interval_status: Some(1),
                current_interval_remaining_percent: None,
                current_weekly_remaining_percent: None,
                remains_time: None,
                weekly_remains_time: None,
                start_time: None,
                end_time: None,
            },
        ];
        assert_eq!(
            pick_primary(&rows).unwrap().current_interval_status,
            Some(1)
        );
    }

    #[test]
    fn pick_primary_fallback_to_first_when_no_status_one() {
        let rows = vec![ApiModelRemain {
            current_interval_status: Some(3),
            current_interval_remaining_percent: None,
            current_weekly_remaining_percent: None,
            remains_time: None,
            weekly_remains_time: None,
            start_time: None,
            end_time: None,
        }];
        assert_eq!(
            pick_primary(&rows).unwrap().current_interval_status,
            Some(3)
        );
    }

    #[test]
    fn pick_primary_returns_none_for_empty() {
        assert!(pick_primary(&[]).is_none());
    }

    #[test]
    fn to_view_model_with_valid_primary() {
        let p = ApiModelRemain {
            current_interval_status: Some(1),
            current_interval_remaining_percent: Some(serde_json::json!(40)),
            current_weekly_remaining_percent: Some(serde_json::json!(96)),
            remains_time: Some(serde_json::json!(3600000)),
            weekly_remains_time: Some(serde_json::json!(604800000)),
            start_time: Some(serde_json::json!(1000)),
            end_time: Some(serde_json::json!(2000)),
        };
        let vm = to_view_model(Some(&p), "now".into());
        assert_eq!(vm.five_hour.used_percent, Some(60.0));
        assert_eq!(vm.weekly.used_percent, Some(4.0));
        assert_eq!(vm.five_hour.reset_after_ms, Some(3600000));
        assert_eq!(vm.state, UsageState::Ok);
    }

    #[test]
    fn to_view_model_no_data_when_only_garbage() {
        let p = ApiModelRemain {
            current_interval_status: Some(1),
            current_interval_remaining_percent: Some(serde_json::json!("abc")),
            current_weekly_remaining_percent: None,
            remains_time: None,
            weekly_remains_time: None,
            start_time: None,
            end_time: None,
        };
        let vm = to_view_model(Some(&p), "now".into());
        assert_eq!(vm.five_hour.used_percent, None);
        assert_eq!(vm.weekly.used_percent, None);
        assert_eq!(vm.state, UsageState::NoData);
    }

    #[test]
    fn to_view_model_empty_rows() {
        let vm = to_view_model(None, "now".into());
        assert_eq!(vm.state, UsageState::NoData);
    }

    #[test]
    fn viewmodel_does_not_leak_raw_or_model_name() {
        // The view-model struct has no `raw` field. Compile-time guarantee.
        let vm = to_view_model(None, "now".into());
        let json = serde_json::to_string(&vm).unwrap();
        assert!(!json.contains("raw"));
        assert!(!json.contains("model_remains"));
        assert!(!json.contains("model_name"));
        assert!(!json.contains("general"));
        assert!(!json.contains("video"));
    }

    #[test]
    fn classify_status_for_each_known_code() {
        assert_eq!(
            classify_status(reqwest::StatusCode::UNAUTHORIZED),
            UsageErrorKind::InvalidKey
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::FORBIDDEN),
            UsageErrorKind::Forbidden
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::TOO_MANY_REQUESTS),
            UsageErrorKind::RateLimited
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
            UsageErrorKind::Server
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::BAD_GATEWAY),
            UsageErrorKind::Server
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::SERVICE_UNAVAILABLE),
            UsageErrorKind::Server
        );
    }

    #[test]
    fn classify_status_5xx_are_retryable() {
        // The fetch retry policy treats 5xx as retryable — encoded in try_once.
        // Here we just check that 5xx returns Server (which try_once routes to Retry).
        let k = classify_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(k, UsageErrorKind::Server);
    }
}
