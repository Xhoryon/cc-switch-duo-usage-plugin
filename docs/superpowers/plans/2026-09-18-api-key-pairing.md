# CC Switch Usage Duo API-Key Pairing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade the existing Usage Limit implementation into a same-API-key multi-gate system that enforces independent local budgets across an HTTP chain without introducing Switch/node pairing.

**Architecture:** Keep `proxy_request_logs` as the authoritative local proxy ledger and `api_key_limits` as local configuration. Add stable namespace-aware key fingerprints, a transactional TTL-backed reservation ledger, terminal upstream budget errors, and a protocol-safe streaming budget meter. Two independent Axum listeners with independent databases exercise the chain behavior.

**Tech Stack:** Rust/Tauri, SQLite/rusqlite, Axum/Hyper/Tokio, existing provider adapters and usage calculator, React/TypeScript, TanStack Query, shadcn/ui, Vitest.

**Spec:** `docs/superpowers/specs/2026-09-18-api-key-pairing-design.md`; authoritative requirements: `../../../../CC Switch Usage Duo Plugin — API Key Pairing Development Specification.md`.

## Implementation status (2026-09-18)

The implementation is complete against the authoritative specification, with the additional local reset schedule requested after the original plan: `NULL`/never, hourly, and daily windows are persisted by schema v22. The repository's existing architecture places the reusable streaming meter in `proxy/response_processor.rs` and the real two-listener HTTP coverage in `proxy/server.rs` tests rather than the originally proposed standalone files. Frontend status remains deliberately local/proxy-only; no node-pairing or upstream runtime state is shown. The release boundary is explicit: the host application remains CC Switch 3.20.3 while this repository publishes Usage Plugin v1.0.1 as tag `v1.0.1`, without signing.

## Global Constraints

- Pairing means `Same API Key → Same API Key Identity → Same Key Fingerprint`; never add Switch/node pairing.
- Do not add Pairing Secret, Node Registration, QR Pairing, Pairing Endpoint, Node Authorization, central registry, fixed computer binding, cross-device database synchronization, or a global chain counter.
- The fingerprint must use a stable provider namespace plus the exact API key; do not lowercase or uppercase the key.
- Never persist a new plaintext API-key copy, log a plaintext key, show a full fingerprint, return a fingerprint in an error, or use a fingerprint as an authentication token.
- Each Switch owns its own local limit, proxy ledger, reservation state, and reset; only `data_source = 'proxy'` usage is authoritative for enforcement.
- Any local or upstream budget block is terminal and must not trigger provider failover or bypass a limited Switch.
- Pre-request budget errors use HTTP 429 with machine-readable fields and no secrets.
- Streaming cutoff must happen at a protocol-safe boundary, cancel/drop the upstream stream, reconcile authoritative usage when available, and never crash or kill the process.
- Reuse the current normalized token semantics and pricing calculator; do not create a second usage or pricing engine.
- Preserve the existing staged Usage Limit work and unrelated user changes; never reset or checkout the worktree.

## File map

- Modify `src-tauri/src/services/usage_limit.rs` for namespace-aware identity, budget state, and the service-facing reservation API.
- Create `src-tauri/src/database/dao/budget_reservation.rs` for reservation CRUD, atomic queries, reconciliation, and stale cleanup.
- Modify `src-tauri/src/database/dao/mod.rs`, `src-tauri/src/database/mod.rs`, and `src-tauri/src/database/schema.rs` for v21 migration, exports, and indexes.
- Modify `src-tauri/src/proxy/handler_context.rs`, `src-tauri/src/proxy/forwarder.rs`, `src-tauri/src/proxy/error.rs`, `src-tauri/src/proxy/error_mapper.rs`, and `src-tauri/src/proxy/handlers.rs` for request IDs, gate lifecycle, and terminal upstream errors.
- Create `src-tauri/src/proxy/streaming_budget.rs` for the reusable meter and event decisions; modify `src-tauri/src/proxy/response_processor.rs` and transformed streaming call sites to pass it through.
- Modify `src-tauri/src/proxy/usage/logger.rs` and response logging callbacks for reconciliation metadata and cutoff diagnostics.
- Create `src-tauri/tests/usage_duo_chain.rs` for independent listener/database chain tests; keep focused unit tests beside their modules.
- Modify `src/types/usageLimit.ts`, `src/lib/api/usageLimit.ts`, `src/lib/query/usageLimit.ts`, `src/components/usage-limit/UsageLimitButton.tsx`, `src/components/usage-limit/UsageLimitDialog.tsx`, provider integration, and four locale files for pricing/upstream status copy.
- Add or modify `tests/components/UsageLimitDialog.test.tsx` and focused frontend tests for the new statuses and local-only wording.
- Update `docs/development_log.md`, root `工作日志.md`, and root `知识库.md` after implementation and verification.

