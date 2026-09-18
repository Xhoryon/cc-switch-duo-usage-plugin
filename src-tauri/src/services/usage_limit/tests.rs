//! 使用限额服务单元测试
//!
//! 覆盖需求 §38 的全部用例：限额关闭 / 金额低于·达到·超过 / Token 低于·达到·超过 /
//! Reset / 换 Key 不继承 / 汇率换算 / 未知定价 / 并发记账。测试统一使用
//! `sk-test-...` 假 key，绝不出现真实凭据。

use super::*;
use crate::database::Database;
use crate::provider::Provider;
use rust_decimal::Decimal;
use std::str::FromStr;

/// 构造一个带静态 API Key 的 Claude provider（settings_config 与代理
/// extract_auth 的读取路径一致：env.ANTHROPIC_AUTH_TOKEN）
fn claude_provider(id: &str, api_key: &str) -> Provider {
    let mut provider = Provider::with_id(
        id.to_string(),
        format!("Test Provider {id}"),
        serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.test.local",
                "ANTHROPIC_AUTH_TOKEN": api_key,
            }
        }),
        None,
    );
    provider.category = Some("custom".to_string());
    provider
}

/// 写入一个已计价的请求日志行（模拟代理记账）
#[allow(clippy::too_many_arguments)]
fn insert_log(
    db: &Database,
    request_id: &str,
    provider_id: &str,
    fingerprint: &str,
    created_at: i64,
    input: i64,
    output: i64,
    total_cost: &str,
) {
    let conn = db.conn.lock().expect("test db lock");
    conn.execute(
        "INSERT INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model, pricing_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_token_semantics, input_cost_usd, output_cost_usd,
            cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
            latency_ms, status_code, created_at, data_source, credential_fingerprint
        ) VALUES (?1, ?2, 'claude', 'test-model', 'test-model', 'test-model',
            ?3, ?4, 0, 0, 2, '0', '0', '0', '0', ?5, 100, 200, ?6, 'proxy', ?7)",
        rusqlite::params![
            request_id,
            provider_id,
            input,
            output,
            total_cost,
            created_at,
            fingerprint
        ],
    )
    .unwrap();
}

fn limit_row(
    provider_id: &str,
    fingerprint: &str,
    enabled: bool,
    limit_type: &str,
    currency: Option<&str>,
    amount: &str,
) -> ApiKeyLimitRow {
    ApiKeyLimitRow {
        provider_id: provider_id.to_string(),
        app_type: "claude".to_string(),
        credential_fingerprint: fingerprint.to_string(),
        enabled,
        limit_type: limit_type.to_string(),
        currency: currency.map(str::to_string),
        limit_amount: amount.to_string(),
        usage_start_at: 1_000_000,
        reset_interval_seconds: None,
        created_at: 1_000_000,
        updated_at: 1_000_000,
    }
}

const FP_A: &str = "aaaa"; // 旧 key 指纹（guard 直测用，无需 provider 行）
const FP_B: &str = "bbbb"; // 新 key 指纹

/// 建 provider 行并写入限额配置（api_key_limits 有 FK → providers，必须先建
/// provider）；返回该 key 的真实指纹。每个测试用独立的 memory DB，provider_id 可复用。
fn seed_limit(
    db: &Database,
    provider_id: &str,
    enabled: bool,
    limit_type: &str,
    currency: Option<&str>,
    amount: &str,
) -> String {
    let key = format!("sk-test-{provider_id}-secret-9876");
    db.save_provider("claude", &claude_provider(provider_id, &key))
        .unwrap();
    let fp = credential_fingerprint("claude", &key);
    db.upsert_api_key_limit(&limit_row(
        provider_id,
        &fp,
        enabled,
        limit_type,
        currency,
        amount,
    ))
    .unwrap();
    fp
}

// ---------- 转发前判定（§38 Rust Unit Tests） ----------

