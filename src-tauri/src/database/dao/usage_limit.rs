//! API Key 使用限额 DAO
//!
//! `api_key_limits` 表的持久化访问，以及从 usage SSOT（`proxy_request_logs`）
//! 聚合指定 credential 在预算窗口内的用量。
//!
//! 设计要点：
//! - 本表只保存限额配置与统计窗口起点（`usage_start_at`），不冗余存储
//!   累计用量（避免与 `proxy_request_logs` 形成双 SSOT）。
//! - 金额比较走两级通道：先 REAL 快速判（远离阈值时 O(1) 放行），贴近
//!   阈值时取原始十进制字符串用 `rust_decimal` 精确求和复核，规避浮点误差。

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use rusqlite::{params, OptionalExtension};

/// 一条 API Key 限额配置
#[derive(Debug, Clone, PartialEq)]
pub struct ApiKeyLimitRow {
    pub provider_id: String,
    pub app_type: String,
    /// API Key 的不可逆 SHA-256 指纹（十六进制），绝不存明文 key
    pub credential_fingerprint: String,
    pub enabled: bool,
    /// "money" 或 "token"
    pub limit_type: String,
    /// "USD" / "CNY"；token 模式下为 None
    pub currency: Option<String>,
    /// 十进制字符串（金额）或纯整数字符串（token 数）
    pub limit_amount: String,
    /// 预算统计窗口起点（unix 秒）
    pub usage_start_at: i64,
    /// 自动重置周期（秒）；None 表示永不自动重置
    pub reset_interval_seconds: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 指定 credential 在窗口内的用量聚合（REAL 快速通道）
#[derive(Debug, Clone, Default)]
pub struct CredentialUsageFast {
    /// 窗口内 `total_cost_usd` 的 REAL 近似和（仅用于快速预判）
    pub total_cost_usd_real: f64,
    /// 归一化 fresh input tokens（与 Usage Dashboard 同一语义）
    pub fresh_input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
}

impl CredentialUsageFast {
    /// Dashboard 语义的 normalized total tokens：
    /// fresh_input + output + cache_creation + cache_read
    pub fn normalized_total_tokens(&self) -> i64 {
        self.fresh_input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_creation_tokens)
            .saturating_add(self.cache_read_tokens)
    }
}

