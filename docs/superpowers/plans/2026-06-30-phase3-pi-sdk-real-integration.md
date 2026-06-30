# Phase 3: Pi SDK Real Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the mock Pi sidecar with real `@earendil-works/pi-coding-agent` SDK calls, add multi-provider worker management (one `PiSidecarSupervisor` per active provider via a `WorkerPool`), inject credentials from keychain at spawn time, bridge subagent usage into the unified `usage_events` table with price catalog lookup, classify and handle auth failures (401 → hard kill), and switch esbuild to CJS format.

**Architecture:** A new `WorkerPool` (in `busytok-subagent`) owns `HashMap<ProviderId, Arc<PiSidecarSupervisor>>` and lazily creates per-provider supervisors with provider-specific env injection (keyring key + base_url). The `SidecarTaskExecutor` is rewired from a single supervisor to the pool, selecting the supervisor by `profile.provider_id`. The `SubagentProfileConfig` gains a `provider_id: Option<String>` field (Phase 4 adds the editing UI). The `BusytokSupervisor.subagent_delegate` handler resolves `profile.provider_id → ProviderConfig → WorkerPool.ensure_worker()`. On the Node side, the mock `turn_auto` handler is replaced with a real `createAgentSession` + `sendTurn` call, returning genuine model output + usage. The Rust executor normalizes the raw sidecar usage into `NormalizedUsageEvent` (with `client_kind: "subagent"`) and writes it to `usage_events` via the store, so the Activity and Overview pages pick it up naturally. Auth failures (401) are detected via a new error classification layer and trigger immediate worker kill + removal from the pool (hard invalidation, bypassing the 5-min restart window).

**Tech Stack:** Rust (async-trait, tokio, rusqlite, tracing, keyring-rs, reqwest), Node.js + TypeScript + esbuild (CJS), `@earendil-works/pi-coding-agent` SDK, existing `busytok-config`/`busytok-subagent`/`busytok-runtime`/`busytok-store`/`busytok-pricing`/`busytok-protocol` crates.

## Global Constraints