---

### Task 1: Lock down the stable API-key identity contract

**Files:**
- Modify: `src-tauri/src/services/usage_limit.rs`
- Test: `src-tauri/src/services/usage_limit/tests.rs`

**Interfaces:**
- Produce `pub fn credential_namespace(app_type: &AppType) -> &'static str`.
- Produce `pub fn credential_fingerprint(namespace: &str, api_key: &str) -> String`.
- Update `resolve_credential(provider, app_type)` to use the stable namespace and exact extracted key.
- Keep masked-key rendering separate from fingerprinting.

- [ ] **Step 1: Write failing identity tests**

Add tests for same namespace/key equality across independent databases, different key/namespace isolation, case sensitivity, 64 lowercase hex output, and adapter-extracted credentials.

```rust
#[test]
fn same_key_and_namespace_pair_across_databases() {
    let left = credential_fingerprint("anthropic", "sk-test-same-key");
    let right = credential_fingerprint("anthropic", "sk-test-same-key");
    assert_eq!(left, right);
    assert_eq!(left.len(), 64);
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
```

- [ ] **Step 2: Run the focused test and verify the old contract fails**

Run: `cd src-tauri && RUSTUP_TOOLCHAIN=stable cargo test --lib usage_limit::tests::same_key_and_namespace_pair_across_databases`

Expected: compile failure or assertion failure because the current helper hashes only the key.

- [ ] **Step 3: Implement the identity contract**

Hash `namespace.as_bytes()`, a zero separator, and `api_key.as_bytes()` with SHA-256. Derive the namespace only from stable `AppType`/adapter protocol family, never from local provider UUID, machine name, Switch address, or database path. Preserve established trimming but never add case conversion. Update all existing call sites.

- [ ] **Step 4: Run identity and previous Usage Limit tests**

Run: `cd src-tauri && RUSTUP_TOOLCHAIN=stable cargo test --lib usage_limit`

Expected: all identity and existing limit tests pass, with no new plaintext key storage or logging.

- [ ] **Step 5: Commit only this task's files**

Run: `git add src-tauri/src/services/usage_limit.rs src-tauri/src/services/usage_limit/tests.rs && git commit --only src-tauri/src/services/usage_limit.rs src-tauri/src/services/usage_limit/tests.rs -m "feat: make usage identity namespace-aware"`.

### Task 2: Add the reservation schema and DAO with crash recovery

**Files:**
- Modify: `src-tauri/src/database/mod.rs`
- Modify: `src-tauri/src/database/schema.rs`
- Modify: `src-tauri/src/database/dao/mod.rs`
- Create: `src-tauri/src/database/dao/budget_reservation.rs`
- Test: migration tests and reservation DAO tests

**Interfaces:**
- Produce `BudgetReservationRow` with request ID, provider/app scope, fingerprint, limit type, reserved/consumed token and cost values, status, and timestamps.
- Produce `Database::insert_pending_budget_reservation(row: &BudgetReservationRow) -> Result<(), AppError>` with atomic insufficient-budget rejection.
- Produce `Database::reconcile_budget_reservation(request_id: &str, consumed_tokens: i64, consumed_cost_usd: &str, now: i64) -> Result<(), AppError>`, `release_budget_reservation(request_id: &str, now: i64) -> Result<(), AppError>`, `cleanup_stale_budget_reservations(now: i64) -> Result<usize, AppError>`, and `sum_pending_budget_reservations(provider_id: &str, app_type: &str, fingerprint: &str, now: i64) -> Result<PendingBudgetTotals, AppError>`.

