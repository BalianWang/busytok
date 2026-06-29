# Subagent Plan 6: Pressure Response Chain + Real Doctor Checks + Queue Counts

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement spec §8.3 5-step pressure response chain (hibernate LRU, pause new tasks, graceful restart, force kill), upgrade the 6 stubbed doctor checks to real probes, and add `queued/running task count` to `ResourceSample` per spec §8.1.

**Architecture:** A new shared `PressureGate` (`Arc<AtomicBool>` + `std::sync::Mutex<PressureAction>`) is constructed in `BusytokSupervisor::construct_sidecar` and threaded into both `PiSidecarSupervisor` (writer — the supervision loop sets the gate on pressure transitions) and `SubagentManager` (reader — `delegate()` checks the gate before queueing). The 5-step escalation lives in a new `PressureResponder` that the supervision loop invokes on escalation transitions. **Ownership (cycle-free):** `PressureResponder` holds `Weak<PiSidecarSupervisor>` + `Weak<SidecarTaskExecutor>` + `Arc<PressureGate>` — ALL weak for supervisor/executor to break the cycle. The strong owners are `BusytokSupervisor` fields: `sidecar_supervisor: Option<Arc<PiSidecarSupervisor>>`, `sidecar_executor: Option<Arc<SidecarTaskExecutor>>` (NEW concrete Arc), `pressure_responder: Option<Arc<PressureResponder>>`. `PiSidecarSupervisor` holds `pressure_responder: Mutex<Option<Weak<PressureResponder>>>` (weak, set via setter after construction) — the supervision loop upgrades this weak ref when it needs to invoke the responder. `construct_sidecar` returns a 5-tuple including the concrete `Arc<SidecarTaskExecutor>` so `PressureResponder::new` can downgrade it. **§8.3 "queue only" (Finding 1 fix):** `delegate()` checks the gate BEFORE insert. When the gate is paused, it inserts the task row as `"queued"` and returns `DelegateResult { status: Queued }` immediately. When the gate is NOT paused, it inserts directly as `"running"` (with `started_at_ms = now`) and executes synchronously — the dispatcher never sees these tasks (Round 4 race-free insert). A background `TaskDispatcher` (spawned by `SubagentManager::spawn_task_dispatcher`) polls every 200ms for queued tasks; when the gate clears, it picks the oldest queued task (atomic SQL pick + flip to "running" with per-subagent FIFO guard per spec §6.4), and calls the refactored `execute_task(&self, task: &SubagentTaskRow, subagent: &LogicalSubagent)` which reads ALL execution params (`prompt_artifact_ref`, `timeout_seconds`, `model_override`) directly from the task row — NO `DelegateRequest` reconstruction, no data loss. Dispatcher shutdown uses `tokio::sync::watch::channel(bool)` (JoinHandle drop = detach, NOT abort); `BusytokSupervisor::Drop` sends the shutdown signal. `ForceKill` clears the gate after kill (Finding 2 fix) so the next delegate can lazy-restart. `PressureResponder.respond()` uses `try_lock` for in-flight deduplication (Finding 3 fix). Doctor checks reuse existing `BusytokPaths` methods, `PROTOCOL_VERSION`, and `SubagentModelsConfig` — no new path resolution. `bundle_manifest_readable` checks `manifest.json` (not `pi-sidecar.bundle.js`). `protocol_version` does a short-lived probe (start, verify, shutdown). Queue counts come from a new `SubagentManager::task_counts()` that queries `queued`/`running` task rows.

**Tech Stack:** Rust (sysinfo, tokio, rusqlite, tracing), existing `busytok-subagent` / `busytok-runtime` / `busytok-store` / `busytok-config` crates.

## Global Constraints

- Spec §8.3 Pressure Response (5-step escalation): 1) Hibernate least-recently-used hot session. 2) Pause new task execution (queue only). 3) If sidecar RSS exceeds soft limit → request graceful restart. 4) Before restart: prepare_hibernate all hot sessions → write memory → restart. 5) If graceful restart fails → force kill. Completed task state in SQLite is never lost.
- Spec §8.1 ResourceMonitor collects: busytok-service RSS, Pi sidecar RSS + CPU, hot session count, **queued/running task count**, system available memory (macOS). Via `sysinfo` crate. Sampling interval = `monitor_interval_seconds` (default 30s).
- Spec §7.1 + §7.3 doctor checks: 11 checks total. The 6 previously-stubbed checks (bundled_node_arch, bundle_manifest_readable, protocol_version, default_model_config, pi_runtime_installed, artifact_store_writable) become real probes reusing `BusytokPaths`, `PROTOCOL_VERSION`, `SubagentModelsConfig`. `bundle_manifest_readable` checks `manifest.json` (spec §5.1 line 549), NOT `pi-sidecar.bundle.js`. `protocol_version` does a short-lived probe (start sidecar if not running, verify, shutdown).
- Spec §3.2 event enum: `sidecar_start | sidecar_stop | session_hot | session_hibernate | memory_pressure | sidecar_restart | task_timeout | rss_limit_exceeded`. No new event types added by Plan 6 (recovery logging stays tracing-only; the latch already updates on recovery).
- `PROTOCOL_VERSION = 1` (`crates/busytok-subagent/src/sidecar/protocol.rs:19`).
- `SubagentManager` and `PiSidecarSupervisor` share only `Arc<Mutex<Database>>` today — Plan 6 adds a shared `PressureGate` constructed in `construct_sidecar` and passed to both.
- `ResourcePressureState` has 3 variants: `Normal`, `Pressure`, `LimitExceeded`. `LimitExceeded` is set ONLY when `exceeds_hard_limit` is true. `Pressure` is set when `under_pressure || exceeds_soft_limit`. There is NO distinct "soft limit exceeded" state — the soft-limit predicate feeds into `Pressure`. Plan 6 must trigger `GracefulRestart` (§8.3 step 3) by checking `exceeds_soft_limit` directly in the supervision loop when the state is `Pressure`, NOT via a state transition.
- `task_queue_max` config field (default 50) exists in `SubagentPiSidecarConfig` but is NOT enforced anywhere — Plan 6 does NOT add queue enforcement (spec §8.3 step 2 is "pause new task execution" under pressure, not a general concurrency cap). `task_queue_max` enforcement remains out of scope.
- Coverage gates: workspace ≥ 82% (hard), per-crate `busytok-subagent` ≥ 90% (hard).
- Logging: use `tracing` crate with `event_code = "subagent.resource.*"` and `event_code = "subagent.pressure.*"` namespaces, following existing patterns in supervisor.rs.
- TDD: every task follows red-green-commit. Tests must verify real behavior, not mocks of mocks. Test bodies must be concrete with real assertions, not stub comments.
- `evict_session` exists in `SidecarTaskExecutor` and takes ONE arg (`adapter_session_id: &str`), looking up the binding internally via `subagent_find_hot_binding_by_session`. It is REACTIVE (fires on `HOT_SESSION_LIMIT_REACHED` from sidecar). Plan 6 step 1 needs a PROACTIVE LRU picker that queries the DB for the oldest hot binding, extracts its `adapter_session_id`, and calls `evict_session`.
- `evict_lru` is NOT on the `TaskExecutor` trait (which only has `execute`). `PressureResponder` MUST hold a concrete `Weak<SidecarTaskExecutor>` (not `Weak<dyn TaskExecutor>`) to call `evict_lru`. `construct_sidecar` must produce a concrete `Arc<SidecarTaskExecutor>` in addition to the `Arc<dyn TaskExecutor>` coerced for `SubagentManager`.
- Arc cycle avoidance: `PressureResponder` holds `Weak<PiSidecarSupervisor>` + `Weak<SidecarTaskExecutor>`. `PiSidecarSupervisor` holds `Weak<PressureResponder>` (via `Mutex<Option<Weak<...>>>`). The strong owners are `BusytokSupervisor` fields (`sidecar_supervisor`, `sidecar_executor`, `pressure_responder`). This ensures all Arcs can reach 0 when `BusytokSupervisor` drops.
- Recovery DB events (`resource_recovered`) are NOT added — the spec §3.2 enum doesn't include them, and the latch already updates on recovery so re-pressurization writes fresh events. Recovery stays tracing-only.
- **Round 2 — Task row is the single source of truth for execution:** `subagent_tasks` adds `timeout_seconds INTEGER` + `model_override TEXT` columns (Task 7 Step 1). `execute_task(&self, task: &SubagentTaskRow, subagent: &LogicalSubagent)` reads ALL execution params from the row — NO `DelegateRequest` reconstruction in the dispatcher. `prompt_artifact_ref`, `timeout_seconds`, `model_override` are preserved end-to-end per spec §4.3/§4.4.
- **Round 2 — Per-subagent FIFO (spec §6.4 line 737):** `pick_oldest_queued_task` SQL excludes subagents that already have a running task: `WHERE status = 'queued' AND subagent_id NOT IN (SELECT subagent_id FROM subagent_tasks WHERE status = 'running') ORDER BY created_at_ms LIMIT 1`. Same logical subagent tasks are serialized; different subagents run concurrently.
- **Round 2 — Dispatcher shutdown via watch channel:** Tokio `JoinHandle` drop = detach (NOT abort). `BusytokSupervisor` holds `shutdown_tx: tokio::sync::watch::Sender<bool>`. Dispatcher loop uses `tokio::select!` on ticker + `shutdown_rx.changed()`. `shutdown_writer()` sends `true` + awaits handle. `Drop` impl sends `true` (can't await — 200ms worst case).
- **Round 2 — `delegate()` never returns `Err(Paused)`:** Under queue-only semantics, `delegate()` returns `Ok(DelegateResult { status: Queued })` when gate is paused. `SubagentError::Paused` is retained ONLY for the dispatcher's re-pause edge case (gate re-pauses between pick and execute).
- **Round 3 — Atomic pick+flip (Finding 1 fix):** `pick_oldest_queued_task` uses `BEGIN IMMEDIATE` transaction + CAS guard (`WHERE id = ? AND status = 'queued'`). If two dispatchers race, the second UPDATE affects 0 rows → returns `None`. RAII `Transaction` auto-rolls-back on drop (per project SQLite lesson learned).
- **Round 3 — No double-flip (Finding 2 fix):** `execute_task()` does NOT flip `queued → running`. It assumes the task is already `"running"` (with `started_at_ms` set). `delegate()` synchronous path inserts directly as `'running'` (Round 4 fix). The dispatcher path relies on `pick_oldest_queued_task`'s atomic flip. Single authoritative `started_at_ms`.
- **Round 3 — Incremental migration (Finding 3 fix):** New `0004_subagent_task_fields.sql` with `ALTER TABLE subagent_tasks ADD COLUMN ...` (NOT modifying `0003_subagent.sql`). `SCHEMA_VERSION` bumped to 4. Existing DBs get the new columns via ALTER; fresh DBs run both migrations in order.
- **Round 3 — Test signature consistency (Finding 4 fix):** All `spawn_task_dispatcher` callsites pass `shutdown_rx: tokio::sync::watch::Receiver<bool>`. Tests create the watch channel, pass the receiver, and do deterministic shutdown (`shutdown_tx.send(true)` + `handle.await`).
- **Round 4 — Race-free insert (Finding 1 fix):** `delegate()` checks the gate BEFORE insert and inserts directly as `'running'` (gate not paused, `started_at_ms = now`) or `'queued'` (gate paused). Dispatcher only picks `'queued'` tasks, so it never sees synchronous-path tasks. This eliminates the race where the dispatcher could pick a just-inserted `'queued'` task before `delegate()` flipped it. `flip_task_to_running` is removed entirely — no longer needed.

---

## File Structure

### Rust — `crates/busytok-subagent/src/`

| File | Responsibility | Action |
|------|---------------|--------|
| `pressure.rs` (NEW) | `PressureGate` (shared pause flag + last action), `PressureAction` enum, `PressureResponder` (drives §8.3 steps 1-5). | **Create** |
| `resource.rs` | Add `queued_task_count: u32` + `running_task_count: u32` to `ResourceSample`; update `sample()` signature; remove "deferred to Plan 6" comments. | **Modify** |
| `sidecar/supervisor.rs` | Wire `PressureGate` into `PiSidecarSupervisor`; add `pub(crate)` accessors (`try_is_running`, `force_kill`, `shutdown_internal`, `reconcile_crash`, `write_resource_event`); add `prepare_hibernate_all` to `SidecarHandle`; pass task counts to `sample()`; invoke `PressureResponder` on escalation; remove "Plan 6 will" comments. | **Modify** |
| `sidecar/executor.rs` | Add `evict_lru()` method (proactive LRU hibernate for §8.3 step 1). | **Modify** |
| `manager.rs` | Add `task_counts()` method (returns queued/running counts from DB); check `PressureGate` in `delegate()` — return `DelegateResult { status: Queued }` when paused (§8.3 step 2 "queue only"); refactor execution into `execute_task()`; add `spawn_task_dispatcher()` (background worker for async queue). | **Modify** |
| `error.rs` | Add `Paused` variant + `code()` arm. | **Modify** |
| `lib.rs` | Re-export `PressureGate`, `PressureAction`, `PressureResponder`. | **Modify** |
| `sidecar/mod.rs` | Re-export `PressureGate` from the sidecar module. | **Modify** |

### Rust — `crates/busytok-runtime/src/`

| File | Responsibility | Action |
|------|---------------|--------|
| `supervisor.rs` | Construct `PressureGate` + `PressureResponder` in `construct_sidecar` (3→5 tuple); thread gate into `SubagentManager` + `PiSidecarSupervisor`; add `sidecar_executor` + `pressure_responder` + `pressure_gate` + `task_dispatcher` fields to `BusytokSupervisor`; spawn background task dispatcher; replace 6 stubbed doctor checks with real probes. | **Modify** |

### Rust — `crates/busytok-store/src/`

| File | Responsibility | Action |
|------|---------------|--------|
| `repository.rs` | Add `timeout_seconds: Option<i64>` + `model_override: Option<String>` to `SubagentTaskRow` (Round 2 Finding 1 fix). | **Modify** |
| `subagent_queries.rs` | Add `find_lru_hot_binding(conn, harness) -> Option<SubagentHarnessBindingRow>` (for §8.3 step 1); add `task_counts_by_status(conn) -> (u32, u32)` (queued, running); add `pick_oldest_queued_task(conn) -> Option<SubagentTaskRow>` (atomic pick + flip to running via `BEGIN IMMEDIATE` + CAS, Round 3 Finding 1 fix; with per-subagent FIFO guard per Round 2 Finding 2 fix); update `insert_task` + all SELECTs for new `timeout_seconds`/`model_override` columns. | **Modify** |
| `db.rs` | Thin wrappers for the above. | **Modify** |
| `schema.rs` | Register migration `(4, include_str!("../migrations/0004_subagent_task_fields.sql"))`; bump `SCHEMA_VERSION` to 4; update `baseline_single_migration` test assertion (Round 3 Finding 3 fix). | **Modify** |

### Rust — `crates/busytok-store/migrations/`

| File | Responsibility | Action |
|------|---------------|--------|
| `0004_subagent_task_fields.sql` (NEW) | `ALTER TABLE subagent_tasks ADD COLUMN timeout_seconds INTEGER; ALTER TABLE subagent_tasks ADD COLUMN model_override TEXT;` (Round 2 Finding 1 fix + Round 3 Finding 3 fix: incremental migration, NOT modifying `0003_subagent.sql`). | **Create** |

### Rust — `crates/busytok-config/src/`

| File | Responsibility | Action |
|------|---------------|--------|
| `paths.rs` | Add `sidecar_manifest_path(runtime_dir) -> PathBuf` (returns `.../manifest.json`) for doctor `bundle_manifest_readable` check. | **Modify** |

### Tests

| File | Responsibility | Action |
|------|---------------|--------|
| `crates/busytok-subagent/tests/resource.rs` | Update tests for new `ResourceSample` fields; add queued/running count tests. | **Modify** |
| `crates/busytok-subagent/tests/pressure.rs` (NEW) | Unit tests for `PressureGate`, `PressureResponder`, `evict_lru` (with mock sidecar). | **Create** |
| `crates/busytok-subagent/tests/manager_queue.rs` (NEW) | Unit tests for async queue dispatcher: `delegate_returns_queued_when_gate_paused`, `dispatcher_executes_queued_task_when_gate_clears`, per-subagent FIFO guard. | **Create** |
| `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs` | Add pressure-response e2e (hibernate LRU on pressure, pause on pressure, graceful restart on soft-limit, force-kill on hard-limit); add real doctor check tests (6 checks + protocol_version short-lived probe). | **Modify** |

---

## Task 1: Add queued/running task count to `ResourceSample`

**Files:**
- Modify: `crates/busytok-subagent/src/resource.rs:23-36` (struct), `:104-150` (`sample()` signature)
- Modify: `crates/busytok-subagent/src/sidecar/supervisor.rs:477-501` (caller — pass counts)
- Modify: `crates/busytok-store/src/subagent_queries.rs` (add `task_counts_by_status`)
- Modify: `crates/busytok-store/src/db.rs` (thin wrapper)
- Test: `crates/busytok-subagent/tests/resource.rs`

**Interfaces:**
- Produces: `ResourceSample { ..., queued_task_count: u32, running_task_count: u32 }` — later tasks read these fields for logging.
- Produces: `Database::subagent_task_counts_by_status() -> Result<(u32, u32)>` — used by the supervision loop.

- [ ] **Step 1: Write the failing test**

Add to `crates/busytok-subagent/tests/resource.rs`:

```rust
#[test]
fn sample_includes_queued_and_running_task_counts() {
    let policy = busytok_config::SubagentResourcePolicyConfig::default();
    let mut monitor = ResourceMonitor::new(policy, 800, 1200);
    let sample = monitor.sample(None, 0, 5, 2);
    assert_eq!(sample.queued_task_count, 5);
    assert_eq!(sample.running_task_count, 2);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p busytok-subagent --test resource -- sample_includes_queued`
Expected: FAIL — `sample()` takes 2 args, not 4; fields don't exist.

- [ ] **Step 3: Add `task_counts_by_status` to the store**

In `crates/busytok-store/src/subagent_queries.rs`, add after `subagent_count_tasks_since`:

```rust
/// Count subagent tasks by status. Returns (queued, running).
pub fn task_counts_by_status(conn: &Connection) -> Result<(u32, u32)> {
    let queued: i64 = conn.query_row(
        "SELECT COUNT(*) FROM subagent_tasks WHERE status = 'queued'",
        [],
        |row| row.get(0),
    )?;
    let running: i64 = conn.query_row(
        "SELECT COUNT(*) FROM subagent_tasks WHERE status = 'running'",
        [],
        |row| row.get(0),
    )?;
    Ok((queued as u32, running as u32))
}
```

In `crates/busytok-store/src/db.rs`, add a thin wrapper near `subagent_count_tasks_since`:

```rust
/// Count subagent tasks by status. Returns (queued, running).
pub fn subagent_task_counts_by_status(&self) -> Result<(u32, u32)> {
    crate::subagent_queries::task_counts_by_status(&self.conn)
}
```

- [ ] **Step 4: Update `ResourceSample` struct**

In `crates/busytok-subagent/src/resource.rs`, add two fields to `ResourceSample` (after `system_available_mb`):

```rust
/// Number of queued tasks (spec §8.1). Provided by the caller from the
/// subagent_tasks table.
pub queued_task_count: u32,
/// Number of running tasks (spec §8.1). Provided by the caller.
pub running_task_count: u32,
```

- [ ] **Step 5: Update `sample()` signature**

In `crates/busytok-subagent/src/resource.rs`, change `sample()` to:

```rust
pub fn sample(
    &mut self,
    sidecar_pid: Option<u32>,
    hot_session_count: u32,
    queued_task_count: u32,
    running_task_count: u32,
) -> ResourceSample {
    // ... existing body ...
    ResourceSample {
        service_rss_mb,
        sidecar_rss_mb,
        sidecar_cpu_percent,
        hot_session_count,
        system_available_mb,
        queued_task_count,
        running_task_count,
    }
}
```

- [ ] **Step 6: Update the supervision loop caller**

In `crates/busytok-subagent/src/sidecar/supervisor.rs` `maybe_sample_resources`, the current signature is `async fn maybe_sample_resources(&self, sidecar_pid: Option<u32>, hot_sessions: u32)`. Update to read task counts from DB and pass them. Add at the top of `maybe_sample_resources`:

```rust
let (queued, running) = match &self.db {
    Some(db) => db.lock().unwrap().subagent_task_counts_by_status().unwrap_or((0, 0)),
    None => (0, 0),
};
```

Then change the `sample()` call to `monitor.sample(sidecar_pid, hot_sessions, queued, running)`. Update the tracing log to include the new fields:

```rust
info!(
    event_code = "subagent.resource.sample",
    service_rss_mb = sample.service_rss_mb,
    sidecar_rss_mb = ?sample.sidecar_rss_mb,
    sidecar_cpu_percent = ?sample.sidecar_cpu_percent,
    hot_session_count = sample.hot_session_count,
    queued_task_count = sample.queued_task_count,
    running_task_count = sample.running_task_count,
    system_available_mb = sample.system_available_mb,
    "resource sample"
);
```

- [ ] **Step 7: Update all other `sample()` callers in tests**

Grep for `.sample(` in `crates/busytok-subagent/tests/resource.rs` and update each call to pass `(None, 0, 0, 0)` or appropriate test values.

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p busytok-subagent --test resource`
Expected: PASS — all 15 tests.

- [ ] **Step 9: Run clippy + fmt**

Run: `cargo clippy -p busytok-subagent -p busytok-store --tests -- -D warnings && cargo fmt --all -- --check`
Expected: clean.

- [ ] **Step 10: Commit**

```bash
git add crates/busytok-subagent/src/resource.rs crates/busytok-subagent/src/sidecar/supervisor.rs crates/busytok-subagent/tests/resource.rs crates/busytok-store/src/subagent_queries.rs crates/busytok-store/src/db.rs
git commit -m "feat(resource): add queued/running task count to ResourceSample (spec §8.1)"
```

---

## Task 2: Create `PressureGate` shared pause signal

**Files:**
- Create: `crates/busytok-subagent/src/pressure.rs`
- Modify: `crates/busytok-subagent/src/lib.rs` (re-export)
- Test: `crates/busytok-subagent/tests/pressure.rs` (NEW)

**Interfaces:**
- Produces: `PressureGate` (constructable as `Arc::new(PressureGate::new())`), with methods:
  - `set_action(&self, action: PressureAction)` — records the last action + sets paused accordingly.
  - `is_paused(&self) -> bool` — reads the flag (used by `SubagentManager::delegate`).
  - `last_action(&self) -> Option<PressureAction>` — returns the last escalation action set.
- Produces: `PressureAction` enum: `Resume`, `HibernateLru`, `PauseNewTasks`, `GracefulRestart`, `ForceKill`.

- [ ] **Step 1: Write the failing test**

Create `crates/busytok-subagent/tests/pressure.rs`:

```rust
#![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
use busytok_subagent::pressure::{PressureAction, PressureGate};

#[test]
fn gate_starts_unpaused_with_resume_action() {
    let gate = PressureGate::new();
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
}

#[test]
fn pause_new_tasks_sets_paused_flag() {
    let gate = PressureGate::new();
    gate.set_action(PressureAction::PauseNewTasks);
    assert!(gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::PauseNewTasks)));
}

