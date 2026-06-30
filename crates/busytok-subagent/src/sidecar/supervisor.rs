//! Pi sidecar supervisor — owns the sidecar process lifecycle.
//!
//! Responsibilities (spec §5.4):
//! - Lazy spawn on first `ensure_started` (initialize handshake)
//! - Background supervision loop: crash detection (try_wait), idle-exit timer,
//!   health pinger
//! - Crash recovery: exponential backoff, restart-attempt cap, DB state
//!   reconciliation (tasks→failed, bindings→crashed, logical status→warm/cold)
//! - Graceful shutdown: prepare_hibernate all → adapter.shutdown → 10s grace
//!   → SIGKILL
//! - Resource event writes (`sidecar_start` / `sidecar_stop` / `sidecar_crash`
//!   / `sidecar_restart`) when a DB handle is attached
//!
//! The supervisor is constructed as `Arc<Self>` so that `SidecarHandle` and
//! the background supervision task share ownership. RPC calls lock the state
//! mutex only long enough to clone the client `Arc` and bump `last_activity`;
//! the actual RPC runs with the state lock released, serialized on the
//! client's own `tokio::Mutex`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{error, info, instrument, warn};

use busytok_config::SubagentResourcePolicyConfig;
use busytok_store::{Database, SubagentResourceEventRow};

use crate::pressure::{PressureAction, PressureGate, PressureResponder};
use crate::resource::{ResourceMonitor, ResourcePressureState, ResourceSample};
use crate::sidecar::client::SidecarRpcClient;
use crate::sidecar::config::SidecarConfig;
use crate::sidecar::protocol::PROTOCOL_VERSION;
use crate::sidecar::SidecarError;

/// Shared DB handle — `std::sync::Mutex` because the supervisor writes
/// resource events and crash-reconciliation synchronously (no `.await` held
/// across the lock). Mirrors the `SubagentManager` pattern.
pub type SharedDb = Arc<std::sync::Mutex<Database>>;

/// Read-only snapshot of a sidecar worker's state for the `runtime_status`
/// handler (spec §4 Phase 2 `workers[]`). Returned by
/// `PiSidecarSupervisor::worker_snapshot()`.
#[derive(Debug, Clone)]
pub struct WorkerSnapshot {
    /// `Running` while the child process is alive, `Stopped` otherwise
    /// (configured-but-not-running).
    pub state: WorkerState,
    /// OS pid of the sidecar child. `None` when `state == Stopped`.
    pub pid: Option<u32>,
    /// Seconds since the sidecar was spawned. `None` when `state == Stopped`.
    pub uptime_seconds: Option<u64>,
    /// Hot session count from the last `adapter.health` response cached in
    /// `SupervisorState::latest_hot_sessions`. `0` before the first tick.
    pub hot_sessions: u32,
    /// System memory usage percentage (0–100). Computed fresh on each call
    /// from the `ResourceMonitor`'s `sysinfo::System`. `None` when no
    /// monitor is attached (never in production — both constructors attach
    /// one).
    pub memory_used_pct: Option<u32>,
    /// Coarse pressure level derived from `ResourcePressureState` + the
    /// `PressureGate`'s last action. Maps the supervisor's internal
    /// Normal/Pressure/LimitExceeded latch to the spec's frontend-facing
    /// `normal/throttled/evicting/restarting` vocabulary.
    pub pressure_level: PressureLevel,
    /// Absolute ms (Unix epoch) when the cached `ResourceSample` was taken,
    /// via `busytok_domain::now_ms()`. `None` before the first resource
    /// sample tick. Enables the frontend freshness display (Task 6).
    pub sampled_at_ms: Option<i64>,
}

/// Worker lifecycle state for `WorkerSnapshot` (spec §4 Phase 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    Running,
    Stopped,
}

/// Frontend-facing pressure level for `WorkerSnapshot` (spec §4 Phase 2).
/// Derived from `ResourcePressureState` + `PressureGate::last_action()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureLevel {
    /// `ResourcePressureState::Normal`.
    Normal,
    /// `Pressure` state + queue paused (`PauseNewTasks` last action).
    Throttled,
    /// `Pressure` state + `HibernateLru` last action (evicting LRU session).
    Evicting,
    /// `LimitExceeded` state (`ForceKill`/`GracefulRestart` in flight).
    Restarting,
}

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);
/// Spec §5.4: rolling 5-min window for crash restart attempts.
const RESTART_WINDOW: Duration = Duration::from_secs(300);
/// Spec §5.4: max 3 crash-restarts per 5-min rolling window (fixed,
/// independent of `max_restart_attempts` which governs backoff only).
const MAX_CRASHES_PER_WINDOW: usize = 3;

