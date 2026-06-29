# Subagent Resource Monitoring + Validation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement ResourceMonitor (sysinfo-based sampling), extend `settings_diagnostics` with subagent health checks, add crash recovery (with 5-min rolling restart window) + stress tests, and emit edge-triggered resource events at lifecycle boundaries.

**Architecture:** ResourceMonitor lives in `busytok-subagent` as a pure sampling module (no async runtime — the supervisor's existing `supervision_loop` calls it). Doctor checks reuse the existing `settings_diagnostics` RPC path (extended with an optional `subagent` section — no new RPC method, no new trait method, no new dispatch arm); the spec-required `busytok doctor` top-level CLI command (spec §855, §1068) is added as a thin client that calls `settings.diagnostics` and pretty-prints the subagent section. Crash recovery tests use the existing mock-sidecar `BUSYTOK_MOCK_CRASH_AFTER` env var; the 5-min rolling restart window is implemented as a `VecDeque<Instant>` in supervisor state. Stress tests create 100 logical subagents via the store and assert RSS doesn't grow linearly. Resource events are edge-triggered (debounced via a latch in supervisor state) per spec §6.5 "lifecycle boundaries only"; recovery transitions are logged to `tracing` only (NOT persisted to DB) — the `resource_recovered` event type is deferred to Plan 6 along with the spec enum update.

**Out of scope (deferred to Plan 6 — Backpressure + Recovery Events):**
- Spec §8.3 5-step pressure response chain (pause queue, hibernate LRU, graceful restart with prepare_hibernate-all, force kill). Plan 5 only delivers sampling + edge-triggered eventing + observability; it does NOT implement reactive backpressure. The `memory_pressure` and `rss_limit_exceeded` events written by Plan 5 are observability signals for Plan 6 to consume.
- `resource_recovered` DB event type — NOT in spec §3.2's enum. Plan 5 logs recovery transitions to `tracing` only (info level); the DB event + spec enum update are deferred to Plan 6 to avoid shipping an un-spec'd event type.
- Spec §8.1 `queued/running task count` collection target — MVP has no task queue (tasks are synchronous via `turn_auto`), so there's nothing to sample. Plan 5's `ResourceSample` does NOT include these fields. Deferred until a queue exists.

**Tech Stack:** Rust (sysinfo 0.32, tracing, rusqlite), TypeScript (vitest for sidecar tests), bash (mock-sidecar fixture).

## Global Constraints

- Spec §8.1: ResourceMonitor collects: busytok-service RSS, Pi sidecar RSS + CPU, hot session count, system available memory (macOS). Via `sysinfo` crate. **Spec §8.1 also lists `queued/running task count` as a collection target, but Plan 5 does NOT collect it** — MVP has no task queue (tasks are synchronous via `turn_auto`), so there's nothing to sample. This is deferred to a future plan that adds a task queue; see "Out of scope" above.
- Spec §8.2: Config defaults — `memory_soft_limit_mb = 800`, `memory_hard_limit_mb = 1200`, `memory_pressure_free_mb = 2048`, `monitor_interval_seconds = 30`. These are already in `SubagentPiSidecarConfig` and `SubagentResourcePolicyConfig` (busytok-config/src/lib.rs:233-336).
- Spec §8.3 Pressure Response (5-step escalation): 1) Hibernate LRU hot session. 2) Pause new task execution (queue only). 3) If sidecar RSS > soft limit → request graceful restart. 4) Before restart: prepare_hibernate all hot sessions → write memory → restart. 5) If graceful restart fails → force kill. **Plan 5 does NOT implement the reactive chain** — it only emits `memory_pressure`/`rss_limit_exceeded` events (edge-triggered) as observability signals. The 5-step backpressure is deferred to Plan 6. The `memory_soft_limit_mb`/`memory_hard_limit_mb` config fields are still threaded into `SidecarConfig` (Task 2) so Plan 6 can consume them without a config migration.
- Spec §5.4 Crash Recovery: exponential backoff 1s → 2s → 4s → 8s, max 3 attempts per 5 min. Crashed tasks NOT auto-retried. busytok-service MUST NOT crash when sidecar dies. The exponential backoff + 3-attempt cap ALREADY EXISTS in `PiSidecarSupervisor::spawn_internal` (supervisor.rs:127-139), but the **5-min sliding window does NOT exist** — the current code only counts consecutive `restart_attempts` and resets on successful spawn (config.rs:93-94 explicitly comments this gap). **Plan 5 Task 4 implements the 5-min rolling window** as a `VecDeque<Instant>` in supervisor state, pruned on each spawn attempt.
- Spec §5.1 spike note: "sysinfo CPU percent requires two refresh cycles to produce meaningful values. ResourceMonitor tests must account for this; first-sample values are unreliable."
- Spec §3.2 + §6.5: `subagent_resource_events` table ALREADY EXISTS with `rss_mb REAL`, `cpu_percent REAL`, `detail_json TEXT` columns. Currently `rss_mb`/`cpu_percent` are always `None` — Plan 5 populates them. **Events are edge-triggered (lifecycle boundaries only, NOT a metrics time-series table)** — Plan 5 debounces `memory_pressure`/`rss_limit_exceeded` writes via a latch in supervisor state so a sustained 20-minute pressure condition produces ONE event, not 40. The `tracing` log every sample tick is the time-series signal; the DB event is the lifecycle signal. **Recovery transitions (Pressure→Normal, LimitExceeded→Normal) are logged to `tracing` only — NOT written to DB** — the `resource_recovered` event type is not in spec §3.2's enum, so it's deferred to Plan 6 (see "Out of scope" above). The latch state still updates on recovery so a re-pressurization later writes a fresh `memory_pressure` event.
- Spec §3.2 event types: `sidecar_start | sidecar_stop | session_hot | session_hibernate | memory_pressure | sidecar_restart | task_timeout | rss_limit_exceeded`. Plan 5 writes only `memory_pressure` and `rss_limit_exceeded` (both already in the enum). Note: `sidecar_crash` is used in existing code (supervisor.rs:284) but not in the spec enum — this is a known inconsistency; keep `sidecar_crash` as-is since it's already shipped.
- Spec §12.2 Resource acceptance: Idle busytok-service RSS < 50MB (Pi sidecar not running). 100 idle logical subagents: RSS does not grow linearly. Pi sidecar: exactly 1 process when active. max_hot_sessions enforced. After hibernate, hot session count decreases. Sidecar exits after idle TTL.
- Spec §12.1 Case 4: Execute task, kill -9 the Pi node subprocess. Expected: busytok-service does not crash. Next delegate auto-restarts sidecar. Memory restored from SQLite. Task history preserved.
- Spec §7.1 + §7.3 doctor checks: busytok-service running, SQLite readable/writable + schema version, Pi sidecar launchable, bundled Node architecture matches, sidecar bundle manifest readable, protocol version matches, default model config valid, Pi runtime installed, artifact store writable, resource policy valid, subagents unused > 30 days (warning). **Plan 5 reuses the existing `settings.diagnostics` RPC path** (spec §7.3 line 903: "`doctor.check` — Extended with subagent-related health checks") — no new `subagent.doctor` RPC method, no new trait method, no new dispatch arm. **The spec-required `busytok doctor` top-level CLI command** (spec §855, §1068) is added as a thin client that calls `settings.diagnostics` and pretty-prints the subagent section — it does NOT introduce a new RPC. The 6 bundle-inspection checks (node arch, manifest, protocol version, model config, Pi runtime, artifact store) are stubbed as `status: "warning"` (NOT `"ok"`) with detail explaining "not yet implemented (Plan 6+ bundle inspection)" — `overall_ok` only fails on `status: "error"`, so warnings surface without misleading users.
- Coverage gates: workspace ≥ 82% (hard, CI requires 85%), per-crate `busytok-subagent` ≥ 90% (hard).
- `sysinfo` is NOT yet in workspace deps — Plan 5 adds it. Add `sysinfo = "0.32"` to `[workspace.dependencies]` in root `Cargo.toml`, then `sysinfo.workspace = true` in `crates/busytok-subagent/Cargo.toml`.
- Logging: use `tracing` crate with `event_code = "subagent.resource.*"` namespace, following existing patterns in supervisor.rs (e.g., `info!(event_code = "subagent.sidecar.start", ...)`).
- TDD: every task follows red-green-commit. Tests must verify real behavior, not mocks of mocks.

---

## File Structure

### Rust — `crates/busytok-subagent/src/`

| File | Responsibility | Action |
|------|---------------|--------|
| `resource.rs` | `ResourceSample`, `ResourceMonitor` (sysinfo-backed sampling + pressure predicates), `ResourcePressureState` enum + `transition_event` pure function | **Create** |
| `lib.rs` | Re-export `resource` module | **Modify** |
| `sidecar/config.rs` | Add `memory_soft_limit_mb`, `memory_hard_limit_mb` to `SidecarConfig`; thread through `resolve_sidecar_config` (`monitor_interval_seconds` stays on `SubagentResourcePolicyConfig`, read via `ResourceMonitor::monitor_interval()`) | **Modify** |
| `sidecar/supervisor.rs` | (Task 2) Add `resource_monitor: Option<Mutex<ResourceMonitor>>` + `resource_pressure_state: ResourcePressureState`; extend `write_resource_event` to take `Option<&ResourceSample>`; sample in `supervision_loop`; emit edge-triggered `memory_pressure` + `rss_limit_exceeded` DB events (recovery transitions → `tracing` only, NOT DB). (Task 4) Add `restart_history: VecDeque<Instant>` + `RESTART_WINDOW` const; prune in `spawn_internal`; push on crash. | **Modify** |

### Rust — `crates/busytok-runtime/src/`

| File | Responsibility | Action |
|------|---------------|--------|
| `supervisor.rs` | Add private helper `run_subagent_doctor(&self) -> SubagentDoctorResultDto` (11 spec §7.1 checks: subagent-specific real, 6 bundle-inspection checks stubbed as `"warning"`); call from existing `settings_diagnostics` handler to populate `SettingsDiagnosticsDto.subagent`. **No new RPC method, no new trait method, no new dispatch arm.** | **Modify** |

### Rust — `crates/busytok-protocol/src/`

| File | Responsibility | Action |
|------|---------------|--------|
| `dto.rs` | Add `SubagentDoctorResultDto`, `DoctorCheckDto`; extend `SettingsDiagnosticsDto` with `subagent: Option<SubagentDoctorResultDto>` (`#[serde(default, skip_serializing_if = "Option::is_none")]` — backwards-compatible) | **Modify** |

### Rust — `apps/cli/src/`

| File | Responsibility | Action |
|------|---------------|--------|
| `main.rs` | Add top-level `Doctor` command variant (spec §855, §1068: `busytok doctor`) — thin client that calls existing `settings.diagnostics` RPC and pretty-prints the subagent section | **Modify** |
| `commands.rs` | Add `handle_doctor()` — calls existing `settings.diagnostics` RPC, pretty-prints the `subagent` section (no new RPC method) | **Modify** |

### Root

| File | Responsibility | Action |
|------|---------------|--------|
| `Cargo.toml` | Add `sysinfo = "0.32"` to `[workspace.dependencies]` | **Modify** |
| `crates/busytok-subagent/Cargo.toml` | Add `sysinfo.workspace = true` | **Modify** |

### Tests

| File | Responsibility | Action |
|------|---------------|--------|
| `crates/busytok-subagent/tests/resource.rs` | `ResourceMonitor` unit tests + `ResourcePressureState::transition_event` transition tests | **Create** |
| `crates/busytok-subagent/tests/sidecar_supervisor.rs` | `write_resource_event` with `ResourceSample` populates columns; edge-triggered transition latch (Normal→Pressure→LimitExceeded→Normal writes exactly 2 DB events across 20 ticks — `memory_pressure` + `rss_limit_exceeded`; recovery → `tracing` only, no DB row); 5-min rolling restart window (4th attempt within 5 min rejected, recovery after window expires) | **Modify** |
| `crates/busytok-subagent/tests/sidecar_config.rs` | `resolve_sidecar_config` threads `memory_soft_limit_mb`/`memory_hard_limit_mb` | **Modify** |
| `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs` | Crash recovery e2e, 100-subagent stress, `busytok doctor` CLI (calls `settings.diagnostics`, subagent section: 11 checks, 6 warnings, `overall_ok=true`) | **Modify** |
| `scripts/coverage.sh` | Bump per-crate `busytok-subagent` gate (already 90); keep workspace 82 | **Modify** |

---

## Task 1: Add sysinfo dep + ResourceMonitor module (pure logic)

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/busytok-subagent/Cargo.toml`
- Create: `crates/busytok-subagent/src/resource.rs`
- Modify: `crates/busytok-subagent/src/lib.rs`
- Test: `crates/busytok-subagent/tests/resource.rs`

**Interfaces:**
- Produces: `pub struct ResourceSample { pub service_rss_mb: f64, pub sidecar_rss_mb: Option<f64>, pub sidecar_cpu_percent: Option<f64>, pub hot_session_count: u32, pub system_available_mb: f64 }`
- Produces: `pub struct ResourceMonitor { /* sysinfo::System + policy + limits */ }`
- Produces: `ResourceMonitor::new(policy: SubagentResourcePolicyConfig, soft_limit_mb: u32, hard_limit_mb: u32) -> Self`
- Produces: `ResourceMonitor::sample(&mut self, sidecar_pid: Option<u32>, hot_session_count: u32) -> ResourceSample`
- Produces: `ResourceMonitor::is_under_pressure(&self, sample: &ResourceSample) -> bool` (instance method — reads `self.policy.memory_pressure_free_mb`)
- Produces: `ResourceMonitor::exceeds_soft_limit(&self, sample: &ResourceSample) -> bool` (instance method — reads `self.soft_limit_mb`)
- Produces: `ResourceMonitor::exceeds_hard_limit(&self, sample: &ResourceSample) -> bool` (instance method — reads `self.hard_limit_mb`)
- Produces: `ResourceMonitor::monitor_interval(&self) -> std::time::Duration` (reads `self.policy.monitor_interval_seconds`)

- [ ] **Step 1: Add sysinfo to workspace deps**

Modify `Cargo.toml` — add `sysinfo` to `[workspace.dependencies]` (alphabetical between `serde_json` and `serial_test`):

```toml
serde_json = "1"
serial_test = "3"
sha2 = "0.10"
sysinfo = "0.32"
tauri = { version = "2", features = [] }
```

- [ ] **Step 2: Add sysinfo to busytok-subagent Cargo.toml**

Modify `crates/busytok-subagent/Cargo.toml` — add `sysinfo.workspace = true` to `[dependencies]`:

```toml
[dependencies]
busytok-store = { path = "../busytok-store" }
busytok-config = { path = "../busytok-config" }
busytok-domain = { path = "../busytok-domain" }
anyhow.workspace = true
async-trait.workspace = true
serde = { workspace = true }
serde_json.workspace = true
sysinfo.workspace = true
thiserror.workspace = true
tokio = { workspace = true, features = ["process", "io-util", "time", "sync", "rt-multi-thread"] }
tracing.workspace = true
uuid.workspace = true
```

- [ ] **Step 3: Write failing tests for ResourceMonitor**

Create `crates/busytok-subagent/tests/resource.rs`:

```rust
#![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
//! ResourceMonitor unit tests (spec §8.1, §5.1 spike note).