impl Database {
    /// 读取某个 Provider 的限额配置（每个 Provider 至多一条）
    pub fn get_api_key_limit(
        &self,
        provider_id: &str,
        app_type: &str,
    ) -> Result<Option<ApiKeyLimitRow>, AppError> {
        let conn = lock_conn!(self.conn);
        conn.query_row(
            "SELECT provider_id, app_type, credential_fingerprint, enabled, limit_type,
                    currency, limit_amount, usage_start_at, reset_interval_seconds,
                    created_at, updated_at
             FROM api_key_limits WHERE provider_id = ?1 AND app_type = ?2",
            params![provider_id, app_type],
            |row| {
                Ok(ApiKeyLimitRow {
                    provider_id: row.get(0)?,
                    app_type: row.get(1)?,
                    credential_fingerprint: row.get(2)?,
                    enabled: row.get::<_, i64>(3)? != 0,
                    limit_type: row.get(4)?,
                    currency: row.get(5)?,
                    limit_amount: row.get(6)?,
                    usage_start_at: row.get(7)?,
                    reset_interval_seconds: row.get(8)?,
                    created_at: row.get(9)?,
                    updated_at: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(|e| AppError::Database(format!("读取限额配置失败: {e}")))
    }

    /// 保存限额配置（首次插入时由调用方决定 usage_start_at）
    pub fn upsert_api_key_limit(&self, row: &ApiKeyLimitRow) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO api_key_limits (
                provider_id, app_type, credential_fingerprint, enabled, limit_type,
                currency, limit_amount, usage_start_at, reset_interval_seconds,
                created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT (provider_id, app_type) DO UPDATE SET
                credential_fingerprint = excluded.credential_fingerprint,
                enabled = excluded.enabled,
                limit_type = excluded.limit_type,
                currency = excluded.currency,
                limit_amount = excluded.limit_amount,
                usage_start_at = excluded.usage_start_at,
                reset_interval_seconds = excluded.reset_interval_seconds,
                updated_at = excluded.updated_at",
            params![
                row.provider_id,
                row.app_type,
                row.credential_fingerprint,
                row.enabled as i64,
                row.limit_type,
                row.currency,
                row.limit_amount,
                row.usage_start_at,
                row.reset_interval_seconds,
                row.created_at,
                row.updated_at,
            ],
        )
        .map_err(|e| AppError::Database(format!("保存限额配置失败: {e}")))?;
        Ok(())
    }

    /// 重置预算窗口：usage_start_at 推进到当前时间，不删除任何历史 Usage
    pub fn reset_api_key_limit_window(
        &self,
        provider_id: &str,
        app_type: &str,
        new_start: i64,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute(
                "UPDATE api_key_limits SET usage_start_at = ?3, updated_at = ?3
                 WHERE provider_id = ?1 AND app_type = ?2",
                params![provider_id, app_type, new_start],
            )
            .map_err(|e| AppError::Database(format!("重置使用量失败: {e}")))?;
        if affected == 0 {
            return Err(AppError::InvalidInput(
                "限额配置不存在，无法重置使用量".to_string(),
            ));
        }
        Ok(())
    }

    /// Credential 轮换重绑：Provider 换了 API Key 时，把配置迁移到新指纹并
    /// 将统计窗口起点重置为当前时间——旧 Key 的历史用量不会计入新 Key。
    /// 返回 true 表示确实发生了重绑。
    pub fn rebind_api_key_limit_credential(
        &self,
        provider_id: &str,
        app_type: &str,
        fingerprint: &str,
        now: i64,
    ) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute(
                "UPDATE api_key_limits
                 SET credential_fingerprint = ?3, usage_start_at = ?4, updated_at = ?4
                 WHERE provider_id = ?1 AND app_type = ?2 AND credential_fingerprint <> ?3",
                params![provider_id, app_type, fingerprint, now],
            )
            .map_err(|e| AppError::Database(format!("重绑限额凭证失败: {e}")))?;
        Ok(affected > 0)
    }

    /// 将已到期的自动重置窗口推进到当前观测时间。
    ///
    /// 重置是惰性的：状态读取或请求闸门第一次发现窗口过期时执行，
    /// 不需要后台定时任务，也不会删除历史 Usage 明细。
    pub fn roll_expired_api_key_limit_window(
        &self,
        provider_id: &str,
        app_type: &str,
        now: i64,
    ) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute(
                "UPDATE api_key_limits
                 SET usage_start_at = ?3, updated_at = ?3
                 WHERE provider_id = ?1 AND app_type = ?2
                   AND reset_interval_seconds IS NOT NULL
                   AND reset_interval_seconds > 0
                   AND usage_start_at <= ?3 - reset_interval_seconds",
                params![provider_id, app_type, now],
            )
            .map_err(|e| AppError::Database(format!("自动重置使用量失败: {e}")))?;
        Ok(affected > 0)
    }

    /// 用量快速聚合（REAL 金额近似 + 精确整数 token 和）。
    /// 只统计 `data_source = 'proxy'` 的请求：预算限额是本地代理观测到的流量的边界。
    pub fn sum_credential_usage_fast(
        &self,
        provider_id: &str,
        app_type: &str,
        fingerprint: &str,
        since: i64,
    ) -> Result<CredentialUsageFast, AppError> {
        let conn = lock_conn!(self.conn);
        let fresh_input = crate::services::sql_helpers::fresh_input_sql("");
        let sql = format!(
            "SELECT COALESCE(SUM(CAST(total_cost_usd AS REAL)), 0),
                    COALESCE(SUM({fresh_input}), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_creation_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0)
             FROM proxy_request_logs
             WHERE provider_id = ?1 AND app_type = ?2
               AND credential_fingerprint = ?3 AND created_at >= ?4
               AND data_source = 'proxy'"
        );
        conn.query_row(
            &sql,
            params![provider_id, app_type, fingerprint, since],
            |row| {
                Ok(CredentialUsageFast {
                    total_cost_usd_real: row.get(0)?,
                    fresh_input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cache_creation_tokens: row.get(3)?,
                    cache_read_tokens: row.get(4)?,
                })
            },
        )
        .map_err(|e| AppError::Database(format!("聚合凭证用量失败: {e}")))
    }

    /// 金额精确复核：取窗口内原始 `total_cost_usd` 十进制字符串，
    /// 由调用方用 `rust_decimal` 精确求和（贴近阈值时才调用）。
    pub fn fetch_credential_cost_strings(
        &self,
        provider_id: &str,
        app_type: &str,
        fingerprint: &str,
        since: i64,
    ) -> Result<Vec<String>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT total_cost_usd FROM proxy_request_logs
                 WHERE provider_id = ?1 AND app_type = ?2
                   AND credential_fingerprint = ?3 AND created_at >= ?4
                   AND data_source = 'proxy'",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![provider_id, app_type, fingerprint, since], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut costs = Vec::new();
        for row in rows {
            costs.push(row.map_err(|e| AppError::Database(e.to_string()))?);
        }
        Ok(costs)
    }

    /// 找出窗口内「有 token 用量但成本为 0」的请求涉及的计价模型，
    /// 供上层逐个检查是否真的缺少定价（免费模型价格 0 不算缺价）。
    pub fn list_zero_cost_pricing_models(
        &self,
        provider_id: &str,
        app_type: &str,
        fingerprint: &str,
        since: i64,
    ) -> Result<Vec<(String, i64)>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT pricing_model, COUNT(*) FROM proxy_request_logs
                 WHERE provider_id = ?1 AND app_type = ?2
                   AND credential_fingerprint = ?3 AND created_at >= ?4
                   AND data_source = 'proxy'
                   AND (input_tokens > 0 OR output_tokens > 0
                        OR cache_read_tokens > 0 OR cache_creation_tokens > 0)
                   AND total_cost_usd = '0'
                   AND IFNULL(pricing_model, '') <> ''
                 GROUP BY pricing_model",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![provider_id, app_type, fingerprint, since], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut models = Vec::new();
        for row in rows {
            models.push(row.map_err(|e| AppError::Database(e.to_string()))?);
        }
        Ok(models)
    }
}
