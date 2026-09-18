//! API Key 使用限额（Budget Limit）相关命令
//!
//! 层级：React → Tauri Command → UsageLimitService（Database impl）→ SQLite。
//! React 不直接访问数据库。

use crate::error::AppError;
use crate::services::usage_limit::{BudgetStatus, UsageLimitConfig};
use crate::store::AppState;
use rust_decimal::Decimal;
use std::str::FromStr;
use tauri::State;

/// 读取指定 Provider 的限额配置与当前状态
#[tauri::command]
pub fn get_usage_limit_status(
    state: State<'_, AppState>,
    provider_id: String,
    app_type: String,
) -> Result<BudgetStatus, AppError> {
    state.db.get_budget_status(&provider_id, &app_type)
}

/// 保存限额配置（新建 / 修改 / 启停），返回保存后的最新状态
#[tauri::command]
pub fn save_usage_limit(
    state: State<'_, AppState>,
    provider_id: String,
    app_type: String,
    config: UsageLimitConfig,
) -> Result<BudgetStatus, AppError> {
    state
        .db
        .save_budget_config(&provider_id, &app_type, &config)
}

/// 重置使用量（usage_start_at 推进到当前时间，不删除历史 Usage）
#[tauri::command]
pub fn reset_usage_limit(
    state: State<'_, AppState>,
    provider_id: String,
    app_type: String,
) -> Result<BudgetStatus, AppError> {
    state.db.reset_budget_usage(&provider_id, &app_type)?;
    state.db.get_budget_status(&provider_id, &app_type)
}

/// 读取本地 USD → CNY 汇率（未配置时返回默认值）
#[tauri::command]
pub fn get_usd_cny_rate(state: State<'_, AppState>) -> Result<String, AppError> {
    let rate = state.db.get_usd_cny_exchange_rate()?;
    Ok(rate.to_string())
}

/// 保存本地 USD → CNY 汇率（必须 > 0）
#[tauri::command]
pub fn set_usd_cny_rate(state: State<'_, AppState>, rate: String) -> Result<(), AppError> {
    let parsed = Decimal::from_str(&rate)
        .map_err(|_| AppError::InvalidInput(format!("汇率格式无效: {rate}")))?;
    state.db.set_usd_cny_exchange_rate(parsed)
}