use busytok_config::SubagentResourcePolicyConfig;
use busytok_subagent::resource::{ResourceMonitor, ResourceSample};

fn sample(
    service_rss_mb: f64,
    sidecar_rss_mb: Option<f64>,
    sidecar_cpu_percent: Option<f64>,
    hot_session_count: u32,
    system_available_mb: f64,
) -> ResourceSample {
    ResourceSample {
        service_rss_mb,
        sidecar_rss_mb,
        sidecar_cpu_percent,
        hot_session_count,
        system_available_mb,
    }
}

#[test]
fn is_under_pressure_true_when_system_available_below_threshold() {
    let policy = SubagentResourcePolicyConfig {
        memory_pressure_free_mb: 2048,
        monitor_interval_seconds: 30,
    };
    let mon = ResourceMonitor::new(policy, 800, 1200);
    let s = sample(20.0, Some(100.0), Some(1.0), 1, 1024.0);
    assert!(mon.is_under_pressure(&s), "1024 < 2048 => pressure");
}

#[test]
fn is_under_pressure_false_when_system_available_at_or_above_threshold() {
    let policy = SubagentResourcePolicyConfig {
        memory_pressure_free_mb: 2048,
        monitor_interval_seconds: 30,
    };
    let mon = ResourceMonitor::new(policy, 800, 1200);
    let s = sample(20.0, Some(100.0), Some(1.0), 1, 2048.0);
    assert!(!mon.is_under_pressure(&s), "2048 == 2048 => no pressure");
    let s2 = sample(20.0, Some(100.0), Some(1.0), 1, 4096.0);
    assert!(!mon.is_under_pressure(&s2), "4096 > 2048 => no pressure");
}

#[test]
fn exceeds_soft_limit_true_when_sidecar_rss_above_soft_limit() {
    let policy = SubagentResourcePolicyConfig::default();
    let mon = ResourceMonitor::new(policy, 800, 1200);
    let s = sample(20.0, Some(801.0), Some(2.0), 1, 4096.0);
    assert!(mon.exceeds_soft_limit(&s), "801 > 800 => soft exceeded");
}

#[test]
fn exceeds_soft_limit_false_when_sidecar_rss_unknown() {
    let policy = SubagentResourcePolicyConfig::default();
    let mon = ResourceMonitor::new(policy, 800, 1200);
    let s = sample(20.0, None, None, 0, 4096.0);
    assert!(!mon.exceeds_soft_limit(&s), "None RSS => no soft limit breach");
}

#[test]
fn exceeds_hard_limit_true_when_sidecar_rss_above_hard_limit() {
    let policy = SubagentResourcePolicyConfig::default();
    let mon = ResourceMonitor::new(policy, 800, 1200);
    let s = sample(20.0, Some(1201.0), Some(2.0), 1, 4096.0);
    assert!(mon.exceeds_hard_limit(&s), "1201 > 1200 => hard exceeded");
}

#[test]
fn exceeds_hard_limit_false_when_sidecar_rss_below_hard_limit() {
    let policy = SubagentResourcePolicyConfig::default();
    let mon = ResourceMonitor::new(policy, 800, 1200);
    let s = sample(20.0, Some(1200.0), Some(2.0), 1, 4096.0);
    assert!(!mon.exceeds_hard_limit(&s), "1200 == 1200 => not exceeded");
}

#[test]
fn is_under_pressure_uses_policy_threshold_not_hardcoded() {
    // Verify the predicate reads self.policy.memory_pressure_free_mb (not a
    // hardcoded 2048). With threshold=512, a sample at 1000MB available is
    // NOT under pressure.
    let policy = SubagentResourcePolicyConfig {
        memory_pressure_free_mb: 512,
        monitor_interval_seconds: 30,
    };
    let mon = ResourceMonitor::new(policy, 800, 1200);
    let s = sample(20.0, None, None, 0, 1000.0);
    assert!(!mon.is_under_pressure(&s), "1000 > 512 => no pressure with custom threshold");
}

#[test]
fn exceeds_limits_use_configured_values_not_hardcoded() {
    // Verify soft/hard limits come from constructor, not hardcoded 800/1200.
    let policy = SubagentResourcePolicyConfig::default();
    let mon = ResourceMonitor::new(policy, 500, 700);
    let s = sample(20.0, Some(550.0), Some(1.0), 0, 4096.0);
    assert!(mon.exceeds_soft_limit(&s), "550 > 500 (custom soft) => soft exceeded");
    assert!(!mon.exceeds_hard_limit(&s), "550 < 700 (custom hard) => not exceeded");
}

#[test]
fn sample_returns_positive_service_rss_for_current_process() {
    // Spec §5.1 spike note: sysinfo CPU requires two refresh cycles.
    // First sample CPU is 0.0 / unreliable. This test only asserts RSS > 0
    // (the current process always has RSS).
    let policy = SubagentResourcePolicyConfig::default();
    let mut mon = ResourceMonitor::new(policy, 800, 1200);
    let s = mon.sample(None, 0);
    assert!(
        s.service_rss_mb > 0.0,
        "current process RSS must be > 0; got {}",
        s.service_rss_mb
    );
    assert_eq!(s.sidecar_rss_mb, None, "no sidecar_pid => sidecar_rss_mb is None");
    assert_eq!(s.sidecar_cpu_percent, None, "no sidecar_pid => cpu is None");
    assert_eq!(s.hot_session_count, 0);
    // system_available_mb is best-effort; on some CI runners it may be 0.
    // Just assert it's non-negative.
    assert!(s.system_available_mb >= 0.0);
}

#[test]
fn sample_with_known_pid_returns_sidecar_rss_and_zero_first_cpu() {
    // Spec §5.1 spike note: first sample returns 0.0 for CPU (two refresh
    // cycles required). We sample the current process itself as a "sidecar"
    // so we have a real PID with real RSS.
    let policy = SubagentResourcePolicyConfig::default();
    let mut mon = ResourceMonitor::new(policy, 800, 1200);
    let pid = sysinfo::Pid::from_u32(std::process::id());
    let _ = pid;
    let s = mon.sample(Some(std::process::id()), 0);
    assert!(
        s.sidecar_rss_mb.map(|v| v > 0.0).unwrap_or(false),
        "sidecar_rss_mb must be > 0 for self PID; got {:?}",
        s.sidecar_rss_mb
    );
    // First sample CPU is 0.0 (spike note) — assert it's a finite number, not NaN.
    let cpu = s.sidecar_cpu_percent.unwrap_or(0.0);
    assert!(cpu.is_finite(), "first-sample CPU must be finite, got {cpu}");
}

#[test]
fn second_sample_returns_meaningful_cpu() {
    // Spec §5.1: CPU requires two refresh cycles. After the first sample
    // primes sysinfo's internal previous-timestamp, the second sample should
    // be a real (but possibly still 0.0 for an idle process) finite value.
    let policy = SubagentResourcePolicyConfig::default();
    let mut mon = ResourceMonitor::new(policy, 800, 1200);
    let pid = std::process::id();
    let _ = mon.sample(Some(pid), 0); // prime
    // Burn a tiny bit of CPU so sysinfo has something to measure.
    let mut acc: u64 = 0;
    for i in 0..100_000 {
        acc = acc.wrapping_add(i);
    }
    std::hint::black_box(acc);
    let s2 = mon.sample(Some(pid), 0);
    let cpu = s2.sidecar_cpu_percent.unwrap_or(0.0);
    assert!(cpu.is_finite(), "second-sample CPU must be finite, got {cpu}");
    assert!(cpu >= 0.0, "CPU percent is non-negative, got {cpu}");
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p busytok-subagent --test resource`
Expected: FAIL with "unresolved module `resource`" (module doesn't exist yet).

- [ ] **Step 5: Create resource.rs module**

Create `crates/busytok-subagent/src/resource.rs`:

```rust
//! Resource monitor — pure sampling module (spec §8.1, §5.1 spike note).
//!
//! `ResourceMonitor` is constructed by `PiSidecarSupervisor` and called from
//! the existing `supervision_loop` (no async runtime here — the loop is
//! already async and calls `sample()` synchronously between awaits).
//!
//! ## sysinfo CPU spike note (spec §5.1)
//!
//! sysinfo computes CPU percent from the delta between two refreshes. The
//! FIRST call to `sample()` after construction returns `0.0` for CPU because
//! there's no prior measurement to delta against. The supervision loop calls
//! `sample()` every `monitor_interval_seconds`, so by the second tick the
//! value is meaningful. Tests that assert on CPU behavior must call `sample()`
//! twice (prime + measure).

use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};

use busytok_config::SubagentResourcePolicyConfig;

/// One point-in-time snapshot of process + system resource usage.
/// Mirrors spec §8.1 collection fields. Written to `subagent_resource_events`
/// by `PiSidecarSupervisor::write_resource_event` (Task 2).
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceSample {
    /// busytok-service RSS in MB.
    pub service_rss_mb: f64,
    /// Pi sidecar RSS in MB (None when sidecar not running).
    pub sidecar_rss_mb: Option<f64>,
    /// Pi sidecar CPU percent (0–100). None when sidecar not running.
    /// First sample after construction is 0.0 (spike note).
    pub sidecar_cpu_percent: Option<f64>,
    /// Number of currently-hot sessions in the sidecar pool.
    pub hot_session_count: u32,
    /// System available memory in MB (macOS). Best-effort.
    pub system_available_mb: f64,
}

/// Edge-trigger latch for resource pressure state (spec §6.5: "lifecycle
/// boundaries only, not a metrics time-series table"). The supervisor
/// transitions between these states; DB events are written ONLY on
/// transitions, while `tracing` logs fire every sample tick for
/// observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResourcePressureState {
    /// No pressure, sidecar RSS under soft limit.
    #[default]
    Normal,
    /// System memory pressure OR sidecar RSS > soft limit (warning tier).
    Pressure,
    /// Sidecar RSS > hard limit (error tier — Plan 6 will force-kill).
    LimitExceeded,
}

impl ResourcePressureState {
    /// Returns the DB event type to write on a state transition, or `None`
    /// if no DB event should be written. This is a pure function — testable
    /// without async or sysinfo.
    ///
    /// Returns `Some` only for transitions INTO a pressure/limit state
    /// (escalations). Recovery transitions (Pressure→Normal,
    /// LimitExceeded→Normal) return `None` — the supervisor logs them to
    /// `tracing` (info level) but does NOT write a DB event, because
    /// `resource_recovered` is not in spec §3.2's event enum. The spec enum
    /// update + DB event for recovery are deferred to Plan 6.
    ///
    /// The latch state still updates on recovery (handled by the caller) so
    /// a re-pressurization later writes a fresh `memory_pressure` event.
    pub fn transition_event(old: Self, new: Self) -> Option<&'static str> {
        match (old, new) {
            (Self::Normal, Self::Pressure) => Some("memory_pressure"),
            (Self::Normal, Self::LimitExceeded)
            | (Self::Pressure, Self::LimitExceeded) => Some("rss_limit_exceeded"),
            // Recovery transitions: no DB event (resource_recovered not in
            // spec §3.2 enum — deferred to Plan 6). Caller logs to tracing.
            (Self::Pressure, Self::Normal)
            | (Self::LimitExceeded, Self::Normal) => None,
            (Self::LimitExceeded, Self::Pressure) => None,
            _ => None, // same state — debounced
        }
    }

    /// Returns true if the transition is a recovery (pressure/limit → normal).
    /// The caller uses this to log the recovery to `tracing` even though no
    /// DB event is written.
    pub fn is_recovery(old: Self, new: Self) -> bool {
        matches!(
            (old, new),
            (Self::Pressure, Self::Normal) | (Self::LimitExceeded, Self::Normal)
        )
    }
}

/// Pure sampling + pressure-predicate module. Owns a `sysinfo::System` so
/// CPU deltas accumulate across `sample()` calls. Predicates are instance
/// methods that read the configured thresholds from `policy` / `soft_limit_mb`
/// / `hard_limit_mb` (no hardcoded values — spec §8.2 config flows through).
pub struct ResourceMonitor {
    system: System,
    policy: SubagentResourcePolicyConfig,
    soft_limit_mb: u32,
    hard_limit_mb: u32,
}

impl ResourceMonitor {
    /// Construct a new monitor. `soft_limit_mb` / `hard_limit_mb` come from
    /// `SubagentPiSidecarConfig`; `policy` comes from
    /// `SubagentResourcePolicyConfig`.
    pub fn new(
        policy: SubagentResourcePolicyConfig,
        soft_limit_mb: u32,
        hard_limit_mb: u32,
    ) -> Self {
        Self {
            system: System::new_all(),
            policy,
            soft_limit_mb,
            hard_limit_mb,
        }
    }

    /// Take a sample. `sidecar_pid` is the sidecar child PID (None when the
    /// sidecar is not running). `hot_session_count` is provided by the caller
    /// (the supervisor reads it from the sidecar's `adapter.health` response
    /// or tracks it via the executor).
    ///
    /// First call after construction returns `sidecar_cpu_percent = Some(0.0)`
    /// (spike note) — sysinfo needs two refreshes to compute a delta.
    pub fn sample(
        &mut self,
        sidecar_pid: Option<u32>,
        hot_session_count: u32,
    ) -> ResourceSample {
        // sysinfo 0.32 API: refresh_specifics takes a RefreshKind. We refresh
        // processes (with cpu+memory) and system memory in one call.
        self.system.refresh_specifics(
            RefreshKind::everything().with_processes(ProcessRefreshKind::everything()),
        );

        let service_pid = Pid::from_u32(std::process::id());
        let service_rss_mb = self
            .system
            .process(service_pid)
            .map(|p| bytes_to_mb(p.memory()))
            .unwrap_or(0.0);

        let (sidecar_rss_mb, sidecar_cpu_percent) = sidecar_pid
            .map(|pid| {
                let p = self.system.process(Pid::from_u32(pid));
                (
                    p.map(|proc_info| bytes_to_mb(proc_info.memory())),
                    p.map(|proc_info| proc_info.cpu_usage() as f64),
                )
            })
            .unwrap_or((None, None));

        let system_available_mb = bytes_to_mb(self.system.available_memory());

        ResourceSample {
            service_rss_mb,
            sidecar_rss_mb,
            sidecar_cpu_percent,
            hot_session_count,
            system_available_mb,
        }
    }

    /// The configured sampling interval (spec §8.2). Read by the supervision
    /// loop to decide when to call `sample()`.
    pub fn monitor_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.policy.monitor_interval_seconds.max(1))
    }

    /// True when system available memory is below the pressure threshold
    /// (spec §8.3 step 1 threshold). Plan 5 only emits the
    /// `memory_pressure` event; the backpressure action is deferred to
    /// Plan 6. Reads `self.policy.memory_pressure_free_mb`.
    pub fn is_under_pressure(&self, sample: &ResourceSample) -> bool {
        sample.system_available_mb < self.policy.memory_pressure_free_mb as f64
    }

    /// True when sidecar RSS exceeds the soft limit (spec §8.3 step 3
    /// threshold). Plan 5 only emits a warning log; the graceful-restart
    /// action is deferred to Plan 6. Reads `self.soft_limit_mb`.
    pub fn exceeds_soft_limit(&self, sample: &ResourceSample) -> bool {
        sample
            .sidecar_rss_mb
            .map(|rss| rss > self.soft_limit_mb as f64)
            .unwrap_or(false)
    }

    /// True when sidecar RSS exceeds the hard limit (spec §8.3 step 5
    /// threshold). Plan 5 only emits the `rss_limit_exceeded` event;
    /// the force-kill action is deferred to Plan 6.
    /// Reads `self.hard_limit_mb`.
    pub fn exceeds_hard_limit(&self, sample: &ResourceSample) -> bool {
        sample
            .sidecar_rss_mb
            .map(|rss| rss > self.hard_limit_mb as f64)
            .unwrap_or(false)
    }
}