#[test]
fn limit_disabled_allows_request() {
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p1", false, "token", None, "1000");
    insert_log(&db, "r1", "p1", &fp, 1_000_100, 5_000, 5_000, "0.5");

    let result = db.check_budget_before_forward("p1", "P1", "claude", &fp);
    assert!(result.is_ok(), "限额关闭时必须放行");
}

#[test]
fn no_limit_config_allows_request() {
    let db = Database::memory().unwrap();
    assert!(db
        .check_budget_before_forward("p-none", "P", "claude", FP_A)
        .is_ok());
}

#[test]
fn reset_interval_rolls_window_and_excludes_old_usage() {
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p-reset", true, "token", None, "1000");
    let now = chrono::Utc::now().timestamp();
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE api_key_limits
             SET usage_start_at = ?1, reset_interval_seconds = ?2
             WHERE provider_id = 'p-reset' AND app_type = 'claude'",
            rusqlite::params![now - 3_600, 3_600_i64],
        )
        .unwrap();
    }
    insert_log(&db, "old-window", "p-reset", &fp, now - 100, 100, 100, "0");

    assert!(db
        .roll_expired_api_key_limit_window("p-reset", "claude", now)
        .unwrap());
    let status = db.get_budget_status("p-reset", "claude").unwrap();

    assert_eq!(status.reset_interval_seconds, Some(3_600));
    assert_eq!(status.usage_start_at, Some(now));
    assert_eq!(status.used_tokens, 0);
}

#[test]
fn money_below_limit_allows_request() {
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p1", true, "money", Some("USD"), "10");
    // $9.99 < $10，快速通道应放行
    insert_log(&db, "r1", "p1", &fp, 1_000_100, 100, 100, "9.99");

    assert!(db
        .check_budget_before_forward("p1", "P1", "claude", &fp)
        .is_ok());
}

#[test]
fn money_reached_blocks_request() {
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p1", true, "money", Some("USD"), "10");
    // 精确 $10.00 → 达到限额，下一请求必须被拒
    insert_log(&db, "r1", "p1", &fp, 1_000_100, 100, 100, "10.00");

    let err = db
        .check_budget_before_forward("p1", "P1", "claude", &fp)
        .unwrap_err();
    assert!(
        err.message.contains("usage limit reached"),
        "{}",
        err.message
    );
    assert_eq!(err.detail["limitType"], "money");
    assert_eq!(err.detail["limit"], "$10");
}

#[test]
fn money_exceeded_blocks_request() {
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p1", true, "money", Some("USD"), "1");
    // $1.02 > $1 → 拒绝（对齐集成测试 Case 2）
    insert_log(&db, "r1", "p1", &fp, 1_000_100, 100, 100, "1.02");

    assert!(db
        .check_budget_before_forward("p1", "P1", "claude", &fp)
        .is_err());
}

#[test]
fn token_below_limit_allows_request() {
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p1", true, "token", None, "1000000");
    insert_log(&db, "r1", "p1", &fp, 1_000_100, 600_000, 300_000, "0.5");

    assert!(db
        .check_budget_before_forward("p1", "P1", "claude", &fp)
        .is_ok());
}

#[test]
fn token_reached_blocks_request() {
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p1", true, "token", None, "1000");
    // 正好 1000 tokens → 达到限额即拒绝（>= 语义）
    insert_log(&db, "r1", "p1", &fp, 1_000_100, 600, 400, "0.5");

    let err = db
        .check_budget_before_forward("p1", "P1", "claude", &fp)
        .unwrap_err();
    assert!(
        err.message.contains("token limit reached"),
        "{}",
        err.message
    );
    assert_eq!(err.detail["limitType"], "token");
}

#[test]
fn token_exceeded_blocks_request() {
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p1", true, "token", None, "1000");
    // 1100 > 1000 → 拒绝（对齐集成测试 Case 1）
    insert_log(&db, "r1", "p1", &fp, 1_000_100, 600, 500, "0.5");

    assert!(db
        .check_budget_before_forward("p1", "P1", "claude", &fp)
        .is_err());
}

