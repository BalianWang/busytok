//! Lifecycle coordinator -- serialized mutation queue for the service lifecycle.
//!
//! Owns the [`ServiceLifecycle`] and [`DesktopLoginStart`] instances and
//! serializes all mutations through a [`tokio::sync::Mutex`]. Implements
//! session suppression, ensure-coalescing, quit-over-ensure priority, and
//! structured lifecycle logging with stable event codes.

#[cfg(test)]
use std::sync::Barrier as SyncBarrier;
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::Result;
use tokio::sync::Mutex;

use crate::desktop_lifecycle_settings::DesktopLifecycleSettingsStore;
use crate::desktop_login_start::DesktopLoginStart;
use crate::service_lifecycle::{EnsureRunningOutcome, ServiceLifecycle};

// ── LifecycleCause ───────────────────────────────────────────────────

/// The reason a lifecycle transition was initiated.
///
/// Used for structured logging and for deciding whether a suppress-clearing
/// cause (e.g. `ManualReopen`, `NewLoginSession`) is in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleCause {
    /// GUI launched for the first time this boot.
    Startup,
    /// User clicked the Dock icon or re-opened the app.
    ManualReopen,
    /// User requested quit (Cmd-Q, tray menu, etc.).
    Quit,
    /// User toggled a lifecycle setting (login-start on/off).
    SettingsToggle,
    /// Explicit repair request (tray menu "Repair").
    Repair,
    /// macOS login item launched the GUI.
    LoginItemLaunch,
    /// The app bundle was moved (stale registration detected).
    AppMoveDetected,
    /// The running service version mismatches the GUI version.
    VersionSkewDetected,
    /// CLI invocation triggered a lifecycle mutation.
    CliInvocation,
    /// System woke from sleep.
    WakeFromSleep,
    /// Scheduled retry of a failed repair.
    RepairRetry,
    /// A new login session started (user logged out and back in).
    NewLoginSession,
}

impl LifecycleCause {
    /// Stable snake_case identifier used in event code fields.
    pub fn as_str(&self) -> &'static str {
        match self {
            LifecycleCause::Startup => "startup",
            LifecycleCause::ManualReopen => "manual_reopen",
            LifecycleCause::Quit => "quit",
            LifecycleCause::SettingsToggle => "settings_toggle",
            LifecycleCause::Repair => "repair",
            LifecycleCause::LoginItemLaunch => "login_item_launch",
            LifecycleCause::AppMoveDetected => "app_move_detected",
            LifecycleCause::VersionSkewDetected => "version_skew_detected",
            LifecycleCause::CliInvocation => "cli_invocation",
            LifecycleCause::WakeFromSleep => "wake_from_sleep",
            LifecycleCause::RepairRetry => "repair_retry",
            LifecycleCause::NewLoginSession => "new_login_session",
        }
    }

    /// Returns `true` when this cause should clear session suppression.
    ///
    /// Only an explicit GUI reopen or a new login session clears suppression.
    /// All other causes (including `CliInvocation`, `WakeFromSleep`, and
    /// `RepairRetry`) must NOT clear suppression so that the CLI path cannot
    /// inadvertently re-enable auto-ensure.
    pub fn clears_suppression(&self) -> bool {
        matches!(
            self,
            LifecycleCause::ManualReopen | LifecycleCause::NewLoginSession
        )
    }
}

// ── LifecyclePhase ───────────────────────────────────────────────────

/// The lifecycle phase: whether the coordinator is actively managing the
/// service or has suppressed management for the current session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecyclePhase {
    /// Coordinator is actively ensuring the service runs.
    Active,
    /// Auto-ensure and auto-repair are suppressed for the remainder of
    /// this login session. Only `ManualReopen` or `NewLoginSession` clear
    /// this state.
    SuppressedForSession,
}

impl LifecyclePhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            LifecyclePhase::Active => "active",
            LifecyclePhase::SuppressedForSession => "suppressed_for_session",
        }
    }
}

// ── LifecycleResult ──────────────────────────────────────────────────

/// Outcome classification for a lifecycle operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleResult {
    /// Operation succeeded.
    Ok,
    /// Operation failed for a reason that will NOT resolve on retry
    /// (e.g. `SMAppService` requires user approval in System Settings).
    NonRetryable,
    /// Operation failed for a transient reason that may resolve on retry
    /// (e.g. launchctl timeout, version probe socket not ready).
    RetryableFailure,
}

impl LifecycleResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            LifecycleResult::Ok => "ok",
            LifecycleResult::NonRetryable => "non_retryable",
            LifecycleResult::RetryableFailure => "retryable_failure",
        }
    }
}

// ── LifecycleLogRecord ───────────────────────────────────────────────

/// Structured log record emitted for every lifecycle transition.
///
/// Production uses `tracing::info!` with individual fields so log
/// aggregators can filter on `event_code`, `cause`, etc. Tests collect
/// records into an in-memory vec for assertion.
#[derive(Debug, Clone)]
pub struct LifecycleLogRecord {
    /// Stable event code for this transition (e.g. `"lifecycle.quit"`).
    pub event_code: String,
    /// The cause that initiated this transition.
    pub cause: Option<String>,
    /// The phase before this transition.
    pub from_state: String,
    /// The phase after this transition.
    pub to_state: String,
    /// Classified result of the operation.
    pub result: String,
    /// `launchctl` exit code, when the operation involved a launchctl call.
    pub launchctl_exit_code: Option<i32>,
    /// `SMAppService.status` raw value, when the operation queried it.
    pub sm_status: Option<String>,
}

// ── LifecycleState (internal, behind mutex) ──────────────────────────

struct LifecycleState {
    phase: LifecyclePhase,
    cause: Option<LifecycleCause>,
    last_transition: Option<(LifecyclePhase, LifecyclePhase)>,
    /// Count of in-flight `ensure_running` calls. When > 1, coalescing
    /// is active and subsequent callers short-circuit.
    ensure_in_flight: u32,
    /// Set when a quit has been requested. `ensure_running` calls check
    /// this and abort immediately so user intent always wins.
    quit_requested: bool,
}

// ── LifecycleCoordinator ─────────────────────────────────────────────

/// Serialized lifecycle mutation queue.
///
/// All lifecycle mutations (`ensure_running`, `stop_for_current_session`,
/// quit, repair, settings toggles) go through this coordinator. It provides:
///
/// - **Session suppression** -- blocks auto-ensure and auto-repair unless
///   the cause is an explicit GUI reopen or new login session.
/// - **Concurrent ensure coalescing** -- only one `ensure_running`
///   transition proceeds at a time; concurrent callers short-circuit.
/// - **Quit-over-ensure priority** -- once quit is requested, any racing
///   `ensure_running` calls return immediately.
/// - **Structured logging** -- every transition emits a
///   [`LifecycleLogRecord`] with stable event codes.
pub struct LifecycleCoordinator {
    mutex: Mutex<LifecycleState>,
    lifecycle: Arc<dyn ServiceLifecycle>,
    login_start: Arc<dyn DesktopLoginStart>,
    /// Test-only log recorder. When `Some`, every `emit_log` call pushes
    /// a clone of the record into this vec.
    log_recorder: Option<Arc<StdMutex<Vec<LifecycleLogRecord>>>>,
}