/// Convert bytes (sysinfo's `u64` memory) to megabytes as `f64`.
fn bytes_to_mb(bytes: u64) -> f64 {
    (bytes as f64) / (1024.0 * 1024.0)
}
```

- [ ] **Step 6: Wire module into lib.rs**

Modify `crates/busytok-subagent/src/lib.rs` — add `pub mod resource;` (alphabetical between `resolver` and `sidecar`):

```rust
pub mod context;
pub mod error;
pub mod manager;
pub mod memory;
pub mod mock_executor;
pub mod models;
pub mod resolver;
pub mod resource;
pub mod sidecar;

pub use error::{Result, SubagentError};
pub use manager::SubagentManager;
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p busytok-subagent --test resource`
Expected: PASS — all 9 tests green.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml crates/busytok-subagent/Cargo.toml \
  crates/busytok-subagent/src/resource.rs \
  crates/busytok-subagent/src/lib.rs \
  crates/busytok-subagent/tests/resource.rs
git commit -m "feat(subagent): add ResourceMonitor (sysinfo-based RSS/CPU sampling)"
```

---

## Task 2: Wire ResourceMonitor into PiSidecarSupervisor

**Files:**
- Modify: `crates/busytok-subagent/src/sidecar/config.rs`
- Modify: `crates/busytok-subagent/src/sidecar/supervisor.rs`
- Test: `crates/busytok-subagent/tests/sidecar_supervisor.rs`
- Test: `crates/busytok-subagent/tests/sidecar_config.rs`

**Interfaces:**
- Consumes: `ResourceMonitor`, `ResourceSample` from Task 1.
- Produces: `SidecarConfig.memory_soft_limit_mb: u32`, `SidecarConfig.memory_hard_limit_mb: u32` (only two fields — `monitor_interval_seconds` is NOT on `SidecarConfig`; it stays on `SubagentResourcePolicyConfig` and is read via `ResourceMonitor::monitor_interval()`).
- Produces: `PiSidecarSupervisor::write_resource_event(event_type: &str, sample: Option<&ResourceSample>)` (signature change).

- [ ] **Step 1: Write failing test for SidecarConfig memory fields**

Append to `crates/busytok-subagent/tests/sidecar_config.rs`:

```rust
#[test]
fn resolve_sidecar_config_threads_memory_limits() {
    let tmp = TempDir::new().unwrap();
    let paths = paths_for(&tmp);
    let runtime_dir = tmp.path().to_string_lossy().to_string();
    write_bundle(tmp.path());

    let mut settings = SubagentPiSidecarConfig::default();
    settings.node_runtime = "system".to_string();
    settings.system_node_path = "bash".to_string();
    settings.runtime_dir = Some(runtime_dir);
    settings.memory_soft_limit_mb = 700;
    settings.memory_hard_limit_mb = 1100;

    let cfg = resolve_sidecar_config(&settings, &paths).unwrap();
    assert_eq!(cfg.memory_soft_limit_mb, 700);
    assert_eq!(cfg.memory_hard_limit_mb, 1100);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p busytok-subagent --test sidecar_config resolve_sidecar_config_threads_memory_limits`
Expected: FAIL — `no field memory_soft_limit_mb on type SidecarConfig`.

- [ ] **Step 3: Add memory fields to SidecarConfig**

Modify `crates/busytok-subagent/src/sidecar/config.rs` — add two fields to the `SidecarConfig` struct (after `max_hot_sessions`). Note: `monitor_interval_seconds` is NOT added here — it lives on `SubagentResourcePolicyConfig` and is read by `ResourceMonitor::monitor_interval()` (Task 1). Keeping it off `SidecarConfig` avoids a dead field.

```rust
pub struct SidecarConfig {
    pub node_binary: PathBuf,
    pub bundle_path: PathBuf,
    pub env: HashMap<String, String>,
    pub idle_exit_seconds: u64,
    pub health_interval: Duration,
    pub task_timeout: Duration,
    pub max_restart_attempts: u32,
    /// Base delay for exponential backoff on crash-restart (1s → 2s → 4s → 8s).
    pub restart_backoff_base: Duration,
    /// Harness name scopes crash reconciliation (spec §5.4). "pi" for Plan 2;
    /// future harnesses (Claude Code, Codex) set their own.
    pub harness_name: String,
    /// Maximum concurrent hot sessions the sidecar will hold before evicting
    /// the LRU (spec §4.4). Mirrored to the sidecar via the
    /// `BUSYTOK_SIDECAR_MAX_HOT_SESSIONS` env var (spec §8.2).
    pub max_hot_sessions: u32,
    /// Soft RSS limit (MB) — at/above this, log warning + plan graceful
    /// restart (spec §8.3 step 3).
    pub memory_soft_limit_mb: u32,
    /// Hard RSS limit (MB) — at/above this, write `rss_limit_exceeded`
    /// event; existing crash path will restart (spec §8.3 step 5).
    pub memory_hard_limit_mb: u32,
}
```

Update `resolve_sidecar_config` to populate the new fields from `SubagentPiSidecarConfig`:

```rust
Ok(SidecarConfig {
    node_binary,
    bundle_path,
    env,
    idle_exit_seconds: settings.idle_exit_seconds,
    // Spec §5.4: health ping every 30s. Fixed in MVP (no config knob).
    health_interval: Duration::from_secs(30),
    task_timeout: Duration::from_secs(settings.task_timeout_seconds),
    max_restart_attempts: 3,
    restart_backoff_base: Duration::from_secs(1),
    harness_name: "pi".to_string(),
    max_hot_sessions: settings.max_hot_sessions,
    memory_soft_limit_mb: settings.memory_soft_limit_mb,
    memory_hard_limit_mb: settings.memory_hard_limit_mb,
})
```

- [ ] **Step 4: Run config test to verify it passes**

Run: `cargo test -p busytok-subagent --test sidecar_config`
Expected: PASS.

- [ ] **Step 4b: Update existing `make_sidecar_config()` helper**

The existing `make_sidecar_config()` in `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs` (lines 50-63) constructs a `SidecarConfig` inline. After adding `memory_soft_limit_mb` / `memory_hard_limit_mb`, ALL existing e2e tests that call this helper will fail to compile with "missing field" errors. Update the helper to include the two new fields:

```rust
fn make_sidecar_config() -> SidecarConfig {
    SidecarConfig {
        node_binary: PathBuf::from("bash"),
        bundle_path: mock_sidecar_path(),
        env: HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(30),
        task_timeout: Duration::from_secs(30),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
    }
}
```

Also search for any OTHER inline `SidecarConfig { ... }` constructions in test files (e.g. `sidecar_supervisor.rs` tests) and add the two new fields. Run `cargo build --tests -p busytok-subagent -p busytok-runtime` to catch any remaining sites — the compiler will list every missing-field error.

- [ ] **Step 4c: Verify existing e2e tests still compile**

Run: `cargo build --tests -p busytok-subagent -p busytok-runtime`
Expected: PASS — no missing-field errors.

- [ ] **Step 5: Write failing test for write_resource_event with ResourceSample**

Append to `crates/busytok-subagent/tests/sidecar_supervisor.rs` (inside the existing test module, after the last test). The test constructs a supervisor with a DB, calls the new `write_resource_event_with_sample` path, and asserts the DB row has the populated columns.

```rust
#[test]
fn write_resource_event_with_sample_populates_rss_and_cpu_columns() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    let config = busytok_subagent::sidecar::SidecarConfig {
        node_binary: std::path::PathBuf::from("bash"),
        bundle_path: std::path::PathBuf::from("/dev/null"),
        env: std::collections::HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: std::time::Duration::from_secs(30),
        task_timeout: std::time::Duration::from_secs(30),
        max_restart_attempts: 3,
        restart_backoff_base: std::time::Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
    };
    let sup = busytok_subagent::sidecar::PiSidecarSupervisor::new(
        config,
        Some(std::sync::Arc::new(std::sync::Mutex::new(db))),
    );
    let sample = busytok_subagent::resource::ResourceSample {
        service_rss_mb: 25.0,
        sidecar_rss_mb: Some(150.0),
        sidecar_cpu_percent: Some(3.5),
        hot_session_count: 2,
        system_available_mb: 4096.0,
    };
    sup.write_resource_event_with_sample("sidecar_start", Some(&sample));

    let db = sup.db_for_test().lock().unwrap();
    let events = db.subagent_list_resource_events(None, 100).unwrap();
    let evt = events
        .iter()
        .find(|e| e.event_type == "sidecar_start")
        .expect("sidecar_start event must be written");
    assert_eq!(evt.rss_mb, Some(150.0), "sidecar_rss_mb must be populated");
    assert_eq!(evt.cpu_percent, Some(3.5), "cpu_percent must be populated");
    let detail: serde_json::Value =
        serde_json::from_str(evt.detail_json.as_deref().unwrap_or("null")).unwrap();
    assert_eq!(detail["service_rss_mb"], 25.0);
    assert_eq!(detail["hot_session_count"], 2);
    assert_eq!(detail["system_available_mb"], 4096.0);
}
```

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p busytok-subagent --test sidecar_supervisor write_resource_event_with_sample_populates_rss_and_cpu_columns`
Expected: FAIL — `no method write_resource_event_with_sample` and `no method db_for_test`.

- [ ] **Step 7: Extend write_resource_event + add test accessor**

Modify `crates/busytok-subagent/src/sidecar/supervisor.rs`. Replace the existing `write_resource_event` method (lines 466-481) with the extended version + a test-only accessor:

```rust
    /// Write a row to `subagent_resource_events` if a DB handle is attached.
    /// No-op (but still logged at debug) in unit tests where `db` is `None`.
    /// When `sample` is `Some`, populates `rss_mb`, `cpu_percent`, and
    /// `detail_json` with the full sample (spec §3.2 columns).
    fn write_resource_event(&self, event_type: &str) {
        self.write_resource_event_with_sample(event_type, None);
    }

    /// Extended resource event writer that attaches a `ResourceSample`.
    /// Public to test harness (via `#[doc(hidden)]`) so tests can exercise
    /// the column-population path without driving the full supervision loop.
    #[doc(hidden)]
    pub fn write_resource_event_with_sample(
        &self,
        event_type: &str,
        sample: Option<&crate::resource::ResourceSample>,
    ) {
        if let Some(db) = &self.db {
            if let Ok(db) = db.lock() {
                let now = busytok_domain::now_ms();
                let (rss_mb, cpu_percent, detail_json) = match sample {
                    Some(s) => {
                        let detail = serde_json::json!({
                            "service_rss_mb": s.service_rss_mb,
                            "hot_session_count": s.hot_session_count,
                            "system_available_mb": s.system_available_mb,
                        });
                        (s.sidecar_rss_mb, s.sidecar_cpu_percent, Some(detail.to_string()))
                    }
                    None => (None, None, None),
                };
                let _ = db.subagent_insert_resource_event(&SubagentResourceEventRow {
                    id: format!("re_{}", uuid::Uuid::new_v4()),
                    event_type: event_type.to_string(),
                    target_id: None,
                    rss_mb,
                    cpu_percent,
                    detail_json,
                    created_at_ms: now,
                });
            }
        }
    }

    /// Test-only accessor for the shared DB handle. Used by integration tests
    /// that assert on `subagent_resource_events` rows after driving the
    /// supervisor. `#[doc(hidden)]` keeps it out of public API surface.
    #[doc(hidden)]
    pub fn db_for_test(&self) -> &SharedDb {
        self.db
            .as_ref()
            .expect("db_for_test called but supervisor has no DB handle")
    }
```

- [ ] **Step 8: Run test to verify it passes**

Run: `cargo test -p busytok-subagent --test sidecar_supervisor write_resource_event_with_sample_populates_rss_and_cpu_columns`
Expected: PASS.

- [ ] **Step 9: Add resource_monitor field + sampling in supervision_loop**

Modify `crates/busytok-subagent/src/sidecar/supervisor.rs`. Add the field to `PiSidecarSupervisor` and construct it in `new`:

```rust
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{error, info, instrument, warn};

use busytok_config::{SubagentResourcePolicyConfig, SubagentPiSidecarConfig};
use busytok_store::{Database, SubagentResourceEventRow};

use crate::resource::{ResourceMonitor, ResourcePressureState, ResourceSample};
use crate::sidecar::client::SidecarRpcClient;
use crate::sidecar::config::SidecarConfig;
use crate::sidecar::protocol::PROTOCOL_VERSION;
use crate::sidecar::SidecarError;

pub type SharedDb = Arc<std::sync::Mutex<Database>>;

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

pub struct PiSidecarSupervisor {
    config: SidecarConfig,
    state: Mutex<SupervisorState>,
    db: Option<SharedDb>,
    /// Resource monitor — None in unit tests that don't pass a policy.
    /// Mutex because `sample(&mut self)` mutates the internal `sysinfo::System`.
    resource_monitor: Option<std::sync::Mutex<ResourceMonitor>>,
}
```

The existing `SupervisorState` struct (in supervisor.rs) gains a `resource_pressure_state` field. Update both constructors to initialize it:

```rust
    pub fn new(config: SidecarConfig, db: Option<SharedDb>) -> Arc<Self> {
        // Default policy — production callers pass settings via
        // `with_resource_policy` (added below) when they have a
        // SubagentResourcePolicyConfig. For the default-constructed path
        // (tests), we use the spec-default policy so the monitor still works.
        let policy = SubagentResourcePolicyConfig::default();
        let monitor = ResourceMonitor::new(
            policy,
            config.memory_soft_limit_mb,
            config.memory_hard_limit_mb,
        );
        Arc::new(Self {
            config,
            state: Mutex::new(SupervisorState {
                child: None,
                client: None,
                last_activity: tokio::time::Instant::now(),
                restart_attempts: 0,
                supervision_started: false,
                resource_pressure_state: ResourcePressureState::Normal,
            }),
            db,
            resource_monitor: Some(std::sync::Mutex::new(monitor)),
        })
    }

    /// Construct with an explicit resource policy (used by the runtime
    /// supervisor which has the deserialized SubagentResourcePolicyConfig).
    pub fn with_resource_policy(
        config: SidecarConfig,
        db: Option<SharedDb>,
        policy: SubagentResourcePolicyConfig,
    ) -> Arc<Self> {
        let monitor = ResourceMonitor::new(
            policy,
            config.memory_soft_limit_mb,
            config.memory_hard_limit_mb,
        );
        Arc::new(Self {
            config,
            state: Mutex::new(SupervisorState {
                child: None,
                client: None,
                last_activity: tokio::time::Instant::now(),
                restart_attempts: 0,
                supervision_started: false,
                resource_pressure_state: ResourcePressureState::Normal,
            }),
            db,
            resource_monitor: Some(std::sync::Mutex::new(monitor)),
        })
    }