#[test]
fn reset_moves_window_so_old_usage_not_counted() {
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p1", true, "money", Some("USD"), "5");
    insert_log(&db, "r-old", "p1", &fp, 1_000_100, 100, 100, "5.10");
    assert!(db
        .check_budget_before_forward("p1", "P1", "claude", &fp)
        .is_err());

    // 重置窗口：旧记录（created_at < 新起点）不再计入
    db.reset_api_key_limit_window("p1", "claude", 2_000_000)
        .unwrap();
    insert_log(&db, "r-new", "p1", &fp, 2_000_100, 100, 100, "0.10");

    assert!(db
        .check_budget_before_forward("p1", "P1", "claude", &fp)
        .is_ok());

    let status = db.get_budget_status("p1", "claude").unwrap();
    assert_eq!(status.used_money_usd, "0.10");
    assert_eq!(status.usage_start_at, Some(2_000_000));
}

#[test]
fn credential_change_does_not_inherit_old_usage() {
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p1", true, "token", None, "1000");
    // 旧 key 已用 900
    insert_log(&db, "r1", "p1", &fp, 1_000_100, 500, 400, "0.5");

    // 用户换成新 key（指纹 B）：guard 检测到轮换 → 重绑 + 窗口重置
    insert_log(&db, "r2", "p1", FP_B, 3_000_100, 300, 300, "0.2");
    db.check_budget_before_forward("p1", "P1", "claude", FP_B)
        .unwrap();

    let row = db.get_api_key_limit("p1", "claude").unwrap().unwrap();
    assert_eq!(row.credential_fingerprint, FP_B, "必须已重绑到新指纹");
    assert!(
        row.usage_start_at > 1_000_000,
        "统计窗口必须已重置（旧 key 用量不继承）"
    );

    // 新 key 用量 600 < 1000 → 放行
    assert!(db
        .check_budget_before_forward("p1", "P1", "claude", FP_B)
        .is_ok());
    // 重绑后窗口起点是「当下」：旧 key 的 900 与重绑前的 600 都不并入新窗口
    let fast = db
        .sum_credential_usage_fast("p1", "claude", FP_B, row.usage_start_at)
        .unwrap();
    assert_eq!(
        fast.normalized_total_tokens(),
        0,
        "旧 key / 窗口前用量不得计入新窗口"
    );

    // 新窗口内的请求正常计入
    insert_log(
        &db,
        "r3",
        "p1",
        FP_B,
        row.usage_start_at + 10,
        300,
        300,
        "0.2",
    );
    let fast = db
        .sum_credential_usage_fast("p1", "claude", FP_B, row.usage_start_at)
        .unwrap();
    assert_eq!(fast.normalized_total_tokens(), 600);
}

#[test]
fn currency_conversion_usd_cny_is_correct() {
    let db = Database::memory().unwrap();
    // 汇率默认 7.2：$10 限额 ≈ ¥72
    let rate = db.get_usd_cny_exchange_rate().unwrap();
    assert_eq!(rate, Decimal::from_str("7.2").unwrap());

    // CNY 限额 ¥7.2 = $1 上限；已用 $1.00 → 达到
    let fp = seed_limit(&db, "p1", true, "money", Some("CNY"), "7.2");
    insert_log(&db, "r1", "p1", &fp, 1_000_100, 100, 100, "1.00");
    assert!(db
        .check_budget_before_forward("p1", "P1", "claude", &fp)
        .is_err());

    // 自定义汇率可改
    db.set_usd_cny_exchange_rate(Decimal::from_str("7.5").unwrap())
        .unwrap();
    assert_eq!(
        db.get_usd_cny_exchange_rate().unwrap(),
        Decimal::from_str("7.5").unwrap()
    );

    // 非法汇率被拒绝
    assert!(
        db.set_usd_cny_exchange_rate(Decimal::ZERO).is_err(),
        "汇率必须 > 0"
    );
}