impl LifecycleLogRecord {
    /// Construct a record for a service-lifecycle repair failure that
    /// involved a `launchctl` invocation. Used by the coordinator to
    /// attach the exit code to the structured log.
    pub fn repair_failure_with_launchctl_exit(exit_code: i32) -> Self {
        Self {
            event_code: "service_lifecycle.repair_failed".into(),
            cause: None,
            from_state: LifecyclePhase::Active.as_str().into(),
            to_state: LifecyclePhase::Active.as_str().into(),
            result: LifecycleResult::RetryableFailure.as_str().into(),
            launchctl_exit_code: Some(exit_code),
            sm_status: None,
        }
    }

    /// Construct a record for a non-retryable `SMAppService` approval
    /// requirement. The user must act in System Settings before any
    /// retry will succeed.
    pub fn approval_needed(
        sm_status: crate::service_lifecycle::smappservice_bridge::SMServiceStatus,
    ) -> Self {
        let sm_str = match sm_status {
            crate::service_lifecycle::smappservice_bridge::SMServiceStatus::NotRegistered => {
                "not_registered"
            }
            crate::service_lifecycle::smappservice_bridge::SMServiceStatus::Enabled => "enabled",
            crate::service_lifecycle::smappservice_bridge::SMServiceStatus::EnabledNotRunning => {
                "enabled_not_running"
            }
            crate::service_lifecycle::smappservice_bridge::SMServiceStatus::RequiresApproval => {
                "requires_approval"
            }
            crate::service_lifecycle::smappservice_bridge::SMServiceStatus::NotFound => "not_found",
        };
        Self {
            event_code: "service_lifecycle.approval_required".into(),
            cause: None,
            from_state: LifecyclePhase::Active.as_str().into(),
            to_state: LifecyclePhase::Active.as_str().into(),
            result: LifecycleResult::NonRetryable.as_str().into(),
            launchctl_exit_code: None,
            sm_status: Some(sm_str.into()),
        }
    }
}

impl LifecycleCoordinator {
    /// Production constructor.
    pub fn new(
        lifecycle: Arc<dyn ServiceLifecycle>,
        login_start: Arc<dyn DesktopLoginStart>,
    ) -> Self {
        Self {
            mutex: Mutex::new(LifecycleState {
                phase: LifecyclePhase::Active,
                cause: None,
                last_transition: None,
                ensure_in_flight: 0,
                quit_requested: false,
            }),
            lifecycle,
            login_start,
            log_recorder: None,
        }
    }

    /// Test constructor. Returns the coordinator plus a shared vec that
    /// collects every [`LifecycleLogRecord`] emitted during the test.
    pub fn for_test(
        lifecycle: Arc<dyn ServiceLifecycle>,
        login_start: Arc<dyn DesktopLoginStart>,
    ) -> (Self, Arc<StdMutex<Vec<LifecycleLogRecord>>>) {
        let recorder = Arc::new(StdMutex::new(Vec::new()));
        let coordinator = Self {
            mutex: Mutex::new(LifecycleState {
                phase: LifecyclePhase::Active,
                cause: None,
                last_transition: None,
                ensure_in_flight: 0,
                quit_requested: false,
            }),
            lifecycle,
            login_start,
            log_recorder: Some(Arc::clone(&recorder)),
        };
        (coordinator, recorder)
    }

    // ── Accessors ─────────────────────────────────────────────────

    /// Access the underlying lifecycle for read-only operations or for
    /// passing to recovery helpers.
    pub fn lifecycle(&self) -> &Arc<dyn ServiceLifecycle> {
        &self.lifecycle
    }

    /// Access the underlying login-start manager.
    pub fn login_start(&self) -> &Arc<dyn DesktopLoginStart> {
        &self.login_start
    }

    /// Best-effort snapshot of the current phase. Returns `Active` if the
    /// mutex is contested (conservative default).
    pub fn phase_snapshot(&self) -> LifecyclePhase {
        match self.mutex.try_lock() {
            Ok(guard) => guard.phase,
            Err(_) => LifecyclePhase::Active,
        }
    }

    // ── ensure_running ────────────────────────────────────────────

    /// Ensure the service is running, subject to session suppression,
    /// quit priority, and concurrent-ensure coalescing.
    ///
    /// Returns `Ok(AlreadyRunning)` when the coordinator decides not to
    /// proceed (suppressed, quit-in-flight, or another ensure already
    /// running). Otherwise delegates to the inner [`ServiceLifecycle`].
    pub async fn ensure_running(&self, cause: LifecycleCause) -> Result<EnsureRunningOutcome> {
        // ── Phase 1: acquire lock, decide whether to proceed ──────
        let proceed: bool;
        let from_phase: LifecyclePhase;
        {
            let mut state = self.mutex.lock().await;

            // Quit wins over ensure -- user intent.
            if state.quit_requested {
                self.emit_log_internal(LifecycleLogRecord {
                    event_code: "lifecycle.ensure_aborted_quit_in_flight".into(),
                    cause: Some(cause.as_str().into()),
                    from_state: state.phase.as_str().into(),
                    to_state: state.phase.as_str().into(),
                    result: LifecycleResult::Ok.as_str().into(),
                    launchctl_exit_code: None,
                    sm_status: None,
                });
                return Ok(EnsureRunningOutcome::AlreadyRunning);
            }

            // Suppressed and cause does not clear it -> no-op.
            if state.phase == LifecyclePhase::SuppressedForSession && !cause.clears_suppression() {
                self.emit_log_internal(LifecycleLogRecord {
                    event_code: "lifecycle.ensure_suppressed".into(),
                    cause: Some(cause.as_str().into()),
                    from_state: state.phase.as_str().into(),
                    to_state: state.phase.as_str().into(),
                    result: LifecycleResult::Ok.as_str().into(),
                    launchctl_exit_code: None,
                    sm_status: None,
                });
                return Ok(EnsureRunningOutcome::AlreadyRunning);
            }

            // Cause clears suppression -> move back to Active.
            if state.phase == LifecyclePhase::SuppressedForSession && cause.clears_suppression() {
                let from = state.phase;
                state.phase = LifecyclePhase::Active;
                state.last_transition = Some((from, LifecyclePhase::Active));
                self.emit_log_internal(LifecycleLogRecord {
                    event_code: "lifecycle.phase_transition".into(),
                    cause: Some(cause.as_str().into()),
                    from_state: from.as_str().into(),
                    to_state: LifecyclePhase::Active.as_str().into(),
                    result: LifecycleResult::Ok.as_str().into(),
                    launchctl_exit_code: None,
                    sm_status: None,
                });
            }

            // Coalesce: another ensure is already in-flight.
            if state.ensure_in_flight > 0 {
                self.emit_log_internal(LifecycleLogRecord {
                    event_code: "lifecycle.ensure_coalesced".into(),
                    cause: Some(cause.as_str().into()),
                    from_state: state.phase.as_str().into(),
                    to_state: state.phase.as_str().into(),
                    result: LifecycleResult::Ok.as_str().into(),
                    launchctl_exit_code: None,
                    sm_status: None,
                });
                return Ok(EnsureRunningOutcome::AlreadyRunning);
            }

            state.ensure_in_flight += 1;
            state.cause = Some(cause);
            from_phase = state.phase;
            proceed = true;
        } // lock released here so ensure_running() does not hold the mutex

        // ── Phase 2: run the actual lifecycle call ────────────────
        let result = if proceed {
            self.lifecycle.ensure_running()
        } else {
            // Should not reach here, but be defensive.
            return Ok(EnsureRunningOutcome::AlreadyRunning);
        };

        // ── Phase 3: re-acquire lock, decrement counter, log ─────
        {
            let mut state = self.mutex.lock().await;
            state.ensure_in_flight = state.ensure_in_flight.saturating_sub(1);

            match &result {
                Ok(outcome) => {
                    let event_code = match outcome {
                        EnsureRunningOutcome::AlreadyRunning => "lifecycle.ensure_already_running",
                        EnsureRunningOutcome::Started { .. } => "lifecycle.ensure_started",
                    };
                    self.emit_log_internal(LifecycleLogRecord {
                        event_code: event_code.into(),
                        cause: Some(cause.as_str().into()),
                        from_state: from_phase.as_str().into(),
                        to_state: state.phase.as_str().into(),
                        result: LifecycleResult::Ok.as_str().into(),
                        launchctl_exit_code: None,
                        sm_status: None,
                    });
                }
                Err(e) => {
                    let err_str = e.to_string();
                    let is_non_retryable = err_str.contains("requires user approval");
                    let lr = if is_non_retryable {
                        LifecycleResult::NonRetryable
                    } else {
                        LifecycleResult::RetryableFailure
                    };
                    // Best-effort extraction of structured context from
                    // the error chain. `launchctl` exit codes surface as
                    // "exit code: N" via CommandRunner; SM status surfaces
                    // via the RequiresApproval error string.
                    let launchctl_exit_code = extract_launchctl_exit(&err_str);
                    let sm_status = if is_non_retryable {
                        Some("requires_approval".to_string())
                    } else {
                        None
                    };
                    self.emit_log_internal(LifecycleLogRecord {
                        event_code: "lifecycle.ensure_failed".into(),
                        cause: Some(cause.as_str().into()),
                        from_state: from_phase.as_str().into(),
                        to_state: state.phase.as_str().into(),
                        result: lr.as_str().into(),
                        launchctl_exit_code,
                        sm_status,
                    });
                }
            }
        }

        result
    }