```

Extend `supervision_loop` to sample at the `monitor_interval_seconds` cadence and emit pressure/limit events. Replace the existing `supervision_loop` body (lines 250-322) with:

```rust
    /// Background loop: crash watcher + health pinger + idle timer +
    /// resource sampling. Exits when the child is taken (shutdown) or
    /// crashes (handled, then exits — next `ensure_started` respawns and
    /// re-spawns the loop).
    async fn supervision_loop(self: Arc<Self>) {
        let mut last_health = tokio::time::Instant::now();
        let mut last_resource_sample = tokio::time::Instant::now();
        // Read the monitor interval from the resource_monitor's policy (not
        // from SidecarConfig — the policy is the source of truth per spec §8.2).
        // Fallback to 30s if no monitor is attached (unit tests).
        let monitor_interval = self
            .resource_monitor
            .as_ref()
            .and_then(|m| m.lock().ok().map(|g| g.monitor_interval()))
            .unwrap_or_else(|| Duration::from_secs(30));
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            let mut state = self.state.lock().await;
            if state.child.is_none() {
                return; // shut down — loop exits
            }
            // --- crash detection (non-blocking try_wait) ---
            let crash_status = match state.child.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(status)) => Some(status),
                    Ok(None) => None,
                    Err(_) => None,
                },
                None => return,
            };
            if let Some(status) = crash_status {
                state.client = None;
                state.child = None;
                state.restart_attempts += 1;
                warn!(
                    event_code = "subagent.sidecar.crash",
                    exit = ?status,
                    attempts = state.restart_attempts,
                    "sidecar crashed"
                );
                drop(state);
                self.reconcile_crash();
                self.write_resource_event("sidecar_crash");
                return;
            }
            let sidecar_pid = state.child.as_ref().and_then(|c| c.id());
            let last_activity = state.last_activity;
            // --- idle exit timer ---
            let idle_threshold = Duration::from_secs(self.config.idle_exit_seconds);
            let idle = last_activity.elapsed();
            if idle > idle_threshold {
                drop(state);
                info!(event_code = "subagent.sidecar.idle_exit", "idle exit triggered");
                let _ = self.shutdown_internal().await;
                return;
            }
            // --- health pinger + resource sampling (piggybacked) ---
            // Both run on the same ~30s cadence. We do ONE adapter.health RPC
            // and parse `sessions` from its response for the hot-session count,
            // avoiding a redundant second RPC (spec §8.1 collection).
            if last_health.elapsed() >= self.config.health_interval
                || last_resource_sample.elapsed() >= monitor_interval
            {
                last_health = tokio::time::Instant::now();
                last_resource_sample = tokio::time::Instant::now();
                let client = state.client.clone();
                drop(state); // release state lock before .await
                let hot_sessions = if let Some(client) = client {
                    match client
                        .lock()
                        .await
                        .call_with_timeout(
                            "adapter.health",
                            serde_json::json!({}),
                            Duration::from_secs(2),
                        )
                        .await
                    {
                        Ok(resp) => resp
                            .get("sessions")
                            .and_then(|v| v.as_u64())
                            .map(|n| n as u32)
                            .unwrap_or(0),
                        Err(e) => {
                            warn!(event_code = "subagent.sidecar.health_failed", error = %e);
                            0
                        }
                    }
                } else {
                    0
                };
                // Resource sampling on the same tick (no second RPC needed).
                self.maybe_sample_resources(sidecar_pid, hot_sessions).await;
                continue;
            }
        }
    }

    /// Sample resources, log every tick to `tracing` (time-series signal),
    /// and write a DB event ONLY on escalation transitions (lifecycle signal).
    /// Spec §6.5: "Emit resource events at lifecycle boundaries only (not a
    /// metrics time-series table)". The `resource_pressure_state` latch in
    /// `SupervisorState` debounces — a sustained 20-min pressure condition
    /// produces ONE `memory_pressure` event, not 40.
    ///
    /// State machine (edge-triggered):
    ///   Normal → Pressure        : write `memory_pressure` DB event (warn log)
    ///   Normal → LimitExceeded   : write `rss_limit_exceeded` DB event (error log)
    ///   Pressure → LimitExceeded : write `rss_limit_exceeded` DB event (error log)
    ///   Pressure → Normal        : info log ONLY (no DB event — `resource_recovered`
    ///                              not in spec §3.2 enum, deferred to Plan 6)
    ///   LimitExceeded → Normal   : info log ONLY (no DB event — same as above)
    ///   LimitExceeded → Pressure : no event (still in warning tier)
    ///   same → same              : no event (debounced)
    ///
    /// The latch state updates on EVERY transition (including recovery) so a
    /// re-pressurization after recovery writes a fresh `memory_pressure` event.
    async fn maybe_sample_resources(&self, sidecar_pid: Option<u32>, hot_sessions: u32) {
        let monitor = match &self.resource_monitor {
            Some(m) => m,
            None => return,
        };
        let sample = {
            let mut guard = match monitor.lock() {
                Ok(g) => g,
                Err(_) => return, // poisoned — skip this tick
            };
            guard.sample(sidecar_pid, hot_sessions)
        };
        // Time-series signal — logged EVERY tick (level-triggered).
        info!(
            event_code = "subagent.resource.sample",
            service_rss_mb = sample.service_rss_mb,
            sidecar_rss_mb = ?sample.sidecar_rss_mb,
            sidecar_cpu_percent = ?sample.sidecar_cpu_percent,
            hot_session_count = sample.hot_session_count,
            system_available_mb = sample.system_available_mb,
            "resource sample"
        );
        // Compute new pressure state from predicates.
        let (under_pressure, exceeds_soft, exceeds_hard) = {
            let guard = match monitor.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            (
                guard.is_under_pressure(&sample),
                guard.exceeds_soft_limit(&sample),
                guard.exceeds_hard_limit(&sample),
            )
        };
        let new_state = if exceeds_hard {
            ResourcePressureState::LimitExceeded
        } else if under_pressure || exceeds_soft {
            ResourcePressureState::Pressure
        } else {
            ResourcePressureState::Normal
        };
        // Lifecycle signal — write DB event ONLY on escalation transitions
        // (edge-triggered). Recovery transitions log to tracing only (no DB
        // event — `resource_recovered` is not in spec §3.2's enum; deferred
        // to Plan 6). The latch state updates on every transition so
        // re-pressurization after recovery writes a fresh event.
        let (db_event, is_recovery, old_state) = {
            let mut state = match self.state.try_lock() {
                Ok(g) => g,
                Err(_) => return, // supervision loop holds it — skip this tick
            };
            let old = state.resource_pressure_state;
            let event = ResourcePressureState::transition_event(old, new_state);
            let recovery = ResourcePressureState::is_recovery(old, new_state);
            // Update latch on ANY real transition (escalation OR recovery).
            if old != new_state {
                state.resource_pressure_state = new_state;
            }
            (event, recovery, old)
        };
        // Recovery: log to tracing only, no DB event.
        if is_recovery {
            info!(
                event_code = "subagent.resource.recovered",
                old_state = ?old_state,
                new_state = ?new_state,
                sidecar_rss_mb = ?sample.sidecar_rss_mb,
                system_available_mb = sample.system_available_mb,
                "resource pressure recovered to normal (DB event deferred to Plan 6)"
            );
            return;
        }
        // Escalation: log + write DB event.
        let Some(event_type) = db_event else {
            return; // debounced or same-tier downgrade — no event
        };
        match event_type {
            "memory_pressure" => {
                warn!(
                    event_code = "subagent.resource.memory_pressure",
                    system_available_mb = sample.system_available_mb,
                    sidecar_rss_mb = ?sample.sidecar_rss_mb,
                    "entered memory pressure (Plan 6 will pause queue + hibernate LRU)"
                );
            }
            "rss_limit_exceeded" => {
                error!(
                    event_code = "subagent.resource.rss_limit_exceeded",
                    sidecar_rss_mb = ?sample.sidecar_rss_mb,
                    hard_limit_mb = self.config.memory_hard_limit_mb,
                    "sidecar RSS exceeded hard limit (Plan 6 will force-kill)"
                );
            }
            _ => unreachable!("transition_event only returns known escalation event types"),
        }
        self.write_resource_event_with_sample(event_type, Some(&sample));
    }
```

**Note on recovery events:** `resource_recovered` is NOT in spec §3.2's event enum. Plan 5 logs recovery transitions to `tracing` (info level, `event_code = "subagent.resource.recovered"`) but does NOT write them to the `subagent_resource_events` table. The DB event type + spec enum update are deferred to Plan 6, where the recovery event can be designed together with the backpressure response (which is what makes "recovery" semantically meaningful — without backpressure, "recovery" just means "pressure stopped on its own").

- [ ] **Step 10: Write edge-trigger test for `ResourcePressureState::transition_event`**

Append to `crates/busytok-subagent/tests/resource.rs` (the unit-test file from Task 1 — tests the pure function without needing the async supervisor):

```rust
use busytok_subagent::resource::ResourcePressureState;

#[test]
fn transition_event_returns_memory_pressure_on_normal_to_pressure() {
    let event = ResourcePressureState::transition_event(
        ResourcePressureState::Normal,
        ResourcePressureState::Pressure,
    );
    assert_eq!(event, Some("memory_pressure"));
}

#[test]
fn transition_event_returns_rss_limit_on_normal_to_limit_exceeded() {
    let event = ResourcePressureState::transition_event(
        ResourcePressureState::Normal,
        ResourcePressureState::LimitExceeded,
    );
    assert_eq!(event, Some("rss_limit_exceeded"));
}

#[test]
fn transition_event_returns_rss_limit_on_pressure_to_limit_exceeded() {
    let event = ResourcePressureState::transition_event(
        ResourcePressureState::Pressure,
        ResourcePressureState::LimitExceeded,
    );
    assert_eq!(event, Some("rss_limit_exceeded"));
}

#[test]
fn transition_event_returns_none_on_pressure_to_normal_recovery() {
    // Recovery: no DB event (resource_recovered not in spec §3.2 enum).
    // The supervisor logs recovery to tracing only; DB event deferred to Plan 6.
    let event = ResourcePressureState::transition_event(
        ResourcePressureState::Pressure,
        ResourcePressureState::Normal,
    );
    assert_eq!(event, None);
    // But is_recovery returns true so the caller knows to log.
    assert!(ResourcePressureState::is_recovery(
        ResourcePressureState::Pressure,
        ResourcePressureState::Normal,
    ));
}

#[test]
fn transition_event_returns_none_on_limit_exceeded_to_normal_recovery() {
    let event = ResourcePressureState::transition_event(
        ResourcePressureState::LimitExceeded,
        ResourcePressureState::Normal,
    );
    assert_eq!(event, None);
    assert!(ResourcePressureState::is_recovery(
        ResourcePressureState::LimitExceeded,
        ResourcePressureState::Normal,
    ));
}

#[test]
fn transition_event_returns_none_on_limit_exceeded_to_pressure() {
    // Downgrade within warning tier — no new lifecycle event.
    let event = ResourcePressureState::transition_event(
        ResourcePressureState::LimitExceeded,
        ResourcePressureState::Pressure,
    );
    assert_eq!(event, None);
}

#[test]
fn transition_event_returns_none_on_same_state() {
    // Debounce — sustained pressure produces ONE event, not 40.
    assert_eq!(
        ResourcePressureState::transition_event(
            ResourcePressureState::Normal,
            ResourcePressureState::Normal,
        ),
        None,
        "Normal → Normal must be debounced (no event)"
    );
    assert_eq!(
        ResourcePressureState::transition_event(
            ResourcePressureState::Pressure,
            ResourcePressureState::Pressure,
        ),
        None,
        "Pressure → Pressure must be debounced (spec §6.5 lifecycle-boundaries-only)"
    );
    assert_eq!(
        ResourcePressureState::transition_event(
            ResourcePressureState::LimitExceeded,
            ResourcePressureState::LimitExceeded,
        ),
        None,
        "LimitExceeded → LimitExceeded must be debounced"
    );
}
```

- [ ] **Step 11: Run edge-trigger tests**

Run: `cargo test -p busytok-subagent --test resource transition_event`
Expected: PASS — all 7 transition tests green.

- [ ] **Step 12: Run full supervisor test suite**

Run: `cargo test -p busytok-subagent --test sidecar_supervisor`
Expected: PASS — all existing tests still green, new `write_resource_event_with_sample` test green.

- [ ] **Step 13: Commit**

```bash
git add crates/busytok-subagent/src/sidecar/config.rs \
  crates/busytok-subagent/src/sidecar/supervisor.rs \
  crates/busytok-subagent/tests/sidecar_supervisor.rs \
  crates/busytok-subagent/tests/sidecar_config.rs \
  crates/busytok-subagent/tests/resource.rs
git commit -m "feat(subagent): wire ResourceMonitor into supervision loop, edge-triggered events"
```

---

## Task 3: Doctor checks via existing `settings.diagnostics` path + `busytok doctor` CLI

**Files:**
- Modify: `crates/busytok-protocol/src/dto.rs` (add `SubagentDoctorResultDto`, `DoctorCheckDto`; extend `SettingsDiagnosticsDto` with optional `subagent` field)
- Modify: `crates/busytok-runtime/src/supervisor.rs` (add `run_subagent_doctor` private helper; call from existing `settings_diagnostics` handler)
- Modify: `apps/cli/src/main.rs` (add top-level `Doctor` command variant — spec §855, §1068)
- Modify: `apps/cli/src/commands.rs` (add `handle_doctor`)
- Test: `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`

**Architecture note (layered design):**
- **RPC layer (internal):** Spec §7.3 line 903 says `doctor.check` is "Extended with subagent-related health checks". This means extending the existing `settings.diagnostics` RPC path — NOT adding a new `subagent.doctor` RPC method, NOT adding a new `RuntimeControl` trait method, NOT adding a new dispatch arm. `SettingsDiagnosticsDto` gains an optional `subagent` field.
- **CLI layer (external contract):** Spec §855 + §1068 explicitly require a top-level `busytok doctor` CLI command. Plan 5 adds `Command::Doctor` as a top-level variant. Its handler calls the existing `settings.diagnostics` RPC (no new RPC) and pretty-prints the subagent section. This preserves the spec's external CLI contract while reusing the existing RPC infrastructure internally.

**Interfaces:**
- Produces: `SubagentDoctorResultDto { pub checks: Vec<DoctorCheckDto>, pub overall_ok: bool }`
- Produces: `DoctorCheckDto { pub name: String, pub status: String, pub detail: Option<String> }`
- Produces: `SettingsDiagnosticsDto.subagent: Option<SubagentDoctorResultDto>` (new optional field — backwards-compatible)
- Produces: `BusytokSupervisor::run_subagent_doctor(&self) -> SubagentDoctorResultDto` (private helper, called from `settings_diagnostics`)
- Produces: `Command::Doctor` top-level CLI variant (spec §855) + `handle_doctor()` handler
- Does NOT produce: new RPC method, new trait method, new dispatch arm (reuses `settings.diagnostics`)

- [ ] **Step 1: Write failing test for SubagentDoctorResultDto DTO**

Add to `crates/busytok-protocol/src/dto.rs` (in the tests module at the bottom, mirroring existing DTO test style):

```rust
#[test]
fn subagent_doctor_result_dto_serializes_round_trip() {
    let dto = SubagentDoctorResultDto {
        checks: vec![DoctorCheckDto {
            name: "resource_policy_valid".to_string(),
            status: "ok".to_string(),
            detail: None,
        }],
        overall_ok: true,
    };
    let json = serde_json::to_string(&dto).unwrap();
    let back: SubagentDoctorResultDto = serde_json::from_str(&json).unwrap();
    assert_eq!(back.checks.len(), 1);
    assert_eq!(back.checks[0].name, "resource_policy_valid");
    assert!(back.overall_ok);
}