#[test]
fn cny_limit_uses_converted_threshold_in_fast_path() {
    let db = Database::memory().unwrap();
    // ¥14.4 / 7.2 = $2 上限；已用 $1.9（快速通道范围内）→ 放行
    let fp = seed_limit(&db, "p1", true, "money", Some("CNY"), "14.4");
    insert_log(&db, "r1", "p1", &fp, 1_000_100, 100, 100, "1.90");
    assert!(db
        .check_budget_before_forward("p1", "P1", "claude", &fp)
        .is_ok());

    // 再加 $0.2 → $2.1 > $2 → 拒绝（精确复核通道）
    insert_log(&db, "r2", "p1", &fp, 1_000_200, 100, 100, "0.20");
    assert!(db
        .check_budget_before_forward("p1", "P1", "claude", &fp)
        .is_err());
}

#[test]
fn money_float_precision_is_decimal_safe() {
    let db = Database::memory().unwrap();
    // 经典浮点陷阱：0.1+0.2 != 0.3。十进制求和必须精确判满。
    let fp = seed_limit(&db, "p1", true, "money", Some("USD"), "0.3");
    insert_log(&db, "r1", "p1", &fp, 1_000_100, 1, 1, "0.1");
    insert_log(&db, "r2", "p1", &fp, 1_000_200, 1, 1, "0.2");

    let err = db
        .check_budget_before_forward("p1", "P1", "claude", &fp)
        .unwrap_err();
    assert!(err.message.contains("limit reached"));
}

// ---------- 未知定价 ----------

#[test]
fn unknown_pricing_is_surfaced_not_silently_zero() {
    // 需求 §10：缺价模型的用量不能按 $0 假装正常——状态必须暴露 unpriced 信息
    let db = Database::memory().unwrap();
    let key = "sk-test-pricing-key";
    let fp = &credential_fingerprint("claude", key);
    db.save_provider("claude", &claude_provider("p1", key))
        .unwrap();
    db.upsert_api_key_limit(&limit_row("p1", fp, true, "money", Some("USD"), "10"))
        .unwrap();

    // 已计价行（先给 test-model 种子定价）+ 未计价行（unknown-x）
    {
        let conn = db.conn.lock().expect("test db lock");
        conn.execute(
            "INSERT INTO model_pricing (model_id, display_name, input_cost_per_million, output_cost_per_million)
             VALUES ('test-model', 'Test Model', '3.0', '15.0')",
            [],
        )
        .unwrap();
    }
    insert_log(&db, "r1", "p1", fp, 1_000_100, 10_000, 10_000, "0.5");
    {
        let conn = db.conn.lock().expect("test db lock");
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, provider_id, app_type, model, request_model, pricing_model,
                input_tokens, output_tokens, input_token_semantics,
                total_cost_usd, latency_ms, status_code, created_at, data_source, credential_fingerprint
             ) VALUES ('r2', 'p1', 'claude', 'unknown-x', 'unknown-x', 'unknown-x',
                5000, 5000, 2, '0', 10, 200, ?2, 'proxy', ?1)",
            rusqlite::params![fp, 1_000_200],
        )
        .unwrap();
    }

    let status = db.get_budget_status("p1", "claude").unwrap();
    assert_eq!(status.unpriced_request_count, 1);
    assert_eq!(status.unpriced_models, vec!["unknown-x".to_string()]);
    // 已计价部分继续有效；未计价行不得悄悄按 0 假装全貌
    assert_eq!(status.used_money_usd, "0.5");
}

// ---------- 并发与数据源边界 ----------