- [ ] **Step 1: Write failing migration/DAO tests**

Test v20→v21 creates `budget_reservations` and its index; pending rows contribute to totals; reconcile/release removes them from active totals; duplicate request IDs are rejected or safely reused; stale rows are released without deleting `proxy_request_logs`.

```rust
#[test]
fn stale_pending_reservations_are_released_without_touching_usage_logs() {
    let db = Database::memory().unwrap();
    let now = 2_000_000;
    insert_proxy_usage_fixture(&db, "usage-1");
    db.insert_pending_budget_reservation(
        "request-1", "provider-1", "claude", "fingerprint", "token",
        100, "0", now - 20, now - 1,
    ).unwrap();

    assert_eq!(db.cleanup_stale_budget_reservations(now).unwrap(), 1);
    assert_eq!(db.count_proxy_usage_fixture("usage-1").unwrap(), 1);
    assert_eq!(db.sum_pending_budget_reservations("provider-1", "claude", "fingerprint", now).unwrap(), 0);
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run: `cd src-tauri && RUSTUP_TOOLCHAIN=stable cargo test --lib budget_reservation schema_migration`

Expected: failure because schema version is 20 and the table/DAO methods do not exist.

- [ ] **Step 3: Implement migration v21 defensively**

Increment `SCHEMA_VERSION` to 21, add migration dispatch, use `CREATE TABLE IF NOT EXISTS`, and guard index creation with the same parent-column checks used by the v20 migration. The table must contain no plaintext credential field.

- [ ] **Step 4: Implement transactional DAO methods**

Use the existing locked SQLite connection. Insert only when current pending reservations plus the caller's requested reservation fit the effective remaining budget. Store money as decimal strings, treat only non-expired `pending` rows as active, and mark stale rows `released` rather than deleting usage logs.

- [ ] **Step 5: Run focused migration/DAO tests and commit**

Run: `cd src-tauri && RUSTUP_TOOLCHAIN=stable cargo test --lib budget_reservation schema_migration`. Then run `git add src-tauri/src/database/mod.rs src-tauri/src/database/schema.rs src-tauri/src/database/dao/mod.rs src-tauri/src/database/dao/budget_reservation.rs && git commit --only src-tauri/src/database/mod.rs src-tauri/src/database/schema.rs src-tauri/src/database/dao/mod.rs src-tauri/src/database/dao/budget_reservation.rs -m "feat: add budget reservation ledger"`.

### Task 3: Implement the atomic local Budget Gate and request lifecycle

**Files:**
- Modify: `src-tauri/src/services/usage_limit.rs`
- Modify: `src-tauri/src/proxy/handler_context.rs`
- Modify: `src-tauri/src/proxy/forwarder.rs`
- Modify: `src-tauri/src/proxy/usage/logger.rs`
- Test: usage-limit and forwarder tests

**Interfaces:**
- Produce `BudgetReservation` with request ID, provider/app scope, fingerprint, limit type, reserved amounts, and expiry.
- Produce `Database::reserve_budget_for_request(request_id: &str, provider_id: &str, app_type: &str, fingerprint: &str, request: &serde_json::Value) -> Result<BudgetReservation, BudgetRejection>`.
- Add request-scoped `budget_request_id` and `budget_reservation` state to the forwarding lifecycle.

- [ ] **Step 1: Write failing lifecycle tests**

Cover first-request reservation, second same-key request blocked by committed plus pending usage, failed-attempt release, successful reconciliation, disabled-limit no-reservation, different-fingerprint isolation, and blocked request making zero upstream transport calls.

- [ ] **Step 2: Run the focused tests and verify failure**

Run: `cd src-tauri && RUSTUP_TOOLCHAIN=stable cargo test --lib reserve_budget concurrent_budget`

Expected: failure because service methods and lifecycle fields are absent.

- [ ] **Step 3: Add request IDs and conservative reservation sizing**

Generate an internal UUID in `RequestContext::new` and pass it into the forwarder. For token mode, use known input plus `max_tokens`/`max_output_tokens`; when no safe output bound exists, reserve the full remaining token budget. For money mode, use the existing model-pricing path; if a safe price is unavailable, return `PRICING_UNAVAILABLE` before forwarding rather than treating cost as zero.

- [ ] **Step 4: Integrate per-attempt reserve, release, and reconcile**

Keep `circuit permit → budget gate → forward`. Release reservation and permit before provider failover. Attach the successful reservation to `ForwardResult`, write authoritative proxy usage with the existing normalized rules, then reconcile/release. Do not count CLI/session imports.

- [ ] **Step 5: Run focused tests and commit**

Run: `cd src-tauri && RUSTUP_TOOLCHAIN=stable cargo test --lib usage_limit forwarder`; then run `git add src-tauri/src/services/usage_limit.rs src-tauri/src/proxy/handler_context.rs src-tauri/src/proxy/forwarder.rs src-tauri/src/proxy/usage/logger.rs && git commit --only src-tauri/src/services/usage_limit.rs src-tauri/src/proxy/handler_context.rs src-tauri/src/proxy/forwarder.rs src-tauri/src/proxy/usage/logger.rs -m "feat: enforce per-key budget reservations"`.

### Task 4: Make budget errors terminal across provider and Switch hops

**Files:**
- Modify: `src-tauri/src/proxy/error.rs`
- Modify: `src-tauri/src/proxy/error_mapper.rs`
- Modify: `src-tauri/src/proxy/forwarder.rs`
- Modify: `src-tauri/src/proxy/handlers.rs`
- Test: error and failover tests

**Interfaces:**
- Add a distinct `ProxyError::UpstreamBudgetBlocked` terminal variant.
- Add `is_cc_switch_budget_response(status, headers) -> bool`.
- Add a safe `X-CC-Switch-Usage-Limit: 1` marker to local limit responses.

- [ ] **Step 1: Write failing terminal-error tests**

Assert local errors serialize as HTTP 429 with `type = "cc_switch_usage_limit"` and `code = "API_KEY_LIMIT_REACHED"`; assert the marker creates `UpstreamBudgetBlocked`; assert categorization is non-retryable; assert a second provider is never attempted.

- [ ] **Step 2: Run focused tests and verify failure**

Run: `cd src-tauri && RUSTUP_TOOLCHAIN=stable cargo test --lib budget_error upstream_budget`

Expected: failure because the variant and marker handling do not exist.

- [ ] **Step 3: Implement safe serialization and upstream recognition**

Add only machine-readable limit fields, no key/fingerprint/node data. Inspect the exact marker plus 429 immediately after `forward()` and before provider-success/failover handling. Release reservation and circuit permit, then return the terminal variant. Ordinary third-party 429 responses remain ordinary responses without the marker.

- [ ] **Step 4: Run error/failover tests and commit**

Run: `cd src-tauri && RUSTUP_TOOLCHAIN=stable cargo test --lib forwarder budget_error`; then run `git add src-tauri/src/proxy/error.rs src-tauri/src/proxy/error_mapper.rs src-tauri/src/proxy/forwarder.rs src-tauri/src/proxy/handlers.rs && git commit --only src-tauri/src/proxy/error.rs src-tauri/src/proxy/error_mapper.rs src-tauri/src/proxy/forwarder.rs src-tauri/src/proxy/handlers.rs -m "feat: make usage limits terminal across hops"`.

### Task 5: Add the streaming budget meter and safe cutoff

**Files:**
- Create: `src-tauri/src/proxy/streaming_budget.rs`
- Modify: `src-tauri/src/proxy/response_processor.rs`
- Modify: streaming paths in `src-tauri/src/proxy/handlers.rs`
- Modify: `src-tauri/src/proxy/usage/logger.rs`
- Test: streaming-budget and response-processor tests

**Interfaces:**
- Produce `StreamingBudgetMeter::new(reservation: BudgetReservation, limit: BudgetLimit) -> Result<Self, AppError>`, `observe_event(&mut self, event: &Value) -> StreamBudgetDecision`, and `finish_with_authoritative_usage(&mut self, usage: Option<&TokenUsage>) -> Result<(), AppError>`.
- Produce `StreamBudgetDecision::{Forward, Cutoff { reason }}`.
- Extend `create_logged_passthrough_stream` with an optional meter while keeping existing non-budget callers valid.

- [ ] **Step 1: Write failing deterministic SSE tests**

Use finite SSE blocks and assert events below the limit are forwarded, the first event at/over the limit is not forwarded, the upstream stream is dropped, final usage reconciles when present, missing final usage records provisional consumption with `limit_reached_midstream`, and no partial SSE block is emitted.

```rust
#[tokio::test]
async fn meter_stops_after_protocol_safe_event_boundary() {
    let mut meter = test_meter_with_token_limit(3);
    assert_eq!(meter.observe_event(&usage_event(2)), StreamBudgetDecision::Forward);
    assert!(matches!(
        meter.observe_event(&usage_event(3)),
        StreamBudgetDecision::Cutoff { .. }
    ));
}
```

- [ ] **Step 2: Run focused tests and verify failure**

Run: `cd src-tauri && RUSTUP_TOOLCHAIN=stable cargo test --lib streaming_budget response_processor::tests::meter`

Expected: failure because the meter and stream hook do not exist.

- [ ] **Step 3: Implement protocol-safe metering**

Reuse current parser configuration and normalized token semantics. Prefer cumulative/delta provider usage fields. For events without usage, use the parser's conservative provisional output estimate. Return `Cutoff` before yielding the over-limit event.

- [ ] **Step 4: Wire all streaming response paths**

Pass the meter through generic passthrough and transformed Claude/Codex/Gemini paths. Run it on the final protocol visible to the Agent while retaining `SseUsageCollector` for authoritative accounting. Break the stream on cutoff so dropping the upstream stream cancels it; reconcile/release afterward.

- [ ] **Step 5: Run stream tests and commit**

Run: `cd src-tauri && RUSTUP_TOOLCHAIN=stable cargo test --lib streaming_budget response_processor`; then run `git add src-tauri/src/proxy/streaming_budget.rs src-tauri/src/proxy/response_processor.rs src-tauri/src/proxy/handlers.rs src-tauri/src/proxy/usage/logger.rs && git commit --only src-tauri/src/proxy/streaming_budget.rs src-tauri/src/proxy/response_processor.rs src-tauri/src/proxy/handlers.rs src-tauri/src/proxy/usage/logger.rs -m "feat: enforce usage limits during streaming"`.

### Task 6: Build the real two-Switch HTTP integration harness

**Files:**
- Create: `src-tauri/tests/usage_duo_chain.rs`
- Modify: `src-tauri/src/proxy/server.rs` only if test-only independent database/listener injection is required
- Test: `src-tauri/tests/usage_duo_chain.rs`

**Interfaces:**
- Produce `spawn_mock_provider`, `spawn_switch`, `send_agent_request`, and `shutdown_listener` helpers with independent listeners and databases.
- The topology must be actual HTTP: Agent → Switch 2 listener → Switch 1 listener → Mock Provider listener.

- [ ] **Step 1: Write the failing topology test**

Start three listeners, configure Switch 2's upstream as Switch 1, configure Switch 1's upstream as the mock provider, send an HTTP request with the same synthetic key, and assert the mock provider counter increments once.

- [ ] **Step 2: Run the topology test and verify failure**

Run: `cd src-tauri && RUSTUP_TOOLCHAIN=stable cargo test --test usage_duo_chain same_key_chain_reaches_provider`

Expected: failure because the independent listener harness does not exist.

- [ ] **Step 3: Implement independent listener helpers**

Use ephemeral `127.0.0.1:0` listeners, independent databases, explicit shutdown channels, and synthetic test credentials only. Do not use a shared global database or a same-function fake gate.

- [ ] **Step 4: Add required chain cases**

Cover same-key identity, different-key isolation, downstream lower limit, upstream lower limit, terminal upstream error/no failover, reset isolation, key replacement isolation, and two local ledger rows for a successful chain request. Use request counters to prove blocked traffic does not reach the next hop.

- [ ] **Step 5: Run chain tests and commit**

Run: `cd src-tauri && RUSTUP_TOOLCHAIN=stable cargo test --test usage_duo_chain`; then run `git add src-tauri/tests/usage_duo_chain.rs src-tauri/src/proxy/server.rs && git commit --only src-tauri/tests/usage_duo_chain.rs src-tauri/src/proxy/server.rs -m "test: cover two-switch usage chain"`.

### Task 7: Add concurrency, streaming-chain, and regression coverage

**Files:**
- Modify: `src-tauri/src/services/usage_limit/tests.rs`
- Modify: `src-tauri/tests/usage_duo_chain.rs`
- Modify: existing regression tests only when the new public contract requires an assertion update

- [ ] **Step 1: Add concurrent same-key requests**

Launch concurrent requests at one listener and assert pending reservations participate in the gate decision, no large budget breach occurs, normalized token totals are correct, and no writes are lost.

- [ ] **Step 2: Add source-isolation and reset tests**

Insert identical activity as a proxy row and a `session_log` row; assert only the proxy row contributes. Reset Switch 2 and assert Switch 1's row, reservation, and status remain unchanged.

- [ ] **Step 3: Add downstream/upstream mid-stream tests**

Use the real two-listener topology with a deterministic mock SSE provider. Downstream exhaustion must stop Agent output and cancel upstream; upstream exhaustion must stop Switch 1 and make Switch 2 terminate without forwarding new content.

- [ ] **Step 4: Run focused and full Rust suites**

Run:

```bash
cd src-tauri
RUSTUP_TOOLCHAIN=stable cargo test --lib usage_limit forwarder response_processor
RUSTUP_TOOLCHAIN=stable cargo test --test usage_duo_chain
RUSTUP_TOOLCHAIN=stable cargo test
```

Expected: all new and existing Rust tests pass; only pre-existing ignored tests remain ignored.

- [ ] **Step 5: Commit test coverage**

Run: `git add src-tauri/src/services/usage_limit/tests.rs src-tauri/tests/usage_duo_chain.rs && git commit --only src-tauri/src/services/usage_limit/tests.rs src-tauri/tests/usage_duo_chain.rs -m "test: cover reservation and streaming regressions"`.

### Task 8: Update frontend status and user-facing contract

**Files:**
- Modify: `src/types/usageLimit.ts`
- Modify: `src/lib/api/usageLimit.ts`
- Modify: `src/lib/query/usageLimit.ts`
- Modify: `src/components/usage-limit/UsageLimitButton.tsx`
- Modify: `src/components/usage-limit/UsageLimitDialog.tsx`
- Modify: provider card/action integration and four locale files
- Test: `tests/components/UsageLimitDialog.test.tsx` and focused frontend tests

**Interfaces:**
- Extend status values with `pricing_unavailable` and a transient upstream-block indicator without node/pairing fields.
- Preserve TanStack Query invalidation on proxy usage events.

- [ ] **Step 1: Add failing frontend tests**

Assert pricing unavailable warns clearly and does not claim `$0` safety; upstream copy says only that an upstream usage limit blocked the request; reset copy says it is local; and no UI string contains Paired Switch, Node ID, Pairing Secret, or full fingerprint.

- [ ] **Step 2: Run focused frontend tests and verify failure**

Run: `pnpm exec vitest run tests/components/UsageLimitDialog.test.tsx`

Expected: failure because the new statuses/copy are not represented.

- [ ] **Step 3: Implement type/query/UI changes**

Render the new statuses, preserve masked-key display, and keep all limit state explicitly local/proxy-only. Do not add a pairing page, remote registration UI, or synchronization control.

- [ ] **Step 4: Run frontend checks and commit**

Run: `pnpm exec vitest run tests/components/UsageLimitDialog.test.tsx && pnpm typecheck && pnpm format:check`; then run `git add src/types/usageLimit.ts src/lib/api/usageLimit.ts src/lib/query/usageLimit.ts src/components/usage-limit src/components/providers src/i18n/locales/en.json src/i18n/locales/ja.json src/i18n/locales/zh.json src/i18n/locales/zh-TW.json tests/components/UsageLimitDialog.test.tsx && git commit --only src/types/usageLimit.ts src/lib/api/usageLimit.ts src/lib/query/usageLimit.ts src/components/usage-limit src/components/providers src/i18n/locales/en.json src/i18n/locales/ja.json src/i18n/locales/zh.json src/i18n/locales/zh-TW.json tests/components/UsageLimitDialog.test.tsx -m "feat: expose local and upstream budget states"`.

### Task 9: Update logs, knowledge base, and final verification

**Files:**
- Modify: `docs/development_log.md`
- Modify: local working notes outside this repository (not published)
- Modify: local knowledge base outside this repository (not published)
- Modify: verification result files only through the existing scripts

- [x] **Step 1: Record the complete implementation**

Record goal, correct pairing model, namespace fingerprint, local independence, reservation lifecycle/TTL, Budget Gate order, streaming cutoff, proxy-only ledger, upstream terminal error, database v21, tests, known limitations, and next steps. Include exactly:

> Usage Duo pairs usage-control state by API-key identity, not by CC Switch node identity. Multiple CC Switch instances do not need to pair with each other.

and:

> Each CC Switch maintains its own local limit and authoritative proxy ledger. The first exhausted gate in the request chain stops further token delivery.

- [x] **Step 2: Run complete verification**

Run from `cc-switch/`:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export RUSTUP_TOOLCHAIN=stable
pnpm typecheck
pnpm format:check
pnpm test:unit
cd src-tauri
cargo fmt --check
cargo clippy --tests
cargo test
cd ..
pnpm build
```

