# CC Switch Usage Duo API-Key Pairing Design

**Date:** 2026-09-18  
**Status:** Approved in chat; ready for implementation planning  
**Source specification:** `../../../../CC Switch Usage Duo Plugin — API Key Pairing Development Specification.md`

> Usage Duo pairs usage-control state by API-key identity, not by CC Switch node identity. Multiple CC Switch instances do not need to pair with each other.

## Goal

Upgrade the existing single-node API-key usage-limit implementation into a local multi-gate enforcement model. A request may traverse `Agent → Switch 2 → Switch 1 → Provider`; each Switch independently identifies the API key, checks its own local budget, records only its own authoritative proxy usage, and stops the chain when its gate is exhausted.

The implementation must not add node pairing, pairing secrets, QR codes, node registration, remote Switch synchronization, or a global chain counter.

## Current baseline

The repository already contains the first Usage Limit implementation in the staged worktree:

- `api_key_limits` stores one local limit configuration per provider/application scope.
- `proxy_request_logs` is the authoritative local usage source when `data_source = 'proxy'`.
- `forward_with_retry_inner` checks the budget before each provider attempt.
- Usage is associated with a SHA-256 credential fingerprint and is not stored as plaintext.
- The frontend already exposes OFF/ACTIVE/WARNING/EXHAUSTED configuration and status UI.

The baseline still needs the Duo-specific reservation lifecycle, upstream budget-error propagation, streaming hard cutoff, and real HTTP chain integration tests.

## Identity model

### Stable fingerprint

`resolve_credential` remains the single credential extraction entry point for both configuration and forwarding. It must use the same adapter/auth strategy used to build the outbound request, preserve the credential's case, and avoid creating another plaintext copy outside the existing in-memory request/provider values.

The fingerprint input is a stable provider namespace plus the exact extracted API key:

```text
SHA-256(provider_namespace || "\0" || exact_api_key)
```

`provider_namespace` is derived from the stable application/provider protocol family, never from a local provider UUID, local machine name, Switch address, or database path. The local provider ID remains a scope for the local limit configuration, not part of cross-device identity. No lowercasing or uppercasing is permitted; only the existing credential extractor's established trimming/normalization may be used.

The resulting lowercase 64-character hexadecimal value is internal-only. It may be used for lookup, aggregation, and reservation binding, but never as an authentication token, provider credential, remote login secret, UI value, or error payload. Logs and UI may show only the existing masked-key form.

### Local independence

The same API key on two computers produces the same fingerprint, but the computers do not exchange state. Each Switch owns:

- its local limit configuration;
- its local `proxy_request_logs` ledger;
- its local pending reservations;
- its local runtime upstream-limit status.

Resetting or changing a limit on one Switch does not mutate another Switch. A key replacement creates a new identity binding and starts a new local usage window.

## Budget data model

### Existing limit table

Keep the existing `api_key_limits` table and its local provider/application scope. The row continues to store configuration and `usage_start_at`; it does not store a redundant usage counter. The effective identity for the row is its credential fingerprint.

### Reservation table

Add schema migration v21 with a local `budget_reservations` table:

```sql
CREATE TABLE budget_reservations (
    request_id TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL,
    app_type TEXT NOT NULL,
    credential_fingerprint TEXT NOT NULL,
    limit_type TEXT NOT NULL,
    reserved_tokens INTEGER NOT NULL DEFAULT 0,
    reserved_cost_usd TEXT NOT NULL DEFAULT '0',
    consumed_tokens INTEGER NOT NULL DEFAULT 0,
    consumed_cost_usd TEXT NOT NULL DEFAULT '0',
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    FOREIGN KEY (provider_id, app_type)
        REFERENCES providers(id, app_type) ON DELETE CASCADE
)
```

Create an index on `(provider_id, app_type, credential_fingerprint, status, expires_at)`. Valid statuses are `pending`, `committed`, and `released`. Only non-expired `pending` rows participate in the effective remaining calculation. A cleanup operation marks stale pending rows as `released`; it never deletes usage logs.

### Reservation semantics

Expose a backend service boundary with these operations:

```rust
pub fn reserve_budget(
    &self,
    request_id: &str,
    provider: &Provider,
    app_type: &AppType,
    fingerprint: &str,
    request: &serde_json::Value,
) -> Result<BudgetReservation, BudgetError>;

pub fn reconcile_budget_reservation(
    &self,
    reservation: &BudgetReservation,
    actual: &TokenUsage,
    cost_usd: &Decimal,
) -> Result<(), AppError>;

pub fn release_budget_reservation(
    &self,
    request_id: &str,
) -> Result<(), AppError>;

pub fn cleanup_stale_budget_reservations(&self, now: i64) -> Result<usize, AppError>;
```

The reserve operation is serialized with the budget query in the existing database mutex/transaction boundary. It computes:

```text
effective_remaining = configured_limit - committed_proxy_usage - pending_reservations
```

For token limits, the request's declared input and `max_tokens`/`max_output_tokens` are used when both are reliably available; when no safe upper bound is available, the reservation uses the entire remaining token budget. For money limits, the same estimate is priced with the existing pricing calculator; if a safe price cannot be established, the request is rejected with `PRICING_UNAVAILABLE` instead of treating cost as zero. This is conservative by design and prevents concurrent requests from bypassing the hard limit.

On completion, actual authoritative usage is written through the existing usage logger and the reservation is reconciled/released. A failed request releases its reservation. A mid-stream cutoff commits the consumed provisional usage, releases the unused amount, and records the cutoff reason. Reservation TTL is short enough to recover from a crashed process without permanently blocking the key.

## Request and forwarding flow

The request path is:

```text
Agent request
  ↓
Switch 2: resolve exact API key → stable fingerprint → reserve/check local gate
  ↓
Switch 1: resolve the same API key → same fingerprint → reserve/check local gate
  ↓
Provider
```

Implementation changes:

1. Generate a per-request budget request ID in `RequestContext` and carry the reservation through `ForwardResult` and response processing.
2. Run the atomic budget gate after circuit-breaker permission and immediately before `forward()` for each provider attempt.
3. If the local gate rejects, release any circuit permit, return a structured HTTP 429 budget error, and stop the retry loop.
4. If an upstream Switch returns the Duo structured budget error, recognize it before normal provider-success/failover handling, convert it to a terminal `UpstreamBudgetBlocked` error, and do not try another provider or bypass the upstream Switch.
5. Do not count CLI/session imports, `session_log`, `codex_session`, or other non-proxy sources toward the gate. Two Switches recording the same request in two local databases is expected and is not a global double count.

## Structured errors

Local pre-request rejection returns HTTP 429 with a safe body:

```json
{
  "error": {
    "type": "cc_switch_usage_limit",
    "code": "API_KEY_LIMIT_REACHED",
    "limit_type": "token",
    "used": 1000000,
    "limit": 1000000
  }
}
```

The body contains no plaintext credential, fingerprint, machine ID, provider secret, or node identity. Upstream blocking may add a safe runtime detail such as `source = "upstream"`; it must not identify a particular remote computer. Budget errors are terminal/non-retryable and remain distinct from network, provider, timeout, and circuit-breaker failures.

## Streaming hard cutoff

Extend the current `create_logged_passthrough_stream` path with a `StreamingBudgetMeter` owned by the response stream. The meter receives complete SSE event blocks at the existing `take_sse_block` boundary and exposes:

```rust
pub struct StreamingBudgetMeter { /* reservation + provisional state */ }

impl StreamingBudgetMeter {
    pub fn observe_event(&mut self, event: &serde_json::Value) -> StreamBudgetDecision;
    pub fn finish_with_authoritative_usage(&mut self, usage: Option<&TokenUsage>);
}

pub enum StreamBudgetDecision {
    Forward,
    Cutoff { reason: &'static str },
}
```

The meter first consumes provider-reported cumulative/delta usage when the protocol exposes it. For events without usage, it applies the protocol parser's conservative provisional output estimate so that a stream cannot continue indefinitely while waiting for a final usage event. The final provider usage, when available, is reconciled through the existing normalized token/cost rules; a cutoff without a final usage event keeps the consumed provisional amount and marks the log as `limit_reached_midstream`.