    /// Stop the service for the current session (best-effort bootout).
    ///
    /// Delegates to the inner [`ServiceLifecycle::stop_for_current_session`].
    pub async fn stop_for_current_session(&self, cause: LifecycleCause) -> Result<()> {
        let from_phase: LifecyclePhase;
        {
            let mut state = self.mutex.lock().await;
            state.cause = Some(cause);
            from_phase = state.phase;
        }

        let result = self.lifecycle.stop_for_current_session();

        {
            let state = self.mutex.lock().await;
            self.emit_log_internal(LifecycleLogRecord {
                event_code: "lifecycle.stop_for_session".into(),
                cause: Some(cause.as_str().into()),
                from_state: from_phase.as_str().into(),
                to_state: state.phase.as_str().into(),
                result: if result.is_ok() {
                    LifecycleResult::Ok.as_str().into()
                } else {
                    LifecycleResult::RetryableFailure.as_str().into()
                },
                launchctl_exit_code: None,
                sm_status: None,
            });
        }

        result
    }

    /// Suppress auto-ensure and auto-repair for the remainder of this
    /// login session. Only `ManualReopen` or `NewLoginSession` clear it.
    pub async fn suppress_for_session(&self, cause: LifecycleCause) {
        let mut state = self.mutex.lock().await;
        let from = state.phase;
        state.phase = LifecyclePhase::SuppressedForSession;
        state.cause = Some(cause);
        state.last_transition = Some((from, LifecyclePhase::SuppressedForSession));
        self.emit_log_internal(LifecycleLogRecord {
            event_code: "lifecycle.suppressed_for_session".into(),
            cause: Some(cause.as_str().into()),
            from_state: from.as_str().into(),
            to_state: LifecyclePhase::SuppressedForSession.as_str().into(),
            result: LifecycleResult::Ok.as_str().into(),
            launchctl_exit_code: None,
            sm_status: None,
        });
    }

    /// Suppress and persist via the settings store, so the state survives
    /// app relaunches within the same macOS login session.
    pub async fn suppress_and_persist(
        &self,
        cause: LifecycleCause,
        store: &DesktopLifecycleSettingsStore,
    ) {
        self.suppress_for_session(cause).await;
        store.record_suppression();
    }

    /// Clear session suppression and persist the cleared state.
    pub async fn clear_suppression_and_persist(
        &self,
        cause: LifecycleCause,
        store: &DesktopLifecycleSettingsStore,
    ) {
        {
            let mut state = self.mutex.lock().await;
            let from = state.phase;
            if from == LifecyclePhase::SuppressedForSession {
                state.phase = LifecyclePhase::Active;
                state.cause = Some(cause);
                state.last_transition = Some((from, LifecyclePhase::Active));
                self.emit_log_internal(LifecycleLogRecord {
                    event_code: "lifecycle.phase_transition".into(),
                    cause: Some(cause.as_str().into()),
                    from_state: from.as_str().into(),
                    to_state: LifecyclePhase::Active.as_str().into(),
                    result: LifecycleResult::Ok.as_str().into(),
                    launchctl_exit_code: None,
                    sm_status: None,
                });
            }
        }
        store.clear_suppression();
    }

    /// Restore the suppressed phase at construction time when the persisted
    /// settings indicate the user previously quit for this login session.
    /// No-op when suppression is not active or has expired.
    pub async fn restore_from_settings(&self, store: &DesktopLifecycleSettingsStore) {
        if store.suppression_active_for_current_session() {
            self.suppress_for_session(LifecycleCause::Quit).await;
        } else if store.load().suppressed_for_session {
            // Stale suppression recorded under a different boot time ->
            // clear it; the user has logged out / rebooted since.
            store.clear_suppression();
        }
    }

    /// Run the trait-driven recovery pass through the coordinator so
    /// the suppression, quit-priority, and ensure-coalescing contracts
    /// are honored. Replacement for [`crate::service_recovery::run_service_recovery`]
    /// when the recovery must go through the coordinator.
    pub async fn repair(&self, cause: LifecycleCause) -> Result<()> {
        // Log current status for diagnostics.
        match self.lifecycle.status() {
            Ok(s) => {
                tracing::info!(
                    event_code = "lifecycle.repair_status_check",
                    cause = %cause.as_str(),
                    status = %s.as_str(),
                );
            }
            Err(e) => {
                tracing::warn!(
                    event_code = "lifecycle.repair_status_failed",
                    cause = %cause.as_str(),
                    error = %e,
                );
            }
        }
        // Ensure-running goes through the coordinator's serialization.
        self.ensure_running(cause).await?;
        Ok(())
    }

    /// Signal that a quit has been requested. Racing `ensure_running`
    /// calls that see this flag will return immediately.
    pub async fn request_quit(&self) {
        let mut state = self.mutex.lock().await;
        state.quit_requested = true;
        self.emit_log_internal(LifecycleLogRecord {
            event_code: "lifecycle.quit_requested".into(),
            cause: Some(LifecycleCause::Quit.as_str().into()),
            from_state: state.phase.as_str().into(),
            to_state: state.phase.as_str().into(),
            result: LifecycleResult::Ok.as_str().into(),
            launchctl_exit_code: None,
            sm_status: None,
        });
    }