#[test]
fn resume_clears_paused_flag() {
    let gate = PressureGate::new();
    gate.set_action(PressureAction::PauseNewTasks);
    assert!(gate.is_paused());
    gate.set_action(PressureAction::Resume);
    assert!(!gate.is_paused());
}

#[test]
fn hibernate_lru_does_not_pause() {
    let gate = PressureGate::new();
    gate.set_action(PressureAction::HibernateLru);
    assert!(!gate.is_paused());
}

#[test]
fn force_kill_sets_paused() {
    let gate = PressureGate::new();
    gate.set_action(PressureAction::ForceKill);
    assert!(gate.is_paused());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p busytok-subagent --test pressure`
Expected: FAIL — module `pressure` doesn't exist.

- [ ] **Step 3: Implement `pressure.rs` (gate + action enum only — responder added in Task 4)**

Create `crates/busytok-subagent/src/pressure.rs`:

```rust
//! Pressure response gate — shared signal between `PiSidecarSupervisor`
//! (writer) and `SubagentManager` (reader) for spec §8.3 backpressure.
//!
//! The supervisor sets the gate when resource pressure escalates; the
//! manager checks `is_paused()` at the top of `delegate()` to block new
//! task creation (§8.3 step 2).

use std::sync::atomic::{AtomicBool, Ordering};

/// Actions the pressure responder can take (spec §8.3 escalation chain).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureAction {
    /// No pressure — normal operation.
    Resume,
    /// §8.3 step 1: hibernate the LRU hot session. Does NOT pause new tasks.
    HibernateLru,
    /// §8.3 step 2: pause new task execution. Sets the pause flag.
    PauseNewTasks,
    /// §8.3 step 3-4: graceful restart (prepare_hibernate all → restart).
    /// Sets the pause flag during restart.
    GracefulRestart,
    /// §8.3 step 5: force-kill the sidecar. Sets the pause flag until restart.
    ForceKill,
}

impl PressureAction {
    /// Whether this action should pause new task acceptance.
    fn pauses(&self) -> bool {
        matches!(self, Self::PauseNewTasks | Self::GracefulRestart | Self::ForceKill)
    }
}

/// Shared pressure gate. Threaded into both `PiSidecarSupervisor` (writer)
/// and `SubagentManager` (reader) via `Arc<PressureGate>`.
pub struct PressureGate {
    paused: AtomicBool,
    /// Last action taken by the pressure responder.
    last_action: std::sync::Mutex<PressureAction>,
}

impl PressureGate {
    pub fn new() -> Self {
        Self {
            paused: AtomicBool::new(false),
            last_action: std::sync::Mutex::new(PressureAction::Resume),
        }
    }

    /// Record an escalation action and update the pause flag accordingly.
    pub fn set_action(&self, action: PressureAction) {
        self.paused.store(action.pauses(), Ordering::Release);
        if let Ok(mut guard) = self.last_action.lock() {
            *guard = action;
        }
    }

    /// Whether `SubagentManager::delegate` should reject new tasks.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// The last escalation action (for logging/observability).
    pub fn last_action(&self) -> Option<PressureAction> {
        self.last_action.lock().ok().map(|g| *g)
    }
}

impl Default for PressureGate {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Re-export from `lib.rs`**

In `crates/busytok-subagent/src/lib.rs`, add:

```rust
pub mod pressure;
pub use pressure::{PressureAction, PressureGate};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p busytok-subagent --test pressure`
Expected: PASS — 5 tests.

- [ ] **Step 6: Run clippy + fmt**

Run: `cargo clippy -p busytok-subagent --tests -- -D warnings && cargo fmt --all -- --check`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/busytok-subagent/src/pressure.rs crates/busytok-subagent/src/lib.rs crates/busytok-subagent/tests/pressure.rs
git commit -m "feat(pressure): add PressureGate shared signal (spec §8.3 step 2)"
```

---

## Task 3: Wire `PressureGate` into `SubagentManager` + `PiSidecarSupervisor` (pause + accessors)

**Files:**
- Modify: `crates/busytok-subagent/src/manager.rs:23-49` (struct + constructor), `:52-69` (delegate top)
- Modify: `crates/busytok-subagent/src/error.rs` (add `Paused` + `code()` arm)
- Modify: `crates/busytok-subagent/src/sidecar/supervisor.rs` (add `pressure_gate` field, `pub(crate)` accessors, `try_is_running`)
- Modify: `crates/busytok-runtime/src/supervisor.rs:242-314` (construct_sidecar 3→5 tuple), `:325-336` (assemble), struct fields
- Test: `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs` (add pause test)

**Interfaces:**
- Consumes: `PressureGate` from Task 2.
- Produces: `SubagentManager::with_pressure_gate(db, settings, adapter, executor, Option<Arc<PressureGate>>)`; `delegate()` returns `Err(SubagentError::Paused)` when gate is paused.
- Produces: `SubagentManager::task_counts() -> (u32, u32)`.
- Produces: `PiSidecarSupervisor::with_resource_policy(config, db, policy, Option<Arc<PressureGate>>)` — 4 args (gate is the new 4th param).
- Produces: `PiSidecarSupervisor` `pub(crate)` accessors: `try_is_running()`, `shutdown_internal()`, `reconcile_crash()`, `write_resource_event()`, `force_kill()`, `pressure_gate()`, `set_pressure_responder(Arc<PressureResponder>)`, `pressure_responder() -> Option<Arc<PressureResponder>>`.
- Produces: `construct_sidecar` returns 5-tuple: `(Arc<dyn TaskExecutor>, Option<Arc<PiSidecarSupervisor>>, Option<String>, Option<Arc<PressureGate>>, Option<Arc<SidecarTaskExecutor>>)`.
- Produces: `BusytokSupervisor` new fields: `pressure_gate`, `sidecar_executor: Option<Arc<SidecarTaskExecutor>>`, `pressure_responder: Option<Arc<PressureResponder>>`.
- Produces: `BusytokSupervisor::pressure_gate() -> Option<&Arc<PressureGate>>`, `BusytokSupervisor::pressure_responder() -> Option<&Arc<PressureResponder>>`.

- [ ] **Step 1: Write the failing test**

Add to `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`:

```rust
#[tokio::test]
#[serial]
async fn delegate_returns_queued_when_pressure_gate_is_paused() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let mut settings = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false; // mock executor
    settings.subagent.enabled = true;
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    // When sidecar is disabled, no pressure gate is constructed. Use a
    // direct SubagentManager test instead by constructing the manager
    // with an explicit gate. This test verifies the wiring path.
    // (If pressure_gate() is None when sidecar disabled, this test
    // constructs a standalone manager + gate to verify the queue logic.)
    let gate = Arc::new(busytok_subagent::PressureGate::new());
    gate.set_action(busytok_subagent::PressureAction::PauseNewTasks);

    let db2 = Arc::new(std::sync::Mutex::new(
        busytok_store::Database::open_in_memory().unwrap(),
    ));
    let settings2 = busytok_config::SubagentSettings {
        enabled: true,
        ..Default::default()
    };
    let exec = Arc::new(busytok_subagent::mock_executor::MockTaskExecutor)
        as Arc<dyn busytok_subagent::mock_executor::TaskExecutor>;
    let manager = busytok_subagent::SubagentManager::with_pressure_gate(
        db2,
        settings2,
        "pi",
        exec,
        Some(gate.clone()),
    );

