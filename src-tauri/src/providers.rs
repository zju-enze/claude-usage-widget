use serde::{Deserialize, Serialize};
use std::time::Duration;

const MINIMAX_USAGE_URL: &str = "https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains";
const DEEPSEEK_USAGE_URL: &str = "https://api.deepseek.com/user/balance";
const ZHIPU_USAGE_URL: &str = "https://open.bigmodel.cn/api/monitor/usage/quota/limit";
const ZAI_USAGE_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_ATTEMPTS: usize = 3;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Provider {
    Minimax,
    Deepseek,
    Zhipu,
}

impl Provider {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Minimax => "minimax",
            Self::Deepseek => "deepseek",
            Self::Zhipu => "zhipu",
        }
    }

    pub(crate) fn from_id(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "minimax" => Some(Self::Minimax),
            "deepseek" => Some(Self::Deepseek),
            "zhipu" => Some(Self::Zhipu),
            _ => None,
        }
    }

    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::Minimax => "MiniMax",
            Self::Deepseek => "DeepSeek",
            Self::Zhipu => "智谱",
        }
    }

    pub(crate) const fn direct_env_names(self) -> &'static [&'static str] {
        match self {
            Self::Minimax => &["MINIMAX_API_KEY", "MINIMAX_CP_TOKEN"],
            Self::Deepseek => &["DEEPSEEK_API_KEY"],
            Self::Zhipu => &["ZAI_API_KEY", "ZHIPUAI_API_KEY", "BIGMODEL_API_KEY"],
        }
    }

    pub(crate) const fn key_filename(self) -> &'static str {
        match self {
            // Preserve the legacy filename so existing MiniMax installations keep working.
            Self::Minimax => "key.bin",
            Self::Deepseek => "key-deepseek.bin",
            Self::Zhipu => "key-zhipu.bin",
        }
    }

    pub(crate) const fn help_url(self) -> &'static str {
        match self {
            Self::Minimax => {
                "https://platform.minimaxi.com/user-center/basic-information/interface-key"
            }
            Self::Deepseek => "https://platform.deepseek.com/api_keys",
            Self::Zhipu => "https://open.bigmodel.cn/usercenter/proj-mgmt/apikeys",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageData {
    pub(crate) metrics: [UsageMetric; 2],
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageMetric {
    pub(crate) label: String,
    pub(crate) percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) value: Option<String>,
    pub(crate) description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) aria_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tone: Option<String>,
}

impl UsageMetric {
    fn percentage(label: &str, percent: Option<f64>, description: String) -> Self {
        Self {
            label: label.to_string(),
            percent,
            value: None,
            description,
            aria_text: None,
            tone: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceError {
    Network,
    Timeout,
    Unauthorized,
    RateLimited,
    Unavailable,
    RequestFailed,
    ResponseTooLarge,
    InvalidResponse,
}

impl ServiceError {
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Unauthorized => "unauthorized",
            Self::RateLimited => "rate_limited",
            Self::Unavailable => "service_unavailable",
            Self::RequestFailed => "request_failed",
            Self::ResponseTooLarge => "response_too_large",
            Self::InvalidResponse => "invalid_response",
        }
    }
}

pub(crate) fn service_error_for_status(status: reqwest::StatusCode) -> ServiceError {
    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            ServiceError::Unauthorized
        }
        reqwest::StatusCode::TOO_MANY_REQUESTS => ServiceError::RateLimited,
        status if status.is_server_error() => ServiceError::Unavailable,
        _ => ServiceError::RequestFailed,
    }
}

pub(crate) fn build_http_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .user_agent(concat!("claude-usage-widget/", env!("CARGO_PKG_VERSION")))
        .build()
}

pub(crate) fn provider_from_base_url(value: &str) -> Option<Provider> {
    let url = reqwest::Url::parse(value.trim()).ok()?;
    if url.scheme() != "https" {
        return None;
    }

    match url.host_str()?.to_ascii_lowercase().as_str() {
        "api.deepseek.com" => Some(Provider::Deepseek),
        "open.bigmodel.cn" | "dev.bigmodel.cn" | "api.z.ai" => Some(Provider::Zhipu),
        "api.minimax.io" | "api.minimaxi.com" | "www.minimaxi.com" => Some(Provider::Minimax),
        _ => None,
    }
}