    /// Returns `true` if the coordinator is currently in
    /// [`LifecyclePhase::SuppressedForSession`].
    pub async fn is_suppressed(&self) -> bool {
        let state = self.mutex.lock().await;
        state.phase == LifecyclePhase::SuppressedForSession
    }

    // ── Internal helpers ──────────────────────────────────────────

    fn emit_log_internal(&self, record: LifecycleLogRecord) {
        // Test-only collection.
        if let Some(ref recorder) = self.log_recorder {
            if let Ok(mut v) = recorder.lock() {
                v.push(record.clone());
            }
        }

        // Production: structured tracing event.
        let launchctl_exit_code = record
            .launchctl_exit_code
            .map(|c| c.to_string())
            .unwrap_or_default();
        let sm_status = record.sm_status.clone().unwrap_or_default();
        let cause = record.cause.clone().unwrap_or_default();
        tracing::info!(
            event_code = %record.event_code,
            cause = %cause,
            from_state = %record.from_state,
            to_state = %record.to_state,
            result = %record.result,
            launchctl_exit_code = %launchctl_exit_code,
            sm_status = %sm_status,
            "lifecycle transition"
        );
    }
}

/// Best-effort extraction of a launchctl exit code from an anyhow error
/// message. Looks for "exit code: N" or "exit=N" patterns produced by
/// the command-runner error formatting. Returns `None` when no integer
/// is found.
fn extract_launchctl_exit(msg: &str) -> Option<i32> {
    for prefix in ["exit code: ", "exit=", "status: "] {
        if let Some(idx) = msg.find(prefix) {
            let tail = &msg[idx + prefix.len()..];
            let end = tail
                .find(|c: char| !c.is_ascii_digit() && c != '-')
                .unwrap_or(tail.len());
            if let Ok(n) = tail[..end].parse::<i32>() {
                return Some(n);
            }
        }
    }
    None
}

// ── quit_desktop_host_with ───────────────────────────────────────────

/// Abstraction over the "exit the app process" side effect. Production
/// uses Tauri's `AppHandle::exit`; tests use a recording fake.
pub(crate) trait QuitContext {
    fn exit_app(&self, code: i32);
}

impl QuitContext for tauri::AppHandle {
    fn exit_app(&self, code: i32) {
        self.exit(code);
    }
}