At a cutoff, the proxy stops yielding new content only after a complete protocol-safe event, drops the upstream stream to cancel the connection, finalizes the local reservation, and closes the downstream body cleanly. It must not panic, kill the process, corrupt SQLite, or inject invalid protocol data. If a protocol supports a valid terminal quota event, it may be emitted; otherwise graceful stream termination plus the local diagnostic is used.

## Frontend behavior

Reuse the existing React/TanStack Query/shadcn UI. Do not add a Pairing page or any Switch/node fields. Keep the masked API-key identity display and local OFF/ACTIVE/WARNING/EXHAUSTED states. Add:

- `PRICING_UNAVAILABLE` status for money limits whose model price is not safely known;
- transient `UPSTREAM_LIMIT_REACHED` runtime status/message when an upstream Switch returns the structured budget error;
- copy explaining that limits apply only to traffic routed through the local proxy and that reset is local to the current Switch.

The UI must never show a full fingerprint, full API key, paired computer, node ID, pairing secret, node latency, or global quota claim.

## Testing strategy

### Unit tests

Add tests for:

- same stable namespace + same exact key → same fingerprint across independent databases;
- different exact keys or namespaces → different fingerprints and isolated limits;
- case sensitivity and no plaintext persistence/logging;
- reservation accounting: committed usage + pending reservations cannot exceed the limit;
- reservation release, reconciliation, TTL cleanup, and crash-like stale rows;
- key replacement and reset isolation;
- money/token/USD/CNY behavior and unknown pricing fail-closed behavior;
- proxy-only ledger filtering and session-log exclusion.

### HTTP chain integration tests

Create a test harness with three independent Axum listeners and databases:

```text
Mock Provider
      ↑
Switch 1 listener + DB 1
      ↑
Switch 2 listener + DB 2
      ↑
Mock Agent
```

The harness must send actual HTTP requests through both listeners. It covers same-key identity, different-key isolation, downstream lower limit, upstream lower limit, terminal upstream error/no failover, reset isolation, key replacement, and a provider request counter proving that blocked requests never reach the next hop.

### Streaming and concurrency tests

Use deterministic mock SSE streams to cover downstream and upstream mid-stream cutoff, upstream cancellation, protocol-safe boundaries, final reconciliation, and no malformed terminal output. Launch concurrent same-key requests against one listener and assert that pending reservations participate in the gate decision.

Existing single-Switch regression tests remain mandatory. The full validation set is frontend typecheck/format/unit tests, `cargo fmt --check`, `cargo clippy --tests`, all Rust tests, and the production build with the repository's working stable toolchain when the pinned local toolchain is unavailable.

## Files and boundaries

Expected primary changes:

- `src-tauri/src/database/schema.rs`, `src-tauri/src/database/mod.rs`, and a new reservation DAO module for v21 migration and persistence.
- `src-tauri/src/services/usage_limit.rs` plus focused identity, reservation, and streaming-budget modules where the existing file would otherwise mix responsibilities.
- `src-tauri/src/proxy/forwarder.rs`, `handler_context.rs`, `error.rs`, `error_mapper.rs`, `response_processor.rs`, and usage logger plumbing for lifecycle, terminal errors, and streaming cutoff.
- `src-tauri/tests/` or the existing Rust test modules for the real HTTP chain and stream harness.
- `src/components/usage-limit/`, provider card integration, four locale files, and usage-limit query/types for UI status/copy.
- `docs/development_log.md`, root `工作日志.md`, and root `知识库.md` for completion records and reusable project facts.

No new pairing service, remote endpoint, node registry, QR flow, cross-device synchronization, or global chain counter will be added.

## Known boundaries

This remains same-key multi-gate enforcement, not a Provider-global quota. Usage produced by clients or machines that bypass the configured CC Switch chain is invisible to the local ledger. Exact mid-stream cutoff quality depends on protocol events; when a provider omits incremental usage, the conservative provisional meter may stop earlier than the final authoritative usage would require. These limits are documented rather than hidden.