pub struct PiSidecarSupervisor {
    config: SidecarConfig,
    state: Mutex<SupervisorState>,
    /// Dedicated spawn lock — held for the entire `spawn_internal` duration
    /// (spawn + adapter.initialize handshake) to prevent two concurrent
    /// `ensure_started` callers from each spawning a sidecar process.
    /// The state lock alone is insufficient because it is released between
    /// the double-check and the actual `cmd.spawn()` + handshake, leaving a
    /// window where a second caller sees `client.is_none()` and proceeds.
    spawn_lock: Mutex<()>,
    db: Option<SharedDb>,
    /// Resource monitor — None in unit tests that don't pass a policy.
    /// Mutex because `sample(&mut self)` mutates the internal `sysinfo::System`.
    resource_monitor: Option<std::sync::Mutex<ResourceMonitor>>,
    /// §8.3 step 2: pressure gate shared with `SubagentManager`. When the
    /// `PressureResponder` (Task 4) escalates, it sets the gate; the manager
    /// checks `is_paused()` at the top of `delegate()` to queue new tasks.
    #[allow(dead_code)] // Read via pressure_gate() accessor; Task 4 consumes it.
    pressure_gate: Option<Arc<PressureGate>>,
    /// Weak ref to the pressure responder — set AFTER construction via
    /// `set_pressure_responder`. Weak (not Arc) to break the reference
    /// cycle: supervisor → responder → executor → supervisor.
    /// The strong owner is `BusytokSupervisor.pressure_responder`.
    pressure_responder: std::sync::Mutex<Option<std::sync::Weak<PressureResponder>>>,
}

pub struct SupervisorState {
    child: Option<Child>,
    /// The RPC client is wrapped in `Arc<Mutex<…>>` so `call_rpc` can clone
    /// the Arc and release the state lock before performing the (potentially
    /// long) RPC call — avoids holding the state mutex across `.await`.
    client: Option<Arc<Mutex<SidecarRpcClient>>>,
    last_activity: tokio::time::Instant,
    restart_attempts: u32,
    /// Set true when the supervision loop is running; prevents double-spawn
    /// of the loop across concurrent `ensure_started` calls.
    supervision_started: bool,
    /// Generation counter to prevent double-loop races. Each `spawn_internal`
    /// that starts a new loop increments this. The loop captures the value
    /// at start and checks it each iteration — if it changed, a newer loop
    /// was spawned (after a shutdown+restart cycle) and this one is stale.
    generation: u64,
    /// Edge-trigger latch for resource pressure (spec §6.5: lifecycle
    /// boundaries only). Updated on EVERY transition (escalation OR recovery)
    /// so re-pressurization after recovery writes a fresh `memory_pressure`
    /// event. DB events fire ONLY on escalation; recovery logs to tracing.
    resource_pressure_state: ResourcePressureState,
    /// Rolling window of crash timestamps (spec §5.4: "max 3 attempts per
    /// 5 min"). Pruned in `spawn_internal` before checking the cap. NOT
    /// reset on successful spawn (unlike `restart_attempts`) — the window
    /// is the hard cap, `restart_attempts` is for backoff calculation.
    pub restart_history: VecDeque<tokio::time::Instant>,
    // ── Phase 2 monitoring state (Task 3) ─────────────────────────
    /// `Some(Instant)` set in `spawn_internal` after a successful spawn;
    /// cleared in `shutdown_internal`. Read by `worker_snapshot()` to
    /// compute `uptime_seconds`. `None` while stopped.
    spawned_at: Option<tokio::time::Instant>,
    /// Latest `ResourceSample` cached by `maybe_sample_resources`. Read by
    /// the future Task 6 `runtime_status` handler (which may expose
    /// additional sample fields like `sidecar_rss_mb` / `sidecar_cpu_percent`
    /// beyond the `WorkerSnapshot` surface). `None` before the first tick.
    latest_sample: Option<ResourceSample>,
    /// Absolute ms (Unix epoch, `busytok_domain::now_ms()`) when
    /// `latest_sample` was captured. `None` before the first tick. Stored
    /// SEPARATELY from the sample (rather than as a field on `ResourceSample`)
    /// because `ResourceSample` is a pure data struct shared with the DB
    /// event writer. Enables the frontend freshness display (Task 6).
    latest_sample_at_ms: Option<i64>,
    /// Cached `hot_session_count` from the last `adapter.health` response
    /// (the value passed to `ResourceMonitor::sample()`). Surfaced as
    /// `WorkerSnapshot::hot_sessions` so the handler doesn't need to RPC
    /// the sidecar.
    latest_hot_sessions: u32,
}