    let req = busytok_subagent::DelegateRequest {
        subagent_name: "paused-test".to_string(),
        subagent_id: None,
        cwd: tmp.path().join("repo").to_string_lossy().to_string(),
        profile: "pi/search-cheap".to_string(),
        intent: None,
        prompt: "should be queued".to_string(),
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
    };
    // §8.3 step 2 "queue only": delegate() accepts the task and returns
    // DelegateResult { status: Queued } — NOT an error. The background
    // TaskDispatcher (Task 7) picks it up when the gate clears.
    let result = manager.delegate(req).await.expect("delegate must succeed (queue-only)");
    assert_eq!(
        result.status,
        busytok_subagent::TaskStatus::Queued,
        "delegate must return Queued status when gate is paused, not execute or error"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p busytok-runtime --test subagent_e2e_sidecar -- delegate_returns_queued_when_pressure_gate`
Expected: FAIL — `with_pressure_gate` doesn't exist; `delegate()` doesn't return `Queued` status when gate is paused.

- [ ] **Step 3: Add `SubagentError::Paused` + `code()` arm**

In `crates/busytok-subagent/src/error.rs`, add the variant (in the enum definition) and the `code()` arm:

```rust
// In the SubagentError enum:
#[error("subagent system paused due to resource pressure")]
Paused,

// In the code() match:
SubagentError::Paused => "subagent.paused",
```

Note: `delegate()` no longer returns `Err(Paused)` under the queue-only design (Step 5 returns `Ok(DelegateResult { status: Queued })` instead). The `Paused` variant is retained for the background `TaskDispatcher` (Task 7) — if the gate re-pauses between picking a queued task and executing it, the dispatcher returns `Err(Paused)` to signal re-queue.

- [ ] **Step 4: Add `pressure_gate` field + constructors to `SubagentManager`**

In `crates/busytok-subagent/src/manager.rs`:

```rust
use crate::pressure::{PressureAction, PressureGate};

pub struct SubagentManager {
    db: SharedDb,
    settings: SubagentSettings,
    adapter: String,
    executor: Arc<dyn TaskExecutor>,
    context_builder: ContextBuilder,
    memory_updater: MemoryUpdater,
    pressure_gate: Option<Arc<PressureGate>>,
}

impl SubagentManager {
    pub fn new(
        db: SharedDb,
        settings: SubagentSettings,
        adapter: &str,
        executor: Arc<dyn TaskExecutor>,
    ) -> Self {
        Self::with_pressure_gate(db, settings, adapter, executor, None)
    }

    pub fn with_pressure_gate(
        db: SharedDb,
        settings: SubagentSettings,
        adapter: &str,
        executor: Arc<dyn TaskExecutor>,
        pressure_gate: Option<Arc<PressureGate>>,
    ) -> Self {
        let context_builder = ContextBuilder::new(settings.context.clone());
        let memory_updater = MemoryUpdater::new(settings.context.clone());
        Self {
            db,
            settings,
            adapter: adapter.to_string(),
            executor,
            context_builder,
            memory_updater,
            pressure_gate,
        }
    }
}
```

- [ ] **Step 5: Add queue-only check + race-free insert in `delegate()` (Finding 1 fix + Round 4 Finding 1 fix)**

spec §8.3 step 2 says "Pause new task execution (queue only)" — tasks are accepted but deferred, NOT rejected. **Round 4 race-free design:** `delegate()` checks the gate BEFORE insert and sets the task row status directly:
- Gate paused → insert as `status: "queued"`, return `DelegateResult { status: Queued }` immediately. The background `TaskDispatcher` (Task 7) picks it up when the gate clears.
- Gate not paused → insert as `status: "running"` (with `started_at_ms = now`). The dispatcher only picks `'queued'` tasks, so it never sees this task — no race.

This eliminates the race where the dispatcher could pick a just-inserted `'queued'` task before `delegate()` flipped it. `insert_task` already takes `row.status` from the `SubagentTaskRow` (verified at `subagent_queries.rs:331`), so no store change needed.

In `crates/busytok-subagent/src/manager.rs`, modify `delegate()` at the existing "2. insert task row" step (line ~102-126). Move the gate check BEFORE the insert and set the status field conditionally:

```rust
// §8.3 step 2: check gate BEFORE insert (Round 4 race-free design).
// Gate paused → insert as "queued" + return early.
// Gate not paused → insert as "running" + execute synchronously.
let paused = self.pressure_gate.as_ref().map(|g| g.is_paused()).unwrap_or(false);
let now = busytok_domain::now_ms();
let task_id = format!("task_{}", uuid::Uuid::new_v4());
{
    let db = self.db.lock().expect("subagent db lock poisoned");
    db.subagent_insert_task(&SubagentTaskRow {
        id: task_id.clone(),
        subagent_id: subagent.id.clone(),
        source_harness: req.source_harness.clone(),
        source_session_id: req.source_session_id.clone(),
        intent: req.intent.clone(),
        profile: req.profile.clone(),
        prompt: Some(req.prompt.clone()),
        prompt_artifact_ref: None,
        output_schema_name: None,
        output_schema_version: 1,
        status: if paused { "queued".to_string() } else { "running".to_string() },
        result_summary: None,
        result_json: None,
        error: None,
        created_at_ms: now,
        started_at_ms: if paused { None } else { Some(now) },
        completed_at_ms: None,
    })
    .map_err(SubagentError::Store)?;
}

info!(
    event_code = "subagent.delegate.start",
    subagent_id = %subagent.id,
    created,
    profile = %req.profile,
    "delegating task"
);

// If paused, return Queued — dispatcher handles it when gate clears.
if paused {
    info!(
        event_code = "subagent.delegate.queued",
        subagent_id = %subagent.id,
        task_id = %task_id,
        action = ?self.pressure_gate.as_ref().and_then(|g| g.last_action()),
        "pressure gate paused — task queued, not executed"
    );
    return Ok(DelegateResult {
        task_id,
        subagent_id: subagent.id.clone(),
        subagent_name: subagent.name.clone(),
        adapter: self.adapter.clone(),
        adapter_session_id: None,
        session_reused: false,
        status: TaskStatus::Queued,
        profile: req.profile.clone(),
        model: req.model_override.clone().or(profile_model),
        summary: None,
        usage: TaskUsage::default(),
    });
}

// Gate not paused — task is already "running". Continue with existing
// build-context → execute → persist flow (Task 7 Step 3 refactors this
// into execute_task()).
```

Note: `SubagentError::Paused` is still needed for the `code()` arm (Task 3 Step 3) because the background dispatcher may use it if the gate re-pauses mid-pick. Keep the `Paused` variant + `code()` arm even though `delegate()` never returns it.

Note: At Task 3 time, the `timeout_seconds`/`model_override` columns don't exist yet (Task 7 Step 1 adds them). The insert above uses the current 17-field `SubagentTaskRow`. Task 7 Step 1 adds the two new columns and updates `insert_task` + all SELECTs.

- [ ] **Step 6: Add `task_counts()` method to `SubagentManager`**

In `crates/busytok-subagent/src/manager.rs`:

```rust
pub fn task_counts(&self) -> (u32, u32) {
    let db = self.db.lock().expect("subagent db lock poisoned");
    db.subagent_task_counts_by_status().unwrap_or((0, 0))
}
```

- [ ] **Step 7: Add `pressure_gate` + `pressure_responder` fields + `pub(crate)` accessors to `PiSidecarSupervisor`**

In `crates/busytok-subagent/src/sidecar/supervisor.rs`:

Add fields to `PiSidecarSupervisor`:
```rust
pub struct PiSidecarSupervisor {
    // ... existing fields ...
    pressure_gate: Option<Arc<PressureGate>>,
    /// Weak ref to the pressure responder — set AFTER construction via
    /// `set_pressure_responder`. Weak (not Arc) to break the reference
    /// cycle: supervisor → responder → executor → supervisor.
    /// The strong owner is `BusytokSupervisor.pressure_responder`.
    pressure_responder: std::sync::Mutex<Option<std::sync::Weak<PressureResponder>>>,
}
```

Update both constructors to accept + store `pressure_gate: Option<Arc<PressureGate>>` and initialize `pressure_responder: Mutex::new(None)`. `new` passes `None` for gate. `with_resource_policy` accepts the new gate param. Update ALL callers of `with_resource_policy` (grep the workspace — only 1 caller in `construct_sidecar`).

Add accessors needed by `PressureResponder` (Task 4) — these are `pub(crate)` so `crate::pressure::PressureResponder` can call them:

```rust
impl PiSidecarSupervisor {
    /// Non-blocking check of whether the sidecar child is currently running.
    pub(crate) fn try_is_running(&self) -> bool {
        self.state
            .try_lock()
            .map(|s| s.child.as_ref().map(|c| c.id().is_some()).unwrap_or(false))
            .unwrap_or(false)
    }

    /// §8.3 step 5: force-kill the sidecar child (SIGKILL, no graceful
    /// shutdown). Used by PressureResponder when graceful restart fails.
    pub(crate) async fn force_kill(&self) {
        let mut child = {
            let mut state = self.state.lock().await;
            state.child.take()
        };
        if let Some(child) = child.as_mut() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        // Reconcile as if it crashed.
        self.reconcile_crash();
        self.write_resource_event("sidecar_crash");
    }

    pub(crate) async fn shutdown_internal(&self) -> Result<(), SidecarError> {
        // ... existing body, just change visibility to pub(crate) ...
    }

    pub(crate) fn reconcile_crash(&self) {
        // ... existing body, just change visibility to pub(crate) ...
    }

    pub(crate) fn write_resource_event(&self, event_type: &str) {
        // ... existing body, just change visibility to pub(crate) ...
    }

    pub(crate) fn pressure_gate(&self) -> Option<&Arc<PressureGate>> {
        self.pressure_gate.as_ref()
    }

    /// Set the pressure responder — called by `BusytokSupervisor` after
    /// both supervisor + responder are constructed. Stores a Weak so the
    /// supervision loop can upgrade it without creating a reference cycle.
    pub(crate) fn set_pressure_responder(&self, responder: Arc<PressureResponder>) {
        *self.pressure_responder.lock().unwrap() = Some(Arc::downgrade(&responder));
    }

    /// Upgrade the weak responder ref — returns None if the responder was
    /// dropped (BusytokSupervisor gone). Called by the supervision loop.
    pub(crate) fn pressure_responder(&self) -> Option<Arc<PressureResponder>> {
        self.pressure_responder
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|w| w.upgrade())
    }
}
```

Note: change `async fn shutdown_internal` and `fn reconcile_crash` and `fn write_resource_event` from private to `pub(crate)`. Do NOT change `state` field visibility — `force_kill` is the encapsulated accessor.

- [ ] **Step 8: Wire `PressureGate` + concrete executor in `construct_sidecar` (3-tuple → 5-tuple)**

In `crates/busytok-runtime/src/supervisor.rs`, update `construct_sidecar` return type to 5-tuple including `Option<Arc<PressureGate>>` + `Option<Arc<SidecarTaskExecutor>>` (the concrete executor Arc, needed by `PressureResponder::new` to downgrade):

```rust
fn construct_sidecar(
    settings: &BusytokSettings,
    paths: &BusytokPaths,
    db: &Arc<Mutex<Database>>,
    sidecar_config_override: Option<busytok_subagent::sidecar::SidecarConfig>,
) -> (
    Arc<dyn busytok_subagent::mock_executor::TaskExecutor>,
    Option<Arc<busytok_subagent::sidecar::PiSidecarSupervisor>>,
    Option<String>,
    Option<Arc<busytok_subagent::PressureGate>>,
    Option<Arc<busytok_subagent::sidecar::SidecarTaskExecutor>>,
) {
    if !settings.subagent.pi_sidecar.enabled {
        return (
            Arc::new(busytok_subagent::mock_executor::MockTaskExecutor)
                as Arc<dyn busytok_subagent::mock_executor::TaskExecutor>,
            None,
            None,
            None,
            None,
        );
    }
    let config_result = match sidecar_config_override {
        Some(cfg) => Ok(cfg),
        None => busytok_subagent::sidecar::config::resolve_sidecar_config(
            &settings.subagent.pi_sidecar,
            paths,
        ),
    };
    match config_result {
        Ok(sidecar_config) => {
            let policy = settings.subagent.resource_policy.clone();
            let gate = Arc::new(busytok_subagent::PressureGate::new());
            let sup = busytok_subagent::sidecar::PiSidecarSupervisor::with_resource_policy(
                sidecar_config,
                Some(Arc::clone(db)),
                policy,
                Some(Arc::clone(&gate)),
            );
            // Construct concrete Arc<SidecarTaskExecutor> FIRST, then clone
            // + coerce one copy to Arc<dyn TaskExecutor> for the manager.
            // The concrete copy is kept by BusytokSupervisor so
            // PressureResponder can downgrade it to Weak<SidecarTaskExecutor>.
            let exec_concrete = Arc::new(
                busytok_subagent::sidecar::SidecarTaskExecutor::with_db(
                    Arc::clone(&sup),
                    Arc::clone(db),
                ),
            );
            let exec: Arc<dyn busytok_subagent::mock_executor::TaskExecutor> =
                Arc::clone(&exec_concrete) as Arc<dyn _>;
            (exec, Some(sup), None, Some(gate), Some(exec_concrete))
        }
        Err(e) => {
            let msg = e.to_string();
            error!(
                event_code = "subagent.sidecar.config_resolve_failed",
                error = %e,
                "sidecar config resolve failed; injecting FailingTaskExecutor"
            );
            (
                Arc::new(busytok_subagent::mock_executor::FailingTaskExecutor {
                    reason: msg.clone(),
                })
                    as Arc<dyn busytok_subagent::mock_executor::TaskExecutor>,
                None,
                Some(msg),
                None,
                None,
            )
        }
    }
}
```

- [ ] **Step 9: Update `assemble_with_sidecar` + `BusytokSupervisor` struct**

Update `assemble_with_sidecar` to accept the 4th + 5th tuple elements (`Option<Arc<PressureGate>>` + `Option<Arc<SidecarTaskExecutor>>`). Add THREE new fields to `BusytokSupervisor`:
- `pressure_gate: Option<Arc<PressureGate>>` — threaded into `SubagentManager::with_pressure_gate`.
- `sidecar_executor: Option<Arc<SidecarTaskExecutor>>` — concrete executor Arc (strong owner, keeps executor alive for responder's Weak upgrade).
- `pressure_responder: Option<Arc<PressureResponder>>` — constructed in `assemble_with_sidecar` from `Weak::downgrade(&sup)` + `Weak::downgrade(&exec_concrete)` + `gate.clone()`, then set on the supervisor via `set_pressure_responder`.

Construction order in `assemble_with_sidecar` (after `subagent_manager` is built, before `Self { ... }`):
```rust
// Construct PressureResponder (if sidecar is enabled).
// Holds Weak refs to supervisor + executor — the strong owners are
// BusytokSupervisor fields (sidecar_supervisor, sidecar_executor).
let pressure_responder = match (&sidecar_supervisor, &sidecar_executor, &pressure_gate) {
    (Some(sup), Some(exec), Some(gate)) => {
        let responder = Arc::new(busytok_subagent::PressureResponder::new(
            Arc::downgrade(sup),
            Arc::downgrade(exec),
            Arc::clone(gate),
        ));
        // Set weak ref on the supervisor so the supervision loop can
        // upgrade it when pressure transitions occur.
        sup.set_pressure_responder(Arc::clone(&responder));
        Some(responder)
    }
    _ => None,
};
```

Add accessors:
```rust
pub fn pressure_gate(&self) -> Option<&Arc<PressureGate>> {
    self.pressure_gate.as_ref()
}
pub fn pressure_responder(&self) -> Option<&Arc<PressureResponder>> {
    self.pressure_responder.as_ref()
}
```

Update BOTH callers of `construct_sidecar` (`build_with_settings` at line 196 and `build_with_sidecar_config` at line 224) to destructure 5 elements.

- [ ] **Step 10: Run tests to verify they pass**

Run: `cargo test -p busytok-subagent && cargo test -p busytok-runtime --test subagent_e2e_sidecar -- delegate_returns_paused`
Expected: PASS.

- [ ] **Step 11: Run clippy + fmt + grep for stale `with_resource_policy` callers**

Run: `cargo clippy --workspace --tests -- -D warnings && cargo fmt --all -- --check && grep -rn "with_resource_policy" crates/ --include="*.rs"`
Expected: clean; all `with_resource_policy` call sites updated.

- [ ] **Step 12: Commit**

```bash
git add -A
git commit -m "feat(pressure): wire PressureGate into SubagentManager + supervisor accessors (§8.3 step 2)"
```

---

## Task 4: Implement `evict_lru` + `prepare_hibernate_all` + `PressureResponder`

**Files:**
- Modify: `crates/busytok-store/src/subagent_queries.rs` (add `find_lru_hot_binding`)
- Modify: `crates/busytok-store/src/db.rs` (thin wrapper)
- Modify: `crates/busytok-subagent/src/sidecar/executor.rs` (add `evict_lru`)
- Modify: `crates/busytok-subagent/src/sidecar/supervisor.rs` (add `prepare_hibernate_all` to `SidecarHandle`; wire responder into `maybe_sample_resources`)
- Modify: `crates/busytok-subagent/src/pressure.rs` (add `PressureResponder`)
- Modify: `crates/busytok-runtime/src/supervisor.rs` (construct + store `PressureResponder`)
- Test: `crates/busytok-subagent/tests/pressure.rs` (add `evict_lru` + responder tests)
- Test: `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs` (pressure-response e2e)

**Interfaces:**
- Produces: `PressureResponder::new(supervisor: Weak<PiSidecarSupervisor>, executor: Weak<SidecarTaskExecutor>, gate: Arc<PressureGate>) -> Self`. Both weak to break the cycle (supervisor → responder → executor → supervisor).
- Produces: `PressureResponder::respond(&self, action: PressureAction)` (async) — upgrades weak refs at call time; returns early if either is dropped.
- Produces: `SidecarTaskExecutor::evict_lru(&self) -> anyhow::Result<()>`.
- Produces: `SidecarHandle::prepare_hibernate_all(&self) -> Result<serde_json::Value, SidecarError>`.
- Ownership: `PressureResponder` is strong-owned by `BusytokSupervisor.pressure_responder` (`Option<Arc<PressureResponder>>`). `PiSidecarSupervisor` holds `Weak<PressureResponder>` (via `Mutex<Option<Weak<...>>>`) set via `set_pressure_responder` after construction. The supervision loop calls `self.pressure_responder()` (upgrades the weak ref) to invoke the responder. **No Arc cycle** because `PressureResponder` holds only `Weak` refs to supervisor + executor.

- [ ] **Step 1: Add `find_lru_hot_binding` to the store**

In `crates/busytok-store/src/subagent_queries.rs`, add (note: 11 columns matching `SubagentHarnessBindingRow`):

```rust
/// Find the least-recently-used hot binding for a harness (spec §8.3 step 1).
/// Returns None if no hot bindings exist.
pub fn find_lru_hot_binding(
    conn: &Connection,
    harness: &str,
) -> Result<Option<SubagentHarnessBindingRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, harness, adapter_session_id, adapter_process_id, \
                is_hot, status, created_at_ms, last_used_at_ms, closed_at_ms, detail_json \
         FROM subagent_harness_bindings \
         WHERE harness = ?1 AND is_hot = 1 \
         ORDER BY last_used_at_ms ASC \
         LIMIT 1",
    )?;
    let row = stmt.query_row(params![harness], |row| {
        Ok(SubagentHarnessBindingRow {
            id: row.get(0)?,
            subagent_id: row.get(1)?,
            harness: row.get(2)?,
            adapter_session_id: row.get(3)?,
            adapter_process_id: row.get(4)?,
            is_hot: row.get(5)?,
            status: row.get(6)?,
            created_at_ms: row.get(7)?,
            last_used_at_ms: row.get(8)?,
            closed_at_ms: row.get(9)?,
            detail_json: row.get(10)?,
        })
    });
    match row {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
```

Add thin wrapper `subagent_find_lru_hot_binding(&self, harness: &str)` in `db.rs`.

- [ ] **Step 2: Write the failing test for `evict_lru`**

Add to `crates/busytok-subagent/tests/pressure.rs`:

```rust
use busytok_subagent::sidecar::{SidecarConfig, SidecarTaskExecutor, PiSidecarSupervisor};
use busytok_store::Database;
use std::sync::Arc;
use std::collections::HashMap;
use std::time::Duration;

#[tokio::test]
async fn evict_lru_hibernates_oldest_hot_binding() {
    // Setup: mock sidecar supervisor + DB with 2 hot bindings.
    let db = Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    let config = SidecarConfig {
        node_binary: std::path::PathBuf::from("/usr/bin/true"),
        bundle_path: std::path::PathBuf::from("/dev/null"),
        env: HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(30),
        task_timeout: Duration::from_secs(300),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
    };
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = Arc::new(SidecarTaskExecutor::with_db(Arc::clone(&sup), Arc::clone(&db)));

    // Seed 2 subagents with hot bindings, oldest first.
    {
        let db_guard = db.lock().unwrap();
        // Insert 2 subagents.
        for name in ["old-sub", "new-sub"] {
            let sub_id = format!("sub-{name}");
            db_guard.subagent_insert_logical_subagent(&busytok_store::SubagentLogicalSubagentRow {
                id: sub_id.clone(),
                name: name.to_string(),
                cwd: "/tmp".to_string(),
                status: "warm".to_string(),
                created_at_ms: 0,
                last_active_at_ms: 0,
                detail_json: None,
            }).unwrap();
        }
        // old-sub binding: last_used_at_ms = 1000
        db_guard.subagent_commit_hot_binding_and_status(
            &busytok_store::SubagentHarnessBindingRow {
                id: "bind-old".to_string(),
                subagent_id: "sub-old-sub".to_string(),
                harness: "pi".to_string(),
                adapter_session_id: Some("sess-old".to_string()),
                adapter_process_id: None,
                is_hot: 1,
                status: "hot".to_string(),
                created_at_ms: 0,
                last_used_at_ms: Some(1000),
                closed_at_ms: None,
                detail_json: None,
            },
            "sub-old-sub",
        ).unwrap();
        // new-sub binding: last_used_at_ms = 2000
        db_guard.subagent_commit_hot_binding_and_status(
            &busytok_store::SubagentHarnessBindingRow {
                id: "bind-new".to_string(),
                subagent_id: "sub-new-sub".to_string(),
                harness: "pi".to_string(),
                adapter_session_id: Some("sess-new".to_string()),
                adapter_process_id: None,
                is_hot: 1,
                status: "hot".to_string(),
                created_at_ms: 0,
                last_used_at_ms: Some(2000),
                closed_at_ms: None,
                detail_json: None,
            },
            "sub-new-sub",
        ).unwrap();
    }

    // This test cannot actually run the sidecar (no mock-sidecar.sh in this
    // unit test context), so we verify the LRU PICKER logic only — that
    // find_lru_hot_binding returns the oldest. The full e2e eviction is
    // covered by the pressure_response e2e test in subagent_e2e_sidecar.rs.
    let lru = db.lock().unwrap()
        .subagent_find_lru_hot_binding("pi")
        .unwrap();
    assert!(lru.is_some(), "must find an LRU binding");
    let lru = lru.unwrap();
    assert_eq!(lru.id, "bind-old", "LRU must be the oldest binding (last_used_at_ms=1000)");
}
```

Note: the full `evict_lru` flow (which calls the sidecar via `evict_session`) is covered by the e2e test in Step 9. This unit test verifies the LRU picker query only.

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p busytok-subagent --test pressure -- evict_lru`
Expected: FAIL — `subagent_find_lru_hot_binding` doesn't exist.

- [ ] **Step 4: Implement `evict_lru` on `SidecarTaskExecutor`**

In `crates/busytok-subagent/src/sidecar/executor.rs`, add. Note `evict_session` takes ONE arg (`adapter_session_id`):

```rust
/// Proactively hibernate the LRU hot session (spec §8.3 step 1).
/// Unlike `evict_session` (reactive, sidecar-named candidate), this picks
/// the LRU from the DB and calls `evict_session` with its adapter_session_id.
pub async fn evict_lru(&self) -> anyhow::Result<()> {
    let adapter_session_id = {
        let db = self.db.lock().expect("db lock poisoned");
        let binding = db
            .subagent_find_lru_hot_binding(&self.supervisor.config().harness_name)
            .map_err(|e| anyhow::anyhow!("find_lru_hot_binding failed: {e}"))?;
        let Some(binding) = binding else {
            info!(event_code = "subagent.pressure.no_lru", "no hot binding to hibernate");
            return Ok(());
        };
        binding
            .adapter_session_id
            .ok_or_else(|| anyhow::anyhow!("LRU hot binding has no adapter_session_id"))?
    };
    // Delegate to the existing eviction flow (prepare_hibernate → commit → close).
    self.evict_session(&adapter_session_id).await
}
```

- [ ] **Step 5: Add `prepare_hibernate_all` to `SidecarHandle`**

In `crates/busytok-subagent/src/sidecar/supervisor.rs`, add to `SidecarHandle`:

```rust
/// §8.3 step 4: prepare ALL hot sessions for hibernate before graceful
/// restart. Calls `session.prepare_hibernate` with `{"all": true}`.
/// Returns the sidecar's response (a map of session_id → {memory_delta, stats}).
pub async fn prepare_hibernate_all(&self) -> Result<serde_json::Value, SidecarError> {
    self.supervisor
        .call_rpc(
            "session.prepare_hibernate",
            serde_json::json!({"all": true}),
        )
        .await
}
```

- [ ] **Step 6: Implement `PressureResponder`**

In `crates/busytok-subagent/src/pressure.rs`, add. Note: `PressureResponder` holds `Weak` refs to BOTH supervisor and executor — this is critical to break the reference cycle (supervisor → responder → executor → supervisor). The strong owners are `BusytokSupervisor` fields.

**In-flight deduplication (Finding 3 fix):** `respond()` acquires a `tokio::Mutex` guard. If another `respond()` is already running, the caller skips (does NOT wait) — this prevents concurrent `GracefulRestart`/`ForceKill` from racing on `prepare_hibernate_all` / `shutdown_internal` / `force_kill`.

**ForceKill recovery (Finding 2 fix):** After `force_kill()`, the responder clears the gate to `Resume`. This allows the next `delegate()` to call `ensure_started()` which lazy-spawns a fresh sidecar. Without this, the system would deadlock in paused state with no path to restart.

```rust
use crate::sidecar::{PiSidecarSupervisor, SidecarTaskExecutor};
use std::sync::{Arc, Weak};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

/// Drives the §8.3 5-step escalation chain. Strong-owned by
/// `BusytokSupervisor.pressure_responder`. Holds Weak refs to supervisor
/// + executor to break the reference cycle (supervisor → responder →
/// executor → supervisor would be a cycle if any were Arc).
pub struct PressureResponder {
    supervisor: Weak<PiSidecarSupervisor>,
    executor: Weak<SidecarTaskExecutor>,
    gate: Arc<PressureGate>,
    /// In-flight guard — ensures only ONE pressure action runs at a time.
    /// `try_lock()` is used; if already held, the caller skips (does not wait).
    in_flight: Mutex<()>,
}

impl PressureResponder {
    pub fn new(
        supervisor: Weak<PiSidecarSupervisor>,
        executor: Weak<SidecarTaskExecutor>,
        gate: Arc<PressureGate>,
    ) -> Self {
        Self { supervisor, executor, gate, in_flight: Mutex::new(()) }
    }

    /// Execute an escalation step. Called by the supervision loop on
    /// pressure-state transitions or soft-limit detection.
    ///
    /// **In-flight deduplication:** if another `respond()` is already
    /// running, this call returns immediately (skip, not wait). This
    /// prevents multiple GracefulRestart/ForceKill from racing when
    /// soft/hard limit persists across sampling intervals.
    pub async fn respond(&self, action: PressureAction) {
        // In-flight deduplication: try-lock, skip if already running.
        let Ok(_guard) = self.in_flight.try_lock() else {
            warn!(
                event_code = "subagent.pressure.already_in_flight",
                ?action,
                "another pressure action is in progress, skipping"
            );
            return;
        };
        // Upgrade weak refs — if either is dropped (BusytokSupervisor gone),
        // there's nothing to do.
        let Some(supervisor) = self.supervisor.upgrade() else {
            warn!(event_code = "subagent.pressure.supervisor_dropped", "supervisor dropped — cannot respond");
            return;
        };
        let Some(executor) = self.executor.upgrade() else {
            warn!(event_code = "subagent.pressure.executor_dropped", "executor dropped — cannot respond");
            return;
        };
        match action {
            PressureAction::Resume => {
                self.gate.set_action(PressureAction::Resume);
                info!(event_code = "subagent.pressure.resume", "pressure cleared");
            }
            PressureAction::HibernateLru => {
                info!(event_code = "subagent.pressure.hibernate_lru", "§8.3 step 1: hibernate LRU");
                if let Err(e) = executor.evict_lru().await {
                    warn!(event_code = "subagent.pressure.hibernate_lru_failed", error = %e);
                }
                self.gate.set_action(PressureAction::HibernateLru);
            }
            PressureAction::PauseNewTasks => {
                info!(event_code = "subagent.pressure.pause", "§8.3 step 2: pause new tasks");
                self.gate.set_action(PressureAction::PauseNewTasks);
                let _ = executor.evict_lru().await;
            }
            PressureAction::GracefulRestart => {
                info!(event_code = "subagent.pressure.graceful_restart", "§8.3 steps 3-4: graceful restart");
                self.gate.set_action(PressureAction::GracefulRestart);
                // Step 4: prepare_hibernate all before restart.
                match supervisor.ensure_started().await {
                    Ok(handle) => {
                        if let Err(e) = handle.prepare_hibernate_all().await {
                            warn!(event_code = "subagent.pressure.prepare_hibernate_failed", error = %e);
                        }
                    }
                    Err(e) => {
                        warn!(event_code = "subagent.pressure.ensure_started_failed", error = %e, "cannot prepare_hibernate_all — sidecar not running");
                    }
                }
                // Step 3: graceful shutdown (next ensure_started respawns).
                if let Err(e) = supervisor.shutdown_internal().await {
                    warn!(event_code = "subagent.pressure.shutdown_failed", error = %e, "graceful shutdown failed — escalating to force kill");
                    // Inline ForceKill escalation (avoids async recursion which
                    // would require `Box::pin`). The `_guard` is still held —
                    // that's fine here because we don't re-enter `respond()`;
                    // we run the kill directly, so `try_lock` is not re-entered.
                    error!(event_code = "subagent.pressure.force_kill", "§8.3 step 5: force kill (escalated from graceful restart)");
                    self.gate.set_action(PressureAction::ForceKill);
                    supervisor.force_kill().await;
                    // CRITICAL: clear gate after force kill so the next
                    // delegate() can call ensure_started() which lazy-spawns
                    // a fresh sidecar (Finding 2 fix).
                    self.gate.set_action(PressureAction::Resume);
                    info!(event_code = "subagent.pressure.force_kill_complete", "force kill done, gate cleared — next delegate will lazy-restart");
                    return;
                }
                // Restart succeeded — clear gate so new tasks can proceed.
                self.gate.set_action(PressureAction::Resume);
            }
            PressureAction::ForceKill => {
                error!(event_code = "subagent.pressure.force_kill", "§8.3 step 5: force kill");
                self.gate.set_action(PressureAction::ForceKill);
                supervisor.force_kill().await;
                // CRITICAL: clear gate after force kill so the next delegate()
                // can call ensure_started() which lazy-spawns a fresh sidecar.
                // Without this, the system deadlocks in paused state with no
                // path to restart (Finding 2 fix).
                self.gate.set_action(PressureAction::Resume);
                info!(event_code = "subagent.pressure.force_kill_complete", "force kill done, gate cleared — next delegate will lazy-restart");
            }
        }
    }
}
```

Note: the `GracefulRestart` failure path inlines the `ForceKill` escalation instead of recursively calling `self.respond(ForceKill)`. This avoids `Box::pin` for async recursion, and is safe because the inlined path runs `force_kill()` directly without re-entering `respond()` — so the in-flight `try_lock` guard is never re-acquired.

Re-export from `lib.rs`: `pub use pressure::PressureResponder;`

- [ ] **Step 7: Wire `PressureResponder` into the supervision loop**

`PressureResponder` is constructed in `assemble_with_sidecar` (Task 3 Step 9) and stored as a strong `Arc` on `BusytokSupervisor`. A `Weak<PressureResponder>` is set on `PiSidecarSupervisor` via `set_pressure_responder` (Task 3 Step 7). The supervision loop upgrades this weak ref when it needs to invoke the responder.

In `maybe_sample_resources`, AFTER the existing latch update + DB event write, invoke the responder. Use `self.pressure_responder()` (the accessor from Task 3 Step 7) which upgrades the weak ref:

- [ ] **Step 8: Define the escalation action mapping**

The key insight (verified against `resource.rs` + `supervisor.rs`):
- `exceeds_hard_limit` → state becomes `LimitExceeded` → `ForceKill` (§8.3 step 5).
- `exceeds_soft_limit` (but NOT hard) → state is `Pressure` (folded with `under_pressure`) → `GracefulRestart` (§8.3 step 3). This is checked as a SEPARATE predicate, NOT via state transition.
- `under_pressure` (system memory low) → state is `Pressure` → `HibernateLru` + `PauseNewTasks` (§8.3 steps 1-2).

In `maybe_sample_resources`, AFTER the existing latch update + DB event write, add the responder invocation. The action is picked based on the NEW state + the soft/hard predicates:

```rust
// §8.3 pressure response actions. Run AFTER the DB event write so the
// observability signal is recorded before the action.
let action = if exceeds_hard {
    // Hard limit exceeded → force kill (step 5).
    Some(PressureAction::ForceKill)
} else if exceeds_soft {
    // Soft limit exceeded (but not hard) → graceful restart (steps 3-4).
    Some(PressureAction::GracefulRestart)
} else if new_state == ResourcePressureState::Pressure && old_state == ResourcePressureState::Normal {
    // Entering pressure (system memory low, no soft/hard exceeded) →
    // hibernate LRU + pause new tasks (steps 1-2).
    Some(PressureAction::PauseNewTasks)
} else if new_state == ResourcePressureState::Normal && is_recovery {
    // Recovery → resume.
    Some(PressureAction::Resume)
} else {
    None
};
if let Some(action) = action {
    if let Some(responder) = self.pressure_responder() {
        // Spawn the response to avoid blocking the supervision loop.
        tokio::spawn(async move {
            responder.respond(action).await;
        });
    }
}
```

Note: `HibernateLru` is folded into `PauseNewTasks` (the `PauseNewTasks` arm also calls `evict_lru`). This matches spec §8.3's intent (steps 1 and 2 happen together on pressure entry).

- [ ] **Step 9: Write the pressure-response e2e tests**

Add to `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`. Use the existing `make_sidecar_config()` helper (which points at mock-sidecar.sh) with modified memory limits, and `new_with_sidecar_config` to construct the supervisor. The `resource_policy.monitor_interval_seconds` is in settings (not SidecarConfig), so save settings to the config file first.

```rust
#[tokio::test]
#[serial]
async fn pressure_response_force_kills_on_rss_limit_exceeded() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let mut settings = make_sidecar_settings();
    settings.subagent.resource_policy.monitor_interval_seconds = 1;
    let paths = BusytokPaths::for_test(tmp.path());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    // Use mock sidecar with hard/soft limit = 1MB (sidecar RSS always exceeds this).
    let mut config = make_sidecar_config();
    config.memory_hard_limit_mb = 1;
    config.memory_soft_limit_mb = 1;

    let supervisor = BusytokSupervisor::new_with_sidecar_config(db.clone(), paths, config);

    // Delegate once to start the sidecar + supervision loop.
    let _ = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "pressure-test".to_string(),
            subagent_id: None,
            cwd: tmp.path().to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "trigger sidecar".to_string(),
            timeout_seconds: Some(5),
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await;

    // Wait for the supervision loop to sample + detect hard limit + force-kill.
    // Poll for up to 10s for a sidecar_crash event.
    let mut crashed = false;
    for _ in 0..100 {
        let events = db.subagent_list_resource_events(None, 200).unwrap();
        if events.iter().any(|e| e.event_type == "sidecar_crash") {
            crashed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(crashed, "sidecar_crash event must be written after hard-limit force-kill");

    // Finding 2 fix: after force-kill, the PressureResponder clears the
    // gate to Resume so the next delegate() can lazy-restart the sidecar.
    // If the gate stayed paused, the system would deadlock with no path
    // to restart.
    let gate = supervisor.pressure_gate().expect("gate must be present");
    assert!(!gate.is_paused(), "gate must be cleared (Resume) after force-kill to allow lazy restart");

    supervisor.shutdown_writer().await.unwrap();
}
```

Add a similar test for `pressure_response_pauses_on_memory_pressure` using `settings.subagent.resource_policy.memory_pressure_free_mb = 999999` (always pressured) and `config.memory_soft_limit_mb = 800` (not exceeded — so only `PauseNewTasks`, not `GracefulRestart`):

- [ ] **Step 10: Run tests to verify they pass**

Run: `cargo test -p busytok-subagent --test pressure && cargo test -p busytok-runtime --test subagent_e2e_sidecar -- pressure_response`
Expected: PASS.

- [ ] **Step 11: Remove "deferred to Plan 6" / "Plan 6 will" comments**

Grep for `Plan 6` across `crates/busytok-subagent/src/` and `crates/busytok-runtime/src/` and remove/update each comment now that Plan 6 is implemented:

```bash
grep -rn "Plan 6\|deferred to Plan 6\|Plan 6 will" crates/busytok-subagent/src/ crates/busytok-runtime/src/ --include="*.rs"
```

Update each comment to reflect the implemented behavior (or remove if now stale).

- [ ] **Step 12: Run clippy + fmt**

Run: `cargo clippy --workspace --tests -- -D warnings && cargo fmt --all -- --check`
Expected: clean.

- [ ] **Step 13: Commit**

```bash
git add -A
git commit -m "feat(pressure): implement §8.3 5-step escalation chain (PressureResponder + evict_lru)"
```

---

## Task 5: Implement the 6 real doctor checks

**Files:**
- Modify: `crates/busytok-config/src/paths.rs:170-174` (add `sidecar_manifest_path` next to `sidecar_bundle_path`)
- Modify: `crates/busytok-runtime/src/supervisor.rs:510-527` (replace stub loop)
- Test: `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs` (update existing doctor test + add new)

**Interfaces:**
- Consumes: `BusytokPaths` (on `self`), `BusytokSettings` (on `self`), `PROTOCOL_VERSION`, `std::env::consts::ARCH`, `PiSidecarSupervisor::try_is_running()` (added in Task 3).
- Produces: `BusytokPaths::sidecar_manifest_path(runtime_dir: Option<&str>) -> PathBuf` (returns `.../manifest.json`).

- [ ] **Step 1: Add `sidecar_manifest_path` to `BusytokPaths`**

In `crates/busytok-config/src/paths.rs`, add next to `sidecar_bundle_path` (line 174):

```rust
/// Path to the sidecar bundle manifest (spec §5.1 line 549).
/// `manifest.json` sits alongside `pi-sidecar.bundle.js` in the runtime dir.
/// Doctor check verifies this file is readable + valid JSON.
pub fn sidecar_manifest_path(&self, runtime_dir: Option<&str>) -> std::path::PathBuf {
    self.sidecar_runtime_dir(runtime_dir).join("manifest.json")
}
```

- [ ] **Step 2: Write the failing tests**

Add to `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`. These are concrete tests with real fixture dirs:

```rust
#[tokio::test]
#[serial]
async fn doctor_bundled_node_arch_check_validates_arch_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = tmp.path().join("rt");
    let arch_dir = runtime_dir.join("node").join(std::env::consts::ARCH);
    std::fs::create_dir_all(&arch_dir).unwrap();
    std::fs::write(arch_dir.join("node"), b"#!/bin/sh\n").unwrap();

    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let mut settings = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings.subagent.pi_sidecar.runtime_dir = Some(runtime_dir.to_string_lossy().to_string());
    settings.save_to_file(&paths.config_dir().join("settings.toml")).unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub.checks.iter().find(|c| c.name == "bundled_node_arch").unwrap();
    assert_eq!(check.status, "ok", "arch matches + node exists → ok: {:?}", check.detail);

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_bundled_node_arch_check_errors_on_missing_node() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = tmp.path().join("rt");
    // Don't create the node binary — should error.
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let mut settings = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings.subagent.pi_sidecar.runtime_dir = Some(runtime_dir.to_string_lossy().to_string());
    settings.save_to_file(&paths.config_dir().join("settings.toml")).unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub.checks.iter().find(|c| c.name == "bundled_node_arch").unwrap();
    assert_eq!(check.status, "error", "missing node → error");
    assert!(check.detail.as_deref().unwrap_or("").contains("not found"), "detail should say not found: {:?}", check.detail);

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_bundle_manifest_readable_check_validates_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = tmp.path().join("rt");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    // Write a valid manifest.json (spec §5.1 line 549).
    std::fs::write(
        runtime_dir.join("manifest.json"),
        br#"{"name":"pi-sidecar","version":"0.1.0"}"#,
    ).unwrap();

    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let mut settings = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings.subagent.pi_sidecar.runtime_dir = Some(runtime_dir.to_string_lossy().to_string());
    settings.save_to_file(&paths.config_dir().join("settings.toml")).unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub.checks.iter().find(|c| c.name == "bundle_manifest_readable").unwrap();
    assert_eq!(check.status, "ok", "valid manifest.json → ok");

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_bundle_manifest_readable_check_fails_on_malformed_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = tmp.path().join("rt");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    // Write a malformed manifest.json (not valid JSON).
    std::fs::write(runtime_dir.join("manifest.json"), b"not json {{{").unwrap();

    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let mut settings = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings.subagent.pi_sidecar.runtime_dir = Some(runtime_dir.to_string_lossy().to_string());
    settings.save_to_file(&paths.config_dir().join("settings.toml")).unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub.checks.iter().find(|c| c.name == "bundle_manifest_readable").unwrap();
    assert_eq!(check.status, "error", "malformed manifest.json → error");

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_default_model_config_check_validates_models() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let mut settings = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings.subagent.models.default_cheap_model = "".to_string(); // empty → error
    settings.save_to_file(&paths.config_dir().join("settings.toml")).unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub.checks.iter().find(|c| c.name == "default_model_config").unwrap();
    assert_eq!(check.status, "error", "empty model field → error");
    assert!(check.detail.as_deref().unwrap_or("").contains("default_cheap_model"), "detail should name the empty field: {:?}", check.detail);

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_artifact_store_writable_check_writes_probe_file() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let mut settings = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings.save_to_file(&paths.config_dir().join("settings.toml")).unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub.checks.iter().find(|c| c.name == "artifact_store_writable").unwrap();
    // artifacts_dir is created by BusytokPaths::for_test → writable.
    assert_eq!(check.status, "ok", "artifacts dir writable → ok: {:?}", check.detail);

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_protocol_version_check_is_warning_when_pi_sidecar_disabled() {
    // When pi_sidecar.enabled = false, there is no sidecar supervisor at
    // all (sidecar_supervisor = None). The check returns "warning" because
    // there is nothing to probe — this is NOT the "sidecar not running" case.
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let mut settings = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings.save_to_file(&paths.config_dir().join("settings.toml")).unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub.checks.iter().find(|c| c.name == "protocol_version").unwrap();
    assert_eq!(check.status, "warning", "pi_sidecar disabled → warning (no supervisor to probe)");
    assert!(check.detail.as_deref().unwrap_or("").contains("disabled"), "detail should mention disabled: {:?}", check.detail);

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_protocol_version_check_probes_via_short_lived_sidecar_when_enabled() {
    // When pi_sidecar.enabled = true but the sidecar is not running, the
    // check does a SHORT-LIVED PROBE: ensure_started() → verify protocol
    // via adapter.initialize → shutdown_internal(). With no bundle present,
    // ensure_started() fails → "error" (NOT "warning"). This is the key
    // difference from the old stub which returned "warning" unconditionally.
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let mut settings = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = true;
    // No bundle/node installed → ensure_started() will fail → "error".
    settings.subagent.pi_sidecar.runtime_dir = Some(tmp.path().join("rt").to_string_lossy().to_string());
    settings.save_to_file(&paths.config_dir().join("settings.toml")).unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub.checks.iter().find(|c| c.name == "protocol_version").unwrap();
    assert_eq!(check.status, "error", "enabled but probe fails → error (not warning)");
    assert!(check.detail.as_deref().unwrap_or("").contains("probe"), "detail should mention probe failure: {:?}", check.detail);

    supervisor.shutdown_writer().await.unwrap();
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p busytok-runtime --test subagent_e2e_sidecar -- doctor_bundled doctor_bundle doctor_default_model doctor_artifact doctor_protocol`
Expected: FAIL — checks still return "warning" stub; `doctor_protocol_version_check_probes_via_short_lived_sidecar_when_enabled` fails because the stub returns "warning" instead of "error".

- [ ] **Step 4: Implement the 6 real checks**

In `crates/busytok-runtime/src/supervisor.rs`, replace the stub loop (lines ~510-527) with individual real checks. Each check reuses existing paths:

```rust
let runtime_dir = self.settings.subagent.pi_sidecar.runtime_dir.as_deref();

// 4. Bundled Node architecture matches (spec §7.1 line 865).
{
    let node_path = self.paths.sidecar_bundled_node_path(runtime_dir);
    let expected_arch = std::env::consts::ARCH;
    let arch_ok = node_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|n| n == expected_arch)
        .unwrap_or(false);
    let node_exists = node_path.exists();
    let ok = arch_ok && node_exists;
    let detail = if !node_exists {
        format!("bundled node not found at {}", node_path.display())
    } else if !arch_ok {
        format!("arch mismatch: expected {expected_arch}")
    } else {
        format!("ok ({expected_arch})")
    };
    checks.push(DoctorCheckDto {
        name: "bundled_node_arch".into(),
        status: if ok { "ok" } else { "error" }.into(),
        detail: Some(detail),
    });
}

// 5. Bundle manifest readable (spec §7.1 line 866, §5.1 line 549).
//    Spec §5.1 defines the bundle directory layout:
//      <sidecar_runtime_dir>/
//        pi-sidecar.bundle.js
//        manifest.json          ← this file
//        node/<arch>/node
//    The check verifies manifest.json EXISTS, is READABLE (open succeeds),
//    and is PARSEABLE as JSON (serde_json::from_str succeeds). A missing or
//    malformed manifest is an "error" — the sidecar cannot be launched
//    without a valid manifest.
{
    let manifest_path = self.paths.sidecar_manifest_path(runtime_dir);
    let (status, detail) = match std::fs::read_to_string(&manifest_path) {
        Ok(contents) => {
            match serde_json::from_str::<serde_json::Value>(&contents) {
                Ok(_v) => ("ok", format!("manifest readable ({})", manifest_path.display())),
                Err(e) => ("error", format!("manifest at {} is not valid JSON: {}", manifest_path.display(), e)),
            }
        }
        Err(e) => ("error", format!("manifest not readable at {}: {}", manifest_path.display(), e)),
    };
    checks.push(DoctorCheckDto {
        name: "bundle_manifest_readable".into(),
        status: status.into(),
        detail: Some(detail),
    });
}

// 6. Protocol version matches (spec §7.1 line 867).
//    Real probe (Finding 4 fix): if sidecar is already running, protocol
//    version was verified during `adapter.initialize` in `ensure_started` → "ok".
//    If not running, do a SHORT-LIVED PROBE: `ensure_started()` (spawns +
//    verifies protocol via adapter.initialize), then `shutdown_internal()`
//    to clean up. This is a real check, not a warning stub.
{
    let expected_pv = busytok_subagent::sidecar::protocol::PROTOCOL_VERSION;
    let (status, detail) = match &self.sidecar_supervisor {
        Some(sup) if sup.try_is_running() => {
            // Already running — protocol was verified during init.
            ("ok", format!("protocol_version={expected_pv}, sidecar running (verified during init)"))
        }
        Some(sup) => {
            // Not running — short-lived probe: start, verify, shutdown.
            match sup.ensure_started().await {
                Ok(_handle) => {
                    // adapter.initialize succeeded → protocol version matched
                    // (mismatch returns SidecarError during init handshake).
                    // Shutdown to clean up the probe process.
                    if let Err(e) = sup.shutdown_internal().await {
                        warn!(event_code = "subagent.doctor.protocol_probe_shutdown_failed", error = %e, "short-lived probe shutdown failed");
                    }
                    ("ok", format!("protocol_version={expected_pv}, verified via short-lived probe"))
                }
                Err(e) => {
                    ("error", format!("protocol probe failed (ensure_started): {e}"))
                }
            }
        }
        None => {
            // No sidecar supervisor configured (pi_sidecar disabled).
            ("warning", "pi_sidecar disabled — cannot probe protocol version".into())
        }
    };
    checks.push(DoctorCheckDto {
        name: "protocol_version".into(),
        status: status.into(),
        detail: Some(detail),
    });
}

// 7. Default model config valid (spec §7.1 line 868).
{
    let models = &self.settings.subagent.models;
    let empty_fields: Vec<&str> = [
        ("default_cheap_model", &models.default_cheap_model),
        ("default_review_model", &models.default_review_model),
        ("default_reasoning_model", &models.default_reasoning_model),
        ("default_coder_model", &models.default_coder_model),
    ]
    .iter()
    .filter(|(_, v)| v.is_empty())
    .map(|(k, _)| *k)
    .collect();
    let ok = empty_fields.is_empty();
    let detail = if ok {
        "all 4 default models configured".to_string()
    } else {
        format!("empty model fields: {}", empty_fields.join(", "))
    };
    checks.push(DoctorCheckDto {
        name: "default_model_config".into(),
        status: if ok { "ok" } else { "error" }.into(),
        detail: Some(detail),
    });
}

// 8. Pi runtime installed (spec §7.1 line 869).
{
    let node_path = self.paths.sidecar_bundled_node_path(runtime_dir);
    let bundle_path = self.paths.sidecar_bundle_path(runtime_dir);
    let ok = node_path.exists() && bundle_path.exists();
    let detail = if ok {
        "node + bundle present".to_string()
    } else {
        format!("missing: node={} bundle={}", node_path.exists(), bundle_path.exists())
    };
    checks.push(DoctorCheckDto {
        name: "pi_runtime_installed".into(),
        status: if ok { "ok" } else { "error" }.into(),
        detail: Some(detail),
    });
}

// 9. Artifact store writable (spec §7.1 line 870).
{
    let artifacts_dir = self.paths.artifacts_dir();
    let dir_exists = artifacts_dir.exists();
    let probe_ok = if dir_exists {
        let probe_path = artifacts_dir.join(".busytok_doctor_probe");
        std::fs::write(&probe_path, b"probe").is_ok()
            && std::fs::remove_file(&probe_path).is_ok()
    } else {
        false
    };
    let detail = if probe_ok {
        format!("writable ({})", artifacts_dir.display())
    } else if !dir_exists {
        format!("artifacts dir missing: {}", artifacts_dir.display())
    } else {
        format!("not writable: {}", artifacts_dir.display())
    };
    checks.push(DoctorCheckDto {
        name: "artifact_store_writable".into(),
        status: if probe_ok { "ok" } else { "error" }.into(),
        detail: Some(detail),
    });
}
```

- [ ] **Step 5: Update the existing `settings_diagnostics_includes_subagent_doctor_with_11_checks` test**

The test currently asserts the 6 checks return `"warning"` with "not yet implemented". Update assertions:
- `bundled_node_arch`, `bundle_manifest_readable`, `pi_runtime_installed` → `"error"` (bundle missing in default test setup).
- `default_model_config` → `"ok"` (default settings have all 4 models) OR `"error"` if defaults are empty — check the default `SubagentModelsConfig` and assert accordingly.
- `artifact_store_writable` → `"ok"` (artifacts dir created by `BusytokPaths::for_test`).
- `protocol_version` → `"error"` (short-lived probe fails because no valid sidecar bundle in default test setup — `ensure_started` returns SidecarError).

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p busytok-runtime --test subagent_e2e_sidecar -- doctor`
Expected: PASS.

- [ ] **Step 7: Run clippy + fmt**

Run: `cargo clippy -p busytok-runtime --tests -- -D warnings && cargo fmt --all -- --check`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(doctor): implement 6 real doctor checks (spec §7.1)"
```

---

## Task 7: Background task dispatcher for §8.3 "queue only" (Finding 1 fix)

spec §8.3 step 2 says "Pause new task execution (queue only)". Task 3 Step 5 made `delegate()` return `DelegateResult { status: Queued }` when the gate is paused. This task adds the background worker that picks up queued tasks when the gate clears and executes them.

**Reviewer fixes applied:**
- Finding 1: `execute_task` consumes `&SubagentTaskRow` directly (NOT reconstructed `DelegateRequest`). `timeout_seconds` and `model_override` are persisted as new columns in `subagent_tasks` so the row is the single source of truth.
- Finding 2: `pick_oldest_queued_task` enforces per-subagent FIFO (spec §6.4 line 737) — only picks from subagents with NO running task.
- Finding 3: Dispatcher shutdown via `tokio::sync::watch::Sender<bool>` — `JoinHandle` drop = detach (NOT abort), so explicit shutdown signal is required.
- Round 3 Finding 1: `pick_oldest_queued_task` is truly atomic — `BEGIN IMMEDIATE` transaction + CAS guard (`WHERE id = ? AND status = 'queued'`). No double-consumption under race.
- Round 3 Finding 2: No double-flip — `execute_task()` does NOT flip `queued → running`. `delegate()` inserts directly as `'running'` (Round 4 fix); dispatcher relies on `pick_oldest_queued_task`'s atomic flip. Single authoritative `started_at_ms`.
- Round 3 Finding 3: Incremental migration `0004_subagent_task_fields.sql` (ALTER TABLE), NOT modifying `0003_subagent.sql`. `SCHEMA_VERSION` → 4.
- Round 3 Finding 4: All `spawn_task_dispatcher` callsites pass `shutdown_rx: watch::Receiver<bool>`. Tests do deterministic shutdown (`send(true)` + `await`).
- Round 4 Finding 1: Race-free insert — `delegate()` checks gate BEFORE insert and inserts directly as `'running'` (when gate not paused) or `'queued'` (when paused). Eliminates the race where the dispatcher could pick a just-inserted `'queued'` task before `delegate()` flipped it. `flip_task_to_running` removed entirely.

**Files:**
- Create: `crates/busytok-store/migrations/0004_subagent_task_fields.sql` (incremental migration — `ALTER TABLE subagent_tasks ADD COLUMN ...` for `timeout_seconds` + `model_override`; Round 3 Finding 3 fix: do NOT modify `0003_subagent.sql`)
- Modify: `crates/busytok-store/src/schema.rs` (register migration `(4, include_str!("../migrations/0004_subagent_task_fields.sql"))`, bump `SCHEMA_VERSION` to 4, update `baseline_single_migration` test assertion to `== 4`)
- Modify: `crates/busytok-store/src/repository.rs` (add fields to `SubagentTaskRow`)
- Modify: `crates/busytok-store/src/subagent_queries.rs` (update INSERT/SELECT + add `pick_oldest_queued_task` with FIFO guard)
- Modify: `crates/busytok-store/src/db.rs` (add wrapper `subagent_pick_oldest_queued_task`)
- Modify: `crates/busytok-subagent/src/manager.rs` (refactor `execute_task` to consume `&SubagentTaskRow` + add `spawn_task_dispatcher` with watch channel + race-free insert-as-running in `delegate()`)
- Modify: `crates/busytok-runtime/src/supervisor.rs` (spawn dispatcher + store shutdown sender)
- Test: `crates/busytok-subagent/tests/manager_queue.rs` (new test file)

**Interfaces:**
- Consumes: `PressureGate` from Task 2, `SubagentManager` fields (`db`, `executor`, `context_builder`, `memory_updater`, `settings`).
- Produces: `SubagentManager::execute_task(&self, task: &SubagentTaskRow, subagent: &LogicalSubagent) -> Result<DelegateResult>` — reads authoritative fields from the row; does NOT flip status (Round 3 Finding 2 fix).
- Produces: `SubagentManager::spawn_task_dispatcher(self: &Arc<Self>, shutdown: tokio::sync::watch::Receiver<bool>) -> JoinHandle` — spawns background worker with shutdown signal.
- Produces: `Database::subagent_pick_oldest_queued_task() -> Option<SubagentTaskRow>` — atomically picks + flips to "running" (BEGIN IMMEDIATE + CAS); enforces per-subagent FIFO.

- [ ] **Step 1: Add `timeout_seconds` + `model_override` columns via `0004_subagent_task_fields.sql` (Round 3 Finding 3 fix)**

**Round 3 Finding 3 fix:** use an incremental migration (`0004_subagent_task_fields.sql`), NOT modifying `0003_subagent.sql`. Existing databases that already ran `0003` need the `ALTER TABLE` to add the new columns; modifying `0003` would only affect fresh DBs and break the upgrade path.

Create `crates/busytok-store/migrations/0004_subagent_task_fields.sql`:

```sql
-- 0004_subagent_task_fields.sql
-- Round 3 Finding 3 fix: incremental migration for new task row columns.
-- Do NOT modify 0003_subagent.sql — existing DBs need the ALTER TABLE.

ALTER TABLE subagent_tasks ADD COLUMN timeout_seconds INTEGER;
ALTER TABLE subagent_tasks ADD COLUMN model_override TEXT;
```

In `crates/busytok-store/src/schema.rs`:
- Bump `SCHEMA_VERSION` from 3 to 4.
- Register the migration: add `(4, include_str!("../migrations/0004_subagent_task_fields.sql"))` to `migrations()`.
- Update the `baseline_single_migration` test assertion from `== 3` to `== 4` (or whatever the current count is).

In `crates/busytok-store/src/repository.rs`, add fields to `SubagentTaskRow` (after `completed_at_ms`):

```rust
pub struct SubagentTaskRow {
    // ... existing 17 fields ...
    pub timeout_seconds: Option<i64>,    // NEW
    pub model_override: Option<String>,  // NEW
}
```

Update ALL existing INSERT/SELECT statements in `subagent_queries.rs` that touch `subagent_tasks` to include the two new columns. This includes `insert_task` (line ~315), `list_tasks` (line ~911), `pick_oldest_queued_task` (Step 2 below), and any other SELECT.

- [ ] **Step 2: Add `pick_oldest_queued_task` with per-subagent FIFO guard (Finding 2 fix) + atomic CAS (Round 3 Finding 1 fix)**

In `crates/busytok-store/src/subagent_queries.rs`, add near `count_tasks_since` (line ~437). The WHERE clause enforces spec §6.4 line 737 (same logical subagent tasks are serialized). **Round 3 Finding 1 fix:** the pick + flip is wrapped in a single `BEGIN IMMEDIATE` transaction with a CAS guard (`WHERE id = ?2 AND status = 'queued'`). If two dispatchers race, the second UPDATE affects 0 rows and returns `None`. Uses RAII `Transaction` (auto-rollback on drop) per the project's SQLite lesson learned.

```rust
/// Atomically pick the oldest "queued" task and flip it to "running".
/// Enforces per-subagent FIFO (spec §6.4 line 737): only picks from
/// subagents that have NO running task. This ensures same-subagent tasks
/// are serialized.
///
/// **Atomicity (Round 3 Finding 1 fix):** pick + flip happen inside a
/// single `BEGIN IMMEDIATE` transaction with a CAS guard
/// (`WHERE id = ? AND status = 'queued'`). If two dispatchers race on
/// the same task, only one UPDATE affects 1 row; the other gets 0 rows
/// and returns `None`. The RAII `Transaction` auto-rolls-back on drop.
pub fn pick_oldest_queued_task(conn: &rusqlite::Connection) -> rusqlite::Result<Option<SubagentTaskRow>> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    // 1. Pick candidate id (still 'queued', per-subagent FIFO).
    let id_opt: Option<String> = tx
        .query_row(
            "SELECT id FROM subagent_tasks
             WHERE status = 'queued'
               AND subagent_id NOT IN (
                   SELECT subagent_id FROM subagent_tasks WHERE status = 'running'
               )
             ORDER BY created_at_ms ASC
             LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(id) = id_opt else {
        tx.commit()?;
        return Ok(None);
    };
    // 2. CAS flip: only updates if still 'queued'. rows_affected == 1 means we won.
    let now = busytok_domain::now_ms();
    let rows = tx.execute(
        "UPDATE subagent_tasks SET status = 'running', started_at_ms = ?1
         WHERE id = ?2 AND status = 'queued'",
        rusqlite::params![now, id],
    )?;
    if rows == 0 {
        // Lost the race — another dispatcher flipped it first.
        tx.commit()?;
        return Ok(None);
    }
    // 3. Fetch the full row (status is now 'running', started_at_ms = now).
    let task = tx
        .query_row(
            "SELECT id, subagent_id, source_harness, source_session_id, intent, profile,
                    prompt, prompt_artifact_ref, output_schema_name, output_schema_version,
                    status, result_summary, result_json, error,
                    created_at_ms, started_at_ms, completed_at_ms,
                    timeout_seconds, model_override
             FROM subagent_tasks WHERE id = ?1",
            rusqlite::params![id],
            |r| {
                Ok(SubagentTaskRow {
                    id: r.get(0)?,
                    subagent_id: r.get(1)?,
                    source_harness: r.get(2)?,
                    source_session_id: r.get(3)?,
                    intent: r.get(4)?,
                    profile: r.get(5)?,
                    prompt: r.get(6)?,
                    prompt_artifact_ref: r.get(7)?,
                    output_schema_name: r.get(8)?,
                    output_schema_version: r.get(9)?,
                    status: r.get(10)?,
                    result_summary: r.get(11)?,
                    result_json: r.get(12)?,
                    error: r.get(13)?,
                    created_at_ms: r.get(14)?,
                    started_at_ms: r.get(15)?,
                    completed_at_ms: r.get(16)?,
                    timeout_seconds: r.get(17)?,
                    model_override: r.get(18)?,
                })
            },
        )
        .optional()?;
    tx.commit()?;
    Ok(task)
}
```

In `crates/busytok-store/src/db.rs`, add wrapper near `subagent_count_tasks_since` (line ~1949):

```rust
pub fn subagent_pick_oldest_queued_task(&self) -> rusqlite::Result<Option<SubagentTaskRow>> {
    subagent_queries::pick_oldest_queued_task(self.conn())
}
```

- [ ] **Step 3: Refactor `delegate()` to extract `execute_task()` + add new columns to insert (Finding 1 fix)**

In `crates/busytok-subagent/src/manager.rs`, refactor the execution portion of `delegate()` (build context → execute → persist) into a new method `execute_task()`. This is called by both `delegate()` (synchronous path) and the background dispatcher (async queue path).

**Note:** The check-gate-before-insert + insert-as-running/queued logic was already implemented in Task 3 Step 5. Task 7 Step 3 does NOT change that logic — it only:
1. Adds `timeout_seconds` + `model_override` fields to the `SubagentTaskRow` insert (Task 7 Step 1 added the columns).
2. Extracts the post-insert execute logic into `execute_task()` so the dispatcher can reuse it.

**Key change (Finding 1 fix):** `execute_task` takes `&SubagentTaskRow` directly, NOT a reconstructed `DelegateRequest`. It reads `task.prompt`, `task.prompt_artifact_ref`, `task.profile`, `task.timeout_seconds`, `task.model_override` — all authoritative fields from the persisted row. No data loss.

**Round 3 Finding 2 fix (no double-flip):** `execute_task()` does NOT flip `queued → running`. It assumes the task is ALREADY `"running"` (with `started_at_ms` set). `delegate()` already inserts as `'running'` (Task 3 Step 5); the dispatcher's `pick_oldest_queued_task` does the atomic flip. Single authoritative `started_at_ms`.

```rust
impl SubagentManager {
    /// Execute a task that is ALREADY "running" (status + started_at_ms set
    /// by the caller). Builds context → executes → persists results.
    /// Called by `delegate()` (synchronous, after inserting as 'running')
    /// and the background dispatcher (after pick_oldest_queued_task which
    /// does the atomic CAS flip).
    ///
    /// **Round 3 Finding 2 fix:** this method does NOT call
    /// `subagent_set_task_status("running")` — the caller sets the status
    /// before calling (via insert-as-running or pick's atomic flip).
    ///
    /// Reads ALL execution params from the task row (Finding 1 fix):
    ///   - task.prompt / task.prompt_artifact_ref
    ///   - task.profile
    ///   - task.timeout_seconds (new column)
    ///   - task.model_override (new column)
    async fn execute_task(&self, task: &SubagentTaskRow, subagent: &LogicalSubagent) -> Result<DelegateResult> {
        let model = task.model_override.clone()
            .or_else(|| subagent.default_model.clone())
            .or_else(|| self.profile_model(&task.profile));
        let timeout_seconds = task.timeout_seconds.map(|t| t as u64);
        let (input, memory_row, tasks_since_last_compaction, profile_cfg) = {
            // ... existing context-building logic from delegate() steps 3 ...
            // Uses: task.subagent_id, subagent.name, subagent.repo_path (cwd),
            //       task.profile, model, task.prompt (or task.prompt_artifact_ref),
            //       timeout_seconds, task.source_harness, task.source_session_id
        };
        let out = self.executor.execute(&input).await.map_err(|e| {
            match e.downcast::<SubagentError>() {
                Ok(se) => se,
                Err(other) => {
                    warn!(event_code = "subagent.delegate.executor_failed", error = %other);
                    SubagentError::Store(other)
                }
            }
        })?;
        // ... existing persist-results logic from delegate() steps 4-5 ...
        // (sets status to "completed" or "failed" via subagent_set_task_status)
        Ok(DelegateResult { /* ... same fields as current delegate ... */ })
    }

    pub async fn delegate(&self, req: DelegateRequest) -> Result<DelegateResult> {
        // ... Task 3 Step 5 already implemented: check gate BEFORE insert,
        //     insert as 'queued' (paused) or 'running' (not paused) ...
        // Task 7 Step 3 change: add timeout_seconds + model_override to the
        // SubagentTaskRow insert (Task 7 Step 1 added the columns).
        // ...
        // Gate not paused — task is already 'running'. Call execute_task.
        self.execute_task(&task_row, &subagent).await
    }
}
```

- [ ] **Step 4: Add `spawn_task_dispatcher` with watch-based shutdown (Finding 3 fix)**

```rust
impl SubagentManager {
    /// Spawn the background task dispatcher (§8.3 step 2 "queue only").
    /// Polls for queued tasks every 200ms; when gate is not paused, picks
    /// the oldest queued task and executes it. Terminates when `shutdown`
    /// receiver sees `true` (sent by `BusytokSupervisor` on drop/shutdown).
    ///
    /// **Finding 3 fix:** `JoinHandle` drop = detach (NOT abort), so we use
    /// `tokio::sync::watch` for explicit shutdown signaling.
    pub fn spawn_task_dispatcher(
        self: &Arc<Self>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(200));
            loop {
                tokio::select! {
                    _ = ticker.tick() => {},
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() {
                            info!(event_code = "subagent.dispatcher.shutdown", "task dispatcher shutting down");
                            return;
                        }
                    }
                }

                // Check gate — if paused, skip.
                if let Some(gate) = &manager.pressure_gate {
                    if gate.is_paused() {
                        continue;
                    }
                }

                // Pick oldest queued task (per-subagent FIFO, atomic pick + flip).
                let task = {
                    let db = manager.db.lock().expect("subagent db lock poisoned");
                    db.subagent_pick_oldest_queued_task()
                        .ok()
                        .flatten()
                };

                if let Some(task) = task {
                    // Resolve the subagent.
                    let subagent = {
                        let db = manager.db.lock().expect("subagent db lock poisoned");
                        db.subagent_get_logical(&task.subagent_id)
                            .ok()
                            .flatten()
                            .map(|row| /* convert to LogicalSubagent */)
                    };
                    let Some(subagent) = subagent else {
                        warn!(event_code = "subagent.queue.subagent_missing", task_id = %task.id);
                        let db = manager.db.lock().expect("subagent db lock poisoned");
                        let _ = db.subagent_set_task_status(&task.id, "failed", None, Some("subagent not found"));
                        continue;
                    };
                    info!(event_code = "subagent.queue.execute", task_id = %task.id, "dispatcher executing queued task");
                    if let Err(e) = manager.execute_task(&task, &subagent).await {
                        warn!(event_code = "subagent.queue.execute_failed", task_id = %task.id, error = %e);
                        let db = manager.db.lock().expect("subagent db lock poisoned");
                        let _ = db.subagent_set_task_status(&task.id, "failed", None, Some(&e.to_string()));
                    }
                }
            }
        })
    }
}
```

- [ ] **Step 5: Wire dispatcher + shutdown into `BusytokSupervisor` (Finding 3 fix)**

In `crates/busytok-runtime/src/supervisor.rs`, in `assemble_with_sidecar` (after `SubagentManager` is constructed), create the shutdown channel + spawn the dispatcher:

```rust
// Create shutdown channel for the task dispatcher (Finding 3 fix).
let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
let dispatcher_handle = subagent_manager.spawn_task_dispatcher(shutdown_rx);
```

Add fields to `BusytokSupervisor`:
```rust
task_dispatcher: Option<tokio::task::JoinHandle<()>>,
dispatcher_shutdown: Option<tokio::sync::watch::Sender<bool>>,
```

Store both in `Self { ... task_dispatcher: Some(dispatcher_handle), dispatcher_shutdown: Some(shutdown_tx), ... }`.

**Shutdown path:** In `shutdown_writer()` (or add a dedicated `shutdown_dispatcher()` method), send `true` on the shutdown channel and await the dispatcher handle:

```rust
// In shutdown_writer() or a new shutdown_dispatcher() method:
if let Some(tx) = self.dispatcher_shutdown.take() {
    let _ = tx.send(true); // signal dispatcher to exit
}
if let Some(handle) = self.task_dispatcher.take() {
    let _ = handle.await; // wait for dispatcher to actually exit
}
```

**Drop safety:** Implement `Drop` for `BusytokSupervisor` that sends `true` on the shutdown channel if not already sent. This ensures the dispatcher exits even if `shutdown_writer()` is not called:

```rust
impl Drop for BusytokSupervisor {
    fn drop(&mut self) {
        if let Some(tx) = self.dispatcher_shutdown.take() {
            let _ = tx.send(true);
        }
        // Note: we can't .await in Drop, so the handle may still be running
        // for a brief moment. The shutdown signal guarantees it will exit
        // on the next select! iteration (within 200ms).
    }
}
```

Note: the `JoinHandle` is NOT stored for awaiting in `Drop` (can't `.await` in `Drop`). The shutdown signal guarantees termination within one poll cycle (200ms). Tests that need deterministic shutdown should call `shutdown_writer()` which awaits the handle.

- [ ] **Step 6: Write the failing tests**

Create `crates/busytok-subagent/tests/manager_queue.rs`:

```rust
use std::sync::{Arc, Mutex};
use busytok_subagent::{SubagentManager, SubagentError};
use busytok_subagent::mock_executor::{ExecutorInput, ExecutorOutput, TaskExecutor};
use busytok_subagent::pressure::{PressureAction, PressureGate};
use busytok_subagent::models::{DelegateRequest, TaskStatus};
use async_trait::async_trait;

// Minimal mock executor that records inputs.
struct RecordingExecutor;
#[async_trait]
impl TaskExecutor for RecordingExecutor {
    async fn execute(&self, _input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        Ok(ExecutorOutput { summary: "done".into(), usage: Default::default() })
    }
}

#[tokio::test]
async fn delegate_returns_queued_when_gate_paused() {
    let db = Arc::new(Mutex::new(busytok_store::Database::open_in_memory().unwrap()));
    let settings = busytok_config::SubagentSettings::default();
    let executor = Arc::new(RecordingExecutor) as Arc<dyn TaskExecutor>;
    let gate = Arc::new(PressureGate::new());
    gate.set_action(PressureAction::PauseNewTasks);
    let manager = SubagentManager::with_pressure_gate(
        Arc::clone(&db), settings, "mock", executor, Some(Arc::clone(&gate)),
    );
    let req = DelegateRequest {
        subagent_name: "test".into(),
        subagent_id: None,
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        intent: None,
        prompt: "hello".into(),
        timeout_seconds: Some(5),
        model_override: None,
        source_harness: None,
        source_session_id: None,
    };
    let result = manager.delegate(req).await.unwrap();
    assert_eq!(result.status, TaskStatus::Queued, "delegate must return Queued when gate is paused");
    assert!(result.summary.is_none(), "queued task has no summary yet");

    // Verify task row is in "queued" status in DB.
    let db = db.lock().unwrap();
    let tasks = db.subagent_list_tasks(&result.subagent_id, 10).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, "queued");
}

#[tokio::test]
async fn dispatcher_executes_queued_task_when_gate_clears() {
    let db = Arc::new(Mutex::new(busytok_store::Database::open_in_memory().unwrap()));
    let settings = busytok_config::SubagentSettings::default();
    let executor = Arc::new(RecordingExecutor) as Arc<dyn TaskExecutor>;
    let gate = Arc::new(PressureGate::new());
    gate.set_action(PressureAction::PauseNewTasks);
    let manager = Arc::new(SubagentManager::with_pressure_gate(
        Arc::clone(&db), settings, "mock", executor, Some(Arc::clone(&gate)),
    ));

    // Round 3 Finding 4 fix: spawn_task_dispatcher takes a watch::Receiver<bool>
    // for shutdown signaling (JoinHandle drop = detach, NOT abort).
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = manager.spawn_task_dispatcher(shutdown_rx);

    // Queue a task.
    let req = DelegateRequest {
        subagent_name: "test".into(),
        subagent_id: None,
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        intent: None,
        prompt: "hello".into(),
        timeout_seconds: Some(5),
        model_override: None,
        source_harness: None,
        source_session_id: None,
    };
    let result = manager.delegate(req).await.unwrap();
    assert_eq!(result.status, TaskStatus::Queued);

    // Clear the gate — dispatcher should pick up + execute.
    gate.set_action(PressureAction::Resume);

    // Poll for up to 5s for the task to complete.
    let mut completed = false;
    for _ in 0..50 {
        let db = db.lock().unwrap();
        let tasks = db.subagent_list_tasks(&result.subagent_id, 10).unwrap();
        if tasks.iter().any(|t| t.status == "completed") {
            completed = true;
            break;
        }
        drop(db);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(completed, "queued task must be executed after gate clears");

    // Deterministic shutdown: send true + await handle.
    let _ = shutdown_tx.send(true);
    let _ = handle.await;
}
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p busytok-subagent --test manager_queue`
Expected: PASS.

- [ ] **Step 8: Run clippy + fmt**

Run: `cargo clippy -p busytok-subagent --tests -- -D warnings && cargo fmt --all -- --check`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(queue): background task dispatcher for §8.3 'queue only'"
```

---

## Task 6: Final coverage + acceptance verification

**Files:**
- Modify: none (test-only verification)

- [ ] **Step 1: Run the full test suite**

Run: `cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 2: Run coverage**

Run: `cargo llvm-cov --workspace --html --output-dir coverage/`
Expected: workspace ≥ 82%, `busytok-subagent` ≥ 90%.

- [ ] **Step 3: Verify spec §12.2 acceptance**

The existing `sidecar_e2e_idle_rss_does_not_leak_after_delegate_shutdown` test covers idle RSS. Verify it still passes.

- [ ] **Step 4: Verify spec §8.3 acceptance (new)**

The new pressure-response tests from Task 4 verify the 5-step chain. Confirm:
- Force-kill on hard-limit exceeded.
- Pause new tasks on pressure (via `PressureGate`).
- Graceful restart on soft-limit exceeded.
- LRU hibernate on pressure.
- Completed task state never lost (existing crash-recovery test covers this).

- [ ] **Step 5: Verify spec §8.1 acceptance (new)**

The updated `ResourceSample` now includes `queued_task_count` + `running_task_count`. Confirm via the resource monitor tests.

- [ ] **Step 6: Final clippy + fmt + grep for stale comments**

Run: `cargo clippy --workspace --tests -- -D warnings && cargo fmt --all -- --check && grep -rn "Plan 6\|deferred to Plan 6\|Plan 6 will" crates/ --include="*.rs"`
Expected: clean; no stale Plan 6 comments.

- [ ] **Step 7: Commit (if any test adjustments needed)**

```bash
git add -A
git commit -m "test(plan6): final coverage + acceptance verification"
```

---

## Self-Review Notes

**Spec coverage:**
- §8.3 5-step chain → Task 4 (PressureResponder).
- §8.1 queued/running count → Task 1.
- §7.1 6 doctor checks → Task 5.
- §3.2 event enum → no new events added (recovery stays tracing-only, per plan).
- §12.2 idle RSS → existing test, verified in Task 6.

**Escalation mapping (verified against real code):**
- `exceeds_hard_limit` → `LimitExceeded` state → `ForceKill` (§8.3 step 5).
- `exceeds_soft_limit` (not hard) → folded into `Pressure` state → `GracefulRestart` (§8.3 steps 3-4). Checked as a separate predicate in `maybe_sample_resources`, NOT via state transition.
- `under_pressure` (system memory low) → `Pressure` state → `PauseNewTasks` (§8.3 steps 1-2, with `HibernateLru` folded in).
- Recovery → `Resume`.

**Ownership / Arc cycle avoidance (verified — no cycles):**
- `PressureResponder` holds `Weak<PiSidecarSupervisor>` + `Weak<SidecarTaskExecutor>` (BOTH weak) + `Arc<PressureGate>`.
- `PiSidecarSupervisor` holds `Weak<PressureResponder>` (via `Mutex<Option<Weak<...>>>`) — set via `set_pressure_responder` after construction. Upgraded by `pressure_responder()` accessor.
- `BusytokSupervisor` is the STRONG owner of: `sidecar_supervisor: Option<Arc<PiSidecarSupervisor>>`, `sidecar_executor: Option<Arc<SidecarTaskExecutor>>` (NEW concrete Arc), `pressure_responder: Option<Arc<PressureResponder>>`, `task_dispatcher: Option<JoinHandle<()>>`.
- `SubagentManager` holds `Arc<dyn TaskExecutor>` (shares the same allocation as `BusytokSupervisor.sidecar_executor`).
- Cycle analysis: supervisor → responder(weak→supervisor, weak→executor) → no strong path back. Executor → supervisor → responder(weak) → no strong path back. All Arcs reach 0 when `BusytokSupervisor` + `SubagentManager` drop.
- `construct_sidecar` returns 5-tuple including `Option<Arc<SidecarTaskExecutor>>` (concrete) so `PressureResponder::new` can call `Arc::downgrade` on it.

**§8.3 "queue only" semantics (Finding 1 fix):**
- `delegate()` does NOT reject with `Err(Paused)` — it accepts the task (inserts as "queued") and returns `DelegateResult { status: Queued }` immediately when gate is paused.
- Background `TaskDispatcher` (Task 7) polls every 200ms; when gate clears, picks oldest queued task (atomic SQL pick + flip to "running") and calls refactored `execute_task()`.
- `execute_task()` is extracted from `delegate()` — same context-build → execute → persist logic, reused by both synchronous and async paths.

**Round 2 — execute_task consumes SubagentTaskRow directly (Finding 1 fix):**
- `subagent_tasks` schema adds `timeout_seconds INTEGER` + `model_override TEXT` columns (Task 7 Step 1). `SubagentTaskRow` gains these fields.
- `execute_task(&self, task: &SubagentTaskRow, subagent: &LogicalSubagent)` reads ALL execution params from the task row directly — NO `DelegateRequest` reconstruction, NO data loss.
- `prompt_artifact_ref`, `timeout_seconds`, `model_override` are preserved end-to-end: caller → task row → dispatcher → executor. Spec §4.3/§4.4 contract is honored.
- `delegate()` inserts the full `DelegateRequest` (including `timeout_seconds` + `model_override`) into the task row at accept time. The row is the single source of truth.
- The `Paused` error variant is retained for the dispatcher's re-pause edge case (gate re-pauses between pick and execute), NOT for `delegate()` which never returns `Err(Paused)`.

**Round 2 — per-subagent FIFO guard (Finding 2 fix):**
- `pick_oldest_queued_task` SQL: `WHERE status = 'queued' AND subagent_id NOT IN (SELECT subagent_id FROM subagent_tasks WHERE status = 'running') ORDER BY created_at_ms LIMIT 1`.
- This enforces spec §6.4 line 737: same logical subagent tasks are serialized (FIFO per subagent). Different subagents can run concurrently.
- The guard is in a single SQL statement (atomic pick + flip to "running") — no separate lock needed.

**Round 2 — watch-based dispatcher shutdown (Finding 3 fix):**
- Tokio `JoinHandle` drop = detach (NOT abort). The plan's original claim was wrong.
- `BusytokSupervisor` holds `shutdown_tx: tokio::sync::watch::Sender<bool>` + `dispatcher_handle: Option<tokio::task::JoinHandle<()>>`.
- Dispatcher loop uses `tokio::select!` on ticker + `shutdown_rx.changed()`. When shutdown signal received, exits cleanly.
- `BusytokSupervisor::shutdown_writer()` sends `true` on shutdown channel + awaits `dispatcher_handle` (deterministic shutdown for tests).
- `Drop` impl for `BusytokSupervisor` sends `true` on shutdown channel (can't `.await` in `Drop` — worst case 200ms before dispatcher exits).

**ForceKill recovery (Finding 2 fix):**
- `ForceKill` arm clears gate to `Resume` after kill — next `delegate()` calls `ensure_started()` which lazy-spawns a fresh sidecar. No deadlock.
- `GracefulRestart` arm also clears gate to `Resume` on success — same reasoning.

**In-flight deduplication (Finding 3 fix):**
- `PressureResponder.respond()` uses `tokio::sync::Mutex::try_lock()` — if another `respond()` is running, the caller skips (not waits). Prevents concurrent `GracefulRestart`/`ForceKill` from racing.
- The `GracefulRestart` failure path inlines the `ForceKill` escalation (runs `force_kill()` directly, no recursive `self.respond(ForceKill)` call). This avoids `Box::pin` for async recursion; the in-flight `try_lock` guard is never re-acquired because `respond()` is not re-entered.

**Doctor checks (Finding 4 + 5 fixes):**
- `bundle_manifest_readable` checks `manifest.json` (spec §5.1 line 549), NOT `pi-sidecar.bundle.js`. Verifies file exists + readable + valid JSON via `serde_json::from_str`.
- `protocol_version` does a short-lived probe: if sidecar not running, `ensure_started()` (spawns + verifies protocol via `adapter.initialize`), then `shutdown_internal()` to clean up. If sidecar already running, protocol was verified during init → "ok". If `pi_sidecar.enabled = false` (no supervisor), returns "warning".

**Round 2 — stale assertion fixes (Finding 4 fix):**
- Task 3 test renamed `delegate_returns_paused_error_when_pressure_gate_is_set` → `delegate_returns_queued_when_pressure_gate_is_paused`; asserts `result.status == TaskStatus::Queued` (NOT `err.code() == "subagent.paused"`).
- Task 4 force-kill e2e asserts `!gate.is_paused()` after force-kill (gate cleared to Resume), NOT `gate.is_paused()`.
- Task 5 protocol_version test split into two: `doctor_protocol_version_check_is_warning_when_pi_sidecar_disabled` (enabled=false → "warning") + `doctor_protocol_version_check_probes_via_short_lived_sidecar_when_enabled` (enabled=true, no bundle → "error" via short-lived probe).
- Task 2 `force_kill_sets_paused` test is CORRECT at the gate level — `set_action(ForceKill)` does set `is_paused() = true`. The responder clears the gate afterward (tested at the e2e level in Task 4).

**Round 3 — atomic pick+flip (Finding 1 fix):**
- `pick_oldest_queued_task` is now truly atomic: `BEGIN IMMEDIATE` → SELECT id → CAS UPDATE (`WHERE id = ? AND status = 'queued'`) → SELECT full row → COMMIT. All inside one RAII `Transaction`.
- If two dispatchers race on the same task, the second UPDATE affects 0 rows → returns `None`. No double-consumption.
- The previous design (SELECT then UPDATE without transaction/CAS) was not atomic despite the plan claiming otherwise. Fixed.
- Uses `conn.transaction_with_behavior(TransactionBehavior::Immediate)?` per the project's SQLite lesson learned (RAII Transaction, not `execute_batch` with manual BEGIN/ROLLBACK).

**Round 3 — no double-flip (Finding 2 fix):**
- `execute_task()` no longer calls `subagent_set_task_status("running")`. It assumes the task is already `"running"` with `started_at_ms` set.
- `delegate()` synchronous path (Round 4 fix): gate check BEFORE insert → insert directly as `'running'` (with `started_at_ms = now`) → `execute_task()`. No flip needed.
- Dispatcher path: `pick_oldest_queued_task()` does the atomic flip inside its transaction → `execute_task()`.
- Single authoritative `started_at_ms` — no double-write. Tests can assert the exact transition timestamp.
- The stale note claiming "dispatcher path doesn't call `subagent_set_task_status` for the initial flip — only `execute_task` does" was wrong (contradicted the code). Removed.

**Round 4 — race-free insert (Finding 1 fix):**
- Previous flow (insert as `'queued'` → check gate → `flip_task_to_running()`) had a race: between insert and flip, the dispatcher could pick the task. `flip_task_to_running()` returned `bool` but `delegate()` only did `.map_err()?` (ignored the `false` race-lost case) → double execution.
- New flow: check gate BEFORE insert. If gate not paused, insert directly as `'running'` (`started_at_ms = now`). If gate paused, insert as `'queued'` and return `DelegateResult { status: Queued }`.
- Dispatcher only picks `'queued'` tasks (`pick_oldest_queued_task` WHERE clause), so it never sees synchronous-path tasks. Race eliminated entirely — not just detected.
- `flip_task_to_running` is removed entirely (no longer needed). `insert_task` already takes `row.status` from the `SubagentTaskRow` (verified at `subagent_queries.rs:331`), so inserting as `'running'` requires no store change.
- `started_at_ms` is set at insert time for the synchronous path — single authoritative timestamp, no second write.

**Round 3 — incremental migration (Finding 3 fix):**
- New file `0004_subagent_task_fields.sql` with two `ALTER TABLE subagent_tasks ADD COLUMN ...` statements.
- `0003_subagent.sql` is NOT modified — existing DBs that already ran `0003` need the ALTER TABLE to add the new columns.
- `SCHEMA_VERSION` bumped to 4. `baseline_single_migration` test assertion updated to `== 4`.
- The previous inconsistency (File Structure said `0004` new file, Task 7 body said modify `0003`) is resolved. All references now point to `0004_subagent_task_fields.sql`.

**Round 3 — test signature consistency (Finding 4 fix):**
- `spawn_task_dispatcher(self: &Arc<Self>, shutdown: tokio::sync::watch::Receiver<bool>) -> JoinHandle` — all callsites pass the receiver.
- `manager_queue.rs` test creates `let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);`, passes `shutdown_rx`, and does deterministic shutdown at the end: `shutdown_tx.send(true)` + `handle.await`.
- The stale `let _handle = manager.spawn_task_dispatcher();` (no args) that would cause a compile error is fixed.

**Known simplifications:**
- `task_queue_max` is NOT enforced (out of scope — spec §8.3 step 2 is pressure-pause, not general concurrency).
- `HibernateLru` is folded into `PauseNewTasks` (the `PauseNewTasks` arm also calls `evict_lru`) — matches spec intent (steps 1-2 happen together).
- `evict_session` takes 1 arg (`adapter_session_id`) — `evict_lru` extracts the LRU's `adapter_session_id` from the DB and delegates.
- `evict_lru` is NOT on the `TaskExecutor` trait (which only has `execute`) — `PressureResponder` holds concrete `Weak<SidecarTaskExecutor>` to call it.
- Dispatcher uses 200ms polling (not `tokio::sync::Notify`) — simpler, and 200ms latency is acceptable for pressure-recovery scenarios.

**Type consistency:**
- `PressureAction` defined in Task 2, used in Tasks 3-4.
- `PressureGate` constructed in `construct_sidecar` (Task 3), stored on `PiSidecarSupervisor` (Task 3) + `SubagentManager` (Task 3), consumed by `PressureResponder` (Task 4).
- `PressureResponder::new` takes `Weak<PiSidecarSupervisor>` + `Weak<SidecarTaskExecutor>` (Task 4) — constructed in `assemble_with_sidecar` (Task 3 Step 9) from `Arc::downgrade`.
- `PiSidecarSupervisor.pressure_responder()` accessor (Task 3 Step 7) upgrades the weak ref — called by `maybe_sample_resources` (Task 4 Step 7).
- `construct_sidecar` returns 5-tuple (Task 3 Step 8) — destructured by both `build_with_settings` and `build_with_sidecar_config`.
- `task_counts_by_status` added in Task 1, called from `SubagentManager::task_counts` (Task 3) and the supervision loop (Task 1).
- `execute_task(&self, task: &SubagentTaskRow, subagent: &LogicalSubagent)` refactored in Task 7 Step 3 — called by `delegate()` (after insert-as-running, Round 4 fix) and `spawn_task_dispatcher` (Task 7 Step 4, after `pick_oldest_queued_task` does the atomic flip). Reads ALL fields from `SubagentTaskRow` (no `DelegateRequest` reconstruction). Does NOT flip status (Round 3 Finding 2 fix).
- `pick_oldest_queued_task(conn)` added in Task 7 Step 2 — called by `spawn_task_dispatcher` (Task 7 Step 4). SQL includes per-subagent FIFO guard + atomic CAS flip inside `BEGIN IMMEDIATE` transaction (Round 3 Finding 1 fix).
- `subagent_set_task_status` (existing, `db.rs:1952`) — used by `execute_task` persist path (sets "completed"/"failed") and dispatcher error paths (sets "failed"). NOT used for the initial queued→running flip (Round 3 Finding 2 fix + Round 4 fix: delegate inserts directly as 'running').
- `LogicalSubagent.repo_path` (not `cwd`) — passed to `execute_task` as the `subagent` arg; `execute_task` builds `ExecutorInput.cwd` from it.
- `find_lru_hot_binding` returns all 11 `SubagentHarnessBindingRow` fields (Task 4) — verified against `repository.rs:564-576`.
- `SidecarHandle::prepare_hibernate_all` added in Task 4 — calls `session.prepare_hibernate` with `{"all": true}`.
- `SubagentError::Paused` + `code()` arm added in Task 3 — verified `code()` is exhaustive match. Used by dispatcher re-pause path, NOT by `delegate()`.
- `subagent_tasks` schema gains `timeout_seconds INTEGER` + `model_override TEXT` via `0004_subagent_task_fields.sql` (Task 7 Step 1, Round 3 Finding 3 fix) — `SubagentTaskRow` + `insert_task` + all SELECTs updated. `SCHEMA_VERSION` → 4.
- `spawn_task_dispatcher(self: &Arc<Self>, shutdown: tokio::sync::watch::Receiver<bool>) -> JoinHandle` — all callsites pass the receiver (Round 3 Finding 4 fix).