- **Spec alignment:** this plan implements spec §4 Phase 3 (lines 225–301). All deliverables, error handling semantics, worker lifecycle rules, and usage flow come directly from the spec. No deviations without explicit discussion.
- **Profile `provider_id` is Phase 3 config, Phase 4 UI:** `SubagentProfileConfig` gains `provider_id: Option<String>` in Phase 3 so the delegate flow can resolve providers. The GUI editing UI (dropdown, cascade-filtered model selection) is Phase 4. Built-in profiles start with `provider_id: None` (unbound) — delegate on an unbound profile returns a validation error: `"profile not bound to a provider"`.
- **WorkerPool reuses `PiSidecarSupervisor`:** the spec says "reuses existing single-worker code, avoids rewriting resource/pressure aggregation." Each per-provider supervisor is a standard `PiSidecarSupervisor` with its own `SidecarConfig` (provider-specific env) + its own `PressureResponder`. No new supervisor type. The `WorkerPool` is a thin ownership + lookup layer. **Shared `PressureGate`:** a SINGLE `Arc<PressureGate>` is created by `construct_sidecar` and shared between `SubagentManager` (reader — pauses the delegate queue under pressure) and ALL supervisors in the pool (writers — `PressureResponder` sets the gate when a worker hits soft/hard limits). Per-worker `PressureResponder` actions (LRU hibernate, graceful restart, force kill) remain per-supervisor, but the pause-new-tasks decision comes from the shared gate. This preserves the spec §8.3 five-step chain semantics across all providers.
- **PressureResponder is per-supervisor (C5/C6 fix):** the existing `PressureResponder` holds `Weak<PiSidecarSupervisor>` + `Weak<SidecarTaskExecutor>` (verified in `pressure.rs:98-100`). It is structurally single-supervisor. The plan does NOT change this — instead, each lazily-created supervisor in the pool gets its OWN `PressureResponder` constructed in `WorkerPool::ensure_worker` (after supervisor construction, before insert). The responder holds `Weak` refs to that specific supervisor + the shared executor. The supervision loop in that supervisor calls `respond()` on pressure transitions, escalating through LRU hibernate → graceful restart → force kill — all on THAT supervisor's process. This is correct: pressure on provider A's worker should restart provider A's process, not provider B's. The shared `PressureGate` ensures the manager pauses the queue globally when ANY worker reports pressure. The `WorkerPool` must own a responder-factory closure (`Arc<dyn Fn(Weak<PiSidecarSupervisor>) -> Arc<PressureResponder> + Send + Sync>`) so `ensure_worker` can construct a responder per supervisor without knowing about `SidecarTaskExecutor` directly (it holds `Weak<SidecarTaskExecutor>` — the pool doesn't own the executor, the runtime does).
- **Breaking behavior — built-in profiles become unbound (I6):** after Task 1, the built-in profiles (`pi/search-cheap`, `pi/review-cheap`, `pi/plan-cheap`) have `provider_id: None`. Delegate on an unbound profile returns `"profile not bound to a provider"`. This is a deliberate behavioral change — Phase 3 requires explicit provider binding. All existing integration tests that delegate via a built-in profile MUST be updated to bind a test provider first. Task 8 includes a test asserting the unbound-profile error (so the regression is documented, not discovered in production).
- **Credential injection at spawn time (spec §2.3):** the service reads the API key from keychain (`ProviderCredentialStore::get_key(provider_id)`) and injects it into the child process env as `OPENAI_API_KEY=<key>` AND `provider.api_key_env_name=<key>` (both names, so the Pi SDK reads `OPENAI_API_KEY` by default while the provider-specific name is preserved for observability). The base URL is injected as `OPENAI_BASE_URL=<base_url>` AND `provider.base_url_env_name.unwrap_or("OPENAI_BASE_URL")=<base_url>`. The key NEVER persists to disk, NEVER enters logs, NEVER travels over network. It travels only via local IPC (Unix socket) from keychain to the service, then via env var from service to the sidecar child process.
- **Auth failure = hard kill (spec §4 Phase 3 error table):** 401 from the model API → task status `failed`, worker immediately killed + removed from pool (bypasses the 5-min rolling restart window). Rate limit (429), timeout, and network errors → task `failed`, worker kept. Sidecar crash → task `failed`, worker killed + removed (goes through existing restart-window logic within that provider's supervisor, but task is NOT retried). The sidecar surfaces HTTP-level errors via new JSON-RPC error codes `-32010` (auth), `-32011` (rate_limit), `-32012` (network). These start at `-32010` to avoid collisions with existing protocol constants (`-32001` SESSION_NOT_FOUND through `-32008` PROTOCOL_MISMATCH in `protocol.rs`). Timeout reuses the existing `-32003` (TASK_TIMEOUT).
- **Usage bridge to `usage_events` (spec §4 Phase 3 usage flow):** the Rust executor normalizes the sidecar's raw usage (`input_tokens`, `output_tokens`, `model`, `provider`) into a `NormalizedUsageEvent` with `client_kind: "subagent"`, computes `total_tokens = input + output` in Rust, looks up `cost_usd` via `busytok_pricing::estimate_cost_with_catalog` (a FREE FUNCTION in the `busytok_pricing` crate root, NOT a method on `PriceCatalog` — verified in `crates/busytok-pricing/src/lib.rs:534`), and writes the event to `usage_events` via `Database::ingest_store_batch` (the public API; `upsert_usage_events_dedup_aware` is an internal free function taking `&Connection`, not a `Database` method — verified in `crates/busytok-store/src/write_queries.rs:255`). The existing `subagent_usage_records` write remains for internal bookkeeping. The Activity page and Overview page pick up subagent usage naturally from `usage_events`.
- **esbuild CJS format (spec §4 Phase 3 + spike finding):** switch `esbuild.config.mjs` from `format: 'esm'` to `format: 'cjs'`. The spike (`apps/pi-sidecar/spike/SPIKE-RESULT.md`) found that ESM bundles fail at runtime because `cross-spawn@7.0.6` (a transitive dep of the SDK) uses CJS `require('child_process')` which esbuild's ESM wrapper rejects. CJS format fixes this natively.
- **Usage bridge lives in `busytok-runtime`, NOT `busytok-subagent` (P0/P1a fix):** the `normalize_task_usage` + `ingest_store_batch` call must execute in the runtime crate's `subagent_delegate` handler (after `SubagentManager::delegate` returns), NOT inside `SubagentManager::execute_task`. This is necessary because: (a) the runtime owns the `GenerationManager` — events MUST be written with the active generation_id (`generation_manager.active_generation_id()`) so they're visible to Overview/Activity read paths (P0: a synthetic `subagent_{task_id}` generation_id makes events invisible); (b) the runtime owns the rollup infrastructure (`build_scan_mutations` in `scan.rs` / `tail.rs`) — the `build_rollups` closure must produce real `daily_usage` + `model_summary` rows so Overview/heatmap/receipt statistics include subagent usage (P1a: `RollupRows::default()` makes subagent tokens invisible to all aggregate panels); (c) `busytok-runtime` already depends on `busytok-pricing`, so no new dependency is needed. `busytok-subagent` does NOT need `busytok-pricing` — `SubagentManager::execute_task` returns `TaskUsage` in `ExecutorOutput`, and the runtime handler normalizes + writes it. The `md5` crate is NOT added — `raw_event_hash` uses a plain format string.
- **Credential rotation via hard removal + lazy re-spawn (spec §4 Phase 3 worker lifecycle, P1b fix):** `ProviderConfig` or API key changes → `WorkerPool::remove_worker_and_kill(provider_id).await` kills the current process AND removes the supervisor from the map in one self-contained async call. The next `ensure_worker(provider_id)` call lazily creates a NEW supervisor with a FRESH keychain read + FRESH env. This is necessary because `SidecarConfig.env` is baked at construction time (`spawn_internal` reads `self.config.env` which is immutable) — a stale-flag-on-the-same-supervisor approach would respawn with the SAME stale env. `PiSidecarSupervisor` has NO `Drop` fallback (verified — no `impl Drop` in `supervisor.rs`), so the kill MUST be explicit and awaited. The interface is `async fn remove_worker_and_kill(&self, provider_id: &str) -> Result<()>` — callers don't need to remember a separate kill step. Both `provider_changed` and `provider_deleted` use `remove_worker_and_kill`; the difference is purely observability (different log event codes). Auth failure also uses `remove_worker_and_kill` (hard kill + removal).
- **Idle TTL:** the existing `idle_exit_seconds` supervision-loop logic already kills the child process after idle timeout. Phase 3 changes: when the process exits (idle or crash), the `WorkerPool` entry remains (the supervisor stays in the map with `state=stopped`), and the next `ensure_worker()` lazily respawns. Auth failure and provider config/key change are the ONLY paths that remove the supervisor from the map entirely.
- **Observability:** every credential read emits `tracing::info!(event_code = "subagent.credential_injected", provider_id = ..., "injected API key into sidecar env")` (key value NEVER logged). Worker pool operations emit `tracing::debug!(event_code = "subagent.worker_pool.*", ...)`. Usage bridge emits `tracing::info!(event_code = "subagent.usage_recorded", task_id = ..., model = ..., ...)`. Auth failure emits `tracing::warn!(event_code = "subagent.auth_failure", provider_id = ..., "auth failure — killing worker")`.
- **Coverage gate:** Rust workspace ≥82%, `busytok-subagent` ≥90% (enforced by `bash scripts/coverage.sh`). Frontend ≥90% on new files (`pnpm exec vitest run --coverage`). Node sidecar ≥90% (`pnpm test` with vitest). All gates run in Task 8.
- **Bootstrap ordering (P1c fix — two-phase init):** `WorkerPool` needs a responder-factory that captures `Weak<SidecarTaskExecutor>`, but `SidecarTaskExecutor` needs `Arc<WorkerPool>`. This is a circular dependency. The plan uses two-phase initialization: (1) construct `WorkerPool` with a `OnceLock<Arc<dyn Fn(Weak<PiSidecarSupervisor>) -> Arc<PressureResponder> + Send + Sync>>` — the factory starts as unset; (2) construct `SidecarTaskExecutor::with_pool(Arc::clone(&pool), db)`; (3) `pool.set_responder_factory(move |weak_sup| Arc::new(PressureResponder::new(weak_sup, Arc::downgrade(&executor), gate)))`. `ensure_worker` is only called AFTER bootstrap completes (during the first `delegate`), so the `OnceLock` is always set by then. If `ensure_worker` is called before `set_responder_factory` (bug), it panics with a clear message — this is a fail-fast invariant, not a runtime fallback.
- **Rust trait wiring:** adding a trait method touches 6 sites (trait def, `BusytokSupervisor` impl, `TestRuntimeControl` mock, `Arc<T>` blanket impl, `AliasConflictRuntime` test mock at `apps/cli/tests/prompt.rs`, `MethodDispatchErrorRuntime` test mock at `crates/busytok-control/tests/server.rs`).
- **TS type regeneration:** after adding/modifying DTOs, run `cargo test -p busytok-protocol generate_typescript_types` to regenerate `packages/busytok-protocol-types/src/generated.ts`.
- **Mock sidecar preservation:** the mock `turn_auto` handler is kept behind an env-var flag `BUSYTOK_USE_MOCK_SIDECAR=1` so Rust-side integration tests (which spawn the sidecar bundle) continue to work without a real API key. The real handler is the default. Tests that don't spawn the sidecar use `MockTaskExecutor` / `FailingTaskExecutor` as before.

---

## File Structure

**Rust (backend):**
- `crates/busytok-config/src/lib.rs` — add `provider_id: Option<String>` to `SubagentProfileConfig`. (Modify)
- `crates/busytok-subagent/src/sidecar/config.rs` — add `provider_id: String` to `SidecarConfig`; extract `resolve_base_sidecar_config` (shared base) from `resolve_sidecar_config`. (Modify)
- `crates/busytok-subagent/src/sidecar/supervisor.rs` — no changes to `SupervisorState` (stale flag dropped; credential rotation uses `remove_worker_and_kill` + lazy re-spawn instead). (No change needed beyond what Task 3 adds for auth-fail kill.)
- `crates/busytok-subagent/src/sidecar/pool.rs` — new `WorkerPool` type. (Create)
- `crates/busytok-subagent/src/sidecar/mod.rs` — re-export `WorkerPool`. (Modify)
- `crates/busytok-subagent/src/sidecar/executor.rs` — rewire from single supervisor to `Arc<WorkerPool>`; add `provider_id` to `ExecutorInput` path; add error classification + auth-fail handling. (Modify — NO usage normalization here; that lives in `busytok-runtime` per P0/P1a)
- `crates/busytok-subagent/src/mock_executor.rs` — add `provider_id: Option<String>` to `ExecutorInput`. (Modify)
- `crates/busytok-subagent/src/models.rs` — add `TaskErrorKind` enum (auth/rate_limit/timeout/crash/network/unknown); add `error_kind: Option<TaskErrorKind>` to `ExecutorOutput`. (Modify)
- `crates/busytok-subagent/src/manager.rs` — thread `provider_id` from profile config into `ExecutorInput`; persist `error_kind` in task row. (Modify)
- `crates/busytok-store/src/subagent_queries.rs` — add `error_kind` column to `subagent_tasks` (migration 0005); add `subagent_set_task_error_kind`. (Modify)
- `crates/busytok-store/src/schema.rs` — bump `SCHEMA_VERSION` to 5; add `(5, SUBAGENT_TASK_ERROR_KIND_SQL)` to `migrations()` Vec. (Modify)
- `crates/busytok-store/migrations/0005_subagent_task_error_kind.sql` — new migration. (Create)
- `crates/busytok-runtime/src/supervisor.rs` — replace `sidecar_supervisor` field with `worker_pool`; update `construct_sidecar` to build `WorkerPool` with a shared `PressureGate`; update `subagent_delegate` to resolve provider from profile + validate model whitelist (spec §3.4) + normalize usage + write to `usage_events` under the active generation_id with real rollup rows (P0/P1a); update `subagent_runtime_status` to aggregate from pool; wire credential store into pool; add async `provider_changed` / `provider_deleted` handlers that call `pool.remove_worker_and_kill(provider_id).await` (P1b). (Modify)
- `crates/busytok-runtime/src/subagent_usage.rs` — new module: `normalize_task_usage` + rollup closure (P0/P1a). (Create)
- `crates/busytok-runtime/tests/supervisor_control.rs` — tests for multi-provider delegate, auth-fail kill, usage_events write + rollup visibility + active generation_id, runtime_status aggregation. (Modify)
- `crates/busytok-subagent/tests/sidecar_pool.rs` — new integration tests for `WorkerPool`. (Create)
- `crates/busytok-subagent/tests/sidecar_supervisor.rs` — tests for auth-fail kill path. (Modify)

**Node (sidecar):**
- `apps/pi-sidecar/package.json` — add `@earendil-works/pi-coding-agent` dependency; remove `"type": "module"`. (Modify)
- `apps/pi-sidecar/esbuild.config.mjs` — switch `format: 'esm'` to `format: 'cjs'`. (Modify)
- `apps/pi-sidecar/src/handlers/turn_auto.ts` — replace mock with real SDK call (behind `BUSYTOK_USE_MOCK_SIDECAR` flag). (Modify)
- `apps/pi-sidecar/src/pi_session.ts` — replace data-only interface with real `PiSdkSession` wrapper. (Modify)
- `apps/pi-sidecar/src/session_pool.ts` — update to manage real SDK sessions. (Modify)
- `apps/pi-sidecar/src/handlers/turn_auto.test.ts` — tests for real + mock paths. (Modify)
- `apps/pi-sidecar/tests/real_sdk.test.ts` — new tests for SDK integration (mocked). (Create)

---

## Task 1: Config schema — `profile.provider_id` + `SidecarConfig.provider_id` + `TaskErrorKind`

**Files:**
- Modify: `crates/busytok-config/src/lib.rs`
- Modify: `crates/busytok-subagent/src/sidecar/config.rs`
- Modify: `crates/busytok-subagent/src/mock_executor.rs`
- Modify: `crates/busytok-subagent/src/models.rs`
- Test: `crates/busytok-config/src/lib.rs` (inline `#[cfg(test)]`)
- Test: `crates/busytok-subagent/src/sidecar/config.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `SubagentProfileConfig.provider_id: Option<String>` (defaults to `None` via `#[serde(default)]`)
- Produces: `SidecarConfig.provider_id: String`
- Produces: `SidecarConfig.api_key_env_name: String` + `SidecarConfig.base_url_env_name: String` (for observability)
- Produces: `resolve_base_sidecar_config(settings, paths) -> Result<SidecarConfig, SidecarError>` (base config without provider-specific env)
- Produces: `ExecutorInput.provider_id: Option<String>`
- Produces: `TaskErrorKind` enum (Auth, RateLimit, Timeout, Crash, Network, Unknown) in `models.rs`

- [ ] **Step 1: Add `provider_id` to `SubagentProfileConfig`**

In `crates/busytok-config/src/lib.rs`, add `provider_id: Option<String>` with `#[serde(default)]` to `SubagentProfileConfig`. Built-in profiles in `default_profiles()` start with `provider_id: None`. Add a test verifying that a config without `provider_id` deserializes to `None` (backward-compat with v0.0.8 configs).

- [ ] **Step 2: Add `provider_id` + credential env names to `SidecarConfig`**

In `crates/busytok-subagent/src/sidecar/config.rs`, add three fields to `SidecarConfig`: `provider_id: String`, `api_key_env_name: String`, `base_url_env_name: String`. Split `resolve_sidecar_config` into:
- `resolve_base_sidecar_config(settings, paths) -> Result<SidecarConfig, SidecarError>` — produces a base config with empty `provider_id` / placeholder env names. Used by tests and as a template.
- `resolve_sidecar_config(settings, paths)` — delegates to `resolve_base_sidecar_config` (backward-compat for existing callers).

The `WorkerPool` (Task 2) will clone the base config and override `provider_id` + env per provider.

- [ ] **Step 3: Add `provider_id` to `ExecutorInput` + `TaskErrorKind` to `models.rs`**

In `crates/busytok-subagent/src/mock_executor.rs`, add `pub provider_id: Option<String>` to `ExecutorInput`. In `crates/busytok-subagent/src/models.rs`, add:
```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskErrorKind {
    Auth,
    RateLimit,
    Timeout,
    Crash,
    Network,
    Unknown,
}
```
Add `pub error_kind: Option<TaskErrorKind>` to `ExecutorOutput`. Update `MockTaskExecutor` to set `error_kind: None`.

- [ ] **Step 4: Verify (fmt + clippy + test)**

```bash
cargo fmt --all --check
cargo clippy -p busytok-config -p busytok-subagent --tests
cargo test -p busytok-config --lib
cargo test -p busytok-subagent --lib
```

---

## Task 2: WorkerPool — multi-provider supervisor management + credential injection

**Files:**
- Create: `crates/busytok-subagent/src/sidecar/pool.rs`
- Modify: `crates/busytok-subagent/src/sidecar/mod.rs`
- Modify: `crates/busytok-subagent/src/sidecar/config.rs`
- Test: `crates/busytok-subagent/tests/sidecar_pool.rs`

**Interfaces:**
- Produces: `WorkerPool` struct owning `HashMap<String, Arc<PiSidecarSupervisor>>` + base `SidecarConfig` + `Arc<Mutex<Database>>` + provider config lookup closure + shared `Arc<PressureGate>` + responder-factory closure
- Produces: `WorkerPool::new(base_config, db, providers, pressure_gate) -> Self` where `providers` is a provider config lookup (`Arc<dyn Fn(&str) -> Option<ProviderConfig> + Send + Sync>`) and `pressure_gate: Option<Arc<PressureGate>>` is the shared gate passed to every supervisor. The responder-factory is NOT passed to `new` — it's set via `set_responder_factory` after the executor is constructed (P1c fix: two-phase init).
- Produces: `WorkerPool::set_responder_factory(&self, factory: Arc<dyn Fn(Weak<PiSidecarSupervisor>) -> Arc<PressureResponder> + Send + Sync>)` — sets the factory on the internal `OnceLock`. Called once during bootstrap AFTER `SidecarTaskExecutor` is constructed.
- Produces: `WorkerPool::ensure_worker(&self, provider_id: &str) -> Result<Arc<PiSidecarSupervisor>, SidecarError>` — SYNCHRONOUS (not async — I2 fix: the body is entirely sync: keychain read + config build + supervisor alloc + responder set + insert; no `.await`). **Locking:** (1) read keychain OUTSIDE the map lock (keychain is I/O — I2 fix: `ProviderCredentialStore::get_key` can take 10-100ms+ on macOS, must not serialize all providers); (2) acquire map lock, check if entry exists (someone else may have created it while we read keychain), if yes → return existing + drop key; (3) if no entry → build config + construct supervisor + construct responder via factory (panics if `set_responder_factory` not yet called — P1c fail-fast) + `sup.set_pressure_responder(responder)` (C6 fix) + insert + return Arc.
- Produces: `async fn WorkerPool::remove_worker_and_kill(&self, provider_id: &str) -> Result<()>` — self-contained hard removal + kill (P1b fix). **Locking (I1 fix):** (1) acquire map lock, (2) `remove` entry → `Option<Arc<PiSidecarSupervisor>>`, (3) DROP the map lock, (4) if `Some(sup)`, `sup.force_kill().await?` OUTSIDE the lock (force_kill awaits child.wait() — must not hold sync mutex across .await). `PiSidecarSupervisor` has NO `Drop` fallback (verified), so the kill MUST be explicit and awaited — this method ensures callers don't forget.
- Produces: `WorkerPool::worker_snapshots(&self) -> Vec<(String, WorkerSnapshot)>` — for runtime_status aggregation
- Produces: `async fn WorkerPool::shutdown_all(&self)` — graceful shutdown all workers (same lock-ordering as `remove_worker_and_kill`: collect all entries under lock, drop lock, then `force_kill().await` each outside lock)
- Produces: `WorkerPool::for_each_supervisor(&self, f: impl Fn(&str, &Arc<PiSidecarSupervisor>))` — for `evict_lru` iteration across all providers (Task 3, I5 fix)
- Produces: `WorkerPool::supervisor_for_session(&self, adapter_session_id: &str) -> Option<(String, Arc<PiSidecarSupervisor>)>` — looks up which provider's supervisor owns a given adapter session (C7 fix: `evict_session` needs this to route `prepare_hibernate`/`close` RPCs to the correct supervisor)

- [ ] **Step 1: Write failing tests for `WorkerPool`**

Create `crates/busytok-subagent/tests/sidecar_pool.rs`. Test cases:
- `ensure_worker_creates_supervisor_lazily` — first call creates; second call returns same Arc
- `ensure_worker_injects_credentials` — verify env map contains `OPENAI_API_KEY` + `api_key_env_name` + `OPENAI_BASE_URL`
- `ensure_worker_sets_pressure_responder` — after ensure_worker, the supervisor has a responder set (C6 fix — verify `sup.pressure_responder().is_some()`)
- `remove_worker_then_ensure_creates_new_supervisor` — after remove, ensure_worker creates a NEW supervisor (with fresh keychain read)
- `worker_snapshots_returns_all_workers` — multiple providers → multiple snapshots
- `ensure_worker_fails_for_unknown_provider` — provider_id not in providers → error
- `ensure_worker_fails_for_disabled_provider` — provider.enabled = false → error
- `ensure_worker_fails_for_missing_api_key` — keyring has no key → error (note: `get_key` returns `Result<Option<String>>`, not `Option<String>` — propagate keychain errors as `SidecarError::Spawn`)
- `ensure_worker_fails_for_keychain_error` — `get_key` returns `Err(...)` → `SidecarError::Spawn("keychain read failed: ...")`
- `ensure_worker_concurrent_same_provider_no_duplicate` — two concurrent calls for same provider → same Arc (no leak); use `tokio::spawn` + `join!` to exercise actual concurrency

Use a mock provider config map + a test keyring (or mock `ProviderCredentialStore`). The tests should NOT spawn real sidecar processes — they verify the config/env construction, not the process lifecycle.

- [ ] **Step 2: Implement `WorkerPool`**

Create `crates/busytok-subagent/src/sidecar/pool.rs`. The `WorkerPool` owns:
- `base_config: SidecarConfig` (cloned per provider, with env overridden)
- `db: Option<Arc<Mutex<Database>>>` (threaded to each supervisor)
- `providers: Arc<dyn Fn(&str) -> Option<ProviderConfig> + Send + Sync>` (lookup closure)
- `pressure_gate: Option<Arc<PressureGate>>` (shared gate — passed to every supervisor)
- `responder_factory: std::sync::OnceLock<Arc<dyn Fn(Weak<PiSidecarSupervisor>) -> Arc<PressureResponder> + Send + Sync>>` (P1c fix — set via `set_responder_factory` AFTER executor construction; `ensure_worker` reads it via `.get().expect("responder_factory not set — bootstrap incomplete")`)
- `workers: Arc<std::sync::Mutex<HashMap<String, Arc<PiSidecarSupervisor>>>>`

`ensure_worker(provider_id)` (SYNCHRONOUS — I2 fix):
1. Look up `ProviderConfig` via the closure (no lock held). If not found → `SidecarError::Spawn("unknown provider")`. If disabled → `SidecarError::Spawn("provider disabled")`.
2. Read API key: `ProviderCredentialStore::get_key(provider_id)` (no lock held — this is OS keychain I/O, 10-100ms+ on macOS). Propagate `Err` as `SidecarError::Spawn("keychain read failed: ...")`. If `Ok(None)` → `SidecarError::Spawn("no API key for provider")`.
3. Acquire the workers map lock. Re-check if entry exists (someone else may have created it while we read keychain — avoids wasted supervisor construction). If yes → return existing Arc (discard the key we just read).
4. Clone `base_config`, override `provider_id`, build env map:
   - `OPENAI_API_KEY` = key value
   - `api_key_env_name` = key value
   - `OPENAI_BASE_URL` = provider.base_url
   - `base_url_env_name.unwrap_or("OPENAI_BASE_URL")` = provider.base_url
   - `BUSYTOK_SIDECAR_MAX_HOT_SESSIONS` (from base config)
5. Construct `PiSidecarSupervisor::with_resource_policy(config, db, policy, pressure_gate.clone())` — passes the shared gate.
6. Construct responder via `self.responder_factory(Arc::downgrade(&sup))` (C5/C6 fix).
7. `sup.set_pressure_responder(responder)` (C6 fix — without this, the supervision loop's `invoke_pressure_responder` no-ops on every pressure transition).
8. Insert into map, return Arc.

Log `tracing::info!(event_code = "subagent.credential_injected", provider_id = ..., "injected API key into sidecar env")` after successful env construction (key value NEVER logged).

- [ ] **Step 3: Re-export from `sidecar/mod.rs`**

Add `pub use pool::WorkerPool;` to `crates/busytok-subagent/src/sidecar/mod.rs`.

- [ ] **Step 4: Verify (fmt + clippy + test)**

```bash
cargo fmt --all --check
cargo clippy -p busytok-subagent --tests
cargo test -p busytok-subagent --test sidecar_pool
```

---

## Task 3: SidecarTaskExecutor — multi-provider wiring + error classification

**Files:**
- Modify: `crates/busytok-subagent/src/sidecar/executor.rs`
- Modify: `crates/busytok-subagent/src/sidecar/supervisor.rs` (add auth-fail detection)
- Test: `crates/busytok-subagent/src/sidecar/executor.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Modifies: `SidecarTaskExecutor` — field changes from `supervisor: Arc<PiSidecarSupervisor>` to `pool: Arc<WorkerPool>`
- Produces: `SidecarTaskExecutor::with_pool(pool, db) -> Self`
- Produces: `classify_sidecar_error(err: &SidecarError) -> TaskErrorKind`
- Produces: `WorkerPool::remove_worker(provider_id)` calls the existing `PiSidecarSupervisor::force_kill()` (no new kill method — M1 fix: reuse existing `force_kill`)

- [ ] **Step 1: Write failing tests for error classification + auth-fail kill**

Test cases (inline `#[cfg(test)]` module in `executor.rs`):
- `classify_auth_failure` — `SidecarError::Application(-32010, "401 Unauthorized", ..)` → `TaskErrorKind::Auth`
- `classify_rate_limit` — `SidecarError::Application(-32011, "429 Too Many Requests", ..)` → `TaskErrorKind::RateLimit`
- `classify_network` — `SidecarError::Application(-32012, "connection refused", ..)` → `TaskErrorKind::Network`
- `classify_timeout` — `SidecarError::Application(-32003, ..)` (TASK_TIMEOUT) → `TaskErrorKind::Timeout`
- `classify_crash` — `SidecarError::Crashed` → `TaskErrorKind::Crash`
- `classify_spawn_network` — `SidecarError::Spawn(...)` with "connection refused" → `TaskErrorKind::Network`
- `classify_unknown` — everything else → `TaskErrorKind::Unknown`

**Error code table (new codes start at `-32010` to avoid collisions with existing `-32001`..`-32008` in `protocol.rs`):**
- `-32010` = auth failure (401) → `TaskErrorKind::Auth` → hard kill + remove worker
- `-32011` = rate limit (429) → `TaskErrorKind::RateLimit` → keep worker, surface error
- `-32012` = network error → `TaskErrorKind::Network` → keep worker, surface error
- `-32003` = timeout (existing `TASK_TIMEOUT`) → `TaskErrorKind::Timeout` → keep worker, surface error
- `-32002` = hot session limit (existing) → eviction flow (unchanged)
- `SidecarError::Crashed` → `TaskErrorKind::Crash` → existing crash-restart logic

Add a regression test asserting no overlap between new codes (`-32010`..`-32012`) and existing protocol constants (`-32001`..`-32008`).

- [ ] **Step 2: Rewire `SidecarTaskExecutor` to use `WorkerPool`**

Change `SidecarTaskExecutor` to hold `pool: Arc<WorkerPool>` instead of `supervisor: Arc<PiSidecarSupervisor>`. In `execute()`:
1. Extract `provider_id` from `input.provider_id`. If `None` → return error `"profile not bound to a provider"`.
2. `let supervisor = self.pool.ensure_worker(provider_id)?` (SYNCHRONOUS — I2 fix: `ensure_worker` is not async)
3. `let handle = supervisor.ensure_started().await?`
4. Build `turn_auto` params (same as before, but include `provider_id` in the params so the sidecar knows which provider to use).
5. Call `handle.turn_auto(params)`.
6. On error: classify via `classify_sidecar_error`. If `Auth` → `self.pool.remove_worker_and_kill(provider_id).await?` (P1b fix: self-contained kill + remove). Record `error_kind` in `ExecutorOutput`.
7. On success: parse result (same as before).

Keep the existing hot-session-limit eviction flow unchanged (it operates on the supervisor, which is still a `PiSidecarSupervisor`).

- [ ] **Step 2b: Migrate `evict_lru` + `evict_session` to pool (C7 fix)**

The existing `evict_lru` reads `self.supervisor.config().harness_name` and `evict_session` calls `self.supervisor.ensure_started()` + `handle.prepare_hibernate`/`handle.close`. With `self.supervisor` removed, both methods must be rewritten:

**`evict_lru` (C7 fix):** iterate ALL supervisors via `self.pool.for_each_supervisor(|provider_id, sup| { ... })`. For each supervisor, call `db.subagent_find_lru_hot_binding(&sup.config().harness_name)` (same query, per-supervisor harness name — all are "pi" currently but this is correct per-supervisor). Collect all candidates across all providers, pick the globally-oldest by `last_used_at_ms`, then call `self.evict_session(&candidate.adapter_session_id)`. This preserves the I5 fix (pool-wide LRU) with the correct per-supervisor harness lookup.

**`evict_session` (C7 fix):** the existing method calls `self.supervisor.ensure_started()` to get a handle for `prepare_hibernate`/`close`. With a pool, the executor must resolve WHICH supervisor owns the `adapter_session_id`. Use `self.pool.supervisor_for_session(adapter_session_id)` (added in Task 2 interfaces) — this queries the DB for the binding's `harness`/provider, then looks up the supervisor in the pool. If found → `sup.ensure_started().await?` → `handle.prepare_hibernate(...)` → persist → `handle.close()`. If not found (binding belongs to a removed provider) → log warning + skip (the session is already gone).

Provide the actual method bodies in the implementation, not just intent — the existing `evict_session` logic (prepare_hibernate → atomic persist → close) is preserved, only the supervisor resolution changes.

- [ ] **Step 3: `remove_worker_and_kill` reuses existing `force_kill` (M1 + P1b fix)**

The `WorkerPool::remove_worker_and_kill(provider_id)` is an `async fn` that: (1) locks the map, removes the entry, drops the lock; (2) if `Some(sup)`, calls `sup.force_kill().await?` (existing `pub(crate) async fn` at `supervisor.rs:366`). No new kill method is added — `force_kill` already does `shutdown_internal` + SIGKILL escalation. The method is self-contained: callers don't need to remember a separate kill step (P1b fix — `PiSidecarSupervisor` has NO `Drop` fallback, so the kill MUST be explicit and awaited).

- [ ] **Step 4: Verify (fmt + clippy + test)**

```bash
cargo fmt --all --check
cargo clippy -p busytok-subagent --tests
cargo test -p busytok-subagent --lib
```

---

## Task 4: BusytokSupervisor wiring — WorkerPool integration + delegate + runtime_status

**Files:**
- Modify: `crates/busytok-runtime/src/supervisor.rs`
- Modify: `crates/busytok-subagent/src/manager.rs`
- Test: `crates/busytok-runtime/tests/supervisor_control.rs`

**Interfaces:**
- Modifies: `BusytokSupervisor` — `sidecar_supervisor` field replaced by `worker_pool: Option<Arc<WorkerPool>>`
- Modifies: `construct_sidecar` — builds `WorkerPool` with a shared `Arc<PressureGate>` (C2 fix: single gate across all workers + manager)
- Modifies: `subagent_delegate` — resolves `profile.provider_id` → validates provider exists + enabled + model whitelist (spec §3.4, M2 fix) → passes to manager
- Modifies: `subagent_runtime_status` — aggregates workers from pool
- Modifies: `SubagentManager::execute_task` — passes `provider_id` from profile config to `ExecutorInput`
- Produces: `async fn BusytokSupervisor::provider_changed(provider_id)` — calls `pool.remove_worker_and_kill(provider_id).await` (P1b: self-contained async kill + remove → lazy re-spawn with fresh credentials on next delegate)
- Produces: `async fn BusytokSupervisor::provider_deleted(provider_id)` — calls `pool.remove_worker_and_kill(provider_id).await` (same mechanism, different log event code)

- [ ] **Step 1: Write failing tests for multi-provider delegate + runtime_status**

Test cases in `supervisor_control.rs`:
- `delegate_fails_for_unbound_profile` — profile with `provider_id: None` → error
- `delegate_fails_for_unknown_provider` — profile with `provider_id: "nonexistent"` → error
- `delegate_fails_for_disabled_provider` — provider exists but `enabled: false` → error
- `delegate_fails_for_model_not_in_whitelist` — profile.model not in provider.models → error (M2 fix)
- `runtime_status_aggregates_multiple_workers` — two providers with supervisors → two worker rows
- `runtime_status_workers_empty_when_pool_none` — no pool → `workers: []`
- `provider_changed_removes_worker_then_respawns` — update provider → worker removed from pool → next delegate creates new worker with fresh credentials
- `provider_deleted_removes_worker` — delete provider → worker removed from pool

- [ ] **Step 2: Update `construct_sidecar` to build `WorkerPool` with shared `PressureGate` + two-phase bootstrap (P1c fix)**

Replace the single-supervisor construction with a two-phase bootstrap:
1. Resolve base config via `resolve_base_sidecar_config(settings, paths)`.
2. Create ONE shared `Arc<PressureGate>` (same gate used by both `SubagentManager` and all supervisors — C2 fix).
3. Build a provider lookup closure: `Arc::new(move |id: &str| settings.providers.iter().find(|p| p.id == id && p.enabled).cloned())`.
4. **Phase 1:** Construct `WorkerPool::new(base_config, Some(db), providers_lookup, Some(Arc::clone(&pressure_gate)))` — the responder-factory `OnceLock` is UNSET at this point.
5. **Phase 1:** Construct `SidecarTaskExecutor::with_pool(Arc::clone(&pool), db)` — captures `Arc<WorkerPool>`.
6. **Phase 2:** `pool.set_responder_factory(Arc::new({ let weak_exec = Arc::downgrade(&sidecar_executor); let gate = Arc::clone(&pressure_gate); move |weak_sup| Arc::new(PressureResponder::new(weak_sup, weak_exec.clone(), gate.clone())) }))` — injects the factory that captures `Weak<SidecarTaskExecutor>` + `Arc<PressureGate>`. This replaces the existing `assemble_with_sidecar` block at supervisor.rs:432-445 which constructed a single responder for the single supervisor.
7. Store `worker_pool: Some(Arc::clone(&pool))` on `BusytokSupervisor`.
8. Pass `Some(pressure_gate)` to `SubagentManager::with_pressure_gate(...)` (same gate — manager pauses the queue under pressure).

The shared `PressureGate` ensures: (a) `SubagentManager::delegate()` pauses the queue when ANY worker reports pressure; (b) each supervisor's `PressureResponder` writes to the same gate. Per-worker `PressureResponder` actions (LRU hibernate, graceful restart, force kill) remain per-supervisor — they operate on the supervisor's own process, not the gate.

- [ ] **Step 3: Update `subagent_delegate` to resolve provider + validate model whitelist (spec §3.4, M2 fix)**

In the delegate handler:
1. Look up `SubagentProfileConfig` by `req.profile`.
2. Read `profile.provider_id`. If `None` → return validation error `"profile not bound to a provider"`.
3. Look up `ProviderConfig` by `provider_id`. If not found → `"provider not found"`. If disabled → `"provider disabled"`.
4. **Model whitelist validation (spec §3.4):** validate `profile.model` is in `provider.models` (a `Vec<String>` — verified in `providers.rs:34`). If not → return validation error `"model '{model}' not in provider '{provider_id}' whitelist"`. This prevents spawning a worker for a model the provider doesn't support. Also handle the edge case where `profile.model` is empty string → same error.
5. `provider_id` is NOT added to `DelegateRequest` (I5 fix — `DelegateRequest` has no `provider_id` field; the provider is resolved from the profile, not passed by the caller). The `SubagentManager::execute_task` method reads `profile.provider_id` from `self.settings.profiles` and sets it on `ExecutorInput.provider_id` directly. This is the single source of truth: profile config → executor input.
6. **Usage bridge (P0/P1a fix):** after `SubagentManager::delegate` returns `DelegateResult` with `status: Completed`, the RUNTIME handler (not the manager) normalizes the usage and writes to `usage_events`:
   - `let gen_id = self.generation_manager.active_generation_id().ok_or_else(|| anyhow!("no active generation"))?;` (P0 fix: use the active generation so events are visible to Overview/Activity read paths)
   - `let event = normalize_task_usage(&result.task_id, &result.subagent_id, &req.cwd, &result.usage, catalog.as_ref());` (function lives in `busytok-runtime`, not `busytok-subagent`)
   - `let batch = StoreWriteBatch::for_test("subagent", &result.task_id).usage_event(event, UsageWritePolicy::InsertOnce);`
   - `db.ingest_store_batch(batch, &gen_id, |effective_events, gen_id| { build_scan_mutations(effective_events, rollup_opts, gen_id).map(|m| RollupRows { daily_usage_rows: m.daily_usage, model_usage_rows: Vec::new(), session_rows: Vec::new(), project_rows: Vec::new(), model_summary_rows: model_rollups_to_rows(&m.model_rollups) }) })?;` (P1a fix: produce REAL rollup rows so Overview/heatmap/receipt statistics include subagent usage — reuse the existing `build_scan_mutations` infra from `scan.rs`/`tail.rs`)
   - Log `tracing::info!(event_code = "subagent.usage_recorded", task_id = ..., model = ..., input_tokens = ..., output_tokens = ..., cost_usd = ?, "recorded subagent usage in unified usage_events")`.
   - Usage event write failure is logged at `warn` level with `event_code = "subagent.usage_write_failed"` but does NOT fail the task — the task result is already persisted; usage is best-effort observability.

- [ ] **Step 4: Update `subagent_runtime_status` to aggregate from pool**

Replace the single-supervisor worker snapshot with:
1. If `worker_pool` is None → `workers: []`.
2. Otherwise → `pool.worker_snapshots()` returns `Vec<(provider_id, WorkerSnapshot)>`. Map each to `SubagentWorkerDto` with `provider_id: Some(...)`.
3. **Aggregate pressure (I4 fix):** `PressureLevel` (supervisor.rs:85) derives `Debug, Clone, Copy, PartialEq, Eq` but NOT `Ord`. Define an explicit severity rank function `PressureLevel::severity(&self) -> u8` (Normal=0, Throttled=1, Evicting=2, Restarting=3) and aggregate by `max_by_key(|w| w.pressure_level.severity())`. Sum `hot_sessions_total` across all workers. Take `max_by_key` for `memory_used_pct`. Use the most recent `worker_sampled_at_ms`.

- [ ] **Step 5: Wire `provider_changed` / `provider_deleted` into provider RPC handlers (I3 fix)**

**I3 fix:** there is NO `provider_set_key` handler — key writes happen INSIDE `provider_update` (supervisor.rs:4918: `if let Some(key) = &req.api_key { ProviderCredentialStore::set_key(...) }`). The wiring is:

In `provider_update` (supervisor.rs:4874): after writing config + (if `req.api_key.is_some()`) writing keychain, call `self.provider_changed(&req.id).await` which calls `pool.remove_worker_and_kill(provider_id).await` (P1b fix: self-contained kill + remove). This covers both metadata changes (base_url, env names, models) AND key rotations.

In `provider_create` (supervisor.rs:4813): typically no worker exists yet (new provider), but call `self.provider_changed(&req.id).await` defensively (no-op if no worker).

In `provider_delete` (supervisor.rs:4931): after deleting config + keychain, call `self.provider_deleted(&provider_id).await` which calls `pool.remove_worker_and_kill(provider_id).await` (P1b fix — same mechanism, different log event code `subagent.provider_deleted` vs `subagent.provider_changed`).

- [ ] **Step 6: Verify (fmt + clippy + test + coverage)**

```bash
cargo fmt --all --check
cargo clippy -p busytok-runtime -p busytok-subagent --tests
cargo test -p busytok-runtime --test supervisor_control
cargo test -p busytok-subagent --test sidecar_pool
bash scripts/coverage.sh
```

---

## Task 5: Usage normalization + `usage_events` bridge (lives in `busytok-runtime`)

> **P0/P1a fix:** this whole task lives in `busytok-runtime`, NOT `busytok-subagent`. The runtime owns the `GenerationManager` (so events get the active generation_id and are visible to Overview/Activity read paths) and the rollup infrastructure (`build_scan_mutations` in `scan.rs` / `tail.rs`). `busytok-subagent` does NOT gain a `busytok-pricing` dependency — `SubagentManager::execute_task` returns `TaskUsage` in `ExecutorOutput`, and the runtime handler normalizes + writes it. See Global Constraints → "Usage bridge lives in `busytok-runtime`, NOT `busytok-subagent`".

**Files:**
- Modify: `crates/busytok-runtime/src/supervisor.rs` (add `normalize_task_usage` + `write_subagent_usage_event` in or near the `subagent_delegate` handler)
- Modify: `crates/busytok-runtime/Cargo.toml` (NO change — `busytok-pricing` is already a dep, verified in `tail.rs:33`; `busytok-aggregator` is already a dep, verified in `tail.rs:19`)
- Modify: `crates/busytok-store/src/subagent_queries.rs` (add `subagent_insert_usage_event` helper if needed for the `subagent_usage_records` bookkeeping write)
- Modify: `crates/busytok-store/src/schema.rs` (bump `SCHEMA_VERSION` to 5, add migration entry)
- Modify: `crates/busytok-store/migrations/0005_subagent_task_error_kind.sql` (add `error_kind` column)
- Modify: `crates/busytok-subagent/src/models.rs` (add `TaskErrorKind` enum + `error_kind: Option<TaskErrorKind>` on `ExecutorOutput` — this part stays in `busytok-subagent` because the executor produces it)
- Test: `crates/busytok-runtime/src/supervisor.rs` (inline tests for `normalize_task_usage`)
- Test: `crates/busytok-runtime/tests/supervisor_control.rs` (integration tests for the usage bridge + rollup visibility)
- Test: `crates/busytok-store/src/subagent_queries.rs` (inline tests)

**Interfaces:**
- Produces (in `busytok-runtime`): `fn normalize_task_usage(task_id, subagent_id, cwd, usage: &TaskUsage, catalog: Option<&PriceCatalog>) -> NormalizedUsageEvent`
- Produces (in `busytok-runtime`): the `subagent_delegate` handler, after `SubagentManager::delegate` returns successfully, resolves the active generation_id via `self.generation_manager.active_generation_id()`, builds a `StoreWriteBatch` containing the normalized event, and calls `db.ingest_store_batch(batch, &generation_id, build_rollups)` where `build_rollups` produces REAL rollup rows via `build_scan_mutations` (P1a fix — `RollupRows::default()` is forbidden; it would make subagent tokens invisible to Overview/heatmap/receipt panels)
- Produces: migration `0005_subagent_task_error_kind.sql` — `ALTER TABLE subagent_tasks ADD COLUMN error_kind TEXT;` + `SCHEMA_VERSION` bump to 5 in `schema.rs`

- [ ] **Step 1: Write failing tests for usage normalization**

Test cases:
- `normalize_usage_populates_required_fields` — `client_kind: "subagent"`, `model`, `input_tokens`, `output_tokens`, `total_tokens = input + output`
- `normalize_usage_handles_missing_tokens` — `input_tokens: None` → treated as 0
- `normalize_usage_computes_cost_via_price_catalog` — with a test catalog, verify `cost_usd` is computed
- `normalize_usage_cost_none_when_catalog_misses` — model not in catalog → `cost_usd: None`
- `normalize_usage_cost_none_when_no_catalog` — `catalog: None` → `cost_usd: None`, `cost_source: None`
- `write_usage_event_inserts_into_usage_events` — after the runtime handler writes, `SELECT count(*) FROM usage_events WHERE client_kind='subagent'` = 1
- `write_usage_event_idempotent_on_same_task_id` — write twice with same `task_id` → only 1 row (dedupe_key works)
- `write_usage_event_uses_active_generation_id` (P0) — the event row's `generation_id` equals `generation_manager.active_generation_id()`, NOT a synthetic `subagent_{task_id}` string; verify by reading the row back
- `write_usage_event_produces_real_rollup_rows` (P1a) — after the handler writes, `SELECT count(*) FROM daily_usage WHERE agent='codex'` ≥ 1 AND `SELECT count(*) FROM model_summary` ≥ 1; this proves `build_scan_mutations` ran (not `RollupRows::default()`)
- `write_usage_event_visible_in_overview_read_path` (P1a end-to-end) — after the handler writes, `read_overview_summary_from_daily_usage(...)` returns totals that include the subagent tokens (proves the Overview panel would render them)

- [ ] **Step 2: Implement `normalize_task_usage` (in `busytok-runtime`)**

In `crates/busytok-runtime/src/supervisor.rs` (or a new `crates/busytok-runtime/src/subagent_usage.rs` module re-exported from `supervisor.rs`):
```rust
use busytok_domain::{AgentKind, NormalizedUsageEvent};
use busytok_pricing::{CostMode, PriceCatalog, TokenUsage};

pub fn normalize_task_usage(
    task_id: &str,
    subagent_id: &str,
    cwd: &str,
    usage: &TaskUsage,
    catalog: Option<&PriceCatalog>,
) -> NormalizedUsageEvent {
    let input = usage.input_tokens.unwrap_or(0).max(0) as u64;
    let output = usage.output_tokens.unwrap_or(0).max(0) as u64;
    let total = input + output;
    let model = usage.model.clone().unwrap_or_default();
    // C1 fix: estimate_cost_with_catalog is a FREE FUNCTION in busytok_pricing,
    // NOT a method on PriceCatalog (verified at lib.rs:534).
    // C2 fix: TokenUsage does NOT derive Default — spell out all 5 fields
    // (verified at lib.rs:17: input_tokens, output_tokens, cached_input_tokens,
    // cache_creation_tokens, reasoning_tokens).
    let cost_usd = catalog.and_then(|cat| {
        busytok_pricing::estimate_cost_with_catalog(
            cat,
            &model,
            TokenUsage {
                input_tokens: input,
                output_tokens: output,
                cached_input_tokens: 0,
                cache_creation_tokens: 0,
                reasoning_tokens: 0,
            },
            usage.cost_usd, // source_cost from sidecar (may be None)
            None,           // speed
            CostMode::Auto,
        )
    });
    // I2 fix: NormalizedUsageEvent has no Default impl. Use minimal_for_test
    // (despite the name, it's the canonical zero-default constructor) and
    // override the fields that matter.
    // I3 fix: AgentKind has no Subagent variant. Use Codex (pi-sidecar wraps
    // a Codex-family SDK); client_kind = "subagent" is the discriminator.
    let event_id = format!("subagent_usage_{}", task_id);
    let mut event = NormalizedUsageEvent::minimal_for_test(&event_id, AgentKind::Codex);
    event.client_kind = Some("subagent".to_string());
    event.model = Some(model.clone());
    event.model_provider = usage.provider.clone();
    event.input_tokens = input as i64;
    event.output_tokens = output as i64;
    event.total_tokens = total as i64;
    event.cost_usd = cost_usd;
    event.estimated_cost_usd = cost_usd;
    event.cost_source = cost_usd.map(|_| "price_catalog".to_string());
    event.cwd = Some(cwd.to_string());
    event.session_id = subagent_id.to_string();
    event.dedupe_key = Some(format!("subagent_task:{}", task_id));
    // C3 fix: md5 crate is NOT a dependency. Use a plain format string —
    // the dedupe_key already provides idempotency, so raw_event_hash just
    // needs to be a stable identifier for the event payload.
    event.raw_event_hash = format!("subagent:{task_id}:{input}:{output}");
    event.timestamp_ms = busytok_domain::now_ms();
    event
}
```

**Notes (C1/C2/C3/I2/I3 fixes):**
- **C1:** `estimate_cost_with_catalog` is a free function in `busytok_pricing` (verified at `lib.rs:534`), called as `busytok_pricing::estimate_cost_with_catalog(cat, ...)`, NOT `PriceCatalog::estimate_cost_with_catalog(...)`.
- **C2:** `TokenUsage` derives `Debug, Clone, Copy` only — NO `Default` (verified at `lib.rs:17`). All 5 fields must be spelled out.
- **C3:** `md5` crate is NOT a workspace dependency (verified — zero matches in all `Cargo.toml`). Use a plain format string for `raw_event_hash`; the `dedupe_key` already ensures idempotency.
- **I2:** `NormalizedUsageEvent` has no `Default` impl (verified in `events.rs`). Use `minimal_for_test(id, agent)` as the canonical zero-default constructor, then override fields.
- **I3:** `AgentKind` has only `ClaudeCode` and `Codex` (no `Subagent` variant — verified in `agent.rs`); use `Codex` since the pi-sidecar wraps a Codex-family SDK, and `client_kind = "subagent"` is the discriminator that downstream consumers (Activity page, Overview page) use to distinguish subagent events from top-level Codex runs.

- [ ] **Step 3: Write usage event in the runtime `subagent_delegate` handler (P0/P1a)**

> This step runs in `busytok-runtime`'s `subagent_delegate` handler, AFTER `SubagentManager::delegate` returns successfully with `ExecutorOutput.usage`. It does NOT run inside `SubagentManager::execute_task` — the manager returns the raw `TaskUsage` and the runtime normalizes + writes it. This is mandatory because only the runtime has access to the active generation_id (P0) and the rollup infrastructure (P1a).

After `SubagentManager::delegate` returns successfully:
1. Resolve the active generation_id: `let generation_id = self.generation_manager.active_generation_id().ok_or_else(|| anyhow!("no active generation"))?;` (P0 — events written with any other generation_id are invisible to Overview/Activity read paths, which filter by active generation; verified at `supervisor.rs:2340`, `supervisor.rs:2858`, `supervisor.rs:2890`).
2. Load the global `PriceCatalog` (the runtime already holds an `Arc<ArcSwap<PriceCatalog>>` or equivalent — reuse the same accessor the scan/tail paths use; do NOT add a new field to `SubagentManager`).
3. `let event = normalize_task_usage(&task.id, &subagent.id, &input.cwd, &out.usage, catalog.as_ref());`
4. Build a `StoreWriteBatch` containing the event and call `db.ingest_store_batch(batch, &generation_id, build_rollups)` where `build_rollups` produces REAL rollup rows via `build_scan_mutations` (P1a — `RollupRows::default()` is forbidden):
   ```rust
   use busytok_aggregator::{build_scan_mutations, model_rollups_to_rows,
       project_rollups_to_rows, session_rollups_to_rows, RollupOptions};
   use busytok_store::{StoreWriteBatch, UsageWritePolicy};

   let batch = StoreWriteBatch::for_test("subagent", &task.id)
       .usage_event(event, UsageWritePolicy::InsertOnce);

   // Reuse the same RollupOptions construction pattern as tail.rs:420-429
   // (timezone from settings). This is the P1a fix — the closure MUST produce
   // real daily_usage + model_summary rows so Overview/heatmap/receipt panels
   // include subagent tokens. Rolling your own RollupRows::default() here is a
   // regression.
   let ro = RollupOptions { timezone: rtz.clone() };
   db.ingest_store_batch(batch, &generation_id, |effective_events, gen_id| {
       let mutations = build_scan_mutations(effective_events, ro.clone(), gen_id)
           .context("failed to build subagent usage rollup mutations")?;
       Ok(busytok_store::RollupRows {
           daily_usage_rows: mutations.daily_usage,
           model_usage_rows: Vec::new(),
           session_rows: session_rollups_to_rows(&mutations.session_rollups),
           project_rows: project_rollups_to_rows(&mutations.project_rollups),
           model_summary_rows: model_rollups_to_rows(&mutations.model_rollups),
       })
   })?;
   ```
   Reference the existing pattern verbatim at `crates/busytok-runtime/src/tail.rs:570-581` — same closure shape, same `build_scan_mutations` call, same `RollupRows` assembly. The only difference is the input event source (subagent delegate vs. file tail).
5. Log `tracing::info!(event_code = "subagent.usage_recorded", task_id = ..., model = ..., input_tokens = ..., output_tokens = ..., cost_usd = ?, "recorded subagent usage in unified usage_events")`.

The existing `subagent_usage_records` write (internal bookkeeping, inside `SubagentManager`) remains unchanged — it is separate from the unified `usage_events` write above. Usage event write failure is logged at `warn` level with `event_code = "subagent.usage_write_failed"` but does NOT fail the task — the task result is already persisted; usage is best-effort observability.

- [ ] **Step 4: Add `error_kind` column migration + persistence (C4 fix: 0005, not 0006)**

Create `crates/busytok-store/migrations/0005_subagent_task_error_kind.sql`:
```sql
ALTER TABLE subagent_tasks ADD COLUMN error_kind TEXT;
```

In `crates/busytok-store/src/schema.rs`:
1. Bump `pub const SCHEMA_VERSION: u32 = 5;` (was 4).
2. Add `const SUBAGENT_TASK_ERROR_KIND_SQL: &str = include_str!("../migrations/0005_subagent_task_error_kind.sql");`
3. Append `(5, SUBAGENT_TASK_ERROR_KIND_SQL)` to the `migrations()` Vec.

Add `error_kind: Option<String>` to `SubagentTaskRow`. In `SubagentManager::execute_task`, after `executor.execute()`, if `out.error_kind.is_some()`, persist it via `subagent_set_task_error_kind(&task.id, error_kind)`.

- [ ] **Step 5: Verify (fmt + clippy + test + coverage)**

```bash
cargo fmt --all --check
cargo clippy -p busytok-runtime -p busytok-store --tests
cargo test -p busytok-runtime --lib
cargo test -p busytok-runtime --test supervisor_control
cargo test -p busytok-store --lib
bash scripts/coverage.sh
```

---

## Task 6: Node sidecar — real Pi SDK integration + esbuild CJS

**Files:**
- Modify: `apps/pi-sidecar/package.json`
- Modify: `apps/pi-sidecar/esbuild.config.mjs`
- Modify: `apps/pi-sidecar/src/handlers/turn_auto.ts`
- Modify: `apps/pi-sidecar/src/pi_session.ts`
- Modify: `apps/pi-sidecar/src/session_pool.ts`
- Modify: `apps/pi-sidecar/src/types.ts` (add error code constants)
- Modify: `apps/pi-sidecar/src/handlers/turn_auto.test.ts`
- Create: `apps/pi-sidecar/tests/real_sdk.test.ts`
- Create: `apps/pi-sidecar/tests/bundle_smoke.test.ts` (P2 fix: real JSON-RPC handshake smoke test against the CJS bundle)

**Interfaces:**
- Produces: real `PiSdkSession` wrapper around `createAgentSession` return value
- Produces: `turnAutoHandlerWithPool` now calls real SDK (or mock if `BUSYTOK_USE_MOCK_SIDECAR=1`)
- Produces: error codes `-32010` (auth), `-32011` (rate_limit), `-32012` (network) — start at `-32010` to avoid collisions with existing `-32001`..`-32008` in `protocol.rs` (C3 fix)
- Produces: CJS bundle output (`format: 'cjs'`)

- [ ] **Step 1: Add `@earendil-works/pi-coding-agent` dependency + switch to CJS**

In `apps/pi-sidecar/package.json`:
- Add `"@earendil-works/pi-coding-agent": "0.80.2"` to `dependencies` (pin to spike-verified version).
- Remove `"type": "module"` (or change to `"type": "commonjs"`).

In `apps/pi-sidecar/esbuild.config.mjs`:
- Change `format: 'esm'` to `format: 'cjs'`.
- Update the banner comment if needed.

Run `pnpm install` from the repo root to install the new dependency into the workspace.

- [ ] **Step 2: Define error code constants**

In `apps/pi-sidecar/src/types.ts`, add:
```ts
// New error codes start at -32010 to avoid collisions with existing
// protocol constants -32001..-32008 (SESSION_NOT_FOUND through
// PROTOCOL_MISMATCH) in Rust protocol.rs.
export const ERROR_CODE_AUTH_FAILURE = -32010;
export const ERROR_CODE_RATE_LIMIT = -32011;
export const ERROR_CODE_NETWORK = -32012;
// -32003 remains TASK_TIMEOUT (reused for timeout classification)
// -32002 remains HOT_SESSION_LIMIT_REACHED
```

- [ ] **Step 3: Implement real `PiSdkSession` wrapper**

In `apps/pi-sidecar/src/pi_session.ts`, replace the data-only `PiSession` interface with a class that wraps the SDK session:
```ts
import { createAgentSession, type AgentSession } from '@earendil-works/pi-coding-agent';

export class PiSdkSession {
  private session: AgentSession;
  readonly adapter_session_id: string;
  readonly logical_subagent_id: string;
  readonly created_at_ms: number;
  last_used_at_ms: number;

  constructor(session: AgentSession, adapter_session_id: string, logical_subagent_id: string) {
    this.session = session;
    this.adapter_session_id = adapter_session_id;
    this.logical_subagent_id = logical_subagent_id;
    this.created_at_ms = Date.now();
    this.last_used_at_ms = this.created_at_ms;
  }

  async sendTurn(prompt: string, options: SendTurnOptions): Promise<TurnResult> {
    // Call the SDK's sendTurn (or equivalent method — verify actual API)
    // Map the SDK response to TurnResult
    // Catch errors and classify (401 → auth, 429 → rate_limit, etc.)
  }

  async close(): Promise<void> {
    // Clean up the SDK session
  }
}
```

Note: the implementer MUST verify the actual Pi SDK API by reading the SDK's TypeScript declarations (`node_modules/@earendil-works/pi-coding-agent/dist/...d.ts`). The method names (`createAgentSession`, `sendTurn`) are from the spike; the actual API may differ. Document any deviations.

- [ ] **Step 4: Replace mock `turn_auto` handler with real SDK call**

In `apps/pi-sidecar/src/handlers/turn_auto.ts`, restructure:
```ts
export function turnAutoHandlerWithPool(pool: SessionPool): RequestHandler {
  return async (params, ctx) => {
    // ... validate params (same as before)
    const useMock = process.env.BUSYTOK_USE_MOCK_SIDECAR === '1';
    if (useMock) {
      return mockTurnAuto(params, pool);
    }
    return realTurnAuto(params, pool);
  };
}
```

`realTurnAuto`:
1. `const { session, reused } = await pool.ensure(logical_subagent_id, model, cwd);`
2. `const result = await session.sendTurn(prompt, { model, tools, timeout_ms, ... });`
3. Map `result` to `TurnAutoResult` with real usage from the SDK.
4. On error: catch + classify + throw `SidecarError` with the appropriate code.

- [ ] **Step 5: Update `SessionPool` to manage real SDK sessions**

The `SessionPool` currently stores data-only `PiSession` objects. Update it to store `PiSdkSession` objects. The `ensure` method now:
1. If a session for `logical_subagent_id` exists in the hot pool → return it (LRU update).
2. If the pool is full → throw `SidecarError('hot session limit reached', -32002, { candidate })`.
3. Otherwise → `const sdkSession = await createAgentSession({ model, workingDir: cwd });` → wrap in `PiSdkSession` → add to pool.

- [ ] **Step 6: Write tests (mocked SDK)**

In `apps/pi-sidecar/tests/real_sdk.test.ts`:
- Mock `@earendil-works/pi-coding-agent` via `vi.mock(...)`.
- Test `turn_auto` with mocked SDK returning a successful response → verify `TurnAutoResult` has real usage.
- Test `turn_auto` with mocked SDK throwing 401 → verify error code `-32010`.
- Test `turn_auto` with mocked SDK throwing 429 → verify error code `-32011`.
- Test `turn_auto` with `BUSYTOK_USE_MOCK_SIDECAR=1` → verify mock path still works.
- Test session reuse (same logical_subagent_id → `session_reused: true`).
- Test hot session limit (full pool → `-32002` with candidate).

- [ ] **Step 7: Verify build + test + CJS bundle smoke (P2 fix)**

```bash
cd apps/pi-sidecar
pnpm install
pnpm typecheck
pnpm test
pnpm build
```

Then run a **real handshake smoke test** against the CJS bundle (P2 fix — the previous `node dist/pi-sidecar.bundle.js --help` / `--version` is a fake green: `main.ts` is a pure stdio JSON-RPC server with NO CLI args, verified at `apps/pi-sidecar/src/main.ts:1`; `--help`/`--version` would just hang). The smoke test must:

1. Spawn the bundle as a child process: `node dist/pi-sidecar.bundle.js` (no args).
2. Send a JSON-RPC `adapter.initialize` request over the child's stdin: `{"jsonrpc":"2.0","method":"adapter.initialize","params":{"protocol_version":1},"id":1}` (the registered handler is at `main.ts:14`; the request shape matches the existing Rust client handshake at `crates/busytok-subagent/src/sidecar/supervisor.rs:655`).
3. Read the response from stdout and assert it is a valid JSON-RPC response with `id: 1` and a non-error `result`.
4. Send `adapter.shutdown` (`main.ts:16`): `{"jsonrpc":"2.0","method":"adapter.shutdown","params":null,"id":2}`.
5. Assert the child process exits with code 0 within a timeout (e.g. 5s).

Write this as a vitest test in `apps/pi-sidecar/tests/bundle_smoke.test.ts` so it runs in `pnpm test`. Use Node's `child_process.spawn` + a line-buffered stdout reader. Skip with `it.skipIf(process.env.CI && process.platform === 'win32')` only if cross-platform flakiness is observed during implementation — do NOT skip by default; the whole point of P2 is that the bundle must be proven to boot, not just parse.

---

## Task 7: Provider change handlers — removal wiring (async kill)

**Files:**
- Modify: `crates/busytok-runtime/src/supervisor.rs`
- Test: `crates/busytok-runtime/tests/supervisor_control.rs`

**Interfaces:**
- Produces: `async fn BusytokSupervisor::provider_changed(provider_id: &str)` — calls `pool.remove_worker_and_kill(provider_id).await` (P1b fix: self-contained async kill + remove; `PiSidecarSupervisor` has NO `Drop` fallback, so the kill MUST be awaited here)
- Produces: `async fn BusytokSupervisor::provider_deleted(provider_id: &str)` — calls `pool.remove_worker_and_kill(provider_id).await` (same mechanism, different log event code)
- Modifies: `provider_update` handler (supervisor.rs:4874) — calls `self.provider_changed(&req.id).await` after config + keychain write
- Modifies: `provider_create` handler (supervisor.rs:4813) — calls `self.provider_changed(&req.id).await` defensively (typically no-op, new provider has no worker yet)
- Modifies: `provider_delete` handler (supervisor.rs:4931) — calls `self.provider_deleted(&provider_id).await` after config + keychain delete

- [ ] **Step 1: Write failing tests for removal wiring**

Test cases:
- `provider_update_removes_worker_then_respawns` — create provider → delegate (spawns worker) → update provider (metadata change) → verify worker removed from pool → next delegate creates new worker with fresh keychain read
- `provider_update_with_api_key_removes_worker_then_respawns` — create provider → delegate → update provider with `req.api_key = Some("new-key")` → verify worker removed from pool → next delegate creates new worker with fresh key (I3 fix: key rotation happens INSIDE `provider_update`, not a separate `provider_set_key` handler)
- `provider_delete_removes_worker` — create provider → delegate → delete provider → verify worker removed from pool
- `provider_update_kills_old_process` (P1b) — create provider → delegate (spawns worker, capture child pid) → update provider → assert the old child pid is NO LONGER alive (the `remove_worker_and_kill` call awaited `force_kill`; a mere `remove_worker` that forgets to kill would leave the orphan process running)
- `provider_changed_no_op_when_pool_none` — no pool → `provider_changed` is a no-op (no panic)

- [ ] **Step 2: Implement `provider_changed` / `provider_deleted` (P1b — async kill)**

Both `await` `pool.remove_worker_and_kill(provider_id)` — the P1b fix. This is a self-contained async call that kills the current process AND removes the supervisor from the map. Callers do NOT need to remember a separate `force_kill` step — the old design (`remove_worker` returns `Arc`, caller remembers to kill) was dropped because Task 7's example impl forgot the kill, and `PiSidecarSupervisor` has NO `Drop` fallback (verified — no `impl Drop` in `supervisor.rs`), so a forgotten kill leaks an orphan process and breaks the "next delegate uses fresh credentials" guarantee (the old process keeps running with the old env). Credential rotation requires a NEW supervisor because `SidecarConfig.env` is baked at construction time.

```rust
async fn provider_changed(&self, provider_id: &str) {
    if let Some(pool) = &self.worker_pool {
        // P1b: self-contained kill + remove. The previous design returned an
        // Arc and relied on the caller to await force_kill — which Task 7's
        // own example forgot. PiSidecarSupervisor has no Drop fallback, so
        // a forgotten kill leaks an orphan process running stale credentials.
        if let Err(e) = pool.remove_worker_and_kill(provider_id).await {
            tracing::warn!(
                event_code = "subagent.provider_changed_kill_failed",
                provider_id = %provider_id,
                error = %e,
                "failed to kill+remove sidecar worker — next delegate may re-spawn with stale credentials"
            );
        }
        tracing::info!(
            event_code = "subagent.provider_changed",
            provider_id = %provider_id,
            "removed sidecar worker from pool — next delegate will re-spawn with fresh credentials"
        );
    }
}

async fn provider_deleted(&self, provider_id: &str) {
    if let Some(pool) = &self.worker_pool {
        if let Err(e) = pool.remove_worker_and_kill(provider_id).await {
            tracing::warn!(
                event_code = "subagent.provider_deleted_kill_failed",
                provider_id = %provider_id,
                error = %e,
                "failed to kill+remove sidecar worker on provider deletion"
            );
        }
        tracing::info!(
            event_code = "subagent.provider_deleted",
            provider_id = %provider_id,
            "removed sidecar worker from pool due to provider deletion"
        );
    }
}
```

- [ ] **Step 3: Wire into provider RPC handlers (I3 + P1b)**

In `provider_update` (supervisor.rs:4874): after writing config + (if `req.api_key.is_some()`) writing keychain via `ProviderCredentialStore::set_key`, call `self.provider_changed(&req.id).await`. This covers BOTH metadata changes (base_url, env names, models) AND key rotations — all trigger `pool.remove_worker_and_kill`. There is NO separate `provider_set_key` handler (I3 fix — verified: key writes happen INSIDE `provider_update` at supervisor.rs:4918). The handler is already `async`, so the `.await` is natural.

In `provider_create` (supervisor.rs:4813): after writing config + keychain, call `self.provider_changed(&req.id).await` defensively (typically no-op — new provider has no worker yet, but the call is safe).

In `provider_delete` (supervisor.rs:4931): after deleting config + `ProviderCredentialStore::delete_key`, call `self.provider_deleted(&provider_id).await`.

- [ ] **Step 4: Verify (fmt + clippy + test)**

```bash
cargo fmt --all --check
cargo clippy -p busytok-runtime --tests
cargo test -p busytok-runtime --test supervisor_control
```

---

## Task 8: End-to-end verification + coverage gate

**Files:**
- Modify: `crates/busytok-runtime/tests/supervisor_control.rs` (integration tests)
- Modify: `apps/pi-sidecar/tests/` (sidecar integration tests)

- [ ] **Step 1: Write end-to-end integration tests**

Integration tests in `supervisor_control.rs` (using mock sidecar — `BUSYTOK_USE_MOCK_SIDECAR=1`):
- `e2e_delegate_with_bound_profile_executes_task` — create provider + set key + bind profile → delegate → task completed → usage recorded in `usage_events`
- `e2e_multi_provider_creates_separate_workers` — two providers → two delegates → two worker entries in `runtime_status`
- `e2e_auth_failure_kills_worker` — mock sidecar returns 401 (error code `-32010`) → task failed with `error_kind: "auth"` → worker removed from pool → next delegate creates new worker
- `e2e_provider_change_removes_and_respawns_worker` — delegate → update provider → worker removed from pool → delegate again → new process spawned (fresh credentials read from keychain)
- `e2e_unbound_profile_fails` — built-in profile with `provider_id: None` → delegate fails with `"profile not bound to a provider"` (I6 fix: documents the breaking behavior regression)
- `e2e_model_not_in_whitelist_fails` — profile.model not in provider.models → delegate fails with `"model not in provider whitelist"` (M2 fix)
- `e2e_usage_events_has_subagent_kind` — after delegate, `SELECT count(*) FROM usage_events WHERE client_kind='subagent'` ≥ 1
- `e2e_usage_events_agent_kind_is_codex` — after delegate, `SELECT agent FROM usage_events WHERE client_kind='subagent'` = `'codex'` (I3 fix: verify discriminator + agent kind round-trip)

- [ ] **Step 2: Run full verification suite**

```bash
# Rust
cargo fmt --all --check
cargo clippy --workspace --tests
cargo test --workspace
bash scripts/coverage.sh  # workspace ≥82%, busytok-subagent ≥90%

# Node sidecar
cd apps/pi-sidecar
pnpm typecheck
pnpm test          # includes bundle_smoke.test.ts (P2 fix: real adapter.initialize handshake)
pnpm build

# Frontend (unchanged but verify no regressions)
cd ../gui
pnpm typecheck
pnpm exec vitest run
pnpm build
```

> P2 fix: the CJS bundle is verified by `bundle_smoke.test.ts` (created in Task 6 Step 7), which spawns the bundle and performs a real `adapter.initialize` → `adapter.shutdown` JSON-RPC handshake. The previous `node dist/pi-sidecar.bundle.js --version` was removed — `main.ts` has no CLI args, so that command would hang and prove nothing.

- [ ] **Step 3: Verify acceptance criteria (spec §6 Phase 3)**

- [ ] `turn_auto` calls real model API (verified: delegate returns real model output) — manual verification with a real API key
- [ ] One worker process per provider (verified: multiple providers → multiple node processes) — automated test `e2e_multi_provider_creates_separate_workers`
- [ ] Auth failure immediately kills worker — automated test `e2e_auth_failure_kills_worker`
- [ ] Usage recorded in unified `usage_events` with `client_kind: "subagent"` — automated test `e2e_usage_events_has_subagent_kind`
- [ ] Subagent usage visible in Overview/daily_usage rollups (P1a) — automated test `write_usage_event_produces_real_rollup_rows` + `write_usage_event_visible_in_overview_read_path` (Task 5 Step 1)
- [ ] Subagent usage written under active generation_id (P0) — automated test `write_usage_event_uses_active_generation_id` (Task 5 Step 1)
- [ ] Provider change kills old worker process (P1b) — automated test `provider_update_kills_old_process` (Task 7 Step 1)
- [ ] Activity page shows subagent token consumption — manual verification (Activity page reads from `usage_events`, no code change needed)
- [ ] esbuild produces CJS bundle — automated test `bundle_smoke.test.ts` (real handshake, not fake CLI args)

- [ ] **Step 4: Update CONTRIBUTING.md (if needed)**

Verify the CONTRIBUTING.md invariant from spec §2.4 is still accurate after Phase 3. No change expected (Phase 1 already updated it).

---

## Acceptance Criteria Summary

| # | Criterion | Verification |
|---|---|---|
| 1 | `turn_auto` calls real model API | Manual: delegate with real key returns real output |
| 2 | One worker per provider | Automated: `e2e_multi_provider_creates_separate_workers` |
| 3 | Auth failure kills worker | Automated: `e2e_auth_failure_kills_worker` |
| 4 | Usage in `usage_events` with `client_kind: "subagent"` | Automated: `e2e_usage_events_has_subagent_kind` |
| 5 | Activity page shows subagent tokens | Manual: Activity page reads `usage_events` |
| 6 | esbuild CJS bundle boots + handshakes (P2) | Automated: `bundle_smoke.test.ts` (real `adapter.initialize` → `adapter.shutdown`) |
| 7 | Model whitelist validation (spec §3.4) | Automated: `e2e_model_not_in_whitelist_fails` |
| 8 | Provider change kills + removes + re-spawns worker (P1b) | Automated: `e2e_provider_change_removes_and_respawns_worker` + `provider_update_kills_old_process` |
| 9 | Error codes avoid protocol collisions | Automated: regression test in Task 3 |
| 10 | Unbound profile fails (I6 regression doc) | Automated: `e2e_unbound_profile_fails` |
| 11 | `agent_kind = codex` round-trips (I3) | Automated: `e2e_usage_events_agent_kind_is_codex` |
| 12 | PressureResponder wired per-supervisor (C5/C6) | Automated: `ensure_worker_sets_pressure_responder` |
| 13 | `evict_lru` operates pool-wide (C7) | Automated: Task 3 inline test |
| 14 | Subagent usage written under active generation_id (P0) | Automated: `write_usage_event_uses_active_generation_id` |
| 15 | Subagent usage produces real `daily_usage` + `model_summary` rollups (P1a) | Automated: `write_usage_event_produces_real_rollup_rows` + `write_usage_event_visible_in_overview_read_path` |
| 16 | Usage bridge lives in `busytok-runtime` (P0/P1a) | Automated: `cargo build -p busytok-subagent` succeeds with NO `busytok-pricing` dep (regression guard) |