impl PiSidecarSupervisor {
    pub fn new(config: SidecarConfig, db: Option<SharedDb>) -> Arc<Self> {
        // Default policy — production callers pass settings via
        // `with_resource_policy` (added below) when they have a
        // SubagentResourcePolicyConfig. For the default-constructed path
        // (tests), we use the spec-default policy so the monitor still works.
        // `new` does NOT accept a pressure gate — the runtime constructs the
        // gate in `construct_sidecar` and threads it through
        // `with_resource_policy`. Tests that need a gate construct one
        // explicitly and pass it to `with_resource_policy`.
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
                generation: 0,
                resource_pressure_state: ResourcePressureState::Normal,
                restart_history: VecDeque::new(),
                spawned_at: None,
                latest_sample: None,
                latest_sample_at_ms: None,
                latest_hot_sessions: 0,
            }),
            spawn_lock: Mutex::new(()),
            db,
            resource_monitor: Some(std::sync::Mutex::new(monitor)),
            pressure_gate: None,
            pressure_responder: std::sync::Mutex::new(None),
        })
    }

    /// Construct with an explicit resource policy (used by the runtime
    /// supervisor which has the deserialized SubagentResourcePolicyConfig).
    /// The `pressure_gate` is the new 4th param — `Some` when constructed by
    /// `construct_sidecar` (production path), `None` in tests that don't
    /// exercise the gate. The gate is shared (`Arc`) with `SubagentManager`
    /// so the manager's `delegate()` sees `is_paused()` flips immediately.
    pub fn with_resource_policy(
        config: SidecarConfig,
        db: Option<SharedDb>,
        policy: SubagentResourcePolicyConfig,
        pressure_gate: Option<Arc<PressureGate>>,
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
                generation: 0,
                resource_pressure_state: ResourcePressureState::Normal,
                restart_history: VecDeque::new(),
                spawned_at: None,
                latest_sample: None,
                latest_sample_at_ms: None,
                latest_hot_sessions: 0,
            }),
            spawn_lock: Mutex::new(()),
            db,
            resource_monitor: Some(std::sync::Mutex::new(monitor)),
            pressure_gate,
            pressure_responder: std::sync::Mutex::new(None),
        })
    }

    /// Access the resolved sidecar config. Used by `SidecarTaskExecutor` to
    /// read `harness_name` (for `find_hot_binding_by_session`) during the
    /// eviction flow.
    pub fn config(&self) -> &SidecarConfig {
        &self.config
    }

    /// Non-blocking check of whether the sidecar child is currently running.
    /// Public so the runtime crate's doctor check (`run_subagent_doctor`) can
    /// inspect the child without acquiring the async state lock (which would
    /// require an `.await` and isn't safe from a sync context). Also used by
    /// `crate::pressure::PressureResponder` (Task 4).
    pub fn try_is_running(&self) -> bool {
        self.state
            .try_lock()
            .map(|s| s.child.as_ref().map(|c| c.id().is_some()).unwrap_or(false))
            .unwrap_or(false)
    }

    /// Read-only snapshot of the worker's current state for the
    /// `runtime_status` handler (spec §4 Phase 2 `workers[]`). ALWAYS returns
    /// `Some` — even when the sidecar is not running — so
    /// "configured-but-stopped" sidecars stay observable. Only the Task 6
    /// handler returns `workers: []` (when `sidecar_supervisor` is `None` /
    /// not configured).
    ///
    /// `state=Stopped` with `pid=None`/`uptime_seconds=None` represents a
    /// configured-but-not-running sidecar. `sampled_at_ms=None` represents
    /// "no resource sample yet" (freshly-constructed supervisor or before
    /// the first supervision-loop tick).
    ///
    /// `memory_used_pct` is computed fresh on each call from the
    /// `ResourceMonitor`'s `sysinfo::System`; the other fields are stamped
    /// by the supervision loop and may be up to `monitor_interval_seconds`
    /// (default 30s) stale — that's the design decision (stamped freshness,
    /// NOT "same moment").
    pub async fn worker_snapshot(&self) -> Option<WorkerSnapshot> {
        let state = self.state.lock().await;
        let is_running = state
            .child
            .as_ref()
            .map(|c| c.id().is_some())
            .unwrap_or(false);
        let worker_state = if is_running {
            WorkerState::Running
        } else {
            WorkerState::Stopped
        };
        let pid = if is_running {
            state.child.as_ref().and_then(|c| c.id())
        } else {
            None
        };
        let uptime_seconds = if is_running {
            state.spawned_at.map(|t| t.elapsed().as_secs())
        } else {
            None
        };
        let hot_sessions = state.latest_hot_sessions;
        // memory_used_pct is computed fresh from the ResourceMonitor's
        // sysinfo::System (refreshed at construction + on each sample tick).
        // Falls back to None if no monitor is attached (never in production
        // — both constructors attach one) or if the std Mutex is poisoned.
        let memory_used_pct = self
            .resource_monitor
            .as_ref()
            .and_then(|m| m.lock().ok().and_then(|g| g.memory_used_pct()));
        // Pressure level mapping (spec §4 Phase 2 normal/throttled/evicting/
        // restarting). The supervisor's internal latch has 3 states; the
        // PressureGate's last_action disambiguates the Pressure tier.
        let pressure_level = match state.resource_pressure_state {
            ResourcePressureState::Normal => PressureLevel::Normal,
            ResourcePressureState::Pressure => {
                match self.pressure_gate.as_ref().and_then(|g| g.last_action()) {
                    Some(PressureAction::HibernateLru) => PressureLevel::Evicting,
                    _ => PressureLevel::Throttled,
                }
            }
            ResourcePressureState::LimitExceeded => PressureLevel::Restarting,
        };
        Some(WorkerSnapshot {
            state: worker_state,
            pid,
            uptime_seconds,
            hot_sessions,
            memory_used_pct,
            pressure_level,
            sampled_at_ms: state.latest_sample_at_ms,
        })
    }

    /// §8.3 step 5: force-kill the sidecar child (SIGKILL, no graceful
    /// shutdown). Used by `PressureResponder` (Task 4) when graceful restart
    /// fails or when RSS exceeds the hard limit. After kill, reconciles DB
    /// state as if the sidecar crashed (so the next `ensure_started` sees a
    /// clean store). `pub(crate)` so the responder can call it without going
    /// through the public `shutdown()` (which would attempt graceful
    /// shutdown first).
    #[allow(dead_code)] // Task 4 (PressureResponder::respond) consumes this.
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

    /// Graceful shutdown without re-acquiring the public `shutdown()` wrapper
    /// — used by `PressureResponder` (Task 4), the supervision loop's idle
    /// exit path, and the runtime crate's doctor check
    /// (`run_subagent_doctor` protocol_version probe). Public so the runtime
    /// crate can call it after a short-lived probe.
    pub async fn shutdown_internal(&self) -> Result<(), SidecarError> {
        let client = { self.state.lock().await.client.take() };
        if let Some(client) = &client {
            // Best-effort: ask the sidecar to prepare all hot sessions for
            // hibernate (Plan 3 tracks per-session state; Plan 2 uses `all`).
            // Plan 3: consume memory_delta from the response.
            let _ = client
                .lock()
                .await
                .call_with_timeout(
                    "session.prepare_hibernate",
                    serde_json::json!({"all": true}),
                    Duration::from_secs(5),
                )
                .await;
            // adapter.shutdown — sidecar should exit 0 after responding.
            let _ = client
                .lock()
                .await
                .call_with_timeout(
                    "adapter.shutdown",
                    serde_json::json!({}),
                    Duration::from_secs(5),
                )
                .await;
        }
        // Kill child with 10s grace (spec §5.4). The sidecar should have
        // exited on adapter.shutdown; this is the fallback.
        // Reset supervision_started so the next spawn_internal re-spawns
        // the supervision loop — without this, an external shutdown()
        // followed by a quick ensure_started() would skip loop revival
        // because the old loop hasn't yet observed child.is_none().
        let child = {
            let mut state = self.state.lock().await;
            state.supervision_started = false;
            // Phase 2: clear spawn time so `worker_snapshot()` reports
            // `Stopped` with `uptime_seconds=None` after shutdown.
            state.spawned_at = None;
            state.child.take()
        };
        if let Some(mut child) = child {
            match tokio::time::timeout(SHUTDOWN_GRACE, child.wait()).await {
                Ok(Ok(_status)) => {}
                Ok(Err(_)) | Err(_) => {
                    warn!(
                        event_code = "subagent.sidecar.shutdown_kill",
                        "grace period expired or wait failed, SIGKILL"
                    );
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
            }
        }
        // Spec §3.3 end-to-end: after the worker process is dead, release hot
        // bindings (status='closed') and roll back logical status to warm/cold
        // so the store never says "hot" with no worker running. This mirrors
        // `reconcile_crash` but uses 'closed' (graceful) instead of 'crashed'.
        // Synchronous — no `.await` held across the DB lock.
        self.reconcile_shutdown();
        info!(event_code = "subagent.sidecar.stop", "sidecar shut down");
        self.write_resource_event("sidecar_stop");
        Ok(())
    }

    /// Converge DB state after a sidecar crash (spec §3.3 + §5.4). Calls
    /// `subagent_reconcile_sidecar_crash` with the harness name from config.
    /// Synchronous (no `.await` held across the DB lock).
    /// `pub(crate)` so `PressureResponder::force_kill` (Task 4) can call it
    /// after killing the child.
    pub(crate) fn reconcile_crash(&self) {
        if let Some(db) = &self.db {
            let db = db.lock().expect("subagent db lock poisoned");
            match db.subagent_reconcile_sidecar_crash(&self.config.harness_name) {
                Ok(counts) => {
                    warn!(
                        event_code = "subagent.sidecar.crash_reconciled",
                        tasks_failed = counts.tasks_failed,
                        bindings_released = counts.bindings_released,
                        status_rolled_back = counts.status_rolled_back,
                        "sidecar crash reconciled"
                    );
                }
                Err(e) => {
                    warn!(
                        event_code = "subagent.sidecar.crash_reconcile_failed",
                        error = %e,
                        "crash reconciliation failed; store may be half-converged"
                    );
                }
            }
        }
    }

    /// Write a row to `subagent_resource_events` if a DB handle is attached.
    /// `pub(crate)` so `PressureResponder::force_kill` (Task 4) can write a
    /// `sidecar_crash` event after killing the child.
    pub(crate) fn write_resource_event(&self, event_type: &str) {
        self.write_resource_event_with_sample(event_type, None);
    }

    /// Access the pressure gate. `pub(crate)` so `PressureResponder` (Task 4)
    /// can call `set_action()` on it. Returns `None` when no gate is attached
    /// (default-constructed supervisors in unit tests).
    #[allow(dead_code)] // Task 4 (PressureResponder::respond) consumes this.
    pub(crate) fn pressure_gate(&self) -> Option<&Arc<PressureGate>> {
        self.pressure_gate.as_ref()
    }

    /// Set the pressure responder — called by `BusytokSupervisor` (in the
    /// runtime crate) after both supervisor + responder are constructed.
    /// Stores a `Weak` so the supervision loop can upgrade it without
    /// creating a reference cycle (supervisor → responder → executor →
    /// supervisor). Public so the runtime crate can call it across crate
    /// boundaries; the field itself remains private.
    pub fn set_pressure_responder(&self, responder: Arc<PressureResponder>) {
        *self.pressure_responder.lock().unwrap() = Some(Arc::downgrade(&responder));
    }

    /// Upgrade the weak responder ref — returns `None` if the responder was
    /// dropped (BusytokSupervisor gone). Called by the supervision loop
    /// (Task 4) when pressure transitions occur.
    #[allow(dead_code)] // Task 4 (PressureResponder::respond) consumes this.
    pub(crate) fn pressure_responder(&self) -> Option<Arc<PressureResponder>> {
        self.pressure_responder
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|w| w.upgrade())
    }

    /// Lazy-spawn the sidecar if not running, then return a handle.
    /// If the sidecar crashed previously, applies exponential backoff before
    /// respawning (capped at `max_restart_attempts`).
    #[instrument(skip(self), fields(event_code = "subagent.sidecar.ensure_started"))]
    pub async fn ensure_started(self: &Arc<Self>) -> Result<SidecarHandle, SidecarError> {
        let needs_spawn = {
            let state = self.state.lock().await;
            state.client.is_none()
                || state
                    .child
                    .as_ref()
                    .map(|c| c.id().is_none())
                    .unwrap_or(true)
        };
        if needs_spawn {
            self.spawn_internal().await?;
        }
        Ok(SidecarHandle {
            supervisor: Arc::clone(self),
        })
    }

    async fn spawn_internal(self: &Arc<Self>) -> Result<(), SidecarError> {
        // Acquire the dedicated spawn lock for the entire spawn+handshake
        // duration. This prevents two concurrent `ensure_started` callers
        // from each spawning a sidecar process — the state lock alone is
        // insufficient because it is released between the double-check and
        // the actual `cmd.spawn()` + `adapter.initialize` handshake.
        let _spawn_guard = self.spawn_lock.lock().await;

        // Exponential backoff if this is a restart after a crash.
        let backoff = {
            let mut state = self.state.lock().await;
            // Re-check under the state lock: a previous `spawn_internal`
            // (which held the spawn_lock) may have already installed a
            // client+child. `child.id().is_some()` guards against the case
            // where a child was set but has since exited (id() returns None
            // after wait()/kill()).
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
            // The cap is a FIXED 3 per spec §5.4, NOT tied to
            // `max_restart_attempts` (which governs backoff only). Tying
            // them would let a deployment that raises `max_restart_attempts`
            // silently allow more crashes per 5 min, violating the spec's
            // safety invariant, and would break `max=0` (first spawn
            // blocked because `len() >= 0` is always true).
            let now = tokio::time::Instant::now();
            state
                .restart_history
                .retain(|t| now.duration_since(*t) < RESTART_WINDOW);
            if state.restart_history.len() >= MAX_CRASHES_PER_WINDOW {
                return Err(SidecarError::Crashed(format!(
                    "max crashes per 5-min window ({MAX_CRASHES_PER_WINDOW}) exceeded ({} recent crashes)",
                    state.restart_history.len()
                )));
            }
            if state.restart_attempts > self.config.max_restart_attempts {
                return Err(SidecarError::Crashed(format!(
                    "max restart attempts ({}) exceeded",
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
        if !backoff.is_zero() {
            warn!(
                event_code = "subagent.sidecar.restart_backoff",
                backoff_ms = backoff.as_millis() as u64,
                "sleeping before restart"
            );
            tokio::time::sleep(backoff).await;
        }

        let mut cmd = Command::new(&self.config.node_binary);
        cmd.arg(&self.config.bundle_path);
        cmd.envs(&self.config.env);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            error!(event_code = "subagent.sidecar.spawn_failed", error = %e);
            SidecarError::Spawn(e.to_string())
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SidecarError::Spawn("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SidecarError::Spawn("no stdout".into()))?;
        // Take stderr and spawn a background line-reader that forwards each
        // line to `tracing`. Without this the piped stderr buffer fills up
        // and blocks the child process — manifesting as random turn_auto /
        // health timeouts. The TS sidecar writes to stderr on error paths
        // (rpc.ts line handler / stop callback exceptions). The task is
        // detached: it exits naturally when the pipe closes (child exits).
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                loop {
                    match reader.next_line().await {
                        Ok(Some(line)) => {
                            warn!(
                                event_code = "subagent.sidecar.stderr",
                                "sidecar stderr: {line}"
                            );
                        }
                        Ok(None) => break, // EOF — child closed stderr
                        Err(e) => {
                            warn!(
                                event_code = "subagent.sidecar.stderr_read_error",
                                error = %e,
                                "stderr reader error"
                            );
                            break;
                        }
                    }
                }
            });
        }
        let mut client = SidecarRpcClient::new(stdin, stdout);
        let init = client
            .call(
                "adapter.initialize",
                serde_json::json!({"protocol_version": PROTOCOL_VERSION}),
            )
            .await?;
        let pv = init
            .get("protocol_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if pv != PROTOCOL_VERSION as u64 {
            return Err(SidecarError::Spawn(format!(
                "protocol mismatch: expected {PROTOCOL_VERSION}, got {pv}"
            )));
        }
        let is_restart = {
            let mut state = self.state.lock().await;
            let is_restart = state.restart_attempts > 0;
            state.child = Some(child);
            state.client = Some(Arc::new(Mutex::new(client)));
            state.last_activity = tokio::time::Instant::now();
            state.restart_attempts = 0; // reset on successful spawn
            state.spawned_at = Some(tokio::time::Instant::now()); // Phase 2: uptime
            if !state.supervision_started {
                state.supervision_started = true;
                // Bump generation so any stale loop from a prior lifecycle
                // detects the change and exits (prevents double-loop when
                // shutdown_internal + quick ensure_started races the old
                // loop's sleep). The new loop captures this value and
                // checks it each iteration.
                state.generation = state.generation.wrapping_add(1);
                let generation = state.generation;
                let self_clone = Arc::clone(self);
                tokio::spawn(async move {
                    self_clone.supervision_loop(generation).await;
                });
            }
            is_restart
        };
        info!(
            event_code = "subagent.sidecar.start",
            sidecar_version = init
                .get("sidecar_version")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
            is_restart,
            "sidecar initialized"
        );
        self.write_resource_event(if is_restart {
            "sidecar_restart"
        } else {
            "sidecar_start"
        });
        Ok(())
    }

    /// Background loop: crash watcher + health pinger + idle timer +
    /// resource sampling. Exits when the child is taken (shutdown) or
    /// crashes (handled, then exits — next `ensure_started` respawns and
    /// re-spawns the loop).
    ///
    /// `my_generation` is the generation captured at spawn time. Each
    /// iteration checks `state.generation` — if it changed, a newer loop
    /// was spawned (after a shutdown+restart cycle raced this loop's
    /// sleep) and this one is stale; exit to avoid double-loop.
    async fn supervision_loop(self: Arc<Self>, my_generation: u64) {
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
            // Stale-loop guard: a newer spawn_internal bumped the generation
            // (after shutdown_internal reset supervision_started and a quick
            // ensure_started re-spawned). Exit instead of double-running.
            if state.generation != my_generation {
                return;
            }
            if state.child.is_none() {
                // Shutdown took the child while we slept. Reset the flag so
                // the next spawn_internal re-spawns this loop.
                state.supervision_started = false;
                return;
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
                state.restart_history.push_back(tokio::time::Instant::now());
                // Reset so the next spawn_internal re-spawns this loop.
                state.supervision_started = false;
                warn!(
                    event_code = "subagent.sidecar.crash",
                    exit = ?status,
                    attempts = state.restart_attempts,
                    recent_crashes = state.restart_history.len(),
                    "sidecar crashed"
                );
                drop(state);
                // Spec §3.3 + §5.4: converge DB state before returning so the
                // next ensure_started sees a consistent store. Failure to
                // reconcile is logged but does NOT block restart — a
                // half-converged store is recoverable on the next task; a
                // blocked restart is worse.
                self.reconcile_crash();
                self.write_resource_event("sidecar_crash");
                return; // loop exits; next ensure_started respawns
            }
            let sidecar_pid = state.child.as_ref().and_then(|c| c.id());
            let last_activity = state.last_activity;
            // --- idle exit timer ---
            // idle_exit_seconds=0 means "exit immediately when idle" (test-
            // friendly). A large value effectively disables idle exit.
            let idle_threshold = Duration::from_secs(self.config.idle_exit_seconds);
            let idle = last_activity.elapsed();
            if idle > idle_threshold {
                // Reset so the next spawn_internal re-spawns this loop.
                state.supervision_started = false;
                drop(state);
                info!(
                    event_code = "subagent.sidecar.idle_exit",
                    "idle exit triggered"
                );
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
    ///   Pressure → Normal        : info log ONLY (no DB event — tracing-only
    ///                              by design: spec §3.2 enum has no
    ///                              `resource_recovered` event type; the latch
    ///                              still updates on recovery so re-pressurization
    ///                              writes a fresh `memory_pressure` event)
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
        let (queued, running) = match &self.db {
            Some(db) => db
                .lock()
                .unwrap()
                .subagent_task_counts_by_status()
                .unwrap_or((0, 0)),
            None => (0, 0),
        };
        let sample = {
            let mut guard = match monitor.lock() {
                Ok(g) => g,
                Err(_) => return, // poisoned — skip this tick
            };
            guard.sample(sidecar_pid, hot_sessions, queued, running)
        };
        // Time-series signal — logged EVERY tick (level-triggered).
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
        // Phase 2: cache the sample + absolute timestamp + hot_sessions for
        // `worker_snapshot()`. The timestamp is captured via
        // `busytok_domain::now_ms()` (the same clock used by
        // `ReadEnvelopeDto::generated_at_ms`) so the frontend can display
        // freshness independently of the supervision loop's poll cadence.
        // The sample may be up to `monitor_interval_seconds` (default 30s)
        // stale by the time `worker_snapshot()` reads it — that's
        // acceptable per the design decision (stamped freshness, NOT
        // "same moment").
        {
            let mut state = self.state.lock().await;
            state.latest_sample = Some(sample.clone());
            state.latest_sample_at_ms = Some(busytok_domain::now_ms());
            state.latest_hot_sessions = hot_sessions;
        }
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
        // event — `resource_recovered` is not in spec §3.2's enum). The latch
        // state updates on every transition so re-pressurization after
        // recovery writes a fresh event.
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
                "resource pressure recovered to normal"
            );
            // §8.3 pressure response: clear the gate on recovery so the
            // queue unpauses. Run AFTER the tracing log so the observability
            // signal is recorded before the action.
            self.invoke_pressure_responder(PressureAction::Resume);
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
                    "entered memory pressure (pausing queue + hibernating LRU)"
                );
            }
            "rss_limit_exceeded" => {
                error!(
                    event_code = "subagent.resource.rss_limit_exceeded",
                    sidecar_rss_mb = ?sample.sidecar_rss_mb,
                    hard_limit_mb = self.config.memory_hard_limit_mb,
                    "sidecar RSS exceeded hard limit (force-killing)"
                );
            }
            _ => unreachable!("transition_event only returns known escalation event types"),
        }
        self.write_resource_event_with_sample(event_type, Some(&sample));

        // §8.3 pressure response actions. Run AFTER the DB event write so the
        // observability signal is recorded before the action.
        //
        // Action mapping (verified against `resource.rs` predicates):
        // - `exceeds_hard` → ForceKill (§8.3 step 5). Checked first because
        //   hard limit is the most severe — even if old_state was already
        //   LimitExceeded, re-kill (the sidecar may have grown beyond the
        //   limit without restarting).
        // - `exceeds_soft` (but NOT hard) → GracefulRestart (§8.3 steps 3-4).
        //   Checked as a SEPARATE predicate, NOT via state transition —
        //   `under_pressure || exceeds_soft` both fold to `Pressure` state,
        //   but only `exceeds_soft` warrants graceful restart.
        // - `new_state == Pressure && old_state == Normal` → PauseNewTasks
        //   (§8.3 steps 1-2). The `PauseNewTasks` arm also calls `evict_lru`,
        //   folding step 1 into step 2 (spec intent: steps 1+2 happen
        //   together on pressure entry). Only fires on the Normal→Pressure
        //   transition (edge-triggered) — sustained Pressure without
        //   soft/hard exceeded does NOT re-pause.
        // - Recovery → Resume (handled above in the `is_recovery` branch).
        let action = if exceeds_hard {
            Some(PressureAction::ForceKill)
        } else if exceeds_soft {
            Some(PressureAction::GracefulRestart)
        } else if new_state == ResourcePressureState::Pressure
            && old_state == ResourcePressureState::Normal
        {
            Some(PressureAction::PauseNewTasks)
        } else {
            None
        };
        if let Some(action) = action {
            self.invoke_pressure_responder(action);
        }
    }

    /// Upgrade the weak responder ref (if set) and spawn `respond(action)`
    /// on a detached task. Spawning avoids blocking the supervision loop on
    /// the (potentially slow) escalation chain (e.g. `force_kill` waits for
    /// the child to exit, `prepare_hibernate_all` is an RPC). The
    /// responder's `in_flight` Mutex deduplicates concurrent escalations.
    fn invoke_pressure_responder(&self, action: PressureAction) {
        if let Some(responder) = self.pressure_responder() {
            tokio::spawn(async move {
                responder.respond(action).await;
            });
        }
    }

    /// Converge DB state after a graceful sidecar shutdown (spec §3.3).
    /// Releases hot bindings (`status='closed'`, NOT `'crashed'`) and rolls
    /// back logical subagent status to `warm`/`cold` for the affected set.
    /// Synchronous (no `.await` held across the DB lock) — the DB lock is a
    /// `std::sync::Mutex`, distinct from `self.state` (`tokio::sync::Mutex`);
    /// acquire it, run the sync store function, release it, with no `.await`
    /// in between.
    fn reconcile_shutdown(&self) {
        if let Some(db) = &self.db {
            let db = db.lock().expect("subagent db lock poisoned");
            match db.subagent_release_hot_bindings_for_shutdown(&self.config.harness_name) {
                Ok(counts) => {
                    info!(
                        event_code = "subagent.sidecar.shutdown_reconciled",
                        bindings_released = counts.bindings_released,
                        status_rolled_back = counts.status_rolled_back,
                        "sidecar shutdown reconciled"
                    );
                }
                Err(e) => {
                    warn!(
                        event_code = "subagent.sidecar.shutdown_reconcile_failed",
                        error = %e,
                        "shutdown reconciliation failed; store may be half-converged"
                    );
                }
            }
        }
    }

    /// Perform one RPC call. Locks state only to clone the client Arc and bump
    /// `last_activity`; the RPC itself runs with the state lock released.
    #[instrument(skip(self, params), fields(method = %method))]
    async fn call_rpc(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, SidecarError> {
        let client = {
            let mut state = self.state.lock().await;
            state.last_activity = tokio::time::Instant::now();
            state
                .client
                .clone()
                .ok_or_else(|| SidecarError::Crashed("sidecar not running".to_string()))?
        };
        // State lock released — RPC serialized on the client's own mutex.
        let mut guard = client.lock().await;
        guard
            .call_with_timeout(method, params, self.config.task_timeout)
            .await
    }

    /// Graceful shutdown: prepare_hibernate all → adapter.shutdown → 10s grace
    /// → SIGKILL. Emits `sidecar_stop` resource event.
    #[instrument(skip(self))]
    pub async fn shutdown(&self) -> Result<(), SidecarError> {
        self.shutdown_internal().await
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
                        (
                            s.sidecar_rss_mb,
                            s.sidecar_cpu_percent,
                            Some(detail.to_string()),
                        )
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

    /// Test-only accessor for the supervisor state. Used by integration
    /// tests that need to pre-populate `restart_history` to test the 5-min
    /// rolling window limiter without driving the full crash/restart cycle.
    #[doc(hidden)]
    pub fn state_for_test(&self) -> &Mutex<SupervisorState> {
        &self.state
    }
}

pub struct SidecarHandle {
    supervisor: Arc<PiSidecarSupervisor>,
}

impl std::fmt::Debug for SidecarHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SidecarHandle").finish_non_exhaustive()
    }
}

impl SidecarHandle {
    pub async fn health(&self) -> Result<serde_json::Value, SidecarError> {
        self.supervisor
            .call_rpc("adapter.health", serde_json::json!({}))
            .await
    }

    pub async fn turn_auto(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, SidecarError> {
        self.supervisor.call_rpc("session.turn_auto", params).await
    }

    /// Prepare a specific session for hibernate (spec §4.4 eviction flow).
    /// Returns `{ memory_delta, stats }`.
    pub async fn prepare_hibernate(
        &self,
        adapter_session_id: &str,
    ) -> Result<serde_json::Value, SidecarError> {
        self.supervisor
            .call_rpc(
                "session.prepare_hibernate",
                serde_json::json!({ "adapter_session_id": adapter_session_id }),
            )
            .await
    }

    /// Close a session (spec §4.4 eviction flow, final step).
    pub async fn close(&self, adapter_session_id: &str) -> Result<serde_json::Value, SidecarError> {
        self.supervisor
            .call_rpc(
                "session.close",
                serde_json::json!({ "adapter_session_id": adapter_session_id }),
            )
            .await
    }

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
}
