use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("上游响应体超过大小上限: {0} 字节")]
    ResponseBodyTooLarge(usize),

    #[error("服务器已在运行")]
    AlreadyRunning,

    #[error("服务器未运行")]
    NotRunning,

    #[error("地址绑定失败: {0}")]
    BindFailed(String),

    #[error("停止超时")]
    StopTimeout,

    #[error("停止失败: {0}")]
    StopFailed(String),

    #[error("请求转发失败: {0}")]
    ForwardFailed(String),

    #[error("无可用的Provider")]
    NoAvailableProvider,

    #[error("所有供应商已熔断，无可用渠道")]
    AllProvidersCircuitOpen,

    #[error("未配置供应商")]
    NoProvidersConfigured,

    #[allow(dead_code)]
    #[error("Provider不健康: {0}")]
    ProviderUnhealthy(String),

    #[error("上游错误 (状态码 {status}): {body:?}")]
    UpstreamError { status: u16, body: Option<String> },

    #[error("超过最大重试次数")]
    MaxRetriesExceeded,

    #[error("数据库错误: {0}")]
    DatabaseError(String),

    #[error("配置错误: {0}")]
    ConfigError(String),

    #[allow(dead_code)]
    #[error("格式转换错误: {0}")]
    TransformError(String),

    #[allow(dead_code)]
    #[error("无效的请求: {0}")]
    InvalidRequest(String),

    #[error("超时: {0}")]
    Timeout(String),

    /// 流式响应空闲超时
    #[allow(dead_code)]
    #[error("流式响应空闲超时: {0}秒无数据")]
    StreamIdleTimeout(u64),

    /// 认证错误
    #[error("认证失败: {0}")]
    AuthError(String),

    /// API Key 使用限额已达到：转发前被 Budget Guard 拦截。
    /// 携带结构化明细（provider/used/limit），错误体 type = cc_switch_usage_limit。
    #[error("{message}")]
    BudgetExhausted {
        message: String,
        detail: serde_json::Value,
    },

    /// The selected upstream reported a hard quota/billing limit. This is a
    /// terminal budget error, never a network failure eligible for failover.
    #[error("上游使用限额已达到")]
    UpstreamBudgetExhausted {
        status: u16,
        detail: serde_json::Value,
    },

    #[allow(dead_code)]
    #[error("内部错误: {0}")]
    Internal(String),
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        let (status, body) = match &self {
            ProxyError::UpstreamError {
                status: upstream_status,
                body: upstream_body,
            } => {
                let http_status =
                    StatusCode::from_u16(*upstream_status).unwrap_or(StatusCode::BAD_GATEWAY);

                // 尝试解析上游响应体为 JSON，如果失败则包装为字符串
                let error_body = if let Some(body_str) = upstream_body {
                    if let Ok(json_body) = serde_json::from_str::<serde_json::Value>(body_str) {
                        // 上游返回的是 JSON，直接透传
                        json_body
                    } else {
                        // 上游返回的不是 JSON，包装为错误消息
                        json!({
                            "error": {
                                "message": body_str,
                                "type": "upstream_error",
                            }
                        })
                    }
                } else {
                    json!({
                        "error": {
                            "message": format!("Upstream error (status {})", upstream_status),
                            "type": "upstream_error",
                        }
                    })
                };

                (http_status, error_body)
            }
            ProxyError::BudgetExhausted { message, detail } => {
                // 限额拒绝：429 + 结构化错误体（与既有 {"error": {...}} 约定一致），
                // 让 CLI 客户端能看到「为什么被拒」与当前用量/上限。
                let mut error_obj = serde_json::Map::new();
                error_obj.insert("message".to_string(), json!(message));
                error_obj.insert("type".to_string(), json!("cc_switch_usage_limit"));
                if let Some(detail_obj) = detail.as_object() {
                    for (key, value) in detail_obj {
                        error_obj.insert(key.clone(), value.clone());
                    }
                }
                (StatusCode::TOO_MANY_REQUESTS, json!({ "error": error_obj }))
            }
            ProxyError::UpstreamBudgetExhausted { status, detail } => {
                let mut error_obj = serde_json::Map::new();
                error_obj.insert(
                    "message".to_string(),
                    json!("Blocked by upstream usage limit"),
                );
                error_obj.insert("type".to_string(), json!("upstream_usage_limit_reached"));
                if let Some(detail_obj) = detail.as_object() {
                    for (key, value) in detail_obj {
                        error_obj.insert(key.clone(), value.clone());
                    }
                }
                error_obj.insert("upstream_status".to_string(), json!(status));
                (StatusCode::TOO_MANY_REQUESTS, json!({ "error": error_obj }))
            }
            _ => {
                let (http_status, message) = match &self {
                    ProxyError::AlreadyRunning => (StatusCode::CONFLICT, self.to_string()),
                    ProxyError::NotRunning => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
                    ProxyError::BindFailed(_) => {
                        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
                    }
                    ProxyError::StopTimeout => {
                        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
                    }
                    ProxyError::StopFailed(_) => {
                        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
                    }
                    ProxyError::ForwardFailed(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
                    ProxyError::NoAvailableProvider => {
                        (StatusCode::SERVICE_UNAVAILABLE, self.to_string())
                    }
                    ProxyError::AllProvidersCircuitOpen => {
                        (StatusCode::SERVICE_UNAVAILABLE, self.to_string())
                    }
                    ProxyError::NoProvidersConfigured => {
                        (StatusCode::SERVICE_UNAVAILABLE, self.to_string())
                    }
                    ProxyError::ProviderUnhealthy(_) => {
                        (StatusCode::SERVICE_UNAVAILABLE, self.to_string())
                    }
                    ProxyError::MaxRetriesExceeded => {
                        (StatusCode::SERVICE_UNAVAILABLE, self.to_string())
                    }
                    ProxyError::DatabaseError(_) => {
                        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
                    }
                    ProxyError::ConfigError(_) => (StatusCode::BAD_REQUEST, self.to_string()),
                    ProxyError::TransformError(_) => {
                        (StatusCode::UNPROCESSABLE_ENTITY, self.to_string())
                    }
                    ProxyError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
                    ProxyError::Timeout(_) => (StatusCode::GATEWAY_TIMEOUT, self.to_string()),
                    ProxyError::StreamIdleTimeout(_) => {
                        (StatusCode::GATEWAY_TIMEOUT, self.to_string())
                    }
                    ProxyError::AuthError(_) => (StatusCode::UNAUTHORIZED, self.to_string()),
                    ProxyError::Internal(_) => {
                        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
                    }
                    ProxyError::ResponseBodyTooLarge(_) => {
                        (StatusCode::BAD_GATEWAY, self.to_string())
                    }
                    ProxyError::UpstreamError { .. }
                    | ProxyError::BudgetExhausted { .. }
                    | ProxyError::UpstreamBudgetExhausted { .. } => {
                        unreachable!()
                    }
                };

                let error_body = json!({
                    "error": {
                        "message": message,
                        "type": "proxy_error",
                    }
                });

                (http_status, error_body)
            }
        };

        (status, Json(body)).into_response()
    }
}

