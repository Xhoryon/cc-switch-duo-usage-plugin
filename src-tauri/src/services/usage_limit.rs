//! API Key 使用限额（Budget Limit）服务
//!
//! 每个 API Key 可以设置一个最大使用量（金额或 Token）。限额的判定与
//! 用量统计完全复用 Usage SSOT（`proxy_request_logs`）：金额取已落库的
//! `total_cost_usd`（十进制字符串，`rust_decimal` 精确求和），Token 取与
//! Usage Dashboard 一致的 normalized total 规则（fresh input + output +
//! cache_creation + cache_read，见 [`crate::services::sql_helpers`]）。
//!
//! 边界（V1）：
//! - 只统计 `data_source = 'proxy'` 的请求——Budget enforcement 只作用于
//!   经过 CC Switch Local Proxy 的流量，不是 Provider 账户的全局配额。
//! - credential 通过不可逆 SHA-256 指纹绑定，任何路径都不落明文 API Key。
//! - 金额比较走「REAL 快速预判 + 贴近阈值 Decimal 精确复核」两级通道，
//!   避免浮点误差在阈值附近产生 9.999999999 这类误判。

use crate::app_config::AppType;
use crate::database::{lock_conn, Database};
use crate::database::{ApiKeyLimitRow, BudgetReservationRow};
use crate::error::AppError;
use crate::provider::Provider;
use crate::proxy::providers::get_adapter;
use crate::services::usage_stats::{find_model_pricing_row, is_placeholder_pricing_model};
use rusqlite::{params, OptionalExtension};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::str::FromStr;

/// 解析应用类型字符串（AppType 无 FromStr，这里按 as_str 匹配）
fn parse_app_type(value: &str) -> Option<AppType> {
    AppType::all().find(|t| t.as_str() == value)
}

/// 限额模式：同一时间只允许一种
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitType {
    Money,
    Token,
}

impl LimitType {
    pub fn as_str(self) -> &'static str {
        match self {
            LimitType::Money => "money",
            LimitType::Token => "token",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "money" => Some(LimitType::Money),
            "token" => Some(LimitType::Token),
            _ => None,
        }
    }
}

/// 金额模式下的币种。数据库内部成本统一以 USD 计（`total_cost_usd`），
/// CNY 限额通过本地可配置汇率换算，不引入在线汇率依赖。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitCurrency {
    Usd,
    Cny,
}

impl LimitCurrency {
    pub fn as_str(self) -> &'static str {
        match self {
            LimitCurrency::Usd => "USD",
            LimitCurrency::Cny => "CNY",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "USD" => Some(LimitCurrency::Usd),
            "CNY" => Some(LimitCurrency::Cny),
            _ => None,
        }
    }

    /// 展示符号（错误消息 / 日志用）
    pub fn symbol(self) -> &'static str {
        match self {
            LimitCurrency::Usd => "$",
            LimitCurrency::Cny => "¥",
        }
    }
}

/// USD → CNY 汇率的 settings 存储键
pub const USD_CNY_RATE_SETTING_KEY: &str = "usage_limit_usd_cny_rate";

/// USD → CNY 默认汇率（本地可配置，不联网获取）
pub const DEFAULT_USD_CNY_RATE: &str = "7.2";

/// WARNING 状态阈值（>= 80% 且 < 100%）
pub const WARNING_THRESHOLD_PERCENT: Decimal = Decimal::from_parts(80, 0, 0, false, 0);

/// REAL 快速通道的安全边距：低于 limit - margin 时确定放行。
/// f64 求和的相对误差量级 ~1e-12，1e-6 的边距覆盖 6 个数量级余量。
const MONEY_FAST_PASS_MARGIN_F64: f64 = 1e-6;

/// 请求被预算拒绝时携带给代理错误响应的载荷
#[derive(Debug, Clone)]
pub struct BudgetRejection {
    /// 人类可读的摘要（进 error.message）
    pub message: String,
    /// 结构化字段（进 error 对象），值均为字符串
    pub detail: serde_json::Value,
}

/// 前端提交的限额配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimitConfig {
    pub enabled: bool,
    /// "money" | "token"
    pub limit_type: String,
    /// "USD" | "CNY"；token 模式下为 None
    pub currency: Option<String>,
    /// 金额（> 0 的十进制字符串）或 token 数（正整数纯数字字符串）
    pub limit_amount: Option<String>,
    /// 自动重置周期（秒）；None 表示永不自动重置
    #[serde(default)]
    pub reset_interval_seconds: Option<i64>,
}

/// enforcement 能力：Proxy 是否真的在该 Provider 的请求路径上
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementSupport {
    /// Proxy 已对该应用启用，请求会经过 Local Proxy，可以强制执行
    Active,
    /// 该应用的 Proxy 接管未开启：流量不经过 CC Switch，无法强制执行
    ProxyDisabled,
    /// 该 Provider 没有可识别的静态 API Key（OAuth 类），无法按 credential 限额
    UnsupportedCredential,
}

impl EnforcementSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            EnforcementSupport::Active => "active",
            EnforcementSupport::ProxyDisabled => "proxy_disabled",
            EnforcementSupport::UnsupportedCredential => "unsupported_credential",
        }
    }
}