pub(crate) fn provider_from_model(value: &str) -> Option<Provider> {
    let model = value.trim().to_ascii_lowercase();
    if model.starts_with("deepseek-") {
        Some(Provider::Deepseek)
    } else if model.starts_with("glm-") {
        Some(Provider::Zhipu)
    } else if model.starts_with("minimax-") {
        Some(Provider::Minimax)
    } else {
        None
    }
}

pub(crate) fn is_valid_api_key(provider: Provider, value: &str) -> bool {
    let length = value.len();
    let valid_shape = match provider {
        Provider::Minimax => value.starts_with("sk-cp-"),
        Provider::Deepseek => value.starts_with("sk-"),
        Provider::Zhipu => true,
    };

    (20..=512).contains(&length)
        && valid_shape
        && value.is_ascii()
        && !value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
}

fn usage_url(provider: Provider, configured_base_url: Option<&str>) -> &'static str {
    match provider {
        Provider::Minimax => MINIMAX_USAGE_URL,
        Provider::Deepseek => DEEPSEEK_USAGE_URL,
        Provider::Zhipu => {
            if configured_base_url
                .and_then(|value| reqwest::Url::parse(value.trim()).ok())
                .filter(|url| url.scheme() == "https")
                .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
                .as_deref()
                == Some("api.z.ai")
            {
                ZAI_USAGE_URL
            } else {
                ZHIPU_USAGE_URL
            }
        }
    }
}

#[derive(Debug)]
enum Attempt {
    Success(Vec<u8>),
    Retry {
        error: ServiceError,
        retry_after: Option<Duration>,
    },
    GiveUp(ServiceError),
}

fn retryable_transport_error(error: &reqwest::Error) -> Option<ServiceError> {
    if error.is_timeout() {
        Some(ServiceError::Timeout)
    } else if error.is_connect() {
        Some(ServiceError::Network)
    } else {
        None
    }
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let seconds = value.trim().parse::<u64>().ok()?;
    Some(Duration::from_secs(seconds).min(MAX_RETRY_AFTER))
}

fn retry_delay(failed_attempt: usize, retry_after: Option<Duration>) -> Duration {
    let backoff = match failed_attempt {
        0 => Duration::from_millis(250),
        _ => Duration::from_millis(750),
    };
    backoff.max(retry_after.unwrap_or_default().min(MAX_RETRY_AFTER))
}

fn append_chunk(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), ServiceError> {
    let next_len = body
        .len()
        .checked_add(chunk.len())
        .ok_or(ServiceError::ResponseTooLarge)?;
    if next_len > MAX_RESPONSE_BYTES {
        return Err(ServiceError::ResponseTooLarge);
    }
    body.extend_from_slice(chunk);
    Ok(())
}

async fn sleep_without_blocking_runtime(delay: Duration) {
    if delay.is_zero() {
        return;
    }
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(delay)).await;
}

async fn request_once(
    client: &reqwest::Client,
    provider: Provider,
    key: &str,
    configured_base_url: Option<&str>,
) -> Attempt {
    let mut request = client
        .get(usage_url(provider, configured_base_url))
        .header(reqwest::header::ACCEPT, "application/json");

    request = match provider {
        Provider::Zhipu => request
            // The official GLM Coding Plan usage plugin sends this token verbatim.
            .header(reqwest::header::AUTHORIZATION, key)
            .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9"),
        Provider::Minimax => request
            .bearer_auth(key)
            .header(reqwest::header::REFERER, "https://platform.minimaxi.com/"),
        Provider::Deepseek => request.bearer_auth(key),
    };

    let mut response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            return match retryable_transport_error(&error) {
                Some(error) => Attempt::Retry {
                    error,
                    retry_after: None,
                },
                None => Attempt::GiveUp(ServiceError::Network),
            };
        }
    };

    let status = response.status();
    if !status.is_success() {
        let error = service_error_for_status(status);
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Attempt::Retry {
                error,
                retry_after: retry_after(response.headers()),
            };
        }
        return Attempt::GiveUp(error);
    }

    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Attempt::GiveUp(ServiceError::ResponseTooLarge);
    }

    let initial_capacity = response
        .content_length()
        .unwrap_or(0)
        .min(MAX_RESPONSE_BYTES as u64) as usize;
    let mut body = Vec::with_capacity(initial_capacity);
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if let Err(error) = append_chunk(&mut body, &chunk) {
                    return Attempt::GiveUp(error);
                }
            }
            Ok(None) => break,
            Err(error) => {
                let error = if error.is_timeout() {
                    ServiceError::Timeout
                } else {
                    ServiceError::Network
                };
                return Attempt::Retry {
                    error,
                    retry_after: None,
                };
            }
        }
    }

    Attempt::Success(body)
}

