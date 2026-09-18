//! Local budget reservation persistence.

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use rusqlite::{params, OptionalExtension};
use rust_decimal::Decimal;
use std::str::FromStr;

/// Persistent local reservation row. It contains only an opaque credential
/// fingerprint; the API key itself never enters this table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BudgetReservationRow {
    pub request_id: String,
    pub provider_id: String,
    pub app_type: String,
    pub credential_fingerprint: String,
    pub limit_type: String,
    pub reserved_tokens: i64,
    pub reserved_cost_usd: String,
    pub consumed_tokens: i64,
    pub consumed_cost_usd: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingBudgetTotals {
    pub tokens: i64,
    pub cost_usd: String,
}

impl Database {
    /// Re-check committed proxy usage and active pending reservations while
    /// holding the database mutex, then insert the new pending row if it fits.
    pub(crate) fn try_insert_pending_budget_reservation(
        &self,
        row: &BudgetReservationRow,
        usage_start_at: i64,
        token_limit: Option<i64>,
        cost_limit_usd: Option<&Decimal>,
        now: i64,
    ) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let fresh_input = crate::services::sql_helpers::fresh_input_sql("");
        let committed_tokens: i64 = conn
            .query_row(
                &format!(
                    "SELECT COALESCE(SUM(({fresh_input}) + output_tokens + cache_creation_tokens + cache_read_tokens), 0)
                     FROM proxy_request_logs
                     WHERE provider_id = ?1 AND app_type = ?2
                       AND credential_fingerprint = ?3 AND created_at >= ?4
                       AND data_source = 'proxy'"
                ),
                params![
                    row.provider_id,
                    row.app_type,
                    row.credential_fingerprint,
                    usage_start_at
                ],
                |result| result.get(0),
            )
            .map_err(|e| AppError::Database(format!("聚合 reservation token 用量失败: {e}")))?;
        let mut pending_tokens = 0i64;
        let mut pending_cost = Decimal::ZERO;
        {
            let mut stmt = conn
                .prepare(
                    "SELECT reserved_tokens, reserved_cost_usd
                     FROM budget_reservations
                     WHERE provider_id = ?1 AND app_type = ?2
                       AND credential_fingerprint = ?3
                       AND status = 'pending' AND expires_at > ?4",
                )
                .map_err(|e| AppError::Database(format!("读取 active reservation 失败: {e}")))?;
            let mut rows = stmt
                .query(params![
                    row.provider_id,
                    row.app_type,
                    row.credential_fingerprint,
                    now
                ])
                .map_err(|e| AppError::Database(format!("查询 active reservation 失败: {e}")))?;
            while let Some(result) = rows
                .next()
                .map_err(|e| AppError::Database(format!("读取 active reservation 行失败: {e}")))?
            {
                pending_tokens =
                    pending_tokens.saturating_add(result.get(0).map_err(|e| {
                        AppError::Database(format!("读取 pending token 失败: {e}"))
                    })?);
                let cost: String = result
                    .get(1)
                    .map_err(|e| AppError::Database(format!("读取 pending cost 失败: {e}")))?;
                pending_cost += Decimal::from_str(cost.trim())
                    .map_err(|e| AppError::Database(format!("pending cost 不是十进制: {e}")))?;
            }
        }

        let fits = if let Some(limit) = token_limit {
            committed_tokens
                .saturating_add(pending_tokens)
                .saturating_add(row.reserved_tokens)
                <= limit
        } else if let Some(limit) = cost_limit_usd {
            let committed_cost = {
                let mut stmt = conn
                    .prepare(
                        "SELECT total_cost_usd FROM proxy_request_logs
                         WHERE provider_id = ?1 AND app_type = ?2
                           AND credential_fingerprint = ?3 AND created_at >= ?4
                           AND data_source = 'proxy'",
                    )
                    .map_err(|e| AppError::Database(format!("读取 proxy cost 失败: {e}")))?;
                let mut rows = stmt
                    .query(params![
                        row.provider_id,
                        row.app_type,
                        row.credential_fingerprint,
                        usage_start_at
                    ])
                    .map_err(|e| AppError::Database(format!("查询 proxy cost 失败: {e}")))?;
                let mut total = Decimal::ZERO;
                while let Some(result) = rows
                    .next()
                    .map_err(|e| AppError::Database(format!("读取 proxy cost 行失败: {e}")))?
                {
                    let cost: String = result
                        .get(0)
                        .map_err(|e| AppError::Database(format!("读取 proxy cost 值失败: {e}")))?;
                    total += Decimal::from_str(cost.trim())
                        .map_err(|e| AppError::Database(format!("proxy cost 不是十进制: {e}")))?;
                }
                total
            };
            committed_cost
                + pending_cost
                + Decimal::from_str(row.reserved_cost_usd.trim())
                    .map_err(|e| AppError::Database(format!("reservation cost 不是十进制: {e}")))?
                <= *limit
        } else {
            false
        };