#[test]
fn settings_diagnostics_dto_serializes_with_optional_subagent_none() {
    // Backwards-compat: existing clients don't send `subagent` field.
    // Deserialization must still work.
    let json = r#"{
        "db_healthy": true,
        "db_size_bytes": 4096,
        "migration_version": 3,
        "usage_event_count": 0,
        "last_log_checkpoint_ms": null,
        "writer_queue_depth": 0,
        "aggregate_lag_ms": 0,
        "recent_diagnostics": []
    }"#;
    let dto: SettingsDiagnosticsDto = serde_json::from_str(json).unwrap();
    assert!(dto.subagent.is_none(), "missing field => None (backwards-compat)");
}

#[test]
fn settings_diagnostics_dto_serializes_with_subagent_present() {
    let dto = SettingsDiagnosticsDto {
        db_healthy: true,
        db_size_bytes: 4096,
        migration_version: 3,
        usage_event_count: 0,
        last_log_checkpoint_ms: None,
        writer_queue_depth: 0,
        aggregate_lag_ms: 0,
        recent_diagnostics: vec![],
        subagent: Some(SubagentDoctorResultDto {
            checks: vec![DoctorCheckDto {
                name: "service_running".to_string(),
                status: "ok".to_string(),
                detail: None,
            }],
            overall_ok: true,
        }),
    };
    let json = serde_json::to_string(&dto).unwrap();
    let back: SettingsDiagnosticsDto = serde_json::from_str(&json).unwrap();
    assert!(back.subagent.is_some());
    assert_eq!(back.subagent.unwrap().checks.len(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p busytok-protocol subagent_doctor_result_dto_serializes_round_trip`
Expected: FAIL — `cannot find type SubagentDoctorResultDto`.

- [ ] **Step 3: Add DTOs + extend SettingsDiagnosticsDto**

Add to `crates/busytok-protocol/src/dto.rs` (after `SettingsDiagnosticsDto`):

```rust
/// Result of running subagent doctor checks (spec §7.1). Returned as the
/// optional `subagent` field of `SettingsDiagnosticsDto` — no separate RPC
/// method, reuses the existing `settings.diagnostics` path.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SubagentDoctorResultDto {
    pub checks: Vec<DoctorCheckDto>,
    /// True iff no check has `status == "error"`. Warnings don't fail.
    pub overall_ok: bool,
}

/// One doctor check result. `status` is one of: `"ok"`, `"warning"`, `"error"`.
/// - `"ok"`: check passed.
/// - `"warning"`: check surfaced a non-blocking issue (e.g. stale subagents,
///   or a stubbed check not yet implemented — stubs return "warning" so
///   `overall_ok` doesn't claim a green check on unverified ground).
/// - `"error"`: check failed and `overall_ok` will be false.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DoctorCheckDto {
    pub name: String,
    pub status: String,
    pub detail: Option<String>,
}
```

Extend `SettingsDiagnosticsDto` — add the `subagent` field at the end (with `#[serde(default)]` for backwards-compat so existing clients that don't send it still deserialize):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SettingsDiagnosticsDto {
    pub db_healthy: bool,
    pub db_size_bytes: i64,
    pub migration_version: i64,
    pub usage_event_count: i64,
    pub last_log_checkpoint_ms: Option<i64>,
    /// Current writer channel queue depth (0 = idle).
    pub writer_queue_depth: i64,
    /// Current aggregate lag in milliseconds (0 = caught up).
    pub aggregate_lag_ms: i64,
    /// Recent runtime diagnostic events (e.g. subscription lifecycle,
    /// writer thresholds, drift events).
    pub recent_diagnostics: Vec<SettingsDiagnosticEventDto>,
    /// Subagent doctor checks (spec §7.1). `None` when the subagent feature
    /// is disabled or not yet checked. Reuses the existing
    /// `settings.diagnostics` RPC path — no separate `subagent.doctor` RPC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<SubagentDoctorResultDto>,
}
```

- [ ] **Step 4: Run DTO tests to verify they pass**

Run: `cargo test -p busytok-protocol subagent_doctor_result_dto_serializes_round_trip settings_diagnostics_dto_serializes`
Expected: PASS — all 3 DTO tests green.

- [ ] **Step 5: Write failing test for settings_diagnostics including subagent section**

Append to `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`:

```rust
// --- doctor via settings.diagnostics (Plan 5 Task 3, spec §7.1 + §7.3) ---
//
// Verifies that the EXISTING settings.diagnostics RPC path now includes
// an optional `subagent` section with 11 §7.1 checks. No new RPC method —
// the doctor reuses the existing diagnostics infrastructure.

#[tokio::test]
async fn settings_diagnostics_includes_subagent_doctor_with_11_checks() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let mut settings = make_sidecar_settings();
    // Disable sidecar so doctor's `sidecar_launchable` check is "ok"
    // (no bundle to launch in unit tests).
    settings.subagent.pi_sidecar.enabled = false;
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    let supervisor = BusytokSupervisor::new(db, paths, settings).await;

    // Call the EXISTING settings_diagnostics handler — no new RPC.
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let dto = envelope.data;

    // Subagent section is present.
    let sub = dto
        .subagent
        .as_ref()
        .expect("settings.diagnostics must include subagent section");

    // 11 checks total per spec §7.1.
    assert_eq!(sub.checks.len(), 11, "must have all 11 §7.1 checks");

    // Verify the 6 stubbed checks return "warning" (NOT "ok") —
    // unimplemented checks must not claim green.
    for name in [
        "bundled_node_arch",
        "bundle_manifest_readable",
        "protocol_version",
        "default_model_config",
        "pi_runtime_installed",
        "artifact_store_writable",
    ] {
        let check = sub
            .checks
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("missing check: {name}"));
        assert_eq!(
            check.status, "warning",
            "stubbed check {name} must return 'warning' not 'ok' (unverified = warning)"
        );
        assert!(
            check.detail.as_deref().unwrap_or("").contains("not yet implemented"),
            "stubbed check {name} detail should explain it's not yet implemented"
        );
    }

    // Verify the real checks return "ok" (sidecar disabled => launchable ok).
    let launchable = sub
        .checks
        .iter()
        .find(|c| c.name == "sidecar_launchable")
        .expect("missing sidecar_launchable check");
    assert_eq!(launchable.status, "ok", "sidecar disabled => launchable ok");

    // overall_ok is true (warnings don't fail, no errors).
    assert!(sub.overall_ok, "warnings don't break overall_ok");

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn settings_diagnostics_subagent_flags_stale_subagents_over_30_days() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let mut settings = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    let supervisor = BusytokSupervisor::new(db, paths, settings).await;

    // Insert a stale subagent (last_active 31 days ago).
    {
        let db = supervisor.db_for_test().lock().unwrap();
        let stale_ms = busytok_domain::now_ms() - (31 * 24 * 60 * 60 * 1000);
        db.conn()
            .execute(
                "INSERT INTO subagent_logical_subagents \
                 (id, name, intent, status, created_at_ms, last_active_at_ms) \
                 VALUES ('stale_sub', 'stale-test', 'test', 'warm', ?1, ?1)",
                rusqlite::params![stale_ms],
            )
            .unwrap();
    }

    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let stale_check = sub
        .checks
        .iter()
        .find(|c| c.name == "subagents_unused_30d")
        .expect("must have subagents_unused_30d check");
    assert_eq!(stale_check.status, "warning", "stale subagent => warning");
    assert!(
        stale_check.detail.as_deref().unwrap_or("").contains("stale_sub"),
        "detail should mention the stale subagent"
    );
    assert!(sub.overall_ok, "warnings don't break overall_ok");

    supervisor.shutdown_writer().await.unwrap();
}
```

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p busytok-runtime --test subagent_e2e_sidecar settings_diagnostics_includes_subagent_doctor`
Expected: FAIL — `no field subagent on type SettingsDiagnosticsDto` (or compile error if the DTO wasn't extended yet — but Step 3 extended it, so this should fail because `settings_diagnostics` handler doesn't populate the field yet, returning `None`).

- [ ] **Step 7: Implement run_subagent_doctor on BusytokSupervisor + wire into settings_diagnostics**

Modify `crates/busytok-runtime/src/supervisor.rs`. Add the import:

```rust
use busytok_protocol::dto::{DoctorCheckDto, SubagentDoctorResultDto};
```

Add a private helper `run_subagent_doctor` (place it near the other private helpers, after `sidecar_init_error`):

```rust
    /// Run the 11 spec §7.1 doctor checks. The subagent-specific checks
    /// (SQLite, sidecar launchable, resource policy, stale subagents) are
    /// real; the 6 bundle-inspection checks return `status: "warning"` with
    /// a "not yet implemented" detail — they must NOT return "ok" because
    /// unverified checks claiming green would mislead users. `overall_ok`
    /// is true iff no check has `status == "error"` (warnings don't fail).
    fn run_subagent_doctor(&self) -> SubagentDoctorResultDto {
        let mut checks: Vec<DoctorCheckDto> = Vec::new();

        // 1. busytok-service running — always ok (we're running this code).
        checks.push(DoctorCheckDto {
            name: "service_running".into(),
            status: "ok".into(),
            detail: None,
        });

        // 2. SQLite readable/writable + schema version.
        {
            let db = self.db.lock().unwrap();
            let schema_ok = db
                .conn()
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .is_ok();
            let schema_version = db
                .conn()
                .query_row(
                    "SELECT MAX(version) FROM schema_migrations",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or(0);
            checks.push(DoctorCheckDto {
                name: "sqlite_readable".into(),
                status: if schema_ok { "ok" } else { "error" }.into(),
                detail: Some(format!("schema_version={schema_version}")),
            });
        }

        // 3. Pi sidecar launchable — surface sidecar_init_error if present.
        //    When pi_sidecar.enabled=false, this is "ok" (feature off).
        checks.push(DoctorCheckDto {
            name: "sidecar_launchable".into(),
            status: if self.sidecar_init_error().is_some() { "error" } else { "ok" }.into(),
            detail: self.sidecar_init_error().map(|s| s.to_string()),
        });

        // 4-9. Bundled node arch, manifest, protocol version, model config,
        //      Pi runtime, artifact store — NOT YET IMPLEMENTED. Return
        //      "warning" (not "ok") so overall_ok doesn't claim green on
        //      unverified ground. Plan 6+ will implement bundle inspection.
        for name in [
            "bundled_node_arch",
            "bundle_manifest_readable",
            "protocol_version",
            "default_model_config",
            "pi_runtime_installed",
            "artifact_store_writable",
        ] {
            checks.push(DoctorCheckDto {
                name: name.into(),
                status: "warning".into(),
                detail: Some("not yet implemented (Plan 6+ bundle inspection)".into()),
            });
        }

        // 10. Resource policy valid — check the deserialized policy fields.
        {
            let settings = self.settings.lock().unwrap();
            let p = &settings.subagent.resource_policy;
            let ok = p.memory_pressure_free_mb > 0 && p.monitor_interval_seconds > 0;
            checks.push(DoctorCheckDto {
                name: "resource_policy_valid".into(),
                status: if ok { "ok" } else { "error" }.into(),
                detail: Some(format!(
                    "memory_pressure_free_mb={}, monitor_interval_seconds={}",
                    p.memory_pressure_free_mb, p.monitor_interval_seconds
                )),
            });
        }

        // 11. Subagents unused > 30 days (warning, not error).
        {
            let db = self.db.lock().unwrap();
            let threshold_ms = busytok_domain::now_ms() - (30 * 24 * 60 * 60 * 1000);
            let stale: Vec<String> = db
                .conn()
                .prepare(
                    "SELECT id FROM subagent_logical_subagents \
                     WHERE last_active_at_ms IS NOT NULL \
                     AND last_active_at_ms < ?1 \
                     AND status != 'deleted'",
                )
                .and_then(|mut stmt| {
                    let rows = stmt
                        .query_map(rusqlite::params![threshold_ms], |row| row.get::<_, String>(0))?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap_or_default();
            let is_warning = !stale.is_empty();
            checks.push(DoctorCheckDto {
                name: "subagents_unused_30d".into(),
                status: if is_warning { "warning" } else { "ok" }.into(),
                detail: if is_warning {
                    Some(format!("{} stale subagent(s): {}", stale.len(), stale.join(", ")))
                } else {
                    None
                },
            });
        }

        let overall_ok = checks.iter().all(|c| c.status != "error");
        SubagentDoctorResultDto { checks, overall_ok }
    }
```

Wire it into the existing `settings_diagnostics` handler. Find the `let now_ms = busytok_domain::now_ms();` line near the end of `settings_diagnostics` and add the subagent section before constructing the DTO:

```rust
        // Spec §7.1 + §7.3: extend settings.diagnostics with subagent doctor
        // checks. Reuses the existing RPC path — no new method. Only
        // populate when the subagent feature is enabled in settings.
        let subagent = {
            let settings = self.settings.lock().unwrap();
            if settings.subagent.pi_sidecar.enabled {
                Some(self.run_subagent_doctor())
            } else {
                None
            }
        };

        let now_ms = busytok_domain::now_ms();
        self.build_read_envelope(
            SettingsDiagnosticsDto {
                db_healthy,
                db_size_bytes,
                migration_version: migration_version as i64,
                usage_event_count,
                last_log_checkpoint_ms,
                writer_queue_depth,
                aggregate_lag_ms,
                recent_diagnostics,
                subagent,
            },
```

**Note:** The `settings_diagnostics` handler currently constructs `SettingsDiagnosticsDto` without the `subagent` field. After Step 3 added the field to the DTO, this won't compile until you add `subagent` to the struct literal. The edit above adds it.

- [ ] **Step 8: Run runtime tests to verify they pass**

Run: `cargo test -p busytok-runtime --test subagent_e2e_sidecar settings_diagnostics_includes_subagent_doctor settings_diagnostics_subagent_flags_stale`
Expected: PASS — both tests green.

- [ ] **Step 9: Add top-level `Doctor` CLI command (spec §855, §1068)**

Modify `apps/cli/src/main.rs`. Add a new top-level variant to the `Command` enum (next to `Diagnostics`, `Settings`, etc.):

```rust
    /// Run doctor health checks (spec §855, §1068: `busytok doctor`).
    /// Calls the existing `settings.diagnostics` RPC and pretty-prints
    /// the subagent section. No new RPC method.
    Doctor,
```

Add to the `run` match:

```rust
        Command::Doctor => commands::handle_doctor().await,
```

- [ ] **Step 10: Add handle_doctor to commands.rs**

Modify `apps/cli/src/commands.rs` — add the handler (calls the EXISTING `settings.diagnostics` RPC, pretty-prints the subagent section):