/// 错误分类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// 可重试错误（网络问题、5xx）
    Retryable, // 网络超时、5xx 错误
    /// 不可重试错误（4xx、认证失败）
    NonRetryable, // 认证失败、参数错误、4xx 错误
    #[allow(dead_code)]
    ClientAbort, // 客户端主动中断
}

/// Detect a hard upstream quota/billing response without treating every 429
/// rate-limit response as a permanent budget exhaustion. The returned detail
/// is deliberately small and contains no upstream body or credential data.
pub fn upstream_budget_detail(status: u16, body: Option<&str>) -> Option<serde_json::Value> {
    let normalized = body
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace(['_', '-'], " ");
    let budget_signal = [
        "quota",
        "insufficient credits",
        "insufficient quota",
        "billing",
        "usage limit",
        "monthly limit",
        "spend limit",
        "credit limit",
        "limit reached",
        "api key limit reached",
        "cc switch usage limit",
        "quota exceeded",
    ]
    .iter()
    .any(|needle| normalized.contains(needle));
    let hard_status = status == 402 || (status == 403 && budget_signal);
    if !(hard_status || (status == 429 && budget_signal)) {
        return None;
    }

    Some(json!({
        "code": "UPSTREAM_API_KEY_LIMIT_REACHED",
        "source": "upstream",
        "budget_signal": budget_signal,
    }))
}

/// 判断错误是否可重试
#[allow(dead_code)]
pub fn categorize_error(error: &reqwest::Error) -> ErrorCategory {
    if error.is_timeout() || error.is_connect() {
        return ErrorCategory::Retryable;
    }

    if let Some(status) = error.status() {
        if status.is_server_error() {
            ErrorCategory::Retryable
        } else if status.is_client_error() {
            ErrorCategory::NonRetryable
        } else {
            ErrorCategory::Retryable
        }
    } else {
        ErrorCategory::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::{upstream_budget_detail, ErrorCategory, ProxyError};

    #[test]
    fn upstream_quota_is_detected_without_copying_body() {
        let detail = upstream_budget_detail(
            429,
            Some(r#"{"error":{"message":"monthly quota exceeded for sk-secret"}}"#),
        )
        .expect("quota response should be detected");
        assert_eq!(detail["code"], "UPSTREAM_API_KEY_LIMIT_REACHED");
        assert!(!detail.to_string().contains("sk-secret"));
    }

    #[test]
    fn ordinary_rate_limit_is_not_upstream_budget_exhaustion() {
        assert!(upstream_budget_detail(429, Some("retry after 30 seconds")).is_none());
    }

    #[test]
    fn upstream_budget_is_a_terminal_category() {
        let error = ProxyError::UpstreamBudgetExhausted {
            status: 429,
            detail: serde_json::json!({"code": "UPSTREAM_API_KEY_LIMIT_REACHED"}),
        };
        assert_eq!(
            super::ErrorCategory::NonRetryable,
            match error {
                ProxyError::UpstreamBudgetExhausted { .. } => ErrorCategory::NonRetryable,
                _ => ErrorCategory::Retryable,
            }
        );
    }
}