#[test]
fn concurrent_accounting_does_not_lose_writes() {
    // 并发语义验证：N 个线程同时写同一 credential 的用量行，
    // 全部写完后聚合值必须等于写入总量（DB Mutex 下无丢失写）
    let db = std::sync::Arc::new(Database::memory().unwrap());
    let fp = seed_limit(&db, "p1", true, "token", None, "1000000000");

    let handles: Vec<_> = (0..8)
        .map(|worker| {
            let db = db.clone();
            let fp = fp.clone();
            std::thread::spawn(move || {
                for i in 0..25 {
                    insert_log(
                        &db,
                        &format!("r-{worker}-{i}"),
                        "p1",
                        &fp,
                        1_000_100 + worker * 1000 + i,
                        1_000,
                        1_000,
                        "0.01",
                    );
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    let fast = db
        .sum_credential_usage_fast("p1", "claude", &fp, 1_000_000)
        .unwrap();
    assert_eq!(fast.normalized_total_tokens(), 8 * 25 * 2_000);
    assert!((fast.total_cost_usd_real - 8.0 * 25.0 * 0.01).abs() < 1e-9);
}

#[test]
fn session_log_rows_are_excluded_from_budget() {
    // 会话日志同步行为 data_source != 'proxy'：不是本地代理的观测，
    // 不能计入本机预算（usage is local observation）
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p1", true, "money", Some("USD"), "1");
    insert_log(&db, "r1", "p1", &fp, 1_000_100, 100, 100, "0.99");
    {
        let conn = db.conn.lock().expect("test db lock");
        conn.execute(
            "UPDATE proxy_request_logs SET data_source = 'session_log' WHERE request_id = 'r1'",
            [],
        )
        .unwrap();
    }
    assert!(db
        .check_budget_before_forward("p1", "P1", "claude", &fp)
        .is_ok());
}

#[test]
fn pending_reservation_counts_against_token_limit_and_release_reopens_budget() {
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p-reservation", true, "token", None, "1000");
    let request = serde_json::json!({
        "model": "test-model",
        "max_tokens": 600
    });

    let first = db
        .reserve_budget_for_request(
            "request-reservation-1",
            "p-reservation",
            "claude",
            &fp,
            &request,
        )
        .unwrap()
        .expect("enabled limit should create a reservation");
    assert_eq!(first.reserved_tokens, 600);

    let second = db.reserve_budget_for_request(
        "request-reservation-2",
        "p-reservation",
        "claude",
        &fp,
        &serde_json::json!({"model": "test-model", "max_tokens": 500}),
    );
    assert!(
        second.is_err(),
        "pending 600 + requested 500 must exceed 1000"
    );

    db.release_budget_reservation("request-reservation-1")
        .unwrap();
    let reopened = db
        .reserve_budget_for_request(
            "request-reservation-3",
            "p-reservation",
            "claude",
            &fp,
            &serde_json::json!({"model": "test-model", "max_tokens": 500}),
        )
        .unwrap();
    assert!(reopened.is_some());
}

#[test]
fn disabled_limit_does_not_create_reservation() {
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p-reservation-off", false, "token", None, "1000");
    let result = db
        .reserve_budget_for_request(
            "request-reservation-off",
            "p-reservation-off",
            "claude",
            &fp,
            &serde_json::json!({"model": "test-model", "max_tokens": 500}),
        )
        .unwrap();
    assert!(result.is_none());
}

#[test]
fn reservation_reconciliation_commits_actual_usage_and_cost() {
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p-reconcile", true, "token", None, "1000");
    let reservation = db
        .reserve_budget_for_request(
            "request-reconcile",
            "p-reconcile",
            "claude",
            &fp,
            &serde_json::json!({"model": "test-model", "max_tokens": 600}),
        )
        .unwrap()
        .unwrap();

    db.reconcile_budget_reservation(&reservation.request_id, 42, "0.125")
        .unwrap();

    let conn = db.conn.lock().unwrap();
    let (status, consumed_tokens, consumed_cost): (String, i64, String) = conn
        .query_row(
            "SELECT status, consumed_tokens, consumed_cost_usd
             FROM budget_reservations WHERE request_id = 'request-reconcile'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "committed");
    assert_eq!(consumed_tokens, 42);
    assert_eq!(consumed_cost, "0.125");
}

#[test]
fn concurrent_reservations_cannot_double_spend_remaining_tokens() {
    use std::sync::{Arc, Barrier};

    let db = Arc::new(Database::memory().unwrap());
    let fp = seed_limit(&db, "p-reservation-concurrent", true, "token", None, "1000");
    let barrier = Arc::new(Barrier::new(4));
    let handles = (0..4)
        .map(|index| {
            let db = db.clone();
            let fp = fp.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                db.reserve_budget_for_request(
                    &format!("request-concurrent-{index}"),
                    "p-reservation-concurrent",
                    "claude",
                    &fp,
                    &serde_json::json!({"model": "test-model", "max_tokens": 600}),
                )
                .ok()
                .flatten()
                .is_some()
            })
        })
        .collect::<Vec<_>>();
    let successes = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .filter(|success| *success)
        .count();
    assert_eq!(successes, 1);
}

// ---------- 输入验证 ----------

#[test]
fn validate_money_amounts() {
    assert_eq!(
        validate_limit_amount(LimitType::Money, "100").unwrap(),
        "100"
    );
    assert_eq!(
        validate_limit_amount(LimitType::Money, " 100.50 ").unwrap(),
        "100.5"
    );
    assert_eq!(
        validate_limit_amount(LimitType::Money, "0.01").unwrap(),
        "0.01"
    );

    for bad in ["", "0", "-5", "abc", "1e3", "NaN", "1,000", "Infinity"] {
        assert!(
            validate_limit_amount(LimitType::Money, bad).is_err(),
            "非法金额 {bad} 必须被拒绝"
        );
    }
}

#[test]
fn validate_token_amounts() {
    assert_eq!(
        validate_limit_amount(LimitType::Token, "5000000").unwrap(),
        "5000000"
    );

    for bad in [
        "",
        "0",
        "-1",
        "3.5",
        "1e6",
        "1_000",
        "5,000,000",
        "abc",
        "18446744073709551616", // u64 溢出
    ] {
        assert!(
            validate_limit_amount(LimitType::Token, bad).is_err(),
            "非法 token 数 {bad} 必须被拒绝"
        );
    }
}

// ---------- 指纹与遮蔽 ----------

#[test]
fn fingerprint_is_irreversible_and_stable() {
    let fp1 = credential_fingerprint("claude", "sk-test-abcdef123456");
    let fp2 = credential_fingerprint("claude", "sk-test-abcdef123456");
    let fp3 = credential_fingerprint("claude", "sk-test-different-key");

    assert_eq!(fp1, fp2, "同一 key 指纹必须一致");
    assert_ne!(fp1, fp3, "不同 key 指纹必须不同");
    assert_eq!(fp1.len(), 64, "SHA-256 hex 长度");
    // 指纹不能包含原始 key
    assert!(!fp1.contains("sk-test"));
}

#[test]
fn same_key_and_namespace_pair_across_databases() {
    let left = credential_fingerprint("anthropic", "sk-test-same-key");
    let right = credential_fingerprint("anthropic", "sk-test-same-key");

    assert_eq!(left, right);
    assert_eq!(left.len(), 64);
    assert!(left
        .chars()
        .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()));
}

#[test]
fn fingerprint_preserves_case_and_namespace() {
    assert_ne!(
        credential_fingerprint("anthropic", "sk-test-Key"),
        credential_fingerprint("anthropic", "sk-test-key")
    );
    assert_ne!(
        credential_fingerprint("anthropic", "sk-test-key"),
        credential_fingerprint("openai", "sk-test-key")
    );
}

#[test]
fn mask_credential_never_leaks_full_key() {
    assert_eq!(mask_credential("sk-ant-abc123def456"), "sk-****f456");
    assert_eq!(mask_credential("short"), "****");
    assert_eq!(mask_credential(""), "****");
    let masked = mask_credential("sk-1234567890");
    assert!(!masked.contains("1234567890"), "遮蔽后不得包含完整 key");
}

// ---------- 保存语义 ----------

#[test]
fn save_validates_and_persists_config() {
    let db = Database::memory().unwrap();
    db.save_provider("claude", &claude_provider("p1", "sk-test-save-key"))
        .unwrap();

    // Token 模式带币种 → 拒绝
    let config = UsageLimitConfig {
        enabled: true,
        limit_type: "token".to_string(),
        currency: Some("USD".to_string()),
        limit_amount: Some("1000".to_string()),
        reset_interval_seconds: None,
    };
    assert!(db.save_budget_config("p1", "claude", &config).is_err());

    // 金额模式缺币种 → 拒绝
    let config = UsageLimitConfig {
        enabled: true,
        limit_type: "money".to_string(),
        currency: None,
        limit_amount: Some("1000".to_string()),
        reset_interval_seconds: None,
    };
    assert!(db.save_budget_config("p1", "claude", &config).is_err());

    // 非法金额 → 拒绝，异常数据不进 SQLite
    let config = UsageLimitConfig {
        enabled: true,
        limit_type: "token".to_string(),
        currency: None,
        limit_amount: Some("-5".to_string()),
        reset_interval_seconds: None,
    };
    assert!(db.save_budget_config("p1", "claude", &config).is_err());
    assert!(db.get_api_key_limit("p1", "claude").unwrap().is_none());

    // 合法保存（CNY）
    let config = UsageLimitConfig {
        enabled: true,
        limit_type: "money".to_string(),
        currency: Some("CNY".to_string()),
        limit_amount: Some("50".to_string()),
        reset_interval_seconds: None,
    };
    let status = db.save_budget_config("p1", "claude", &config).unwrap();
    assert!(status.enabled);
    assert_eq!(status.limit_type.as_deref(), Some("money"));
    assert_eq!(status.currency.as_deref(), Some("CNY"));
    assert_eq!(status.limit_amount.as_deref(), Some("50"));
    assert_eq!(status.enforcement, EnforcementSupport::ProxyDisabled);

    // 关闭（enabled=false）保留配置
    let config = UsageLimitConfig {
        enabled: false,
        limit_type: "money".to_string(),
        currency: Some("CNY".to_string()),
        limit_amount: Some("50".to_string()),
        reset_interval_seconds: None,
    };
    let status = db.save_budget_config("p1", "claude", &config).unwrap();
    assert!(!status.enabled);
    assert_eq!(status.limit_amount.as_deref(), Some("50"), "关闭不能删配置");
    assert_eq!(status.state, "off");

    // 换 key → 保存时重绑新指纹 + 重置窗口
    let old_row = db.get_api_key_limit("p1", "claude").unwrap().unwrap();
    db.save_provider("claude", &claude_provider("p1", "sk-test-rotated-key-9999"))
        .unwrap();
    let config = UsageLimitConfig {
        enabled: true,
        limit_type: "money".to_string(),
        currency: Some("CNY".to_string()),
        limit_amount: Some("50".to_string()),
        reset_interval_seconds: None,
    };
    db.save_budget_config("p1", "claude", &config).unwrap();
    let new_row = db.get_api_key_limit("p1", "claude").unwrap().unwrap();
    assert_ne!(
        old_row.credential_fingerprint, new_row.credential_fingerprint,
        "保存时必须绑定当前 key 的指纹"
    );
    assert!(
        new_row.usage_start_at >= old_row.usage_start_at,
        "换 key 后统计窗口重置"
    );

    // OAuth provider（无静态 key）→ 拒绝
    let mut oauth_provider = claude_provider("p2", "sk-test-oauth");
    oauth_provider.settings_config = serde_json::json!({});
    oauth_provider.meta = Some(crate::provider::ProviderMeta {
        provider_type: Some("codex_oauth".to_string()),
        ..Default::default()
    });
    db.save_provider("claude", &oauth_provider).unwrap();
    let config = UsageLimitConfig {
        enabled: true,
        limit_type: "token".to_string(),
        currency: None,
        limit_amount: Some("1000".to_string()),
        reset_interval_seconds: None,
    };
    assert!(db.save_budget_config("p2", "claude", &config).is_err());
}

#[test]
fn save_persists_reset_interval_and_rejects_sub_hour_values() {
    let db = Database::memory().unwrap();
    db.save_provider(
        "claude",
        &claude_provider("p-reset-config", "sk-test-reset-config"),
    )
    .unwrap();

    let config = UsageLimitConfig {
        enabled: true,
        limit_type: "token".to_string(),
        currency: None,
        limit_amount: Some("1000".to_string()),
        reset_interval_seconds: Some(6 * 60 * 60),
    };
    let status = db
        .save_budget_config("p-reset-config", "claude", &config)
        .unwrap();
    assert_eq!(status.reset_interval_seconds, Some(6 * 60 * 60));
    assert_eq!(
        db.get_api_key_limit("p-reset-config", "claude")
            .unwrap()
            .unwrap()
            .reset_interval_seconds,
        Some(6 * 60 * 60)
    );

    let invalid = UsageLimitConfig {
        reset_interval_seconds: Some(30 * 60),
        ..config
    };
    let error = db
        .save_budget_config("p-reset-config", "claude", &invalid)
        .unwrap_err();
    assert!(error.to_string().contains("整小时"));
}

// ---------- 状态机 ----------

#[test]
fn status_states_off_active_warning_exhausted() {
    let db = Database::memory().unwrap();
    db.save_provider("claude", &claude_provider("p1", "sk-test-state-base-0000"))
        .unwrap();

    // 无配置 → off
    let status = db.get_budget_status("p1", "claude").unwrap();
    assert_eq!(status.state, "off");

    let fp = seed_limit(&db, "p1", true, "money", Some("USD"), "10");

    // 24% → active
    insert_log(&db, "r1", "p1", &fp, 1_000_100, 10, 10, "2.40");
    let status = db.get_budget_status("p1", "claude").unwrap();
    assert_eq!(status.state, "active");
    assert_eq!(
        status
            .percent_used
            .as_deref()
            .map(|p| Decimal::from_str(p).unwrap()),
        Some(Decimal::from(24))
    );

    // 85% → warning
    insert_log(&db, "r2", "p1", &fp, 1_000_200, 10, 10, "6.10");
    let status = db.get_budget_status("p1", "claude").unwrap();
    assert_eq!(status.state, "warning");

    // 104% → exhausted（数值不 clamp，进度条由前端视觉封顶）
    insert_log(&db, "r3", "p1", &fp, 1_000_300, 10, 10, "1.90");
    let status = db.get_budget_status("p1", "claude").unwrap();
    assert_eq!(status.state, "exhausted");
    assert_eq!(
        status
            .percent_used
            .as_deref()
            .map(|p| Decimal::from_str(p).unwrap()),
        Some(Decimal::from(104))
    );

    // 关闭 → off，但用量字段照常返回（同一指纹，仅开关翻转）
    seed_limit(&db, "p1", false, "money", Some("USD"), "10");
    let status = db.get_budget_status("p1", "claude").unwrap();
    assert_eq!(status.state, "off");
    assert_eq!(status.used_money_usd, "10.40");
}

#[test]
fn enforcement_reflects_proxy_toggle() {
    let db = Database::memory().unwrap();
    db.save_provider("claude", &claude_provider("p1", "sk-test-enforcement-key"))
        .unwrap();

    let status = db.get_budget_status("p1", "claude").unwrap();
    assert_eq!(status.enforcement, EnforcementSupport::ProxyDisabled);

    db.set_proxy_flags_sync("claude", true, false).unwrap();
    let status = db.get_budget_status("p1", "claude").unwrap();
    assert_eq!(status.enforcement, EnforcementSupport::Active);

    // masked_credential 绝不包含完整 key
    let masked = status.masked_credential.unwrap();
    assert!(!masked.contains("sk-test-enforcement-key"));
    assert!(masked.ends_with("****") || masked.contains("****"));
}

#[test]
fn reset_command_moves_window() {
    let db = Database::memory().unwrap();
    let fp = seed_limit(&db, "p1", true, "token", None, "100000");
    insert_log(&db, "r1", "p1", &fp, 1_000_100, 50_000, 50_000, "0.5");

    let before = db.get_budget_status("p1", "claude").unwrap();
    assert_eq!(before.used_tokens, 100_000);

    db.reset_budget_usage("p1", "claude").unwrap();

    let after = db.get_budget_status("p1", "claude").unwrap();
    assert_eq!(after.used_tokens, 0, "重置后窗口内用量为 0");
    assert!(after.usage_start_at.unwrap() >= before.usage_start_at.unwrap());
}
