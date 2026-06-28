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