/// 预算状态（供前端 Provider Card 图标与 Dialog 使用）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetStatus {
    pub provider_id: String,
    pub app_type: String,
    pub enabled: bool,
    pub limit_type: Option<String>,
    pub currency: Option<String>,
    pub limit_amount: Option<String>,
    pub usage_start_at: Option<i64>,
    /// 自动重置周期（秒）；None 表示永不自动重置
    pub reset_interval_seconds: Option<i64>,
    /// 窗口内已用金额（USD 原值，十进制字符串）
    pub used_money_usd: String,
    /// 限额币种下的已用金额（CNY 限额时已按汇率换算）
    pub used_money_in_currency: Option<String>,
    /// 窗口内已用 token（与 Dashboard 一致的 normalized total）
    pub used_tokens: i64,
    /// 使用百分比（不 clamp，可超过 100）；未启用限额时为 None
    pub percent_used: Option<String>,
    /// "off" | "active" | "warning" | "exhausted"
    pub state: String,
    /// 窗口内「有 token 用量但当前无定价」的请求数（金额模式可靠性提示）
    pub unpriced_request_count: i64,
    /// 上述请求涉及的模型列表（引导配置 custom pricing）
    pub unpriced_models: Vec<String>,
    pub enforcement: EnforcementSupport,
    /// 当前 credential 指纹对应的遮蔽展示（如 sk-****ABCD），无静态 key 时为 None
    pub masked_credential: Option<String>,
}

/// A request-scoped local reservation. The row is kept opaque to the
/// transport layer and contains no plaintext credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetReservation {
    pub request_id: String,
    pub provider_id: String,
    pub app_type: String,
    pub credential_fingerprint: String,
    pub limit_type: LimitType,
    pub reserved_tokens: i64,
    pub reserved_cost_usd: String,
    pub expires_at: i64,
}

const BUDGET_RESERVATION_TTL_SECONDS: i64 = 300;

/// 自动重置周期的最小粒度为 1 小时，前端按小时/天换算后传入。
pub const MIN_RESET_INTERVAL_SECONDS: i64 = 60 * 60;

/// credential 解析结果：代理与命令层共用的身份识别入口
#[derive(Debug, Clone, PartialEq)]
pub enum CredentialResolution {
    /// 有静态 API Key：携带不可逆指纹与遮蔽展示
    StaticKey {
        fingerprint: String,
        masked_key: String,
    },
    /// 无静态 key（OAuth / 官方登录），按 credential 限额不可用
    Unsupported,
}

/// 返回跨设备稳定的 credential namespace。
///
/// 应用类型对应稳定的协议/认证族；不能使用本地 provider ID，因为同一条
/// Switch 链上的两个节点通常会为同一上游保存不同的本地 provider ID。
pub fn credential_namespace(app_type: &AppType) -> &str {
    app_type.as_str()
}

