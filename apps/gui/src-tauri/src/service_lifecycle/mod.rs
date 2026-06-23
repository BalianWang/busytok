//! Service lifecycle port — install/ensure_running/status/uninstall across
//! macOS (`SMAppService`) and Windows (Scheduled Task).
//!
//! On macOS the runtime entry point is [`smappservice::SmAppServiceLifecycle`],
//! constructed during Tauri setup with a real main-thread executor (see
//! [`crate::lifecycle_coordinator::LifecycleCoordinator`]). The old
//! zero-argument singleton is gone — every caller must reach the lifecycle
//! via the coordinator.

#![allow(dead_code)] // enum variants + methods used via dyn dispatch or future phases

use std::sync::Arc;

use anyhow::{Context, Result};

pub mod bundle_layout;
pub mod command_runner;
#[cfg(target_os = "macos")]
pub mod launchd_job_snapshot;
#[cfg(target_os = "macos")]
pub mod managed_launch_agent;
#[cfg(target_os = "macos")]
pub(crate) mod proc_pidpath;
#[cfg(target_os = "macos")]
pub mod smappservice;
#[cfg(target_os = "macos")]
pub mod smappservice_bridge;
pub mod task_scheduler;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    NewlyInstalled,
    AlreadyPresent,
    Upgraded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureRunningOutcome {
    AlreadyRunning,
    Started { install_outcome: InstallOutcome },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleStatus {
    NotRegistered,
    RegisteredInactive,
    Running,
    Disabled,
    NeedsAttention,
}

impl LifecycleStatus {
    /// Stable snake_case string identifier for each variant. Used for
    /// diagnostics, IPC payloads, and tests. Must match the variant name
    /// in snake_case (e.g. `RegisteredInactive` -> `"registered_inactive"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            LifecycleStatus::NotRegistered => "not_registered",
            LifecycleStatus::RegisteredInactive => "registered_inactive",
            LifecycleStatus::Running => "running",
            LifecycleStatus::Disabled => "disabled",
            LifecycleStatus::NeedsAttention => "needs_attention",
        }
    }
}

pub trait ServiceLifecycle: Send + Sync {
    fn ensure_registered(&self) -> Result<InstallOutcome>;
    fn ensure_running(&self) -> Result<EnsureRunningOutcome>;
    fn status(&self) -> Result<LifecycleStatus>;
    fn stop_for_current_session(&self) -> Result<()>;
    fn uninstall(&self) -> Result<()>;
    /// Probe the running service's build identity, when available. Used by
    /// diagnostics to compute version-skew against the GUI bundle identity.
    /// Returns `Ok(None)` when the probe is unavailable (no socket yet,
    /// unsupported platform, etc.) rather than an error.
    fn probe_service_identity(&self) -> Result<Option<String>> {
        Ok(None)
    }
}

/// Construct a one-shot macOS [`ServiceLifecycle`] for paths that run
/// outside the Tauri-managed coordinator — specifically the
/// `--uninstall-self` flow, which executes synchronously on the GUI main
/// thread before the Tauri app is built.
///
/// **Contract:** must be called on the application main thread. The
/// returned lifecycle uses a current-thread executor that asserts
/// `pthread_main_np()`; off-main-thread calls will panic.
#[cfg(target_os = "macos")]
pub fn macos_lifecycle_for_uninstall_self() -> Result<Arc<dyn ServiceLifecycle>> {
    use busytok_config::BusytokPaths;
    use busytok_platform::PlatformPaths;

    use crate::service_lifecycle::bundle_layout::BundleLayout;
    use crate::service_lifecycle::command_runner::SystemCommandRunner;
    use crate::service_lifecycle::smappservice::{
        ControlClientVersionProbe, SmAppServiceLifecycle,
    };
    use crate::service_lifecycle::smappservice_bridge::MainThreadExecutor;

    /// Minimal executor that runs closures inline on the calling thread.
    /// Used only for the `--uninstall-self` path which is already on the
    /// GUI main thread. Panics if invoked off main-thread — that contract
    /// is enforced because `--uninstall-self` is dispatched from
    /// `lib::run()` on the main thread.
    struct MainThreadOnly;

    impl MainThreadExecutor for MainThreadOnly {
        fn run_on_main_thread(&self, f: Box<dyn FnOnce() + Send>) {
            let on_main = unsafe { libc::pthread_main_np() } == 1;
            assert!(
                on_main,
                "MainThreadOnly executor invoked off the main thread; \
                 --uninstall-self must run on the GUI main thread"
            );
            f();
        }
    }

    let layout = resolve_bundle_layout()?;
    let paths = BusytokPaths::new();
    let platform = PlatformPaths::new();
    let executor: Arc<dyn MainThreadExecutor> = Arc::new(MainThreadOnly);
    let socket_path = paths.control_endpoint().unwrap_or_default();
    let version_probe: Arc<dyn crate::service_lifecycle::smappservice::VersionProbe> =
        Arc::new(ControlClientVersionProbe::new(socket_path));
    let runner: Box<dyn crate::service_lifecycle::command_runner::CommandRunner> =
        Box::new(SystemCommandRunner);
    Ok(Arc::new(SmAppServiceLifecycle::new_with_executor(
        layout,
        paths,
        platform,
        executor,
        version_probe,
        runner,
    )))
}

/// Resolve the [`BundleLayout`] for the currently-running GUI bundle by
/// walking up from `current_exe()` to the enclosing `.app`.
#[cfg(target_os = "macos")]
pub(crate) fn resolve_bundle_layout(
) -> Result<crate::service_lifecycle::bundle_layout::BundleLayout> {
    use crate::service_lifecycle::bundle_layout::BundleLayout;
    let exe = std::env::current_exe().context("resolving current executable")?;
    // Walk up until we find a directory whose name ends with `.app`.
    let mut cursor = exe.parent();
    while let Some(dir) = cursor {
        if dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(".app"))
            .unwrap_or(false)
        {
            return Ok(BundleLayout::for_app_root(dir));
        }
        cursor = dir.parent();
    }
    anyhow::bail!(
        "could not locate enclosing .app bundle for current executable at {}",
        exe.display()
    )
}
