use std::time::Duration;

const MINIMAX_USAGE_URL: &str = "https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains";
const DEEPSEEK_USAGE_URL: &str = "https://api.deepseek.com/user/balance";
const ZHIPU_USAGE_URL: &str = "https://open.bigmodel.cn/api/monitor/usage/quota/limit";
const ZAI_USAGE_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";
const MAX_RESPONSE_BYTES: usize = 512 * 1024;

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

#[derive(Debug)]
pub(crate) enum ServiceError {
    Network,
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

fn validate_usage_response(
    provider: Provider,
    value: serde_json::Value,
) -> Result<serde_json::Value, ServiceError> {
    let is_valid = match provider {
        Provider::Minimax => value
            .get("model_remains")
            .and_then(serde_json::Value::as_array)
            .is_some(),
        Provider::Deepseek => {
            value
                .get("is_available")
                .and_then(serde_json::Value::as_bool)
                .is_some()
                && value
                    .get("balance_infos")
                    .and_then(serde_json::Value::as_array)
                    .is_some()
        }
        Provider::Zhipu => value
            .pointer("/data/limits")
            .or_else(|| value.get("limits"))
            .and_then(serde_json::Value::as_array)
            .is_some(),
    };

    is_valid
        .then_some(value)
        .ok_or(ServiceError::InvalidResponse)
}

pub(crate) async fn request_usage(
    client: &reqwest::Client,
    provider: Provider,
    key: &str,
    configured_base_url: Option<&str>,
) -> Result<serde_json::Value, ServiceError> {
    let mut request = client
        .get(usage_url(provider, configured_base_url))
        .header(reqwest::header::ACCEPT, "application/json");

    request = match provider {
        Provider::Zhipu => request
            // The official GLM Coding Plan usage plugin sends this monitor token verbatim.
            .header(reqwest::header::AUTHORIZATION, key)
            .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9"),
        Provider::Minimax => request
            .bearer_auth(key)
            .header(reqwest::header::REFERER, "https://platform.minimaxi.com/"),
        Provider::Deepseek => request.bearer_auth(key),
    };

    let mut response = request.send().await.map_err(|_| ServiceError::Network)?;
    let status = response.status();
    if !status.is_success() {
        eprintln!(
            "[{}] usage request failed with HTTP {status}",
            provider.id()
        );
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
    validate_usage_response(provider, value)
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
    fn validates_each_official_usage_response_shape() {
        assert!(validate_usage_response(
            Provider::Minimax,
            serde_json::json!({ "model_remains": [] })
        )
        .is_ok());
        assert!(validate_usage_response(
            Provider::Deepseek,
            serde_json::json!({ "is_available": true, "balance_infos": [] })
        )
        .is_ok());
        assert!(validate_usage_response(
            Provider::Zhipu,
            serde_json::json!({ "data": { "limits": [] } })
        )
        .is_ok());
        assert!(validate_usage_response(Provider::Zhipu, serde_json::json!({})).is_err());
    }

    #[test]
    fn uses_the_zai_monitor_host_for_overseas_configuration() {
        assert_eq!(
            usage_url(Provider::Zhipu, Some("https://api.z.ai/api/anthropic")),
            ZAI_USAGE_URL
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
}