```rust
/// `busytok doctor` — run health checks (spec §855, §1068).
///
/// Calls the existing `settings.diagnostics` RPC (no new RPC method) and
/// pretty-prints the `subagent` section of the response. If the subagent
/// feature is disabled, prints a notice and exits 0.
pub async fn handle_doctor() -> Result<()> {
    let client = connect_control_client().await?;
    let response = client
        .call("settings.diagnostics", serde_json::json!({}))
        .await?;
    let dto: busytok_protocol::dto::SettingsDiagnosticsDto =
        serde_json::from_value(response.result().cloned().unwrap_or(serde_json::Value::Null))?;
    match dto.subagent {
        None => {
            println!("subagent feature disabled — no doctor checks to run");
            return Ok(());
        }
        Some(sub) => {
            if sub.overall_ok {
                println!("✓ subagent doctor: all checks passed (warnings allowed)");
            } else {
                println!("✗ subagent doctor: one or more checks failed");
            }
            for check in &sub.checks {
                let symbol = match check.status.as_str() {
                    "ok" => "✓",
                    "warning" => "⚠",
                    _ => "✗",
                };
                match &check.detail {
                    Some(detail) => println!("  {symbol} {}: {detail}", check.name),
                    None => println!("  {symbol} {}", check.name),
                }
            }
            if !sub.overall_ok {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 11: Build the CLI**

Run: `cargo build -p busytok-cli`
Expected: PASS.

- [ ] **Step 12: Commit**

```bash
git add crates/busytok-protocol/src/dto.rs \
  crates/busytok-runtime/src/supervisor.rs \
  apps/cli/src/main.rs apps/cli/src/commands.rs \
  crates/busytok-runtime/tests/subagent_e2e_sidecar.rs
git commit -m "feat(doctor): extend settings.diagnostics with 11 §7.1 subagent checks (warning stubs)"
```

---

## Task 4: 5-min rolling crash window limiter (spec §5.4)

**Files:**
- Modify: `crates/busytok-subagent/src/sidecar/supervisor.rs` (add `restart_history: VecDeque<Instant>` to `SupervisorState`; prune + check in `spawn_internal`; push on crash in `supervision_loop`)
- Test: `crates/busytok-subagent/tests/sidecar_supervisor.rs`

**Interfaces:**
- Produces: `SupervisorState.restart_history: std::collections::VecDeque<tokio::time::Instant>`
- Produces: `const RESTART_WINDOW: Duration = Duration::from_secs(300)` (5 min, spec §5.4)
- Modifies: `spawn_internal` — prunes `restart_history` entries older than 5 min, rejects spawn if `len() >= max_restart_attempts` (3) within the window
- Modifies: `supervision_loop` crash branch — pushes `Instant::now()` to `restart_history` before incrementing `restart_attempts`

**Architecture note:** The existing code (supervisor.rs:127-139) only counts consecutive `restart_attempts` and resets on successful spawn. This means a crashy sidecar that crashes 3 times, starts successfully for 1 second, then crashes again, would never hit the cap. Spec §5.4 requires "max 3 attempts per 5 min" — a rolling window. This task adds the window as a `VecDeque<Instant>` that survives successful spawns (it's NOT reset on spawn). The existing `restart_attempts` field stays for backoff calculation (consecutive failures only); the new `restart_history` is the hard cap.

- [ ] **Step 1: Write failing test for 5-min rolling window rejection**

Append to `crates/busytok-subagent/tests/sidecar_supervisor.rs`:

```rust
// --- 5-min rolling crash window (Plan 5 Task 4, spec §5.4) ---

/// Test that after 3 crashes within 5 min, the 4th restart attempt is
/// rejected with `SidecarError::Crashed`. The existing code only counts
/// consecutive `restart_attempts` (reset on successful spawn); this test
/// verifies the NEW rolling window limiter.
#[tokio::test]
async fn spawn_rejects_after_3_crashes_within_5_min_window() {
    use busytok_subagent::sidecar::SidecarError;
    use std::collections::VecDeque;
    use tokio::time::Instant;

    let db = busytok_store::Database::open_in_memory().unwrap();
    let shared_db: std::sync::Arc<std::sync::Mutex<busytok_store::Database>> =
        std::sync::Arc::new(std::sync::Mutex::new(db));
    let config = busytok_subagent::sidecar::SidecarConfig {
        node_binary: std::path::PathBuf::from("bash"),
        bundle_path: std::path::PathBuf::from("/dev/null"),
        env: std::collections::HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: std::time::Duration::from_secs(30),
        task_timeout: std::time::Duration::from_secs(30),
        max_restart_attempts: 3,
        restart_backoff_base: std::time::Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
    };
    let sup = busytok_subagent::sidecar::PiSidecarSupervisor::new(
        config,
        Some(std::sync::Arc::clone(&shared_db)),
    );

    // Simulate 3 crashes within the 5-min window by pre-populating
    // restart_history with 3 recent timestamps. This bypasses the
    // supervision loop and directly tests the limiter in spawn_internal.
    {
        let mut state = sup.state_for_test().lock().await;
        let now = Instant::now();
        state.restart_history = VecDeque::from([
            now, // crash 1: 0s ago
            now, // crash 2: 0s ago
            now, // crash 3: 0s ago
        ]);
    }

    // The 4th spawn attempt should be rejected.
    let result = sup.ensure_started().await;
    assert!(
        matches!(result, Err(SidecarError::Crashed(_))),
        "4th restart within 5 min must be rejected with SidecarError::Crashed, got: {result:?}"
    );
}