pub(crate) async fn request_usage(
    client: &reqwest::Client,
    provider: Provider,
    key: &str,
    configured_base_url: Option<&str>,
) -> Result<UsageData, ServiceError> {
    for failed_attempt in 0..MAX_ATTEMPTS {
        match request_once(client, provider, key, configured_base_url).await {
            Attempt::Success(body) => return parse_usage(provider, &body),
            Attempt::GiveUp(error) => return Err(error),
            Attempt::Retry { error, retry_after } => {
                if failed_attempt + 1 >= MAX_ATTEMPTS {
                    return Err(error);
                }
                sleep_without_blocking_runtime(retry_delay(failed_attempt, retry_after)).await;
            }
        }
    }
    Err(ServiceError::Network)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum NumberLike {
    Number(f64),
    String(String),
}

impl NumberLike {
    fn finite(&self) -> Option<f64> {
        let value = match self {
            Self::Number(value) => *value,
            Self::String(value) => value.parse::<f64>().ok()?,
        };
        value.is_finite().then_some(value)
    }

    fn display(&self) -> Option<String> {
        match self {
            Self::String(value) if !value.trim().is_empty() => Some(value.clone()),
            Self::String(_) => None,
            Self::Number(value) if value.is_finite() => Some(value.to_string()),
            Self::Number(_) => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct MinimaxResponse {
    model_remains: Vec<MinimaxRemain>,
}

#[derive(Debug, Deserialize)]
struct MinimaxRemain {
    #[serde(default)]
    current_interval_status: Option<i32>,
    #[serde(default)]
    current_interval_remaining_percent: Option<NumberLike>,
    #[serde(default)]
    current_weekly_remaining_percent: Option<NumberLike>,
    #[serde(default)]
    remains_time: Option<NumberLike>,
    #[serde(default)]
    weekly_remains_time: Option<NumberLike>,
    #[serde(default)]
    start_time: Option<NumberLike>,
    #[serde(default)]
    end_time: Option<NumberLike>,
}

#[derive(Debug, Deserialize)]
struct DeepseekResponse {
    is_available: bool,
    balance_infos: Vec<DeepseekBalance>,
}

#[derive(Debug, Deserialize)]
struct DeepseekBalance {
    currency: String,
    total_balance: String,
    granted_balance: String,
    topped_up_balance: String,
}

#[derive(Debug, Deserialize)]
struct ZhipuResponse {
    #[serde(default)]
    data: Option<ZhipuData>,
    #[serde(default)]
    limits: Option<Vec<ZhipuLimit>>,
}

#[derive(Debug, Deserialize)]
struct ZhipuData {
    limits: Vec<ZhipuLimit>,
}

#[derive(Debug, Deserialize)]
struct ZhipuLimit {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    percentage: Option<NumberLike>,
    #[serde(default, rename = "currentValue")]
    current_value: Option<NumberLike>,
    #[serde(default, rename = "currentUsage")]
    current_usage: Option<NumberLike>,
    #[serde(default)]
    usage: Option<NumberLike>,
    #[serde(default)]
    total: Option<NumberLike>,
    #[serde(default)]
    totol: Option<NumberLike>,
}

fn parse_usage(provider: Provider, body: &[u8]) -> Result<UsageData, ServiceError> {
    match provider {
        Provider::Minimax => serde_json::from_slice::<MinimaxResponse>(body)
            .map_err(|_| ServiceError::InvalidResponse)
            .and_then(normalize_minimax),
        Provider::Deepseek => serde_json::from_slice::<DeepseekResponse>(body)
            .map_err(|_| ServiceError::InvalidResponse)
            .and_then(normalize_deepseek),
        Provider::Zhipu => serde_json::from_slice::<ZhipuResponse>(body)
            .map_err(|_| ServiceError::InvalidResponse)
            .and_then(normalize_zhipu),
    }
}

fn clamped_percent(value: Option<&NumberLike>) -> Option<f64> {
    value?.finite().map(|value| value.clamp(0.0, 100.0))
}

fn remaining_to_used(value: Option<&NumberLike>) -> Option<f64> {
    clamped_percent(value).map(|remaining| 100.0 - remaining)
}

fn duration_text(value: Option<&NumberLike>) -> Option<String> {
    let milliseconds = value?.finite()?;
    if milliseconds <= 0.0 || milliseconds > i64::MAX as f64 {
        return None;
    }
    let total_minutes = ((milliseconds / 60_000.0).floor() as u64).max(1);
    let days = total_minutes / (24 * 60);
    let hours = (total_minutes % (24 * 60)) / 60;
    let minutes = total_minutes % 60;
    Some(if days > 0 {
        format!("{days} 天 {hours} 时")
    } else if hours > 0 {
        format!("{hours} 时 {minutes} 分")
    } else {
        format!("{minutes} 分")
    })
}

fn reset_description(prefix: &str, value: Option<&NumberLike>) -> String {
    duration_text(value)
        .map(|duration| format!("{prefix} · {duration} 后重置"))
        .unwrap_or_else(|| prefix.to_string())
}

fn local_time(value: Option<&NumberLike>) -> Option<String> {
    let milliseconds = value?.finite()?;
    if milliseconds <= 0.0 || milliseconds > i64::MAX as f64 {
        return None;
    }
    let datetime = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(milliseconds as i64)?;
    let offset = chrono::FixedOffset::east_opt(8 * 60 * 60)?;
    Some(datetime.with_timezone(&offset).format("%H:%M").to_string())
}

fn normalize_minimax(response: MinimaxResponse) -> Result<UsageData, ServiceError> {
    let primary = response
        .model_remains
        .iter()
        .find(|item| item.current_interval_status == Some(1))
        .or_else(|| response.model_remains.first())
        .ok_or(ServiceError::InvalidResponse)?;

    let interval_label = match (
        local_time(primary.start_time.as_ref()),
        local_time(primary.end_time.as_ref()),
    ) {
        (Some(start), Some(end)) => format!("{start}–{end}"),
        _ => "本周期".to_string(),
    };

    Ok(UsageData {
        metrics: [
            UsageMetric::percentage(
                "5 小时",
                remaining_to_used(primary.current_interval_remaining_percent.as_ref()),
                reset_description(&interval_label, primary.remains_time.as_ref()),
            ),
            UsageMetric::percentage(
                "7 天",
                remaining_to_used(primary.current_weekly_remaining_percent.as_ref()),
                reset_description("本周", primary.weekly_remains_time.as_ref()),
            ),
        ],
    })
}

fn currency_amount(currency: &str, amount: &str) -> String {
    match currency {
        "CNY" => format!("¥{amount}"),
        "USD" => format!("${amount}"),
        "" => format!("余额 {amount}"),
        _ => format!("{currency} {amount}"),
    }
}

fn normalize_deepseek(response: DeepseekResponse) -> Result<UsageData, ServiceError> {
    let balance = response
        .balance_infos
        .iter()
        .find(|item| item.currency == "CNY")
        .or_else(|| response.balance_infos.first())
        .ok_or(ServiceError::InvalidResponse)?;
    let total = currency_amount(&balance.currency, &balance.total_balance);
    let granted = currency_amount(&balance.currency, &balance.granted_balance);
    let topped_up = currency_amount(&balance.currency, &balance.topped_up_balance);
    let (status_value, status_description, status_aria, tone) = if response.is_available {
        ("可用", "余额足以继续调用", "API 可用", "good")
    } else {
        ("不可用", "余额不足，请充值后重试", "API 不可用", "bad")
    };

    Ok(UsageData {
        metrics: [
            UsageMetric {
                label: "账户余额".to_string(),
                percent: None,
                value: Some(total.clone()),
                description: format!("赠送 {granted} · 充值 {topped_up}"),
                aria_text: Some(format!("账户余额 {total}")),
                tone: None,
            },
            UsageMetric {
                label: "API 状态".to_string(),
                percent: Some(100.0),
                value: Some(status_value.to_string()),
                description: status_description.to_string(),
                aria_text: Some(status_aria.to_string()),
                tone: Some(tone.to_string()),
            },
        ],
    })
}

fn quota_description(limit: Option<&ZhipuLimit>, fallback: &str) -> String {
    let Some(limit) = limit else {
        return fallback.to_string();
    };
    let current = limit
        .current_value
        .as_ref()
        .or(limit.current_usage.as_ref());
    let total = limit
        .usage
        .as_ref()
        .or(limit.total.as_ref())
        .or(limit.totol.as_ref());
    match (
        current.and_then(NumberLike::display),
        total.and_then(NumberLike::display),
    ) {
        (Some(current), Some(total)) => format!("{current} / {total}"),
        _ => fallback.to_string(),
    }
}

fn normalize_zhipu(response: ZhipuResponse) -> Result<UsageData, ServiceError> {
    let limits = response
        .data
        .map(|data| data.limits)
        .or(response.limits)
        .ok_or(ServiceError::InvalidResponse)?;
    let tokens = limits.iter().find(|item| item.kind == "TOKENS_LIMIT");
    let tools = limits.iter().find(|item| item.kind == "TIME_LIMIT");
    if tokens.is_none() && tools.is_none() {
        return Err(ServiceError::InvalidResponse);
    }

    Ok(UsageData {
        metrics: [
            UsageMetric::percentage(
                "5 小时",
                clamped_percent(tokens.and_then(|item| item.percentage.as_ref())),
                quota_description(tokens, "GLM Coding Plan Token 配额"),
            ),
            UsageMetric::percentage(
                "MCP 月额度",
                clamped_percent(tools.and_then(|item| item.percentage.as_ref())),
                quota_description(tools, "GLM Coding Plan MCP 配额"),
            ),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_supported_https_provider_hosts() {
        assert_eq!(
            provider_from_base_url("https://api.deepseek.com/anthropic"),
            Some(Provider::Deepseek)
        );
        assert_eq!(
            provider_from_base_url("https://open.bigmodel.cn/api/anthropic"),
            Some(Provider::Zhipu)
        );
        assert_eq!(provider_from_base_url("http://api.deepseek.com"), None);
        assert_eq!(
            provider_from_base_url("https://api.deepseek.com.example.org"),
            None
        );
    }

    #[test]
    fn converts_provider_ids_without_guessing() {
        assert_eq!(Provider::from_id("minimax"), Some(Provider::Minimax));
        assert_eq!(Provider::from_id(" DeepSeek "), Some(Provider::Deepseek));
        assert_eq!(Provider::from_id("zhipu"), Some(Provider::Zhipu));
        assert_eq!(Provider::from_id("unknown"), None);
    }

    #[test]
    fn validates_provider_specific_key_shapes() {
        assert!(is_valid_api_key(Provider::Minimax, "sk-cp-12345678901234"));
        assert!(!is_valid_api_key(
            Provider::Minimax,
            "sk-ant-12345678901234"
        ));
        assert!(is_valid_api_key(Provider::Deepseek, "sk-12345678901234567"));
        assert!(is_valid_api_key(Provider::Zhipu, "1234567890.abcdefghijkl"));
        assert!(!is_valid_api_key(
            Provider::Zhipu,
            "1234567890 abcdefghijkl"
        ));
    }

    #[test]
    fn parses_and_normalizes_minimax_without_exposing_vendor_fields() {
        let usage = parse_usage(
            Provider::Minimax,
            br#"{
                "model_remains": [{
                    "current_interval_status": 1,
                    "current_interval_remaining_percent": 64,
                    "current_weekly_remaining_percent": "28",
                    "remains_time": 5400000,
                    "weekly_remains_time": 97200000,
                    "start_time": 1767225600000,
                    "end_time": 1767243600000,
                    "model_name": "must-not-cross-boundary"
                }]
            }"#,
        )
        .unwrap();

        assert_eq!(usage.metrics[0].label, "5 小时");
        assert_eq!(usage.metrics[0].percent, Some(36.0));
        assert!(usage.metrics[0].description.contains("1 时 30 分 后重置"));
        assert_eq!(usage.metrics[1].percent, Some(72.0));
        let serialized = serde_json::to_string(&usage).unwrap();
        assert!(!serialized.contains("model_remains"));
        assert!(!serialized.contains("model_name"));
        assert!(!serialized.contains("must-not-cross-boundary"));
    }

    #[test]
    fn parses_and_normalizes_deepseek_balance() {
        let usage = parse_usage(
            Provider::Deepseek,
            br#"{
                "is_available": true,
                "balance_infos": [{
                    "currency": "CNY",
                    "total_balance": "110.00",
                    "granted_balance": "10.00",
                    "topped_up_balance": "100.00"
                }]
            }"#,
        )
        .unwrap();

        assert_eq!(usage.metrics[0].value.as_deref(), Some("¥110.00"));
        assert_eq!(usage.metrics[0].description, "赠送 ¥10.00 · 充值 ¥100.00");
        assert_eq!(usage.metrics[1].value.as_deref(), Some("可用"));
        assert_eq!(usage.metrics[1].tone.as_deref(), Some("good"));
    }

    #[test]
    fn parses_both_zhipu_envelope_shapes() {
        for body in [
            br#"{"data":{"limits":[{"type":"TOKENS_LIMIT","percentage":42},{"type":"TIME_LIMIT","percentage":"17","currentValue":3,"usage":20}]}}"#.as_slice(),
            br#"{"limits":[{"type":"TOKENS_LIMIT","percentage":42},{"type":"TIME_LIMIT","percentage":17,"currentUsage":3,"totol":20}]}"#.as_slice(),
        ] {
            let usage = parse_usage(Provider::Zhipu, body).unwrap();
            assert_eq!(usage.metrics[0].percent, Some(42.0));
            assert_eq!(usage.metrics[1].percent, Some(17.0));
            assert_eq!(usage.metrics[1].description, "3 / 20");
        }
    }

    #[test]
    fn rejects_invalid_or_empty_vendor_responses() {
        assert_eq!(
            parse_usage(Provider::Minimax, br#"{"model_remains":[]}"#),
            Err(ServiceError::InvalidResponse)
        );
        assert_eq!(
            parse_usage(
                Provider::Deepseek,
                br#"{"is_available":true,"balance_infos":[]}"#
            ),
            Err(ServiceError::InvalidResponse)
        );
        assert_eq!(
            parse_usage(Provider::Zhipu, br#"{"data":{"limits":[]}}"#),
            Err(ServiceError::InvalidResponse)
        );
        assert_eq!(
            parse_usage(Provider::Zhipu, br#"{"unexpected":true}"#),
            Err(ServiceError::InvalidResponse)
        );
    }

    #[test]
    fn uses_the_zai_monitor_host_only_for_https_overseas_configuration() {
        assert_eq!(
            usage_url(Provider::Zhipu, Some("https://api.z.ai/api/anthropic")),
            ZAI_USAGE_URL
        );
        assert_eq!(
            usage_url(Provider::Zhipu, Some("http://api.z.ai/api/anthropic")),
            ZHIPU_USAGE_URL
        );
        assert_eq!(
            usage_url(
                Provider::Zhipu,
                Some("https://open.bigmodel.cn/api/anthropic")
            ),
            ZHIPU_USAGE_URL
        );
    }

    #[test]
    fn maps_remote_statuses_to_stable_error_codes_and_retry_classes() {
        assert_eq!(
            service_error_for_status(reqwest::StatusCode::UNAUTHORIZED),
            ServiceError::Unauthorized
        );
        assert_eq!(
            service_error_for_status(reqwest::StatusCode::TOO_MANY_REQUESTS),
            ServiceError::RateLimited
        );
        assert_eq!(
            service_error_for_status(reqwest::StatusCode::BAD_GATEWAY),
            ServiceError::Unavailable
        );
        assert_eq!(
            service_error_for_status(reqwest::StatusCode::BAD_REQUEST),
            ServiceError::RequestFailed
        );
        assert_eq!(ServiceError::Timeout.code(), "timeout");
    }

    #[test]
    fn retry_after_and_backoff_are_capped() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "999".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(MAX_RETRY_AFTER));
        assert_eq!(
            retry_delay(0, retry_after(&headers)),
            Duration::from_secs(10)
        );
        assert_eq!(retry_delay(0, None), Duration::from_millis(250));
        assert_eq!(retry_delay(1, None), Duration::from_millis(750));
    }

    #[test]
    fn response_body_cap_accepts_exact_boundary_and_rejects_one_more_byte() {
        let mut body = vec![0; MAX_RESPONSE_BYTES - 1];
        append_chunk(&mut body, &[0]).unwrap();
        assert_eq!(body.len(), MAX_RESPONSE_BYTES);
        assert_eq!(
            append_chunk(&mut body, &[0]),
            Err(ServiceError::ResponseTooLarge)
        );
    }
}