/// 计算 API Key 的不可逆 SHA-256 指纹（小写十六进制）。
///
/// namespace 与 key 之间使用明确的 NUL 分隔，避免拼接歧义。API key 保持
/// 当前 credential storage 的大小写，不做 lower/upper 转换。
pub fn credential_fingerprint(namespace: &str, api_key: &str) -> String {
    let mut input = Vec::with_capacity(namespace.len() + 1 + api_key.len());
    input.extend_from_slice(namespace.as_bytes());
    input.push(0);
    input.extend_from_slice(api_key.as_bytes());
    let digest = Sha256::digest(input);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// 遮蔽 API Key 用于日志/展示：保留前 3 位与末 4 位，如 `sk-****ABCD`。
/// 不足以安全遮蔽的短 key 一律 `****`。
pub fn mask_credential(api_key: &str) -> String {
    let chars: Vec<char> = api_key.chars().collect();
    if chars.len() <= 8 {
        return "****".to_string();
    }
    let prefix: String = chars.iter().take(3).collect();
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    format!("{prefix}****{suffix}")
}

/// 解析 Provider 当前 credential 的指纹身份。
///
/// 复用代理转发同款的 adapter `extract_auth` 逻辑，保证「存进限额表的
/// 指纹」与「转发时实际使用 Key 的指纹」来自同一解析路径。OAuth 类
/// Provider（managed account / 动态 token）没有稳定的静态 Key，返回
/// [`CredentialResolution::Unsupported`]。
pub fn resolve_credential(provider: &Provider, app_type: &AppType) -> CredentialResolution {
    // OAuth 类 provider 的 auth token 由代理动态注入，不是稳定静态 key
    if provider.uses_managed_account_auth()
        || provider.uses_proxy_injected_oauth()
        || provider.is_codex_oauth()
        || provider.is_xai_oauth()
    {
        return CredentialResolution::Unsupported;
    }

    let Some(adapter) = get_adapter(app_type) else {
        return CredentialResolution::Unsupported;
    };
    let Some(auth) = adapter.extract_auth(provider) else {
        return CredentialResolution::Unsupported;
    };
    let key = auth.api_key.trim();
    if key.is_empty() {
        return CredentialResolution::Unsupported;
    }
    CredentialResolution::StaticKey {
        fingerprint: credential_fingerprint(credential_namespace(app_type), key),
        masked_key: mask_credential(key),
    }
}

/// 读取本地 USD → CNY 汇率（未配置时用默认值；非法存量值回退默认并告警）
impl Database {
    pub fn get_usd_cny_exchange_rate(&self) -> Result<Decimal, AppError> {
        let raw = self.get_setting(USD_CNY_RATE_SETTING_KEY)?;
        parse_exchange_rate(raw.as_deref())
    }

    /// 保存本地 USD → CNY 汇率（必须 > 0）
    pub fn set_usd_cny_exchange_rate(&self, rate: Decimal) -> Result<(), AppError> {
        if rate <= Decimal::ZERO {
            return Err(AppError::InvalidInput(
                "USD → CNY 汇率必须大于 0".to_string(),
            ));
        }
        if rate > Decimal::from(1_000_000) {
            return Err(AppError::InvalidInput(
                "USD → CNY 汇率数值异常，请检查输入".to_string(),
            ));
        }
        self.set_setting(USD_CNY_RATE_SETTING_KEY, &rate.to_string())
    }
}

/// 解析汇率字符串；None / 空 / 非法值回退默认（不因存量脏数据拒绝服务）
fn parse_exchange_rate(raw: Option<&str>) -> Result<Decimal, AppError> {
    match raw {
        None | Some("") => Ok(Decimal::from_str(DEFAULT_USD_CNY_RATE)
            .map_err(|e| AppError::Database(format!("默认汇率解析失败: {e}")))?),
        Some(value) => match Decimal::from_str(value.trim()) {
            Ok(rate) if rate > Decimal::ZERO => Ok(rate),
            _ => {
                log::warn!("汇率设置非法（{value}），回退默认值 {DEFAULT_USD_CNY_RATE}");
                Decimal::from_str(DEFAULT_USD_CNY_RATE)
                    .map_err(|e| AppError::Database(format!("默认汇率解析失败: {e}")))
            }
        },
    }
}

/// 校验并解析用户提交的限额金额
///
/// - money：> 0 的十进制数（拒绝 0 / 负数 / 非法 / 科学计数法以外的脏输入）
/// - token：正整数（拒绝小数 / 0 / 负数 / 非法）
pub fn validate_limit_amount(limit_type: LimitType, raw_amount: &str) -> Result<String, AppError> {
    let trimmed = raw_amount.trim();
    match limit_type {
        LimitType::Money => {
            if trimmed.is_empty() {
                return Err(AppError::InvalidInput("限额金额不能为空".to_string()));
            }
            let amount = Decimal::from_str(trimmed)
                .map_err(|_| AppError::InvalidInput(format!("限额金额格式无效: {trimmed}")))?;
            if amount <= Decimal::ZERO {
                return Err(AppError::InvalidInput("限额金额必须大于 0".to_string()));
            }
            // 统一存十进制规范化字符串，避免 "100.00" / "1E2" 这类同值异形
            Ok(amount.normalize().to_string())
        }
        LimitType::Token => {
            if trimmed.is_empty() {
                return Err(AppError::InvalidInput("限额 Token 数不能为空".to_string()));
            }
            // 只接受纯数字正整数：显式拒绝小数、负号、+、空格分节、科学计数法
            if !trimmed.bytes().all(|b| b.is_ascii_digit()) {
                return Err(AppError::InvalidInput(
                    "限额 Token 数必须是正整数".to_string(),
                ));
            }
            let tokens: u64 = trimmed
                .parse()
                .map_err(|_| AppError::InvalidInput("限额 Token 数超出可表示范围".to_string()))?;
            if tokens == 0 {
                return Err(AppError::InvalidInput(
                    "限额 Token 数必须大于 0".to_string(),
                ));
            }
            Ok(tokens.to_string())
        }
    }
}

/// 校验用户提交的自动重置周期。
///
/// `None` 表示永不重置；有值时必须是至少 1 小时且按整小时设置，
/// 这样小时/天配置可以在前后端之间无损往返，也避免产生不可展示的分钟级窗口。
pub fn validate_reset_interval_seconds(raw_interval: Option<i64>) -> Result<Option<i64>, AppError> {
    let Some(seconds) = raw_interval else {
        return Ok(None);
    };
    if seconds < MIN_RESET_INTERVAL_SECONDS || seconds % MIN_RESET_INTERVAL_SECONDS != 0 {
        return Err(AppError::InvalidInput(
            "自动重置周期必须是至少 1 小时的整小时数".to_string(),
        ));
    }
    Ok(Some(seconds))
}

impl Database {
    /// 金额精确求和：取窗口内原始 `total_cost_usd` 十进制字符串，
    /// 用 `rust_decimal` 逐行累加（贴近阈值 / 状态展示走这条精确路径）。
    /// Token 数不在此处理——调用方从 [`Database::sum_credential_usage_fast`]
    /// 的整数和取值，SQLite 整数求和本身无浮点误差。
    fn sum_credential_costs_exact(
        &self,
        provider_id: &str,
        app_type: &str,
        fingerprint: &str,
        since: i64,
    ) -> Result<Decimal, AppError> {
        let costs =
            self.fetch_credential_cost_strings(provider_id, app_type, fingerprint, since)?;
        let mut total = Decimal::ZERO;
        for cost in &costs {
            let parsed = Decimal::from_str(cost.trim()).unwrap_or_else(|_| {
                log::warn!("预算统计遇到非十进制成本行（按 0 计）: {cost}");
                Decimal::ZERO
            });
            total += parsed;
        }
        Ok(total)
    }

    /// Proxy 转发前的预算闸门。
    ///
    /// - 未启用限额 → 放行（零开销路径是一次 PK 查询）
    /// - credential 指纹与配置不一致（用户换了 Key）→ 自动重绑并把统计
    ///   窗口重置为当前时间，旧 Key 的用量不计入新 Key
    /// - 达到/超过限额 → 返回 [`BudgetRejection`]，调用方必须拒绝转发
    ///
    /// 并发语义：判定读与用量写都在 DB Mutex 下串行；真实用量在响应结束
    /// 后才落库，同时进行的多个请求仍可能产生少量 overshoot，属于本功能
    /// 的设计内行为（不为理论零 overshoot 破坏转发并发）。
    pub fn check_budget_before_forward(
        &self,
        provider_id: &str,
        provider_name: &str,
        app_type: &str,
        fingerprint: &str,
    ) -> Result<(), BudgetRejection> {
        let mut row = match self.get_api_key_limit(provider_id, app_type) {
            Ok(Some(row)) => row,
            Ok(None) => return Ok(()),
            Err(e) => {
                // 判定失败不能阻断所有请求：记录告警后放行，与熔断器降级同思路
                log::warn!("[Budget] 读取限额配置失败，请求放行: {e}");
                return Ok(());
            }
        };
        if !row.enabled {
            return Ok(());
        }

        // Credential 轮换：重绑到新指纹并重置统计窗口（旧 Key 用量不继承）
        let now = chrono::Utc::now().timestamp();
        if row.credential_fingerprint != fingerprint {
            match self.rebind_api_key_limit_credential(provider_id, app_type, fingerprint, now) {
                Ok(true) => {
                    log::info!(
                        "[Budget] Provider {provider_name} ({app_type}) credential 已变更，限额绑定至新指纹，统计窗口重置"
                    );
                    if let Ok(Some(current)) = self.get_api_key_limit(provider_id, app_type) {
                        row = current;
                    }
                }
                Ok(false) => {}
                Err(e) => {
                    log::warn!("[Budget] 重绑限额凭证失败，按原指纹继续判定: {e}");
                }
            }
        }

        if self
            .roll_expired_api_key_limit_window(provider_id, app_type, now)
            .unwrap_or(false)
        {
            if let Ok(Some(current)) = self.get_api_key_limit(provider_id, app_type) {
                row = current;
            }
        }

        let limit = match self.evaluate_budget(&row, fingerprint) {
            Ok(evaluated) => evaluated,
            Err(e) => {
                log::warn!("[Budget] 计算用量失败，请求放行: {e}");
                return Ok(());
            }
        };

        if !limit.exhausted {
            return Ok(());
        }

        Err(limit.build_rejection(provider_name))
    }

    /// Atomically reserve room for a request before forwarding it.
    ///
    /// `None` means no enabled local limit exists. A `Some` reservation is
    /// counted together with committed proxy usage for subsequent requests.
    /// The DAO performs the final usage + pending check while holding the same
    /// SQLite mutex, so two concurrent requests cannot both reserve the same
    /// remaining budget.
    pub fn reserve_budget_for_request(
        &self,
        request_id: &str,
        provider_id: &str,
        app_type: &str,
        fingerprint: &str,
        request: &Value,
    ) -> Result<Option<BudgetReservation>, BudgetRejection> {
        let mut row = match self.get_api_key_limit(provider_id, app_type) {
            Ok(Some(row)) => row,
            Ok(None) => return Ok(None),
            Err(e) => {
                return Err(BudgetRejection {
                    message: format!("无法读取本地预算配置: {e}"),
                    detail: serde_json::json!({"code": "BUDGET_STORAGE_ERROR"}),
                })
            }
        };
        if !row.enabled {
            return Ok(None);
        }

        let now = chrono::Utc::now().timestamp();
        if row.credential_fingerprint != fingerprint {
            self.rebind_api_key_limit_credential(provider_id, app_type, fingerprint, now)
                .map_err(|e| BudgetRejection {
                    message: format!("无法重绑 API Key 使用限额: {e}"),
                    detail: serde_json::json!({"code": "BUDGET_STORAGE_ERROR"}),
                })?;
        }

        self.roll_expired_api_key_limit_window(provider_id, app_type, now)
            .map_err(|e| BudgetRejection {
                message: format!("无法自动重置预算窗口: {e}"),
                detail: serde_json::json!({"code": "BUDGET_STORAGE_ERROR"}),
            })?;
        row = self
            .get_api_key_limit(provider_id, app_type)
            .map_err(|e| BudgetRejection {
                message: format!("无法读取自动重置后的预算配置: {e}"),
                detail: serde_json::json!({"code": "BUDGET_STORAGE_ERROR"}),
            })?
            .ok_or_else(|| BudgetRejection {
                message: "预算配置在自动重置过程中消失".to_string(),
                detail: serde_json::json!({"code": "BUDGET_STORAGE_ERROR"}),
            })?;

        let _ = self.cleanup_stale_budget_reservations(now);
        let limit_type = LimitType::parse(&row.limit_type).ok_or_else(|| BudgetRejection {
            message: format!("限额类型非法: {}", row.limit_type),
            detail: serde_json::json!({"code": "INVALID_BUDGET_CONFIG"}),
        })?;

        let (reserved_tokens, reserved_cost_usd, token_limit, cost_limit_usd) = match limit_type {
            LimitType::Token => {
                let limit_tokens =
                    row.limit_amount
                        .parse::<i64>()
                        .map_err(|_| BudgetRejection {
                            message: format!("限额 Token 数非法: {}", row.limit_amount),
                            detail: serde_json::json!({"code": "INVALID_BUDGET_CONFIG"}),
                        })?;
                let used = self
                    .sum_credential_usage_fast(
                        provider_id,
                        app_type,
                        fingerprint,
                        row.usage_start_at,
                    )
                    .map_err(|e| BudgetRejection {
                        message: format!("无法计算预算用量: {e}"),
                        detail: serde_json::json!({"code": "BUDGET_STORAGE_ERROR"}),
                    })?
                    .normalized_total_tokens();
                let pending = self
                    .sum_pending_budget_reservations(provider_id, app_type, fingerprint, now)
                    .map_err(|e| BudgetRejection {
                        message: format!("无法计算预算 reservation: {e}"),
                        detail: serde_json::json!({"code": "BUDGET_STORAGE_ERROR"}),
                    })?
                    .tokens;
                let remaining = limit_tokens.saturating_sub(used).saturating_sub(pending);
                if remaining <= 0 {
                    return Err(build_token_rejection(
                        used.saturating_add(pending),
                        limit_tokens,
                    ));
                }
                let requested = requested_token_reservation(request, remaining);
                if requested > remaining {
                    return Err(build_token_rejection(
                        used.saturating_add(pending),
                        limit_tokens,
                    ));
                }
                (requested, "0".to_string(), Some(limit_tokens), None)
            }
            LimitType::Money => {
                let currency = row
                    .currency
                    .as_deref()
                    .and_then(LimitCurrency::parse)
                    .unwrap_or(LimitCurrency::Usd);
                let limit_amount =
                    Decimal::from_str(row.limit_amount.trim()).map_err(|_| BudgetRejection {
                        message: format!("限额金额非法: {}", row.limit_amount),
                        detail: serde_json::json!({"code": "INVALID_BUDGET_CONFIG"}),
                    })?;
                let rate = self
                    .get_usd_cny_exchange_rate()
                    .map_err(|e| BudgetRejection {
                        message: format!("无法读取 USD/CNY 汇率: {e}"),
                        detail: serde_json::json!({"code": "BUDGET_STORAGE_ERROR"}),
                    })?;
                let limit_usd = match currency {
                    LimitCurrency::Usd => limit_amount,
                    LimitCurrency::Cny => limit_amount / rate,
                };
                let used_usd = self
                    .sum_credential_costs_exact(
                        provider_id,
                        app_type,
                        fingerprint,
                        row.usage_start_at,
                    )
                    .map_err(|e| BudgetRejection {
                        message: format!("无法计算预算用量: {e}"),
                        detail: serde_json::json!({"code": "BUDGET_STORAGE_ERROR"}),
                    })?;
                let pending = self
                    .sum_pending_budget_reservations(provider_id, app_type, fingerprint, now)
                    .map_err(|e| BudgetRejection {
                        message: format!("无法计算预算 reservation: {e}"),
                        detail: serde_json::json!({"code": "BUDGET_STORAGE_ERROR"}),
                    })?;
                let pending_usd = Decimal::from_str(&pending.cost_usd).unwrap_or(Decimal::ZERO);
                let remaining_usd = limit_usd - used_usd - pending_usd;
                if remaining_usd <= Decimal::ZERO {
                    return Err(build_money_rejection(
                        used_usd + pending_usd,
                        limit_amount,
                        currency,
                    ));
                }
                // Until a provider returns authoritative usage, reserve the
                // full remaining USD budget. Reconciliation releases unused
                // room and prevents concurrent unknown-output requests from
                // bypassing the hard limit.
                (
                    0,
                    remaining_usd.normalize().to_string(),
                    None,
                    Some(limit_usd),
                )
            }
        };

        let expires_at = now.saturating_add(BUDGET_RESERVATION_TTL_SECONDS);
        let reservation_row = BudgetReservationRow {
            request_id: request_id.to_string(),
            provider_id: provider_id.to_string(),
            app_type: app_type.to_string(),
            credential_fingerprint: fingerprint.to_string(),
            limit_type: limit_type.as_str().to_string(),
            reserved_tokens,
            reserved_cost_usd: reserved_cost_usd.clone(),
            consumed_tokens: 0,
            consumed_cost_usd: "0".to_string(),
            status: "pending".to_string(),
            created_at: now,
            updated_at: now,
            expires_at,
        };
        let inserted = self
            .try_insert_pending_budget_reservation(
                &reservation_row,
                row.usage_start_at,
                token_limit,
                cost_limit_usd.as_ref(),
                now,
            )
            .map_err(|e| BudgetRejection {
                message: format!("无法建立预算 reservation: {e}"),
                detail: serde_json::json!({"code": "BUDGET_STORAGE_ERROR"}),
            })?;
        if !inserted {
            return Err(BudgetRejection {
                message: "并发请求已占用当前 API Key 的剩余额度".to_string(),
                detail: serde_json::json!({"code": "API_KEY_LIMIT_REACHED"}),
            });
        }
        Ok(Some(BudgetReservation {
            request_id: reservation_row.request_id,
            provider_id: reservation_row.provider_id,
            app_type: reservation_row.app_type,
            credential_fingerprint: reservation_row.credential_fingerprint,
            limit_type,
            reserved_tokens: reservation_row.reserved_tokens,
            reserved_cost_usd: reservation_row.reserved_cost_usd,
            expires_at: reservation_row.expires_at,
        }))
    }

    pub fn release_budget_reservation(&self, request_id: &str) -> Result<(), AppError> {
        self.release_budget_reservation_row(request_id, chrono::Utc::now().timestamp())
    }

    pub fn reconcile_budget_reservation(
        &self,
        request_id: &str,
        consumed_tokens: i64,
        consumed_cost_usd: &str,
    ) -> Result<(), AppError> {
        self.reconcile_budget_reservation_row(
            request_id,
            consumed_tokens,
            consumed_cost_usd,
            chrono::Utc::now().timestamp(),
        )
    }

    /// 按配置计算当前用量并给出是否达到限额
    fn evaluate_budget(
        &self,
        row: &ApiKeyLimitRow,
        fingerprint: &str,
    ) -> Result<EvaluatedBudget, AppError> {
        let fast = self.sum_credential_usage_fast(
            &row.provider_id,
            &row.app_type,
            fingerprint,
            row.usage_start_at,
        )?;
        let limit_type = LimitType::parse(&row.limit_type)
            .ok_or_else(|| AppError::Database(format!("限额类型非法: {}", row.limit_type)))?;

        match limit_type {
            LimitType::Token => {
                let limit_tokens: i64 = row.limit_amount.parse().map_err(|_| {
                    AppError::Database(format!("限额 Token 数非法: {}", row.limit_amount))
                })?;
                let used = fast.normalized_total_tokens();
                Ok(EvaluatedBudget {
                    limit_type,
                    currency: None,
                    limit_display: format_tokens_for_message(limit_tokens),
                    used_display: format_tokens_for_message(used),
                    exhausted: used >= limit_tokens,
                })
            }
            LimitType::Money => {
                let currency = row
                    .currency
                    .as_deref()
                    .and_then(LimitCurrency::parse)
                    .unwrap_or(LimitCurrency::Usd);
                let limit_amount = Decimal::from_str(row.limit_amount.trim()).map_err(|_| {
                    AppError::Database(format!("限额金额非法: {}", row.limit_amount))
                })?;
                let limit_in_currency = match currency {
                    LimitCurrency::Usd => limit_amount,
                    LimitCurrency::Cny => limit_amount / self.get_usd_cny_exchange_rate()?,
                };

                // 快速通道：REAL 和明显低于阈值时直接放行（省去逐行 Decimal 求和）
                let limit_f64 = limit_in_currency.to_f64().unwrap_or(f64::INFINITY);
                if fast.total_cost_usd_real < limit_f64 - MONEY_FAST_PASS_MARGIN_F64 {
                    return Ok(EvaluatedBudget {
                        limit_type,
                        currency: Some(currency),
                        limit_display: format_money_for_message(limit_amount, currency),
                        used_display: format_money_for_message(
                            Decimal::from_f64_retain(fast.total_cost_usd_real)
                                .unwrap_or(Decimal::ZERO),
                            currency,
                        ),
                        exhausted: false,
                    });
                }

                // 贴近/超过阈值：逐行 Decimal 精确复核
                let used_money_usd = self.sum_credential_costs_exact(
                    &row.provider_id,
                    &row.app_type,
                    fingerprint,
                    row.usage_start_at,
                )?;
                let used_in_currency = match currency {
                    LimitCurrency::Usd => used_money_usd,
                    LimitCurrency::Cny => used_money_usd * self.get_usd_cny_exchange_rate()?,
                };
                Ok(EvaluatedBudget {
                    limit_type,
                    currency: Some(currency),
                    limit_display: format_money_for_message(limit_amount, currency),
                    used_display: format_money_for_message(used_in_currency, currency),
                    exhausted: used_in_currency >= limit_in_currency,
                })
            }
        }
    }

    /// 前端状态查询：卡片图标与 Dialog 共用
    pub fn get_budget_status(
        &self,
        provider_id: &str,
        app_type: &str,
    ) -> Result<BudgetStatus, AppError> {
        let app_type_enum = parse_app_type(app_type)
            .ok_or_else(|| AppError::InvalidInput(format!("未知应用类型: {app_type}")))?;
        let provider = self
            .get_provider_by_id(provider_id, app_type)?
            .ok_or_else(|| AppError::InvalidInput(format!("Provider 不存在: {provider_id}")))?;

        let resolution = resolve_credential(&provider, &app_type_enum);
        let enforcement = match &resolution {
            CredentialResolution::Unsupported => EnforcementSupport::UnsupportedCredential,
            CredentialResolution::StaticKey { .. } => {
                let proxy_enabled = self.is_proxy_enabled_for_app(app_type).unwrap_or(false);
                if proxy_enabled {
                    EnforcementSupport::Active
                } else {
                    EnforcementSupport::ProxyDisabled
                }
            }
        };

        let now = chrono::Utc::now().timestamp();
        if let Err(error) = self.roll_expired_api_key_limit_window(provider_id, app_type, now) {
            log::warn!("[Budget] 自动重置限额窗口失败，继续读取当前窗口: {error}");
        }
        let row = self.get_api_key_limit(provider_id, app_type)?;
        let (fingerprint, masked_credential) = match &resolution {
            CredentialResolution::StaticKey {
                fingerprint,
                masked_key,
            } => (Some(fingerprint.clone()), Some(masked_key.clone())),
            CredentialResolution::Unsupported => (None, None),
        };

        let mut status = BudgetStatus {
            provider_id: provider_id.to_string(),
            app_type: app_type.to_string(),
            enabled: false,
            limit_type: None,
            currency: None,
            limit_amount: None,
            usage_start_at: None,
            reset_interval_seconds: None,
            used_money_usd: "0".to_string(),
            used_money_in_currency: None,
            used_tokens: 0,
            percent_used: None,
            state: "off".to_string(),
            unpriced_request_count: 0,
            unpriced_models: Vec::new(),
            enforcement,
            masked_credential,
        };

        let Some(row) = row else {
            return Ok(status);
        };

        status.enabled = row.enabled;
        status.limit_type = Some(row.limit_type.clone());
        status.currency = row.currency.clone();
        status.limit_amount = Some(row.limit_amount.clone());
        status.usage_start_at = Some(row.usage_start_at);
        status.reset_interval_seconds = row.reset_interval_seconds;

        let Some(fingerprint) = fingerprint else {
            // OAuth provider：理论上不会有配置行，防御性直接返回
            return Ok(status);
        };

        // 指纹不一致 → 用户已换 Key：用量按「当前 credential」统计（新 Key 从 0
        // 开始，旧 Key 的历史不并入）；配置行的重绑由转发 guard / 保存动作完成，
        // 读接口保持无副作用。
        let rotated = row.credential_fingerprint != fingerprint;
        let usage_fingerprint = if rotated {
            &fingerprint
        } else {
            &row.credential_fingerprint
        };
        let fast = self.sum_credential_usage_fast(
            provider_id,
            app_type,
            usage_fingerprint,
            row.usage_start_at,
        )?;
        let used_money_usd = self.sum_credential_costs_exact(
            provider_id,
            app_type,
            usage_fingerprint,
            row.usage_start_at,
        )?;

        // 未知定价检测：有 token 用量但成本为 0 的请求，逐个计价模型核对
        let zero_cost_models = self.list_zero_cost_pricing_models(
            provider_id,
            app_type,
            usage_fingerprint,
            row.usage_start_at,
        )?;
        {
            let conn = lock_conn!(self.conn);
            for (model, count) in zero_cost_models {
                let priced = find_model_pricing_row(&conn, &model)?.is_some();
                if !priced && !is_placeholder_pricing_model(&model) {
                    status.unpriced_request_count += count;
                    status.unpriced_models.push(model);
                }
            }
        }

        status.used_money_usd = used_money_usd.to_string();
        status.used_tokens = fast.normalized_total_tokens();

        if !row.enabled {
            status.state = "off".to_string();
            return Ok(status);
        }

        let limit_type = LimitType::parse(&row.limit_type)
            .ok_or_else(|| AppError::Database(format!("限额类型非法: {}", row.limit_type)))?;
        let (percent, exhausted) = match limit_type {
            LimitType::Token => {
                let limit: i64 = row.limit_amount.parse().map_err(|_| {
                    AppError::Database(format!("限额 Token 数非法: {}", row.limit_amount))
                })?;
                let percent = if limit > 0 {
                    Decimal::from(fast.normalized_total_tokens()) * Decimal::from(100)
                        / Decimal::from(limit)
                } else {
                    Decimal::ZERO
                };
                (percent, fast.normalized_total_tokens() >= limit)
            }
            LimitType::Money => {
                let currency = row
                    .currency
                    .as_deref()
                    .and_then(LimitCurrency::parse)
                    .unwrap_or(LimitCurrency::Usd);
                let limit_amount = Decimal::from_str(row.limit_amount.trim()).map_err(|_| {
                    AppError::Database(format!("限额金额非法: {}", row.limit_amount))
                })?;
                let used_in_currency = match currency {
                    LimitCurrency::Usd => used_money_usd,
                    LimitCurrency::Cny => used_money_usd * self.get_usd_cny_exchange_rate()?,
                };
                status.used_money_in_currency = Some(used_in_currency.to_string());
                let percent = if limit_amount > Decimal::ZERO {
                    used_in_currency * Decimal::from(100) / limit_amount
                } else {
                    Decimal::ZERO
                };
                (percent, used_in_currency >= limit_amount)
            }
        };

        status.percent_used = Some(percent.round_dp(2).to_string());
        status.state = if exhausted || percent >= Decimal::from(100) {
            "exhausted".to_string()
        } else if percent >= WARNING_THRESHOLD_PERCENT {
            "warning".to_string()
        } else {
            "active".to_string()
        };
        let _ = rotated; // 仅作展示语义，状态按当前 credential 实测返回
        Ok(status)
    }

    /// 保存限额配置（新建 / 修改 / 启停共用）。
    ///
    /// - 校验金额/Token 输入，非法数据不进 SQLite
    /// - 保存时以「当前 credential 指纹」落库；指纹变化视为换 Key，
    ///   统计窗口重置为当前时间（与转发路径 guard 的重绑语义一致）
    /// - 关闭（enabled=false）只改开关，配置与历史窗口全部保留
    /// - 首次创建时 usage_start_at = now；再次打开沿用原窗口（可用
    ///   reset_budget_usage 显式重置）
    pub fn save_budget_config(
        &self,
        provider_id: &str,
        app_type: &str,
        config: &UsageLimitConfig,
    ) -> Result<BudgetStatus, AppError> {
        let app_type_enum = parse_app_type(app_type)
            .ok_or_else(|| AppError::InvalidInput(format!("未知应用类型: {app_type}")))?;
        let provider = self
            .get_provider_by_id(provider_id, app_type)?
            .ok_or_else(|| AppError::InvalidInput(format!("Provider 不存在: {provider_id}")))?;

        let fingerprint = match resolve_credential(&provider, &app_type_enum) {
            CredentialResolution::StaticKey { fingerprint, .. } => fingerprint,
            CredentialResolution::Unsupported => {
                return Err(AppError::InvalidInput(
                    "该 Provider 没有可识别的静态 API Key，无法设置使用限额".to_string(),
                ));
            }
        };

        let limit_type = LimitType::parse(&config.limit_type).ok_or_else(|| {
            AppError::InvalidInput(format!("限额类型非法: {}", config.limit_type))
        })?;
        let currency = match limit_type {
            LimitType::Token => {
                if config.currency.is_some() {
                    return Err(AppError::InvalidInput("Token 限额不需要币种".to_string()));
                }
                None
            }
            LimitType::Money => {
                let currency = config
                    .currency
                    .as_deref()
                    .and_then(LimitCurrency::parse)
                    .ok_or_else(|| {
                        AppError::InvalidInput("金额限额必须提供币种（USD 或 CNY）".to_string())
                    })?;
                Some(currency)
            }
        };
        let raw_amount = config
            .limit_amount
            .as_deref()
            .ok_or_else(|| AppError::InvalidInput("缺少限额数值".to_string()))?;
        let limit_amount = validate_limit_amount(limit_type, raw_amount)?;
        let reset_interval_seconds =
            validate_reset_interval_seconds(config.reset_interval_seconds)?;

        let now = chrono::Utc::now().timestamp();
        let existing = self.get_api_key_limit(provider_id, app_type)?;

        let (usage_start_at, created_at) = match &existing {
            None => (now, now),
            Some(existing) => {
                // 换 Key：窗口重置；否则沿用原窗口（关→开不丢历史进度）
                let rotated = existing.credential_fingerprint != fingerprint;
                let start = if rotated {
                    now
                } else {
                    existing.usage_start_at
                };
                (start, existing.created_at)
            }
        };

        self.upsert_api_key_limit(&ApiKeyLimitRow {
            provider_id: provider_id.to_string(),
            app_type: app_type.to_string(),
            credential_fingerprint: fingerprint,
            enabled: config.enabled,
            limit_type: limit_type.as_str().to_string(),
            currency: currency.map(|c| c.as_str().to_string()),
            limit_amount,
            usage_start_at,
            reset_interval_seconds,
            created_at,
            updated_at: now,
        })?;

        self.get_budget_status(provider_id, app_type)
    }

    /// 重置使用量：usage_start_at 推进到当前时间，不删除任何历史 Usage
    pub fn reset_budget_usage(&self, provider_id: &str, app_type: &str) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();
        self.reset_api_key_limit_window(provider_id, app_type, now)
    }

    /// 读取某应用的 Proxy 接管开关（同步路径用；DAO 的 get_proxy_config_for_app
    /// 是 async 签名，这里只需要 enabled 一个布尔量）。
    fn is_proxy_enabled_for_app(&self, app_type: &str) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let enabled: Option<i64> = conn
            .query_row(
                "SELECT enabled FROM proxy_config WHERE app_type = ?1",
                params![app_type],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AppError::Database(e.to_string()))?;
        drop(conn);
        Ok(enabled.unwrap_or(0) != 0)
    }
}

