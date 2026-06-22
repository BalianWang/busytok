#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceBootstrapState {
    Starting,
    /// Reserved for when service needs bootout/rebootstrap. The bootout+
    /// rebootstrap path in bootstrap.rs logs but does not yet emit this
    /// status. UI behavior matches Starting (prompt execution disabled).
    #[allow(dead_code)]
    Repairing,
    Ready,
    Unavailable,
}

impl ServiceBootstrapState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Repairing => "repairing",
            Self::Ready => "ready",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ServiceStatusEvent {
    pub status: ServiceBootstrapState,
    pub since_ms: u64,
    pub reason: Option<String>,
}

// ── Background service diagnostics ─────────────────────────────────────

/// User-facing state classification for the desktop background service.
///
/// This is derived from the lifecycle coordinator phase and the underlying
/// `ServiceLifecycle::status()` result. It is NOT the same as the bootstrap
/// `ServiceBootstrapState` which describes the initial async warm-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopBackgroundServiceState {
    /// The background service is running and reachable.
    Running,
    /// The background service is in the process of starting up (bootstrap
    /// or repair in progress).
    Starting,
    /// The service is not registered with the OS (not installed).
    NotRegistered,
    /// The service has been stopped for the current login session via an
    /// explicit user action (e.g. "Stop Background Service").
    StoppedForThisSession,
    /// The service is registered but not running, and requires user
    /// attention (e.g. approval in System Settings).
    NeedsAttention,
}

impl DesktopBackgroundServiceState {
    /// Stable snake_case identifier for use in event codes and IPC payloads.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Starting => "starting",
            Self::NotRegistered => "not_registered",
            Self::StoppedForThisSession => "stopped_for_this_session",
            Self::NeedsAttention => "needs_attention",
        }
    }

    /// Returns `true` when the user can take action to resolve this state
    /// (open System Settings, re-register, repair, etc.).
    ///
    /// `StoppedForThisSession` is NOT actionable — it is an explicit user
    /// choice (the user invoked Quit Busytok Desktop). The UI must not
    /// offer a repair action for it; the recovery path is to reopen
    /// `Busytok.app` or wait for a new login session.
    pub fn is_actionable(&self) -> bool {
        matches!(self, Self::NotRegistered | Self::NeedsAttention)
    }
}

/// Diagnostics payload returned by the `desktop_background_service_diagnostics`
/// Tauri command. Used by the frontend to render the Background Service
/// section on the Settings page.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DesktopBackgroundServiceDiagnostics {
    /// Current state of the background service.
    pub state: DesktopBackgroundServiceState,
    /// Whether the frontend should show a "Repair" / "Fix" action button.
    pub actionable: bool,
    /// Build version of the currently running GUI (CARGO_PKG_VERSION).
    pub gui_build_identity: String,
    /// Build version of the running background service, if detectable.
    pub service_build_identity: Option<String>,
    /// True when the GUI and service build identities differ (may indicate
    /// a partial upgrade).
    pub version_skew: bool,
    /// True when the current GUI process is running in desktop-host mode
    /// (tray, shortcut, window-close-as-hide). Used by smoke + diagnostics
    /// to verify Quit actually stopped the desktop-host.
    pub host_mode_active: bool,
}