        if !fits {
            return Ok(false);
        }

        conn.execute(
            "INSERT INTO budget_reservations (
                request_id, provider_id, app_type, credential_fingerprint, limit_type,
                reserved_tokens, reserved_cost_usd, consumed_tokens, consumed_cost_usd,
                status, created_at, updated_at, expires_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                row.request_id,
                row.provider_id,
                row.app_type,
                row.credential_fingerprint,
                row.limit_type,
                row.reserved_tokens,
                row.reserved_cost_usd,
                row.consumed_tokens,
                row.consumed_cost_usd,
                row.status,
                row.created_at,
                row.updated_at,
                row.expires_at,
            ],
        )
        .map_err(|e| AppError::Database(format!("插入预算 reservation 失败: {e}")))?;
        Ok(true)
    }

    #[allow(dead_code)]
    pub(crate) fn insert_pending_budget_reservation(
        &self,
        row: &BudgetReservationRow,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO budget_reservations (
                request_id, provider_id, app_type, credential_fingerprint, limit_type,
                reserved_tokens, reserved_cost_usd, consumed_tokens, consumed_cost_usd,
                status, created_at, updated_at, expires_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                row.request_id,
                row.provider_id,
                row.app_type,
                row.credential_fingerprint,
                row.limit_type,
                row.reserved_tokens,
                row.reserved_cost_usd,
                row.consumed_tokens,
                row.consumed_cost_usd,
                row.status,
                row.created_at,
                row.updated_at,
                row.expires_at,
            ],
        )
        .map_err(|e| AppError::Database(format!("插入预算 reservation 失败: {e}")))?;
        Ok(())
    }

    pub(crate) fn sum_pending_budget_reservations(
        &self,
        provider_id: &str,
        app_type: &str,
        fingerprint: &str,
        now: i64,
    ) -> Result<PendingBudgetTotals, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT reserved_tokens, reserved_cost_usd
                 FROM budget_reservations
                 WHERE provider_id = ?1 AND app_type = ?2
                   AND credential_fingerprint = ?3
                   AND status = 'pending' AND expires_at > ?4",
            )
            .map_err(|e| AppError::Database(format!("读取预算 reservation 失败: {e}")))?;
        let mut rows = stmt
            .query(params![provider_id, app_type, fingerprint, now])
            .map_err(|e| AppError::Database(format!("查询预算 reservation 失败: {e}")))?;
        let mut tokens = 0i64;
        let mut cost_usd = Decimal::ZERO;
        while let Some(row) = rows
            .next()
            .map_err(|e| AppError::Database(format!("读取预算 reservation 行失败: {e}")))?
        {
            tokens = tokens
                .saturating_add(row.get::<_, i64>(0).map_err(|e| {
                    AppError::Database(format!("读取 reservation token 失败: {e}"))
                })?);
            let raw_cost: String = row
                .get(1)
                .map_err(|e| AppError::Database(format!("读取 reservation cost 失败: {e}")))?;
            let parsed = Decimal::from_str(raw_cost.trim()).map_err(|e| {
                AppError::Database(format!("预算 reservation cost 不是十进制: {e}"))
            })?;
            cost_usd += parsed;
        }
        Ok(PendingBudgetTotals {
            tokens,
            cost_usd: cost_usd.normalize().to_string(),
        })
    }

    pub(crate) fn reconcile_budget_reservation_row(
        &self,
        request_id: &str,
        consumed_tokens: i64,
        consumed_cost_usd: &str,
        now: i64,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute(
                "UPDATE budget_reservations
                 SET consumed_tokens = ?2, consumed_cost_usd = ?3,
                     status = 'committed', updated_at = ?4
                 WHERE request_id = ?1 AND status = 'pending'",
                params![request_id, consumed_tokens, consumed_cost_usd, now],
            )
            .map_err(|e| AppError::Database(format!("结算预算 reservation 失败: {e}")))?;
        if affected == 0 {
            let status: Option<String> = conn
                .query_row(
                    "SELECT status FROM budget_reservations WHERE request_id = ?1",
                    [request_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| AppError::Database(format!("读取 reservation 状态失败: {e}")))?;
            if status.as_deref() == Some("committed") {
                return Ok(());
            }
            return Err(AppError::InvalidInput(format!(
                "预算 reservation 不存在或已结束: {request_id}"
            )));
        }
        Ok(())
    }

    pub(crate) fn release_budget_reservation_row(
        &self,
        request_id: &str,
        now: i64,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE budget_reservations
             SET status = 'released', updated_at = ?2
             WHERE request_id = ?1 AND status = 'pending'",
            params![request_id, now],
        )
        .map_err(|e| AppError::Database(format!("释放预算 reservation 失败: {e}")))?;
        Ok(())
    }

    pub(crate) fn cleanup_stale_budget_reservations(&self, now: i64) -> Result<usize, AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE budget_reservations
             SET status = 'released', updated_at = ?1
             WHERE status = 'pending' AND expires_at <= ?1",
            params![now],
        )
        .map_err(|e| AppError::Database(format!("清理过期预算 reservation 失败: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::Database;
    use rusqlite::params;

    fn reservation_row() -> super::BudgetReservationRow {
        super::BudgetReservationRow {
            request_id: "request-1".to_string(),
            provider_id: "provider-1".to_string(),
            app_type: "claude".to_string(),
            credential_fingerprint: "fingerprint".to_string(),
            limit_type: "token".to_string(),
            reserved_tokens: 100,
            reserved_cost_usd: "0".to_string(),
            consumed_tokens: 0,
            consumed_cost_usd: "0".to_string(),
            status: "pending".to_string(),
            created_at: 1_999_980,
            updated_at: 1_999_980,
            expires_at: 2_000_001,
        }
    }

    #[test]
    fn v21_reservation_table_and_index_exist() {
        let db = Database::memory().unwrap();
        let conn = db.conn.lock().unwrap();
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'budget_reservations'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let index_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_budget_reservations_identity'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 1);
        assert_eq!(index_count, 1);
    }

    #[test]
    fn stale_pending_reservations_are_released_without_touching_usage_logs() {
        let db = Database::memory().unwrap();
        db.save_provider(
            "claude",
            &crate::provider::Provider::with_id(
                "provider-1".to_string(),
                "Reservation Provider".to_string(),
                serde_json::json!({"env": {"ANTHROPIC_AUTH_TOKEN": "sk-test-reservation"}}),
                None,
            ),
        )
        .unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO proxy_request_logs (request_id, provider_id, app_type, model, input_tokens, output_tokens, latency_ms, status_code, created_at, data_source)
                 VALUES ('usage-1', 'provider-1', 'claude', 'test-model', 1, 1, 1, 200, 1999990, 'proxy')",
                [],
            ).unwrap();
        }
        let row = reservation_row();
        db.insert_pending_budget_reservation(&row).unwrap();

        assert_eq!(
            db.sum_pending_budget_reservations("provider-1", "claude", "fingerprint", 2_000_000)
                .unwrap()
                .tokens,
            100
        );
        assert_eq!(db.cleanup_stale_budget_reservations(2_000_002).unwrap(), 1);
        assert_eq!(
            db.sum_pending_budget_reservations("provider-1", "claude", "fingerprint", 2_000_002)
                .unwrap()
                .tokens,
            0
        );
        let conn = db.conn.lock().unwrap();
        let usage_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE request_id = ?1",
                params!["usage-1"],
                |result| result.get(0),
            )
            .unwrap();
        assert_eq!(usage_count, 1);
    }
}