/// 一次限额判定的结果（内部结构）
struct EvaluatedBudget {
    limit_type: LimitType,
    currency: Option<LimitCurrency>,
    limit_display: String,
    used_display: String,
    exhausted: bool,
}

impl EvaluatedBudget {
    fn build_rejection(&self, provider_name: &str) -> BudgetRejection {
        let (message, limit_type_str, currency_str) = match self.limit_type {
            LimitType::Money => (
                format!(
                    "CC Switch usage limit reached. Provider: {provider_name}. Used: {} / Limit: {}",
                    self.used_display, self.limit_display
                ),
                "money",
                self.currency.map(|c| c.as_str()).unwrap_or("USD"),
            ),
            LimitType::Token => (
                format!(
                    "CC Switch token limit reached. Provider: {provider_name}. Used: {} / Limit: {}",
                    self.used_display, self.limit_display
                ),
                "token",
                "",
            ),
        };
        BudgetRejection {
            message,
            detail: serde_json::json!({
                "code": "API_KEY_LIMIT_REACHED",
                "source": "local",
                "provider": provider_name,
                "limitType": limit_type_str,
                "currency": currency_str,
                "used": self.used_display,
                "limit": self.limit_display,
            }),
        }
    }
}

fn requested_token_reservation(request: &Value, remaining: i64) -> i64 {
    let declared = ["max_tokens", "max_output_tokens"]
        .into_iter()
        .filter_map(|key| request.get(key).and_then(Value::as_i64))
        .filter(|value| *value > 0)
        .max();
    // A declared output ceiling is a caller contract. Do not silently clamp
    // it to the remaining budget: that would turn an over-budget request into
    // a partially admitted request and make the reservation gate ineffective.
    declared.unwrap_or(remaining)
}

