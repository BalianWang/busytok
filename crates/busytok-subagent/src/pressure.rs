//! Pressure response gate — shared signal between `PiSidecarSupervisor`
//! (writer) and `SubagentManager` (reader) for spec §8.3 backpressure.
//!
//! The supervisor sets the gate when resource pressure escalates; the
//! manager checks `is_paused()` at the top of `delegate()` to block new
//! task creation (§8.3 step 2).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::sidecar::{PiSidecarSupervisor, SidecarTaskExecutor};

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
        matches!(
            self,
            Self::PauseNewTasks | Self::GracefulRestart | Self::ForceKill
        )
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

/// Drives the §8.3 5-step escalation chain. Strong-owned by
/// `BusytokSupervisor.pressure_responder`. Holds `Weak` refs to supervisor
/// and executor to break the reference cycle (supervisor → responder →
/// executor → supervisor would be a cycle if any were `Arc`).
///
/// **In-flight deduplication (Finding 3 fix):** `respond()` acquires a
/// `tokio::Mutex` guard. If another `respond()` is already running, the
/// caller skips (does NOT wait) — this prevents concurrent
/// `GracefulRestart`/`ForceKill` from racing on `prepare_hibernate_all` /
/// `shutdown_internal` / `force_kill`.
///
/// **ForceKill recovery (Finding 2 fix):** After `force_kill()`, the
/// responder clears the gate to `Resume`. This allows the next `delegate()`
/// to call `ensure_started()` which lazy-spawns a fresh sidecar. Without
/// this, the system would deadlock in paused state with no path to restart.
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
        Self {
            supervisor,
            executor,
            gate,
            in_flight: Mutex::new(()),
        }
    }

    /// Access the underlying gate (used by wiring + tests).
    pub fn gate(&self) -> &Arc<PressureGate> {
        &self.gate
    }

    /// Best-effort upgrade of the supervisor weak ref (used by wiring +
    /// tests). Returns `None` if the supervisor was dropped.
    pub fn supervisor(&self) -> Option<Arc<PiSidecarSupervisor>> {
        self.supervisor.upgrade()
    }

    /// Best-effort upgrade of the executor weak ref (used by wiring +
    /// tests). Returns `None` if the executor was dropped.
    pub fn executor(&self) -> Option<Arc<SidecarTaskExecutor>> {
        self.executor.upgrade()
    }

    /// Execute an escalation step. Called by the supervision loop on
    /// pressure-state transitions or soft/hard-limit detection.
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
            warn!(
                event_code = "subagent.pressure.supervisor_dropped",
                "supervisor dropped — cannot respond"
            );
            return;
        };
        let Some(executor) = self.executor.upgrade() else {
            warn!(
                event_code = "subagent.pressure.executor_dropped",
                "executor dropped — cannot respond"
            );
            return;
        };
        match action {
            PressureAction::Resume => {
                self.gate.set_action(PressureAction::Resume);
                info!(event_code = "subagent.pressure.resume", "pressure cleared");
            }
            PressureAction::HibernateLru => {
                info!(
                    event_code = "subagent.pressure.hibernate_lru",
                    "§8.3 step 1: hibernate LRU"
                );
                if let Err(e) = executor.evict_lru().await {
                    warn!(
                        event_code = "subagent.pressure.hibernate_lru_failed",
                        error = %e
                    );
                }
                self.gate.set_action(PressureAction::HibernateLru);
            }
            PressureAction::PauseNewTasks => {
                info!(
                    event_code = "subagent.pressure.pause",
                    "§8.3 step 2: pause new tasks"
                );
                self.gate.set_action(PressureAction::PauseNewTasks);
                let _ = executor.evict_lru().await;
            }
            PressureAction::GracefulRestart => {
                info!(
                    event_code = "subagent.pressure.graceful_restart",
                    "§8.3 steps 3-4: graceful restart"
                );
                self.gate.set_action(PressureAction::GracefulRestart);
                // Step 4: prepare_hibernate all before restart.
                match supervisor.ensure_started().await {
                    Ok(handle) => {
                        if let Err(e) = handle.prepare_hibernate_all().await {
                            warn!(
                                event_code = "subagent.pressure.prepare_hibernate_failed",
                                error = %e
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            event_code = "subagent.pressure.ensure_started_failed",
                            error = %e,
                            "cannot prepare_hibernate_all — sidecar not running"
                        );
                    }
                }
                // Step 3: graceful shutdown (next ensure_started respawns).
                if let Err(e) = supervisor.shutdown_internal().await {
                    warn!(
                        event_code = "subagent.pressure.shutdown_failed",
                        error = %e,
                        "graceful shutdown failed — escalating to force kill"
                    );
                    // Inline ForceKill escalation (avoids async recursion which
                    // would require `Box::pin`). The `_guard` is still held —
                    // that's fine here because we don't re-enter `respond()`;
                    // we run the kill directly.
                    error!(
                        event_code = "subagent.pressure.force_kill",
                        "§8.3 step 5: force kill (escalated from graceful restart)"
                    );
                    self.gate.set_action(PressureAction::ForceKill);
                    supervisor.force_kill().await;
                    // CRITICAL: clear gate after force kill so the next
                    // delegate() can call ensure_started() which lazy-spawns
                    // a fresh sidecar (Finding 2 fix).
                    self.gate.set_action(PressureAction::Resume);
                    info!(
                        event_code = "subagent.pressure.force_kill_complete",
                        "force kill done, gate cleared — next delegate will lazy-restart"
                    );
                    return;
                }
                // Restart succeeded — clear gate so new tasks can proceed.
                self.gate.set_action(PressureAction::Resume);
            }
            PressureAction::ForceKill => {
                error!(
                    event_code = "subagent.pressure.force_kill",
                    "§8.3 step 5: force kill"
                );
                self.gate.set_action(PressureAction::ForceKill);
                supervisor.force_kill().await;
                // CRITICAL: clear gate after force kill so the next delegate()
                // can call ensure_started() which lazy-spawns a fresh sidecar.
                // Without this, the system deadlocks in paused state with no
                // path to restart (Finding 2 fix).
                self.gate.set_action(PressureAction::Resume);
                info!(
                    event_code = "subagent.pressure.force_kill_complete",
                    "force kill done, gate cleared — next delegate will lazy-restart"
                );
            }
        }
    }
}