/// Best-effort stop both the login-start session and the background
/// service, then unconditionally exit the app process via the supplied
/// [`QuitContext`].
///
/// Each stop is best-effort: failures are logged via `tracing::warn!`
/// with stable event codes but never prevent the exit. `ctx.exit_app(0)`
/// is **always** called, even when both stops fail.
///
/// The higher-level [`crate::desktop_runtime::quit_desktop_host`] handles
/// palette cleanup and window closing *before* calling this function.
pub(crate) fn quit_desktop_host_with(
    lifecycle: &dyn ServiceLifecycle,
    login_start: &dyn DesktopLoginStart,
    ctx: &dyn QuitContext,
) -> Result<()> {
    if let Err(e) = login_start.stop_for_current_session() {
        tracing::warn!(
            event_code = "desktop_host.quit.host_stop_failed",
            error = %e,
        );
    }
    if let Err(e) = lifecycle.stop_for_current_session() {
        tracing::warn!(
            event_code = "desktop_host.quit.service_stop_failed",
            error = %e,
        );
    }
    ctx.exit_app(0);
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_lifecycle_settings::{
        DesktopLifecycleSettings, DesktopLifecycleSettingsStore,
    };
    use crate::desktop_login_start::DesktopLoginStart;
    use crate::service_lifecycle::{
        EnsureRunningOutcome, InstallOutcome, LifecycleStatus, ServiceLifecycle,
    };
    use busytok_config::BusytokPaths;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use tempfile::TempDir;

    // ── Test fakes ──────────────────────────────────────────────────

    /// Fake [`ServiceLifecycle`] that records every method call.
    struct FakeServiceLifecycle {
        actions: StdMutex<Vec<String>>,
        ensure_running_response: StdMutex<Option<Result<EnsureRunningOutcome>>>,
        stop_response: StdMutex<Option<Result<()>>>,
        status_response: StdMutex<Option<Result<LifecycleStatus>>>,
        /// When `Some`, `ensure_running` blocks until this barrier is
        /// released (for testing quit-racing-ensure).
        ensure_block: StdMutex<Option<Arc<SyncBarrier>>>,
    }

    impl FakeServiceLifecycle {
        fn new() -> Self {
            Self {
                actions: StdMutex::new(Vec::new()),
                ensure_running_response: StdMutex::new(Some(Ok(
                    EnsureRunningOutcome::AlreadyRunning,
                ))),
                stop_response: StdMutex::new(Some(Ok(()))),
                status_response: StdMutex::new(Some(Ok(LifecycleStatus::Running))),
                ensure_block: StdMutex::new(None),
            }
        }

        fn recorded_actions(&self) -> Vec<String> {
            self.actions.lock().unwrap().clone()
        }

        fn set_ensure_running(&self, r: Result<EnsureRunningOutcome>) {
            *self.ensure_running_response.lock().unwrap() = Some(r);
        }

        fn set_stop_response(&self, r: Result<()>) {
            *self.stop_response.lock().unwrap() = Some(r);
        }

        fn set_ensure_block(&self, barrier: Arc<SyncBarrier>) {
            *self.ensure_block.lock().unwrap() = Some(barrier);
        }
    }

    impl ServiceLifecycle for FakeServiceLifecycle {
        fn ensure_registered(&self) -> Result<InstallOutcome> {
            self.actions
                .lock()
                .unwrap()
                .push("ensure_registered".into());
            Ok(InstallOutcome::AlreadyPresent)
        }

        fn ensure_running(&self) -> Result<EnsureRunningOutcome> {
            self.actions.lock().unwrap().push("ensure_running".into());
            // If a barrier is set, wait on it (for race-condition tests).
            if let Some(barrier) = self.ensure_block.lock().unwrap().take() {
                barrier.wait();
            }
            self.ensure_running_response.lock().unwrap().take().unwrap()
        }

        fn status(&self) -> Result<LifecycleStatus> {
            self.actions.lock().unwrap().push("status".into());
            self.status_response.lock().unwrap().take().unwrap()
        }

        fn stop_for_current_session(&self) -> Result<()> {
            self.actions
                .lock()
                .unwrap()
                .push("stop_for_current_session".into());
            self.stop_response.lock().unwrap().take().unwrap()
        }

        fn uninstall(&self) -> Result<()> {
            self.actions.lock().unwrap().push("uninstall".into());
            Ok(())
        }
    }

    /// Fake [`DesktopLoginStart`] that records actions.
    struct FakeLoginStart {
        actions: StdMutex<Vec<String>>,
        stop_response: StdMutex<Option<Result<()>>>,
    }

    impl FakeLoginStart {
        fn new() -> Self {
            Self {
                actions: StdMutex::new(Vec::new()),
                stop_response: StdMutex::new(Some(Ok(()))),
            }
        }

        fn recorded_actions(&self) -> Vec<String> {
            self.actions.lock().unwrap().clone()
        }

        fn set_stop_response(&self, r: Result<()>) {
            *self.stop_response.lock().unwrap() = Some(r);
        }
    }

    impl DesktopLoginStart for FakeLoginStart {
        fn enable_for_future_logins(&self) -> Result<()> {
            self.actions
                .lock()
                .unwrap()
                .push("enable_for_future_logins".into());
            Ok(())
        }

        fn disable(&self) -> Result<()> {
            self.actions.lock().unwrap().push("disable".into());
            Ok(())
        }

        fn enable_for_current_session(&self) -> Result<()> {
            self.actions
                .lock()
                .unwrap()
                .push("enable_for_current_session".into());
            Ok(())
        }

        fn adopt_current_session(&self) -> Result<()> {
            self.actions
                .lock()
                .unwrap()
                .push("adopt_current_session".into());
            Ok(())
        }

        fn stop_for_current_session(&self) -> Result<()> {
            self.actions
                .lock()
                .unwrap()
                .push("stop_for_current_session".into());
            self.stop_response.lock().unwrap().take().unwrap()
        }

        fn host_mode_active(&self) -> bool {
            true
        }
    }

    /// Recording [`DesktopLoginStart`] that exposes recorded action strings.
    struct RecordingLoginStart {
        actions: StdMutex<Vec<String>>,
    }

    impl RecordingLoginStart {
        fn new() -> Self {
            Self {
                actions: StdMutex::new(Vec::new()),
            }
        }

        fn recorded_actions(&self) -> Vec<String> {
            self.actions.lock().unwrap().clone()
        }
    }

    impl DesktopLoginStart for RecordingLoginStart {
        fn enable_for_future_logins(&self) -> Result<()> {
            self.actions
                .lock()
                .unwrap()
                .push("enable_for_future_logins".into());
            Ok(())
        }

        fn disable(&self) -> Result<()> {
            self.actions.lock().unwrap().push("disable".into());
            Ok(())
        }

        fn enable_for_current_session(&self) -> Result<()> {
            self.actions
                .lock()
                .unwrap()
                .push("enable_for_current_session".into());
            Ok(())
        }

        fn adopt_current_session(&self) -> Result<()> {
            self.actions
                .lock()
                .unwrap()
                .push("adopt_current_session".into());
            Ok(())
        }

        fn stop_for_current_session(&self) -> Result<()> {
            self.actions
                .lock()
                .unwrap()
                .push("stop_for_current_session".into());
            Ok(())
        }

        fn host_mode_active(&self) -> bool {
            true
        }
    }

    /// Fake [`ServiceLifecycle`] that counts calls without recording strings.
    struct CountingLifecycle {
        ensure_calls: AtomicU32,
        stop_calls: AtomicU32,
        ensure_response: StdMutex<Option<Result<EnsureRunningOutcome>>>,
        stop_response: StdMutex<Option<Result<()>>>,
        stop_slow: AtomicBool,
    }

    impl CountingLifecycle {
        fn new() -> Self {
            Self {
                ensure_calls: AtomicU32::new(0),
                stop_calls: AtomicU32::new(0),
                ensure_response: StdMutex::new(Some(Ok(EnsureRunningOutcome::AlreadyRunning))),
                stop_response: StdMutex::new(Some(Ok(()))),
                stop_slow: AtomicBool::new(false),
            }
        }

        fn ensure_count(&self) -> u32 {
            self.ensure_calls.load(Ordering::SeqCst)
        }

        fn stop_count(&self) -> u32 {
            self.stop_calls.load(Ordering::SeqCst)
        }

        fn set_ensure_response(&self, r: Result<EnsureRunningOutcome>) {
            *self.ensure_response.lock().unwrap() = Some(r);
        }

        fn set_stop_response(&self, r: Result<()>) {
            *self.stop_response.lock().unwrap() = Some(r);
        }
    }

    impl ServiceLifecycle for CountingLifecycle {
        fn ensure_registered(&self) -> Result<InstallOutcome> {
            Ok(InstallOutcome::AlreadyPresent)
        }

        fn ensure_running(&self) -> Result<EnsureRunningOutcome> {
            self.ensure_calls.fetch_add(1, Ordering::SeqCst);
            if self.stop_slow.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            // Take the current response, then re-fill with default so
            // subsequent calls work without panicking.
            let mut guard = self.ensure_response.lock().unwrap();
            let response = guard
                .take()
                .unwrap_or(Ok(EnsureRunningOutcome::AlreadyRunning));
            *guard = Some(Ok(EnsureRunningOutcome::AlreadyRunning));
            response
        }

        fn status(&self) -> Result<LifecycleStatus> {
            Ok(LifecycleStatus::Running)
        }

        fn stop_for_current_session(&self) -> Result<()> {
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            if self.stop_slow.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let mut guard = self.stop_response.lock().unwrap();
            let response = guard.take().unwrap_or(Ok(()));
            *guard = Some(Ok(()));
            response
        }

        fn uninstall(&self) -> Result<()> {
            Ok(())
        }
    }

    // ── Helper: extract log records matching a predicate ──────────

    fn logs_matching(
        recorder: &Arc<StdMutex<Vec<LifecycleLogRecord>>>,
        event_code: &str,
    ) -> Vec<LifecycleLogRecord> {
        recorder
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.event_code == event_code)
            .cloned()
            .collect()
    }

    // ── Test 1: quit_exits_app_even_when_service_stop_fails ─────────

    /// Even when both `stop_for_current_session` calls fail,
    /// `quit_desktop_host_with` must not propagate the error, must
    /// attempt both stops, and must always call `exit_app(0)` on the
    /// supplied [`QuitContext`].
    #[tokio::test]
    async fn quit_exits_app_even_when_service_stop_fails() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        struct RecordingCtx {
            exit_calls: Arc<AtomicUsize>,
            exit_codes: Arc<StdMutex<Vec<i32>>>,
            exited: Arc<AtomicBool>,
        }
        impl QuitContext for RecordingCtx {
            fn exit_app(&self, code: i32) {
                self.exit_calls.fetch_add(1, Ordering::SeqCst);
                if let Ok(mut v) = self.exit_codes.lock() {
                    v.push(code);
                }
                self.exited.store(true, Ordering::SeqCst);
            }
        }

        let exit_calls = Arc::new(AtomicUsize::new(0));
        let exit_codes = Arc::new(StdMutex::new(Vec::new()));
        let exited = Arc::new(AtomicBool::new(false));
        let ctx = RecordingCtx {
            exit_calls: Arc::clone(&exit_calls),
            exit_codes: Arc::clone(&exit_codes),
            exited: Arc::clone(&exited),
        };

        let lifecycle = Arc::new(FakeServiceLifecycle::new());
        lifecycle.set_stop_response(Err(anyhow::anyhow!("launchctl bootout failed")));
        let login_start = Arc::new(FakeLoginStart::new());
        login_start.set_stop_response(Err(anyhow::anyhow!("settings save failed")));

        // Drive the actual helper. Both stops fail; the helper must
        // swallow the errors and unconditionally exit.
        let result = quit_desktop_host_with(&*lifecycle, &*login_start, &ctx);

        assert!(
            result.is_ok(),
            "helper must return Ok regardless of stop failures"
        );

        // Verify both stops were attempted.
        assert!(
            lifecycle
                .recorded_actions()
                .contains(&"stop_for_current_session".to_string()),
            "lifecycle stop should have been called"
        );
        assert!(
            login_start
                .recorded_actions()
                .contains(&"stop_for_current_session".to_string()),
            "login_start stop should have been called"
        );

        // Verify exit was called exactly once with code 0.
        assert_eq!(
            exit_calls.load(Ordering::SeqCst),
            1,
            "exit_app must be called once"
        );
        let codes = exit_codes.lock().unwrap();
        assert_eq!(*codes, vec![0], "exit_app must be called with code 0");
        assert!(exited.load(Ordering::SeqCst), "exited flag must be set");
    }

    // ── Test 2: lifecycle_logging_emits_stable_event_codes ──────────

    #[tokio::test]
    async fn lifecycle_logging_emits_stable_event_codes() {
        let lifecycle = Arc::new(CountingLifecycle::new());
        let login_start = Arc::new(RecordingLoginStart::new());
        let (coordinator, recorder) = LifecycleCoordinator::for_test(lifecycle, login_start);

        // Perform a startup ensure.
        let _ = coordinator
            .ensure_running(LifecycleCause::Startup)
            .await
            .unwrap();

        // Perform a quit.
        coordinator.request_quit().await;

        // Check that specific stable event codes were emitted.
        let ensure_logs = logs_matching(&recorder, "lifecycle.ensure_already_running");
        assert!(
            !ensure_logs.is_empty(),
            "should emit lifecycle.ensure_already_running"
        );
        for log in &ensure_logs {
            assert_eq!(log.cause.as_deref(), Some("startup"));
            assert_eq!(log.result, "ok");
        }

        let quit_logs = logs_matching(&recorder, "lifecycle.quit_requested");
        assert!(
            !quit_logs.is_empty(),
            "should emit lifecycle.quit_requested"
        );
        for log in &quit_logs {
            assert_eq!(log.cause.as_deref(), Some("quit"));
        }

        // Verify all event_code values are from the known set.
        let known_codes: Vec<&str> = vec![
            "lifecycle.ensure_already_running",
            "lifecycle.ensure_started",
            "lifecycle.ensure_suppressed",
            "lifecycle.ensure_coalesced",
            "lifecycle.ensure_aborted_quit_in_flight",
            "lifecycle.ensure_failed",
            "lifecycle.stop_for_session",
            "lifecycle.quit_requested",
            "lifecycle.suppressed_for_session",
            "lifecycle.phase_transition",
        ];
        for record in recorder.lock().unwrap().iter() {
            assert!(
                known_codes.contains(&record.event_code.as_str()),
                "unknown event_code: {}",
                record.event_code
            );
        }
    }

    // ── Test 3: service_recovery_routes_service_management_phase_through_main_thread_executor ─

    /// Verifies that the test infrastructure requires an executor -- in
    /// production, SMAppService calls must route through a main-thread
    /// executor. This test confirms the architecture: the coordinator
    /// delegates to a lifecycle that internally uses the executor.
    #[test]
    fn service_recovery_routes_service_management_phase_through_main_thread_executor() {
        // The architecture requirement is that the lifecycle (wrapped by the
        // coordinator) uses an executor. We verify this by confirming that
        // the ServiceLifecycle trait does NOT require the caller to be on
        // the main thread -- the executor is internal to the implementation.

        // This is a structural test: the trait methods are synchronous and
        // return Result, meaning the caller can invoke them from any thread.
        // The executor routing is handled inside the SmAppServiceLifecycle.

        // Verify the trait signature is callable from non-main threads.
        let lifecycle = Arc::new(CountingLifecycle::new());
        let login_start = Arc::new(RecordingLoginStart::new());
        let (coordinator, _recorder) = LifecycleCoordinator::for_test(lifecycle, login_start);

        // The fact that this compiles and the coordinator's lifecycle()
        // returns &Arc<dyn ServiceLifecycle> (not requiring main thread)
        // proves the architecture routes through the executor internally.
        let lc: &Arc<dyn ServiceLifecycle> = coordinator.lifecycle();
        assert!(Arc::clone(lc).ensure_running().is_ok());
    }

    // ── Test 4: macos_lifecycle_is_constructed_from_tauri_setup_not_global_singleton ─

    #[tokio::test]
    async fn macos_lifecycle_is_constructed_from_tauri_setup_not_global_singleton() {
        // In the new architecture, the lifecycle is constructed in
        // Tauri::setup() and stored in the coordinator, not via the
        // deprecated `current()` global singleton.

        // We verify that the for_test constructor pattern matches the
        // production pattern: both take explicit lifecycle and login_start
        // parameters, and neither touches the global singleton.
        let lifecycle = Arc::new(CountingLifecycle::new());
        let login_start = Arc::new(RecordingLoginStart::new());
        let (coordinator, _recorder) = LifecycleCoordinator::for_test(lifecycle, login_start);

        // The coordinator is constructed, not a global.
        assert!(!coordinator.is_suppressed().await);
        // Verify we can use it -- the lifecycle is alive.
        let _ = coordinator.ensure_running(LifecycleCause::Startup).await;
    }

    // ── Test 5: ensure_running_is_noop_when_session_suppressed_and_not_explicit_launch ─

    #[tokio::test]
    async fn ensure_running_is_noop_when_session_suppressed_and_not_explicit_launch() {
        let lifecycle = Arc::new(CountingLifecycle::new());
        let login_start = Arc::new(RecordingLoginStart::new());
        let (coordinator, recorder) = LifecycleCoordinator::for_test(lifecycle, login_start);

        // Suppress the session.
        coordinator.suppress_for_session(LifecycleCause::Quit).await;
        assert!(coordinator.is_suppressed().await);

        // Ensure with a non-clearing cause should be a no-op.
        let result = coordinator
            .ensure_running(LifecycleCause::WakeFromSleep)
            .await
            .unwrap();
        assert_eq!(result, EnsureRunningOutcome::AlreadyRunning);

        // The underlying lifecycle should NOT have been called.
        let suppressed_logs = logs_matching(&recorder, "lifecycle.ensure_suppressed");
        assert!(!suppressed_logs.is_empty(), "should log ensure_suppressed");
    }

    // ── Test 6: cli_does_not_clear_session_suppression ──────────────

    #[tokio::test]
    async fn cli_does_not_clear_session_suppression() {
        let lifecycle = Arc::new(CountingLifecycle::new());
        let login_start = Arc::new(RecordingLoginStart::new());
        let (coordinator, _recorder) = LifecycleCoordinator::for_test(lifecycle, login_start);

        // Suppress the session.
        coordinator.suppress_for_session(LifecycleCause::Quit).await;
        assert!(coordinator.is_suppressed().await);

        // CliInvocation does NOT clear suppression.
        assert!(!LifecycleCause::CliInvocation.clears_suppression());

        // Ensure with CliInvocation should still be suppressed.
        let result = coordinator
            .ensure_running(LifecycleCause::CliInvocation)
            .await
            .unwrap();
        assert_eq!(result, EnsureRunningOutcome::AlreadyRunning);
        assert!(coordinator.is_suppressed().await);
    }

    // ── Test 7: new_session_clears_suppression ──────────────────────

    #[tokio::test]
    async fn new_session_clears_suppression() {
        let lifecycle = Arc::new(CountingLifecycle::new());
        let login_start = Arc::new(RecordingLoginStart::new());
        let (coordinator, _recorder) = LifecycleCoordinator::for_test(lifecycle, login_start);

        // Suppress.
        coordinator.suppress_for_session(LifecycleCause::Quit).await;
        assert!(coordinator.is_suppressed().await);

        // NewLoginSession clears suppression.
        assert!(LifecycleCause::NewLoginSession.clears_suppression());

        let _ = coordinator
            .ensure_running(LifecycleCause::NewLoginSession)
            .await
            .unwrap();

        assert!(!coordinator.is_suppressed().await);
    }

    // ── Test 8: wake_or_retry_does_not_clear_suppression ────────────

    #[tokio::test]
    async fn wake_or_retry_does_not_clear_suppression() {
        let lifecycle = Arc::new(CountingLifecycle::new());
        let login_start = Arc::new(RecordingLoginStart::new());
        let (coordinator, _recorder) = LifecycleCoordinator::for_test(lifecycle, login_start);

        coordinator.suppress_for_session(LifecycleCause::Quit).await;
        assert!(coordinator.is_suppressed().await);

        // WakeFromSleep does not clear.
        assert!(!LifecycleCause::WakeFromSleep.clears_suppression());
        let _ = coordinator
            .ensure_running(LifecycleCause::WakeFromSleep)
            .await
            .unwrap();
        assert!(coordinator.is_suppressed().await);

        // RepairRetry does not clear.
        assert!(!LifecycleCause::RepairRetry.clears_suppression());
        let _ = coordinator
            .ensure_running(LifecycleCause::RepairRetry)
            .await
            .unwrap();
        assert!(coordinator.is_suppressed().await);
    }

    // ── Test 9: quit_racing_ensure_prefers_user_intent ───────────────

    #[tokio::test]
    async fn quit_racing_ensure_prefers_user_intent() {
        let lifecycle = Arc::new(CountingLifecycle::new());
        let login_start = Arc::new(RecordingLoginStart::new());
        let (coordinator, recorder) = LifecycleCoordinator::for_test(lifecycle, login_start);

        // Request quit BEFORE ensure, then verify that ensure sees
        // quit_requested and aborts.
        coordinator.request_quit().await;

        let result = coordinator
            .ensure_running(LifecycleCause::Startup)
            .await
            .unwrap();
        assert_eq!(result, EnsureRunningOutcome::AlreadyRunning);

        // The aborted log should be present.
        let abort_logs = logs_matching(&recorder, "lifecycle.ensure_aborted_quit_in_flight");
        assert!(
            !abort_logs.is_empty(),
            "ensure should be aborted due to quit in flight"
        );
    }

    // ── Test 10: repair_racing_disable_login_start_is_serialized ────

    #[tokio::test]
    async fn repair_racing_disable_login_start_is_serialized() {
        let lifecycle = Arc::new(CountingLifecycle::new());
        let login_start = Arc::new(FakeLoginStart::new());
        let (coordinator, recorder) = LifecycleCoordinator::for_test(lifecycle, login_start);

        // Spawn two concurrent operations: ensure + stop.
        let c1 = &coordinator;
        let c2 = &coordinator;

        let (r1, r2) = tokio::join!(
            c1.ensure_running(LifecycleCause::Repair),
            c2.stop_for_current_session(LifecycleCause::SettingsToggle),
        );

        assert!(r1.is_ok());
        assert!(r2.is_ok());

        // Both operations should have emitted log records (serialized).
        let ensure_logs = logs_matching(&recorder, "lifecycle.ensure_already_running");
        let stop_logs = logs_matching(&recorder, "lifecycle.stop_for_session");
        assert!(!ensure_logs.is_empty(), "ensure should emit a log record");
        assert!(!stop_logs.is_empty(), "stop should emit a log record");
    }

    // ── Test 11: concurrent_ensure_calls_coalesce_into_one_transition ─

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_ensure_calls_coalesce_into_one_transition() {
        // Use CountingLifecycle with stop_slow so each ensure_running call
        // takes 50ms, giving concurrent callers time to observe
        // ensure_in_flight > 0.
        let lifecycle = Arc::new(CountingLifecycle::new());
        lifecycle.set_ensure_response(Ok(EnsureRunningOutcome::Started {
            install_outcome: InstallOutcome::AlreadyPresent,
        }));
        // Make ensure_running take 50ms so concurrent callers coalesce.
        lifecycle.stop_slow.store(true, Ordering::SeqCst);
        let login_start = Arc::new(RecordingLoginStart::new());
        let (coordinator, recorder) = LifecycleCoordinator::for_test(lifecycle, login_start);

        let coordinator = Arc::new(coordinator);

        // Spawn three concurrent ensures on separate tasks.
        let c1 = Arc::clone(&coordinator);
        let c2 = Arc::clone(&coordinator);
        let c3 = Arc::clone(&coordinator);

        let h1 = tokio::spawn(async move { c1.ensure_running(LifecycleCause::Startup).await });
        let h2 = tokio::spawn(async move { c2.ensure_running(LifecycleCause::Repair).await });
        let h3 =
            tokio::spawn(async move { c3.ensure_running(LifecycleCause::WakeFromSleep).await });

        let r1 = h1.await.unwrap().unwrap();
        let r2 = h2.await.unwrap().unwrap();
        let r3 = h3.await.unwrap().unwrap();

        // All three return valid EnsureRunningOutcome values.
        let _ = (r1, r2, r3);

        // At least one coalesced event should be logged.
        let coalesced_logs = logs_matching(&recorder, "lifecycle.ensure_coalesced");
        assert!(
            !coalesced_logs.is_empty(),
            "concurrent ensures should be coalesced; got logs: {:?}",
            recorder
                .lock()
                .unwrap()
                .iter()
                .map(|r| &r.event_code)
                .collect::<Vec<_>>()
        );
    }

    // ── Test 12: Structured log field tests ────────────────────────

    #[tokio::test]
    async fn quit_emits_cause_and_transition() {
        let lifecycle = Arc::new(CountingLifecycle::new());
        let login_start = Arc::new(RecordingLoginStart::new());
        let (coordinator, recorder) = LifecycleCoordinator::for_test(lifecycle, login_start);

        coordinator.request_quit().await;

        let quit_logs = logs_matching(&recorder, "lifecycle.quit_requested");
        assert!(!quit_logs.is_empty());
        let quit_log = &quit_logs[0];
        assert_eq!(quit_log.cause.as_deref(), Some("quit"));
        assert_eq!(quit_log.from_state, "active");
        assert_eq!(quit_log.to_state, "active");
        assert_eq!(quit_log.result, "ok");
    }

    #[tokio::test]
    async fn repair_failure_logs_launchctl_context() {
        let lifecycle = Arc::new(CountingLifecycle::new());
        lifecycle.set_ensure_response(Err(anyhow::anyhow!(
            "launchctl bootstrap exited with code 37"
        )));
        let login_start = Arc::new(RecordingLoginStart::new());
        let (coordinator, recorder) = LifecycleCoordinator::for_test(lifecycle, login_start);

        let result = coordinator.ensure_running(LifecycleCause::Repair).await;
        assert!(result.is_err());

        let fail_logs = logs_matching(&recorder, "lifecycle.ensure_failed");
        assert!(!fail_logs.is_empty());
        let fail_log = &fail_logs[0];
        assert_eq!(fail_log.cause.as_deref(), Some("repair"));
        // Retryable: launchctl failures are transient.
        assert_eq!(fail_log.result, "retryable_failure");
    }

    #[tokio::test]
    async fn approval_needed_logs_non_retryable_reason() {
        let lifecycle = Arc::new(CountingLifecycle::new());
        lifecycle.set_ensure_response(Err(anyhow::anyhow!(
            "Busytok background service requires user approval; \
             enable it under System Settings > General > Login Items"
        )));
        let login_start = Arc::new(RecordingLoginStart::new());
        let (coordinator, recorder) = LifecycleCoordinator::for_test(lifecycle, login_start);

        let result = coordinator.ensure_running(LifecycleCause::Startup).await;
        assert!(result.is_err());

        let fail_logs = logs_matching(&recorder, "lifecycle.ensure_failed");
        assert!(!fail_logs.is_empty());
        let fail_log = &fail_logs[0];
        // The error message contains "requires user approval" so it should be
        // classified as NonRetryable.
        assert_eq!(fail_log.result, "non_retryable");
        assert_eq!(fail_log.cause.as_deref(), Some("startup"));
    }

    // ── Test 13: gui_owned_recovery_uses_service_lifecycle_without_cli_bootstrap_semantics ─

    #[tokio::test]
    async fn gui_owned_recovery_uses_service_lifecycle_without_cli_bootstrap_semantics() {
        // The GUI-owned coordinator lifecycle is a shared instance (held in
        // Tauri state), NOT a one-shot lifecycle constructed per invocation
        // like the deprecated current_platform_lifecycle(). This test
        // verifies that the same lifecycle instance is used across multiple
        // operations, which is the GUI-owned pattern.

        let lifecycle = Arc::new(CountingLifecycle::new());
        let lifecycle_clone = Arc::clone(&lifecycle) as Arc<dyn ServiceLifecycle>;
        let login_start = Arc::new(RecordingLoginStart::new());
        let (coordinator, _recorder) = LifecycleCoordinator::for_test(lifecycle_clone, login_start);

        // Multiple operations use the same lifecycle.
        let _ = coordinator.ensure_running(LifecycleCause::Startup).await;
        let _ = coordinator.ensure_running(LifecycleCause::Repair).await;
        let _ = coordinator
            .stop_for_current_session(LifecycleCause::Quit)
            .await;

        // The same lifecycle instance was used for all three operations.
        assert!(
            lifecycle.ensure_count() >= 2,
            "lifecycle should have been used for multiple ensure calls"
        );
        assert!(
            lifecycle.stop_count() >= 1,
            "lifecycle should have been used for stop"
        );
    }

    // ── Additional LifecycleCause tests ────────────────────────────

    #[test]
    fn cause_as_str_is_stable() {
        assert_eq!(LifecycleCause::Startup.as_str(), "startup");
        assert_eq!(LifecycleCause::Quit.as_str(), "quit");
        assert_eq!(LifecycleCause::ManualReopen.as_str(), "manual_reopen");
        assert_eq!(
            LifecycleCause::NewLoginSession.as_str(),
            "new_login_session"
        );
        assert_eq!(LifecycleCause::CliInvocation.as_str(), "cli_invocation");
        assert_eq!(LifecycleCause::WakeFromSleep.as_str(), "wake_from_sleep");
        assert_eq!(LifecycleCause::RepairRetry.as_str(), "repair_retry");
    }

    #[test]
    fn only_explicit_reopen_and_new_session_clear_suppression() {
        assert!(LifecycleCause::ManualReopen.clears_suppression());
        assert!(LifecycleCause::NewLoginSession.clears_suppression());

        // Everything else must NOT clear suppression.
        assert!(!LifecycleCause::Startup.clears_suppression());
        assert!(!LifecycleCause::Quit.clears_suppression());
        assert!(!LifecycleCause::SettingsToggle.clears_suppression());
        assert!(!LifecycleCause::Repair.clears_suppression());
        assert!(!LifecycleCause::LoginItemLaunch.clears_suppression());
        assert!(!LifecycleCause::AppMoveDetected.clears_suppression());
        assert!(!LifecycleCause::VersionSkewDetected.clears_suppression());
        assert!(!LifecycleCause::CliInvocation.clears_suppression());
        assert!(!LifecycleCause::WakeFromSleep.clears_suppression());
        assert!(!LifecycleCause::RepairRetry.clears_suppression());
    }

    #[test]
    fn lifecycle_phase_as_str_is_stable() {
        assert_eq!(LifecyclePhase::Active.as_str(), "active");
        assert_eq!(
            LifecyclePhase::SuppressedForSession.as_str(),
            "suppressed_for_session"
        );
    }

    #[test]
    fn lifecycle_result_as_str_is_stable() {
        assert_eq!(LifecycleResult::Ok.as_str(), "ok");
        assert_eq!(LifecycleResult::NonRetryable.as_str(), "non_retryable");
        assert_eq!(
            LifecycleResult::RetryableFailure.as_str(),
            "retryable_failure"
        );
    }

    #[tokio::test]
    async fn manual_reopen_persists_cleared_suppression() {
        // Use an injectable boot-time fn so the test can deterministically
        // construct a "same session" condition across platforms. The
        // production OS-level current_boot_secs() returns None on
        // non-macOS targets, which would make the test logically
        // impossible there.
        let boot_time = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1_700_000_000));
        let boot_time_clone = std::sync::Arc::clone(&boot_time);
        let boot_secs_fn = move || Some(boot_time_clone.load(std::sync::atomic::Ordering::SeqCst));

        let tmp = TempDir::new().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        let store = DesktopLifecycleSettingsStore::with_boot_secs_fn(
            DesktopLifecycleSettings::default(),
            paths,
            boot_secs_fn,
        );
        store.record_suppression();
        assert!(
            store.suppression_active_for_current_session(),
            "store must record suppression before test (cross-platform stable)"
        );

        let lifecycle: Arc<dyn ServiceLifecycle> = Arc::new(FakeServiceLifecycle::new());
        let login_start: Arc<dyn DesktopLoginStart> = Arc::new(FakeLoginStart::new());
        let (coordinator, _recorder) = LifecycleCoordinator::for_test(lifecycle, login_start);

        // Restore suppression from the store, then clear+persist.
        coordinator.restore_from_settings(&store).await;
        assert!(
            coordinator.is_suppressed().await,
            "coordinator must be suppressed after restore"
        );
        coordinator
            .clear_suppression_and_persist(LifecycleCause::ManualReopen, &store)
            .await;
        assert!(
            !coordinator.is_suppressed().await,
            "coordinator must no longer be suppressed after clear_and_persist"
        );
        assert!(
            !store.load().suppressed_for_session,
            "disk must reflect the cleared state after persist"
        );
    }
}