fn build_token_rejection(used: i64, limit: i64) -> BudgetRejection {
    let used_display = format_tokens_for_message(used);
    let limit_display = format_tokens_for_message(limit);
    BudgetRejection {
        message: format!(
            "CC Switch token limit reached. Used: {used_display} / Limit: {limit_display}"
        ),
        detail: serde_json::json!({
            "code": "API_KEY_LIMIT_REACHED",
            "source": "local",
            "limitType": "token",
            "currency": "",
            "used": used_display,
            "limit": limit_display,
        }),
    }
}

fn build_money_rejection(
    used_usd: Decimal,
    limit_amount: Decimal,
    currency: LimitCurrency,
) -> BudgetRejection {
    let used_display = format_money_for_message(used_usd, LimitCurrency::Usd);
    let limit_display = format_money_for_message(limit_amount, currency);
    BudgetRejection {
        message: format!(
            "CC Switch usage limit reached. Used: {used_display} / Limit: {limit_display}"
        ),
        detail: serde_json::json!({
            "code": "API_KEY_LIMIT_REACHED",
            "source": "local",
            "limitType": "money",
            "currency": currency.as_str(),
            "used": used_display,
            "limit": limit_display,
        }),
    }
}

/// 错误消息里的金额展示：保留 2 位小数（消息是给人看的，精确值在结构化字段里）
fn format_money_for_message(amount: Decimal, currency: LimitCurrency) -> String {
    format!("{}{}", currency.symbol(), amount.round_dp(2).normalize())
}

/// 错误消息里的 token 展示：千分位分隔
fn format_tokens_for_message(tokens: i64) -> String {
    let sign = if tokens < 0 { "-" } else { "" };
    let digits = tokens.unsigned_abs().to_string();
    let mut grouped = String::new();
    for (idx, ch) in digits.chars().enumerate() {
        if idx > 0 && (digits.len() - idx) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{sign}{grouped}")
}

#[cfg(test)]
mod tests;