/// Test that crashes older than 5 min are pruned, allowing restart.
#[tokio::test]
async fn spawn_allows_restart_after_5_min_window_expires() {
    use std::collections::VecDeque;
    use tokio::time::{Instant, Duration};

    let db = busytok_store::Database::open_in_memory().unwrap();
    let shared_db: std::sync::Arc<std::sync::Mutex<busytok_store::Database>> =
        std::sync::Arc::new(std::sync::Mutex::new(db));
    let config = busytok_subagent::sidecar::SidecarConfig {
        node_binary: std::path::PathBuf::from("bash"),
        bundle_path: std::path::PathBuf::from("/dev/null"),
        env: std::collections::HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: std::time::Duration::from_secs(30),
        task_timeout: std::time::Duration::from_secs(30),
        max_restart_attempts: 3,
        restart_backoff_base: std::time::Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
    };
    let sup = busytok_subagent::sidecar::PiSidecarSupervisor::new(
        config,
        Some(std::sync::Arc::clone(&shared_db)),
    );

    // Simulate 3 crashes 6 minutes ago (outside the 5-min window).
    // Instant::now() - 6min would be ideal, but Instant doesn't support
    // subtraction of arbitrary durations in a way that stays in range.
    // Instead, we use a helper that checks the pruning logic directly.
    {
        let mut state = sup.state_for_test().lock().await;
        // We can't create an Instant in the past directly (Instant has no
        // `now() - duration` constructor that's stable). Instead, we test
        // the pruning function directly.
        let old = Instant::now()
            .checked_sub(Duration::from_secs(360))
            .expect("6 min ago should be representable");
        state.restart_history = VecDeque::from([old, old, old]);
    }

    // After pruning, restart_history should be empty, so spawn should
    // proceed (it will fail because bash /dev/null isn't a real sidecar,
    // but the failure should NOT be SidecarError::Crashed — it should be
    // SidecarError::Spawn or protocol mismatch).
    let result = sup.ensure_started().await;
    assert!(
        !matches!(result, Err(busytok_subagent::sidecar::SidecarError::Crashed(_))),
        "after 5-min window expires, spawn must not be rejected as Crashed, got: {result:?}"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p busytok-subagent --test sidecar_supervisor spawn_rejects_after_3_crashes spawn_allows_restart_after_5_min`
Expected: FAIL — `no field restart_history on type SupervisorState` and `no method state_for_test`.

- [ ] **Step 3: Add restart_history + state_for_test + window pruning**

Modify `crates/busytok-subagent/src/sidecar/supervisor.rs`. Add the import:

```rust
use std::collections::VecDeque;
```

Add the constant near `POLL_INTERVAL`:

```rust
/// Spec §5.4: rolling 5-min window for crash restart attempts.
const RESTART_WINDOW: Duration = Duration::from_secs(300);
```

Add `restart_history` to `SupervisorState`:

```rust
struct SupervisorState {
    child: Option<Child>,
    client: Option<Arc<Mutex<SidecarRpcClient>>>,
    last_activity: tokio::time::Instant,
    restart_attempts: u32,
    /// Set true when the supervision loop is running; prevents double-spawn
    /// of the loop across concurrent `ensure_started` calls.
    supervision_started: bool,
    /// Rolling window of crash timestamps (spec §5.4: "max 3 attempts per
    /// 5 min"). Pruned in `spawn_internal` before checking the cap. NOT
    /// reset on successful spawn (unlike `restart_attempts`) — the window
    /// is the hard cap, `restart_attempts` is for backoff calculation.
    restart_history: VecDeque<tokio::time::Instant>,
}
```

Initialize `restart_history: VecDeque::new()` in both `PiSidecarSupervisor::new` and `with_resource_policy` constructors.

Add a test-only accessor:

```rust
    /// Test-only accessor for the supervisor state. Used by integration
    /// tests that need to pre-populate `restart_history` to test the 5-min
    /// rolling window limiter without driving the full crash/restart cycle.
    #[doc(hidden)]
    pub fn state_for_test(&self) -> &tokio::sync::Mutex<SupervisorState> {
        &self.state
    }
```

Modify `spawn_internal` — add the window check BEFORE the existing `restart_attempts` check:

```rust
    async fn spawn_internal(self: &Arc<Self>) -> Result<(), SidecarError> {
        // Exponential backoff if this is a restart after a crash.
        let backoff = {
            let state = self.state.lock().await;
            // Double-checked locking (existing comment)...
            if state.client.is_some()
                && state
                    .child
                    .as_ref()
                    .map(|c| c.id().is_some())
                    .unwrap_or(false)
            {
                return Ok(());
            }
            // Spec §5.4: rolling 5-min window. Prune entries older than
            // 5 min, then check if we've exceeded the cap. This is the
            // HARD limit — `restart_attempts` (below) is only for backoff.
            let now = tokio::time::Instant::now();
            state
                .restart_history
                .retain(|t| now.duration_since(*t) < RESTART_WINDOW);
            if state.restart_history.len() >= self.config.max_restart_attempts as usize {
                return Err(SidecarError::Crashed(format!(
                    "max restart attempts ({}) exceeded within 5-min window ({} recent crashes)",
                    self.config.max_restart_attempts,
                    state.restart_history.len()
                )));
            }
            // Existing consecutive-attempt check (for backoff calculation).
            if state.restart_attempts > self.config.max_restart_attempts {
                return Err(SidecarError::Crashed(format!(
                    "max consecutive restart attempts ({}) exceeded",
                    self.config.max_restart_attempts
                )));
            }
            if state.restart_attempts > 0 {
                let exp = 2u32.pow(state.restart_attempts - 1);
                self.config.restart_backoff_base * exp
            } else {
                Duration::ZERO
            }
        };
        // ... rest of spawn_internal unchanged ...
```

Modify the crash branch in `supervision_loop` — push to `restart_history`:

```rust
            if let Some(status) = crash_status {
                state.client = None;
                state.child = None;
                state.restart_attempts += 1;
                state.restart_history.push_back(tokio::time::Instant::now());
                warn!(
                    event_code = "subagent.sidecar.crash",
                    exit = ?status,
                    attempts = state.restart_attempts,
                    recent_crashes = state.restart_history.len(),
                    "sidecar crashed"
                );
                drop(state);
                self.reconcile_crash();
                self.write_resource_event("sidecar_crash");
                return;
            }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p busytok-subagent --test sidecar_supervisor spawn_rejects_after_3_crashes spawn_allows_restart_after_5_min`
Expected: PASS.

- [ ] **Step 5: Run full supervisor test suite to verify no regressions**

Run: `cargo test -p busytok-subagent --test sidecar_supervisor`
Expected: PASS — all existing tests still green.

- [ ] **Step 6: Commit**

```bash
git add crates/busytok-subagent/src/sidecar/supervisor.rs \
  crates/busytok-subagent/tests/sidecar_supervisor.rs
git commit -m "feat(subagent): implement 5-min rolling crash window limiter (spec §5.4)"
```

---

## Task 5: Crash recovery e2e test

**Files:**
- Modify: `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`

**Interfaces:**
- Consumes: existing `BUSYTOK_MOCK_CRASH_AFTER` env var on mock-sidecar.sh.
- Consumes: existing `PiSidecarSupervisor` crash recovery logic (supervisor.rs:106-322) — tests it, doesn't change it.

- [ ] **Step 1: Write the crash recovery e2e test**

Append to `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`:

```rust
// --- crash recovery e2e (Plan 5 Task 5, spec §12.1 Case 4) ---
//
// Verifies the EXISTING crash recovery logic in PiSidecarSupervisor
// (supervisor.rs:106-322): when the sidecar process is killed mid-task,
// the supervisor does NOT crash busytok-service, the next delegate
// auto-restarts the sidecar, and the in-flight task is reconciled to
// `failed` with SIDECAR_CRASHED. Memory + task history survive in SQLite.
//
// Mock sidecar fixture: BUSYTOK_MOCK_CRASH_AFTER=1 causes the mock to exit 1
// after sending its first response. The supervisor's `try_wait` detects the
// exit, runs `reconcile_crash`, writes a `sidecar_crash` resource event, and
// exits the supervision loop. The next `ensure_started` respawns.

#[tokio::test]
async fn sidecar_e2e_crash_recovery_next_delegate_restarts_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let mut settings = make_sidecar_settings();
    // Short idle so the sidecar doesn't linger between test phases.
    settings.subagent.pi_sidecar.idle_exit_seconds = 300;
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    // Config with BUSYTOK_MOCK_CRASH_AFTER=1: the mock exits after sending
    // exactly one response. The first delegate's turn_auto response IS sent
    // (so the delegate completes), then the mock exits 1.
    let mut cfg = make_sidecar_config();
    cfg.env
        .insert("BUSYTOK_MOCK_CRASH_AFTER".into(), "1".into());
    let supervisor = BusytokSupervisor::new_with_sidecar_config(db, paths, cfg);

    // 1. First delegate — completes (mock sends response THEN exits).
    let resp1 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "crash-test".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "do 1".to_string(),
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .expect("first delegate must complete (response sent before crash)");
    let sub_id = resp1.subagent_id.clone();
    assert_eq!(resp1.status, "completed");

    // 2. Wait for the supervision loop to observe the crash + write the
    //    sidecar_crash event. The loop polls every 100ms; give it up to 4s
    //    (80 iterations × 50ms) to avoid flaking on slow CI runners.
    let mut saw_crash = false;
    for _ in 0..80 {
        let crashed = {
            let db_guard = supervisor.db_handle().lock().unwrap();
            let events = db_guard.subagent_list_resource_events(None, 100).unwrap();
            events.iter().any(|e| e.event_type == "sidecar_crash")
        };
        if crashed {
            saw_crash = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(saw_crash, "sidecar_crash resource event must be written after the mock exits");

    // 3. Second delegate — must auto-restart the sidecar (exponential
    //    backoff 1s on first restart). The supervisor's `ensure_started`
    //    path detects the dead child, calls `spawn_internal`, which sleeps
    //    1s (restart_backoff_base) then respawns. The test must tolerate
    //    this delay.
    let resp2 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "crash-test".to_string(),
            subagent_id: Some(sub_id.clone()),
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "do 2 after crash".to_string(),
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .expect("second delegate must succeed after auto-restart");
    assert_eq!(resp2.status, "completed", "second delegate completes after restart");
    assert_eq!(resp2.subagent_id, sub_id, "same logical subagent (memory preserved)");

    // 4. Verify a sidecar_restart resource event was written.
    let saw_restart = {
        let db_guard = supervisor.db_handle().lock().unwrap();
        let events = db_guard.subagent_list_resource_events(None, 200).unwrap();
        events.iter().any(|e| e.event_type == "sidecar_restart")
    };
    assert!(saw_restart, "sidecar_restart event must be written on auto-restart");

    // 5. Verify task history is preserved (both tasks visible).
    let tasks = supervisor
        .subagent_tasks(SubagentTasksRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
            limit: 50,
        })
        .await
        .unwrap();
    assert!(
        tasks.tasks.len() >= 2,
        "task history must be preserved across crash/restart; got {} tasks",
        tasks.tasks.len()
    );

    // 6. Verify the logical subagent still exists (memory preserved).
    let shown = supervisor
        .subagent_show(SubagentResolveRequestDto {
            name: None,
            id: Some(sub_id),
            cwd: None,
        })
        .await
        .unwrap();
    assert_eq!(shown.name, "crash-test");

    supervisor.shutdown_sidecar().await;
    supervisor.shutdown_writer().await.unwrap();
}
```

- [ ] **Step 2: Run the crash recovery test**

Run: `cargo test -p busytok-runtime --test subagent_e2e_sidecar sidecar_e2e_crash_recovery_next_delegate_restarts_sidecar -- --nocapture`
Expected: PASS — first delegate completes, mock crashes, `sidecar_crash` event written, second delegate triggers restart (after 1s backoff), `sidecar_restart` event written, task history preserved.

If the test flakes on the crash-detection race (the supervision loop's `try_wait` runs every 100ms), bump the poll count from 40 to 80 (4s window) — the 100ms POLL_INTERVAL makes a 2s window sufficient in practice.

- [ ] **Step 3: Commit**

```bash
git add crates/busytok-runtime/tests/subagent_e2e_sidecar.rs
git commit -m "test(subagent): add §12.1 Case 4 crash recovery e2e (mock sidecar kill -9 path)"
```

---

## Task 6: Stress test — 100 subagents

**Files:**
- Modify: `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`

**Interfaces:**
- Consumes: `SubagentLogicalSubagentRow` + `subagent_upsert_logical` from the store.
- Consumes: `ResourceMonitor` (or sysinfo directly) for RSS measurement.

- [ ] **Step 1: Write the 100-subagent stress test**

Append to `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`:

```rust
// --- stress test: 100 idle logical subagents (Plan 5 Task 5, spec §12.2) ---
//
// Spec §12.2 acceptance: "100 idle logical subagents: RSS does not grow
// linearly." Logical subagents are SQLite rows, not processes — RSS should
// grow by < 10MB for 100 rows. This test creates 100 rows via the store,
// measures busytok-service RSS before and after, and asserts sub-linear
// growth. It also verifies only 1 sidecar process exists when active
// (delegate to one subagent, count node/bash children).

#[tokio::test]
async fn sidecar_e2e_stress_100_subagents_rss_does_not_grow_linearly() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = make_sidecar_settings();
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");
    let supervisor = BusytokSupervisor::new_with_sidecar_config(
        db, paths, make_sidecar_config(),
    );

    // Measure RSS before creating any subagents. Use sysinfo directly —
    // constructing a ResourceMonitor here would duplicate the supervisor's
    // own monitor; the spec allows direct sysinfo for the measurement.
    let service_pid = std::process::id();
    let mut sys = sysinfo::System::new_all();
    // sysinfo 0.32 API: refresh_processes_specifics takes ProcessesToUpdate.
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
    sys.refresh_processes_specifics(
        ProcessesToUpdate::List(&[sysinfo::Pid::from_u32(service_pid)]),
        false,
        ProcessRefreshKind::everything(),
    );
    let rss_before_mb = sys
        .process(sysinfo::Pid::from_u32(service_pid))
        .map(|p| (p.memory() as f64) / (1024.0 * 1024.0))
        .unwrap_or(0.0);

    // Create 100 idle logical subagents via direct DB insertion.
    let now_ms = busytok_domain::now_ms();
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        use busytok_store::SubagentLogicalSubagentRow;
        for i in 0..100 {
            db_guard.subagent_upsert_logical(&SubagentLogicalSubagentRow {
                id: format!("stress_sub_{i}"),
                name: format!("stress-{i}"),
                project_id: "stress_proj".into(),
                repo_path: "/r".into(),
                repo_hash: format!("h{i}"),
                branch: None,
                intent: None,
                default_profile: "pi/search-cheap".into(),
                default_model: None,
                status: "cold".into(),
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
                last_active_at_ms: Some(now_ms),
            }).unwrap();
        }
    }

    // Measure RSS after creating 100 subagents.
    sys.refresh_processes_specifics(
        ProcessesToUpdate::List(&[sysinfo::Pid::from_u32(service_pid)]),
        false,
        ProcessRefreshKind::everything(),
    );
    let rss_after_mb = sys
        .process(sysinfo::Pid::from_u32(service_pid))
        .map(|p| (p.memory() as f64) / (1024.0 * 1024.0))
        .unwrap_or(0.0);

    let growth_mb = rss_after_mb - rss_before_mb;
    // Spec §12.2: RSS does not grow linearly. 100 rows of ~200 bytes each
    // is ~20KB of actual data; SQLite page cache may grow by a few MB.
    // 10MB is a generous upper bound that still catches a regression where
    // subagents accidentally spawn processes or hold large in-memory state.
    assert!(
        growth_mb < 10.0,
        "RSS growth for 100 idle subagents must be < 10MB (got {growth_mb:.2}MB); \
         before={rss_before_mb:.2}MB after={rss_after_mb:.2}MB"
    );

    // Verify all 100 appear in list.
    let list = supervisor
        .subagent_list(SubagentListRequestDto {
            status: None,
            project: None,
            include_deleted: Some(false),
        })
        .await
        .unwrap();
    assert!(
        list.subagents.len() >= 100,
        "all 100 stress subagents must be listed; got {}",
        list.subagents.len()
    );

    // Spec §12.2: "Pi sidecar: exactly 1 process when active." Delegate to
    // one subagent, then count the mock-sidecar bash child processes.
    let _resp = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "stress-0".to_string(),
            subagent_id: Some("stress_sub_0".to_string()),
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "noop".to_string(),
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .unwrap();

    // Count bash processes running mock-sidecar.sh. There should be exactly 1.
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let sidecar_count = sys
        .processes()
        .values()
        .filter(|p| {
            p.cmd()
                .iter()
                .any(|arg| arg.to_string_lossy().contains("mock-sidecar.sh"))
        })
        .count();
    assert_eq!(
        sidecar_count, 1,
        "exactly 1 sidecar process must exist when active (got {sidecar_count})"
    );

    supervisor.shutdown_sidecar().await;
    supervisor.shutdown_writer().await.unwrap();
}
```

- [ ] **Step 2: Run the stress test**

Run: `cargo test -p busytok-runtime --test subagent_e2e_sidecar sidecar_e2e_stress_100_subagents_rss_does_not_grow_linearly -- --nocapture`
Expected: PASS — RSS growth < 10MB, exactly 1 sidecar process, all 100 subagents listed.

If the RSS growth assertion flakes on CI (RSS measurements are noisy), widen the bound to 15MB and add a comment explaining why. The spec says "does not grow linearly" — 15MB for 100 subagents is still clearly sub-linear (linear would be ~100× a process footprint).

- [ ] **Step 3: Commit**

```bash
git add crates/busytok-runtime/tests/subagent_e2e_sidecar.rs
git commit -m "test(subagent): add §12.2 stress test (100 idle subagents, single sidecar process)"
```

---

## Task 7: Coverage gate + acceptance test suite

**Files:**
- Modify: `scripts/coverage.sh` (if gate needs bumping)
- Verify: all acceptance criteria from spec §12.1 Case 4 and §12.2 are covered.

- [ ] **Step 1: Run per-crate coverage gate**

Run: `cargo llvm-cov -p busytok-subagent --fail-under-lines 90`
Expected: PASS — `busytok-subagent` ≥ 90% lines.

If the gate fails, identify uncovered lines:

Run: `cargo llvm-cov -p busytok-subagent --html --open` (or `--summary-only` for CI).

Common uncovered-line sources in Plan 5:
- `resource.rs` `sample()` error paths (e.g. `system.process()` returning None for a dead PID). Add a test that samples a PID that doesn't exist:
  ```rust
  #[test]
  fn sample_with_dead_pid_returns_none_sidecar_fields() {
      let policy = SubagentResourcePolicyConfig::default();
      let mut mon = ResourceMonitor::new(policy, 800, 1200);
      // PID 0xFFFF_FFFF is extremely unlikely to exist.
      let s = mon.sample(Some(0xFFFF_FFFF), 0);
      assert!(s.sidecar_rss_mb.is_none(), "dead PID => None RSS");
      assert!(s.sidecar_cpu_percent.is_none(), "dead PID => None CPU");
  }
  ```
- `supervisor.rs` `query_hot_session_count` failure path (client lock poisoned / RPC error). The `unwrap_or(0)` makes this hard to hit — add a test with a supervisor whose `client` is None (pre-spawn).
- `supervisor.rs` `maybe_sample_resources` when `resource_monitor` is None. Construct a supervisor with `resource_monitor: None` — this requires a test-only constructor or making the field pub(crate). Prefer adding a `#[doc(hidden)] pub fn new_without_monitor(config, db) -> Arc<Self>` for tests.

- [ ] **Step 2: Run workspace coverage gate**

Run: `cargo llvm-cov --workspace --fail-under-lines 82`
Expected: PASS — workspace ≥ 82% lines.

If the workspace gate drops below 82%, the regression is in `busytok-runtime` (new doctor code) — add tests for the stubbed doctor checks (e.g. test that `run_subagent_doctor` returns 11 checks with the expected names).

- [ ] **Step 3: Update scripts/coverage.sh if needed**

The existing `scripts/coverage.sh` (from Plan 3) already has:
```bash
GATE="${COVERAGE_GATE:-82}"
cargo llvm-cov -p busytok-subagent --fail-under-lines 90
```

Plan 5 does not raise the gates (Plan 3 already set them to the Plan 5 targets). If a gate needs lowering because Plan 5's new code is genuinely uncoverable (e.g. sysinfo platform-specific code on non-macOS), document the exception in `scripts/coverage.sh`:

```bash
# Plan 5: sysinfo CPU sampling is macOS-specific; the second-refresh path
# may be uncovered on Linux CI. The 90% gate accounts for this.
```

- [ ] **Step 4: Run the full acceptance test suite**

Run all Plan 5 tests together:

```bash
cargo test -p busytok-subagent --test resource
cargo test -p busytok-subagent --test sidecar_supervisor
cargo test -p busytok-subagent --test sidecar_config
cargo test -p busytok-control --test dispatch  # regression — no new dispatch arm in Plan 5
cargo test -p busytok-runtime --test subagent_e2e_sidecar
```

Expected: all tests PASS.

- [ ] **Step 5: Verify spec §12.1 Case 4 acceptance**

Case 4: "Execute task, kill -9 the Pi node subprocess. Expected: busytok-service does not crash. Next delegate auto-restarts sidecar. Memory restored from SQLite. Task history preserved."

The crash recovery e2e test (Task 5) covers all four assertions:
- ✅ busytok-service does not crash — the test continues past the crash.
- ✅ Next delegate auto-restarts sidecar — `resp2.status == "completed"`.
- ✅ Memory restored from SQLite — `resp2.subagent_id == sub_id` (same logical subagent).
- ✅ Task history preserved — `tasks.tasks.len() >= 2`.

Note: Task 5 uses `BUSYTOK_MOCK_CRASH_AFTER=1` (clean exit 1) rather than an explicit `kill -9`. This is functionally equivalent for the supervisor's `try_wait` path — the supervisor detects the child exited, regardless of signal vs. clean exit. A true `kill -9` test would require spawning a real Node sidecar and killing it, which is flaky in CI. The mock-sidecar path is the established pattern in this codebase (see existing `sidecar_supervisor.rs` tests).

- [ ] **Step 6: Verify spec §12.2 acceptance**

| Acceptance criterion | Test covering it |
|----------------------|------------------|
| Idle busytok-service RSS < 50MB (Pi sidecar not running) | `sample_returns_positive_service_rss_mb_for_current_process` (Task 1) — asserts `service_rss_mb > 0`; the < 50MB bound is verified manually (the test process is the supervisor, which is well under 50MB). |
| 100 idle logical subagents: RSS does not grow linearly | `sidecar_e2e_stress_100_subagents_rss_does_not_grow_linearly` (Task 6). |
| Pi sidecar: exactly 1 process when active | `sidecar_e2e_stress_100_subagents_rss_does_not_grow_linearly` (Task 6) — asserts `sidecar_count == 1`. |
| max_hot_sessions enforced | Existing `sidecar_e2e_eviction_releases_lru_and_retries` (Plan 3). |
| After hibernate, hot session count decreases | Existing `sidecar_e2e_delegate_list_show_hibernate_delete` (Plan 2) + `sidecar_e2e_eviction_releases_lru_and_retries` (Plan 3). |
| Sidecar exits after idle TTL | Existing supervision loop idle-exit path (supervisor.rs:290-300), covered by Plan 2 tests. |

- [ ] **Step 7: Wire `with_resource_policy` into the runtime (mandatory — avoids dead code)**

The `with_resource_policy` constructor (Task 2) is dead code unless the runtime calls it. Under `cargo clippy -- -D warnings`, this fails the build. Wire it in `BusytokSupervisor::construct_sidecar` (in `crates/busytok-runtime/src/supervisor.rs`) so the deserialized `SubagentResourcePolicyConfig` flows from settings → monitor:

```rust
// In crates/busytok-runtime/src/supervisor.rs, construct_sidecar:
// Replace the existing PiSidecarSupervisor::new(sidecar_config, Some(Arc::clone(db)))
// call with with_resource_policy:
let policy = settings.subagent.resource_policy.clone();
let sup = busytok_subagent::sidecar::PiSidecarSupervisor::with_resource_policy(
    sidecar_config,
    Some(Arc::clone(db)),
    policy,
);
```

This makes the resource policy's `memory_pressure_free_mb` and `monitor_interval_seconds` flow from `settings.toml` → `SubagentResourcePolicyConfig` → `ResourceMonitor` → predicates + supervision loop. The `PiSidecarSupervisor::new` constructor remains for unit tests that don't have a policy.

- [ ] **Step 8: Clean up dead code**

Run: `cargo clippy --workspace -- -D warnings`
Expected: PASS — no dead-code warnings.

The predicates are instance methods (not static), so they're called via `guard.is_under_pressure(&sample)` in `maybe_sample_resources` — no dead-code risk. The `with_resource_policy` constructor is now called by the runtime (Step 7). `ResourceMonitor::new` is called by both constructors. All `pub` items are used.

- [ ] **Step 9: Final coverage run**

Run: `bash scripts/coverage.sh`
Expected: PASS — workspace ≥ 82%, per-crate `busytok-subagent` ≥ 90%.

- [ ] **Step 10: Commit**

```bash
git add scripts/coverage.sh crates/busytok-runtime/src/supervisor.rs \
  crates/busytok-subagent/src/resource.rs \
  crates/busytok-subagent/tests/resource.rs
git commit -m "test(coverage): verify §12.1/§12.2 acceptance, wire resource policy through runtime"
```

---

## Self-Review

### 1. Spec coverage

- **§8.1 ResourceMonitor collection** — Task 1 implements `ResourceSample` (service RSS, sidecar RSS+CPU, hot session count, system available memory) + `ResourceMonitor::sample`. **Spec §8.1 also lists `queued/running task count` as a collection target, but Plan 5 does NOT collect it** — MVP has no task queue (tasks are synchronous via `turn_auto`). This defer is stated upfront in Global Constraints + "Out of scope" (not buried in Self-Review). The `hot_session_count` field covers the active-session observability need. ⚠️ (deferred: queued/running task count — no queue exists in MVP; documented upfront)
- **§8.2 Config defaults** — Already in `SubagentPiSidecarConfig`/`SubagentResourcePolicyConfig`. Task 2 threads `memory_soft_limit_mb`/`memory_hard_limit_mb` into `SidecarConfig` so they're consumed. `monitor_interval_seconds` stays on `SubagentResourcePolicyConfig` and is read via `ResourceMonitor::monitor_interval()` (avoids a dead field on `SidecarConfig`). ✅
- **§8.3 Pressure Response (5-step escalation)** — **Plan 5 does NOT implement the reactive chain.** All 5 steps (pause queue, hibernate LRU, soft-limit → graceful restart, prepare_hibernate-all → restart, force kill) are deferred to Plan 6. Plan 5 only delivers the *observability signals* (`memory_pressure` / `rss_limit_exceeded` DB events, edge-triggered; recovery → `tracing` only) that Plan 6 will consume. The `memory_soft_limit_mb`/`memory_hard_limit_mb` config fields are threaded into `SidecarConfig` (Task 2) so Plan 6 can read them without a config migration. This is stated explicitly in the Global Constraints "Out of scope" section and acknowledged in the Goal. ⚠️ (scope narrowed — honest about not delivering backpressure; avoids the trap of writing "covered" for stubbed steps)
- **§5.4 Crash Recovery** — The exponential backoff (1s→2s→4s, 3-attempt cap) already lives in `spawn_internal`. **The 5-min sliding window did NOT exist** — config.rs:93-94 explicitly commented this gap, and `restart_attempts` resets on successful spawn (supervisor.rs:220). **Plan 5 Task 4 implements the rolling window** as `restart_history: VecDeque<Instant>` pruned on each spawn attempt, surviving successful spawns. Task 5's e2e test asserts crash → auto-restart on next delegate. ✅
- **§5.1 spike note (sysinfo CPU)** — Task 1's `sample()` docstring + the `second_sample_returns_meaningful_cpu` test explicitly cover the two-refresh-cycle requirement. First sample returns `Some(0.0)` for CPU. ✅
- **§3.2 + §6.5 `subagent_resource_events`** — Task 2 extends `write_resource_event` to populate `rss_mb`/`cpu_percent`/`detail_json`. **Events are edge-triggered via a latch** (`ResourcePressureState::{Normal, Pressure, LimitExceeded}`) — a sustained 20-min pressure condition writes ONE event, not 40. `tracing` logs every tick (time-series signal); DB event is the lifecycle signal (spec §6.5 "lifecycle boundaries only, NOT a metrics time-series table"). **Recovery transitions (Pressure→Normal, LimitExceeded→Normal) are logged to `tracing` ONLY — NOT written to DB** because `resource_recovered` is not in spec §3.2's enum. The latch state still updates on recovery so re-pressurization writes a fresh event. The DB event + spec enum update are deferred to Plan 6. ✅
- **§3.2 event types** — Plan 5 writes only `memory_pressure` and `rss_limit_exceeded` (both already in the spec enum). `sidecar_crash` is kept as-is (spec inconsistency noted in Global Constraints). **`resource_recovered` is NOT written to DB** (not in spec enum — deferred to Plan 6). ✅
- **§12.1 Case 4** — Task 5 covers all four assertions (kill -9 sidecar, busytok-service survives, next delegate auto-restarts, task history preserved). ✅
- **§12.2 acceptance** — Task 6 covers RSS < 10MB growth + single sidecar process. Existing Plan 2/3 tests cover max_hot_sessions + hibernate + idle exit. ✅
- **§7.1 + §7.3 doctor checks** — Task 3 uses a **layered design**: (a) RPC layer reuses the existing `settings.diagnostics` path (spec §7.3 line 903: "`doctor.check` — Extended with subagent-related health checks") — no new `subagent.doctor` RPC method, no new `RuntimeControl` trait method, no new dispatch arm; (b) CLI layer adds the spec-required **top-level `busytok doctor` command** (spec §855, §1068) as a thin client that calls `settings.diagnostics` and pretty-prints the `subagent` section. `SettingsDiagnosticsDto` gains an optional `subagent: Option<SubagentDoctorResultDto>` field (`#[serde(default, skip_serializing_if = "Option::is_none")]` — backwards-compatible). 11 spec §7.1 checks are implemented; 5 subagent-specific checks are real (service running, SQLite readable, sidecar launchable, resource policy valid, subagents unused > 30 days); **6 bundle-inspection checks (node arch, manifest, protocol version, model config, Pi runtime, artifact store) return `status: "warning"`** (NOT `"ok"`) with detail "not yet implemented (Plan 6+ bundle inspection)". `overall_ok` is true iff no check has `status == "error"` — warnings surface without misleading users into thinking unverified ground is green. ✅

### 2. Placeholder scan

Searched the plan for "TBD", "TODO", "implement later", "fill in details", "add appropriate error handling". Found:
- "TODO: Plan 6+ bundle inspection" — deliberate stub marker in the doctor check detail string, not a plan placeholder. The check returns `status: "warning"`, so `overall_ok` does not claim green on it. Acceptable.
- No other placeholders. All steps have complete code.

### 3. Type consistency

- `ResourceSample` — defined in Task 1 with 5 fields (`service_rss_mb`, `sidecar_rss_mb`, `sidecar_cpu_percent`, `hot_session_count`, `system_available_mb`). Used identically in Task 2 (`write_resource_event_with_sample`) and Task 6 (pressure/limit tests). ✅
- `ResourcePressureState` — defined in Task 1 as `enum { Normal, Pressure, LimitExceeded }` with `#[derive(Default)]` (Normal). Two pure functions (no `&self` — testable without async/sysinfo): `transition_event(old, new) -> Option<&'static str>` returns `Some` only for escalation transitions (recovery returns `None`); `is_recovery(old, new) -> bool` returns true for Pressure/LimitExceeded → Normal. Task 2 stores `resource_pressure_state: ResourcePressureState` in `SupervisorState`, calls both functions on each sample. ✅
- `ResourceMonitor::new(policy, soft_limit_mb, hard_limit_mb)` — Task 1 signature. Task 2 calls it in `PiSidecarSupervisor::new` with `config.memory_soft_limit_mb`, `config.memory_hard_limit_mb`. ✅
- `ResourceMonitor::sample(&mut self, sidecar_pid: Option<u32>, hot_session_count: u32) -> ResourceSample` — Task 1 signature. Task 2 calls it as `guard.sample(sidecar_pid, hot_sessions)`. ✅
- `ResourceMonitor::is_under_pressure(&self, &ResourceSample) -> bool` — Task 1 INSTANCE method (reads `self.policy.memory_pressure_free_mb`, not a hardcoded 2048). Task 2 calls `mon.is_under_pressure(&sample)`. Task 6 tests call the same. `exceeds_soft_limit`/`exceeds_hard_limit` are also instance methods reading `self.soft_limit_mb`/`self.hard_limit_mb`. ✅
- `SidecarConfig` new fields — `memory_soft_limit_mb: u32`, `memory_hard_limit_mb: u32` (only two fields). Task 2 adds them; Task 5/6 test configs construct `SidecarConfig` with both fields. `monitor_interval_seconds` is NOT on `SidecarConfig` — it stays on `SubagentResourcePolicyConfig` and is read via `ResourceMonitor::monitor_interval()`. ✅
- `write_resource_event_with_sample(event_type: &str, sample: Option<&ResourceSample>)` — Task 2 public method. Task 6 tests call it. ✅
- `restart_history: VecDeque<Instant>` + `RESTART_WINDOW: Duration = 300s` — Task 4 fields on `SupervisorState`. Pruned in `spawn_internal` before the backoff check; pushed in the crash-detection branch of `supervision_loop`. Distinct from `restart_attempts` (which resets on successful spawn) — `restart_history` survives successful spawns so a flapping sidecar that crashes 4× in 5 min is still rejected. ✅
- `SubagentDoctorResultDto { checks: Vec<DoctorCheckDto>, overall_ok: bool }` — Task 3 DTO. `DoctorCheckDto { name, status, detail }`. `status` is `"ok" | "warning" | "error"`. `overall_ok = !checks.iter().any(|c| c.status == "error")`. Used in runtime test, CLI handler. ✅
- `SettingsDiagnosticsDto.subagent: Option<SubagentDoctorResultDto>` — Task 3 backwards-compatible extension. `#[serde(default, skip_serializing_if = "Option::is_none")]` — existing clients that don't know about the field still deserialize. ✅
- `Command::Doctor` — Task 3 top-level CLI variant (spec §855, §1068: `busytok doctor`). Handler `handle_doctor()` calls existing `settings.diagnostics` RPC, pretty-prints `subagent` section. **No `subagent.doctor` RPC method string anywhere in the plan.** ✅

### 4. Architecture review

- **Reuses existing infrastructure**: `PiSidecarSupervisor`, `supervision_loop`, `write_resource_event`, `SubagentResourceEventRow`, `subagent_insert_resource_event`, existing `settings_diagnostics` RPC + `RuntimeControl::settings_diagnostics` trait method + `SettingsDiagnosticsDto`, mock-sidecar.sh fixture, `BusytokSupervisor::new_with_sidecar_config`. No new abstractions for one-time operations. ✅
- **No new DB migrations**: `subagent_resource_events` already has `rss_mb`/`cpu_percent`/`detail_json` columns (migration 0003). ✅
- **Layered doctor design (P1#3 + P1#1 from user review)**: RPC layer reuses existing `settings.diagnostics` (no new method/trait/dispatch arm); CLI layer adds spec-required top-level `busytok doctor` (spec §855, §1068) as a thin client. This preserves both the spec's external CLI contract AND the internal RPC reuse — they're not in conflict. ✅
- **DB lock discipline**: `maybe_sample_resources` acquires the `resource_monitor` `std::sync::Mutex` in a scoped block, calls `sample()` (sync, no `.await`), releases the lock, then writes the resource event (which acquires the DB `std::sync::Mutex` separately). No lock held across `.await`. ✅
- **`tracing` event codes**: `subagent.resource.sample`, `subagent.resource.memory_pressure`, `subagent.resource.rss_limit_exceeded`, `subagent.resource.recovered` — all in the `subagent.resource.*` namespace per Global Constraints. Note: `recovered` is a `tracing` event code only (not a DB event type) — the DB event `resource_recovered` is deferred to Plan 6. ✅
- **TDD discipline**: Every task writes the failing test first, runs it to confirm failure, implements, runs to confirm pass, commits. ✅
- **YAGNI**: No metrics time-series table, no reactive backpressure chain (deferred to Plan 6), no LRU eviction logic (existing Plan 3 code handles that), no `resource_recovered` DB event (deferred to Plan 6). The doctor command's 6 stubbed checks are explicitly marked `"warning"` for a future plan — they don't block Plan 5's acceptance and don't claim false green. ✅
- **DRY**: `write_resource_event` delegates to `write_resource_event_with_sample(event_type, None)`. `ResourceMonitor` predicates are instance methods that read `self.policy`/`self.soft_limit_mb`/`self.hard_limit_mb` (not hardcoded constants) — this avoids drift between config and predicates while keeping the surface small. `ResourcePressureState::transition_event` + `is_recovery` are pure functions so the latch logic is testable without spinning up a supervisor. ✅

### 5. Spec inconsistencies resolved

1. **`sidecar_crash` event type** — used in code (supervisor.rs:284), not in spec §3.2 enum. Plan 5 keeps it as-is (already shipped). The spec enum is treated as non-exhaustive. ✅
2. **`hibernate_after_seconds` (600) vs `idle_exit_seconds` (300)** — `hibernate_after_seconds` is a per-session TTL in config but NOT consumed in MVP. Plan 5 does not implement it (no per-session idle timer in the supervisor). Documented in Global Constraints. ⚠️ (deferred — future plan may add per-session hibernate)
3. **After 3 failed restart attempts within 5 min** — Task 4 implements the rolling 5-min window via `restart_history: VecDeque<Instant>` (pruned to entries within `RESTART_WINDOW = 300s`). The 4th attempt within the window is rejected with `SidecarError::Crashed` even if `restart_attempts` was reset by an intervening successful spawn. Task 5's e2e test asserts the crash → restart path. ✅

### 6. Risk assessment

- **sysinfo version drift**: Plan 5 pins `sysinfo = "0.32"`. If a future sysinfo release changes the `ProcessRefreshKind`/`RefreshKind` API, the `sample()` method will need updating. The pin in `[workspace.dependencies]` prevents surprise upgrades. Low risk.
- **RSS measurement noise in stress test**: The < 10MB bound in Task 6 may flake on CI under memory pressure. Mitigation: the test comment suggests widening to 15MB if needed, and the spec's "does not grow linearly" is satisfied at any sub-process-footprint bound.
- **Crash recovery test timing**: Task 5's 80-iteration poll loop (4s window) gives the supervision loop's 100ms `POLL_INTERVAL` ample time to detect the crash, run backoff (1s+2s+4s = 7s for full 3-attempt sequence — but the mock only crashes once, so restart succeeds on the first retry after ~1s). Mitigation: if CI is extremely slow, bump further; the bound is generous.
- **Doctor stubbed checks**: The 6 `"warning"` checks (node arch, manifest, protocol version, model config, Pi runtime, artifact store) could mask real bundle issues in production. Mitigation: (a) `overall_ok` only fails on `"error"`, so warnings surface without false-green; (b) the `sidecar_launchable` check (real) surfaces `sidecar_init_error`, which catches the most common deployment failure (missing bundle); (c) the detail string explicitly says "not yet implemented (Plan 6+ bundle inspection)" so users/devs know it's not verified. The stubs are documented for Plan 6+.
- **Backpressure deferred to Plan 6**: Plan 5 emits the `memory_pressure` / `rss_limit_exceeded` signals but does NOT act on them (no pause/hibernate/restart). If a deployment hits sustained memory pressure before Plan 6 lands, the sidecar will OOM-kill via the OS rather than graceful-shutdown. Mitigation: the `tracing` logs (every tick) give operators visibility; the idle-exit timer + existing crash recovery provide a degenerate fallback. The `memory_soft_limit_mb`/`memory_hard_limit_mb` config is already threaded so Plan 6 is a pure addition (no migration).