Expected: all required commands pass. If the pinned 1.95 toolchain remains damaged, record the stable-toolchain workaround without changing unrelated toolchain files. Current evidence uses the installed local Node CLIs because Corepack signature verification fails for the pinned pnpm package, and uses `RUSTUP_TOOLCHAIN=stable` for Rust/Tauri.

- [x] **Step 3: Run existing verification scripts**

Run `bash quick-verify.sh > quick-verify-results.txt 2>&1` and `bash run-verify.sh > verify-results.txt 2>&1`; inspect exit codes and record results.

- [x] **Step 4: Audit forbidden architecture and secrets**

Search changed files for `Pairing Secret`, `Node Pairing`, `QR Code`, `Node Registration`, `Paired Switch`, new plaintext-key fields, full key values, and full fingerprints in logs/UI/errors/tests outside synthetic fixtures.

- [x] **Step 5: Update and commit completion documentation**

Release documentation, four locale README files, the v1.0.1 release notes, plugin manifest, and the unsigned GitHub release workflow are included in the final release commit. The root workspace logs are updated in the same completion change but remain outside this repository's commit.

The code and verification checklist below records implementation evidence. The final release commit and `v1.0.1` tag are created only after the fresh verification gate passes.

## Self-review checklist

- [x] Sections 1–8: identity-only pairing and no node protocol covered by Tasks 1, 3, 4, and 9.
- [x] Sections 9–20: independent limits, request gate, terminal upstream block, and no bypass covered by Tasks 2–4 and 6.
- [x] Sections 21–27: streaming meter, cutoff, cancellation, provisional usage, and reconciliation covered by Tasks 5 and 7.
- [x] Sections 28–36: normalized tokens, pricing, CNY, unknown pricing, proxy-only ledger, and no global counter covered by Tasks 3, 7, and 9.
- [x] Sections 37–43: reservations, request binding, lifecycle, TTL recovery, structured 429, and terminal errors covered by Tasks 2–4.
- [x] Sections 44–57: remote boundary, UI, local reset, SQLite/index, frontend/backend reuse covered by Tasks 2, 4, 8, and 9.
- [x] Sections 58–71: HTTP topology, streaming, concurrency, double-count, reset/key replacement, and forbidden architectures covered by Tasks 6, 7, and 9.
- [x] Sections 72–75: Definition of Done, preflight inspection, development log, release notes, and final report covered by Task 9.
- [x] No plan placeholder remains; every task has files, interfaces, failing-test steps, implementation steps, and validation commands.
