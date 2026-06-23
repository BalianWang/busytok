//! macOS launchd-backed service lifecycle.
//!
//! This is the runtime entry point for installing / starting / stopping /
//! uninstalling the Busytok background service on macOS. The current
//! implementation loads the bundled LaunchAgent plist into the user's Aqua
//! domain with `launchctl bootstrap`/`bootout`. We intentionally avoid
//! calling `SMAppService` on the startup/service mutation path because
//! ServiceManagement can throw Objective-C exceptions that cannot safely
//! cross Rust FFI boundaries.
//!
//! ## Repair ladder
//!
//! [`SmAppServiceLifecycle::ensure_running`] implements the repair ladder
//! documented in the macOS public launch spec:
//!
//! 1. [`cleanup_legacy_launch_agents`] — best-effort bootout + remove any
//!    handwritten `~/Library/LaunchAgents/com.busytok.*.plist` files left
//!    behind by older builds. Recorded as `cleanup_legacy`.
//! 2. Inspect the launchd job snapshot (`launchctl print gui/<uid>/<label>`).
//!    If the job is absent, `bootstrap` the bundled plist, then `kickstart`
//!    and wait for readiness. Recorded as `bootstrap` / `kickstart`.
//! 3. Parse the launchd job snapshot and compare the registered program path
//!    with [`BundleLayout::service_binary_path`]. If they differ the loaded
//!    job is stale (typical cause: the app bundle was moved) -> run
//!    `bootout -> bootstrap -> kickstart -> wait_for_ready` and report
//!    `Upgraded`. Recorded as `inspect_registration` then `bootout` /
//!    `bootstrap` / `kickstart`.
//! 4. Probe the running service's build identity (see [`VersionProbe`]). If
//!    it mismatches the GUI's build identity -> run the same bootout /
//!    bootstrap / kickstart / wait ladder and report `Upgraded`. Recorded as
//!    `detect_version_skew`.
//! 5. Otherwise the launchd job is healthy. If the readiness socket is already
//!    open -> `AlreadyRunning`. If not -> `kickstart -> wait_for_ready` and
//!    report `Started { AlreadyPresent }`.
//!
//! [`stop_for_current_session`] issues `launchctl bootout` against the
//! current Aqua domain — this stops the running instance for the current
//! session. The next app launch will bootstrap the managed user-domain
//! plist again when background service startup is enabled.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use busytok_config::BusytokPaths;
use busytok_platform::PlatformPaths;
use tracing::{error, info, warn};

use super::bundle_layout::{BundleLayout, SERVICE_LABEL};
use super::command_runner::{
    current_uid, launchctl_bootout, launchctl_bootout_strict, launchctl_bootstrap_strict,
    launchctl_domain, launchctl_kickstart_strict, launchctl_label, launchctl_print, CommandRunner,
    CommandStatus,
};
use super::launchd_job_snapshot::LaunchdJobSnapshot;
use super::managed_launch_agent::{ensure_managed_plist_current, managed_plist_path};
use super::proc_pidpath::executable_path_for_pid;
use super::smappservice_bridge::{MainThreadExecutor, SMAppServiceHandle, SMServiceStatus};
use super::{EnsureRunningOutcome, InstallOutcome, LifecycleStatus, ServiceLifecycle};

// ── Public types shared with the rest of the GUI ───────────────────

/// Build identity of the GUI process. Used to detect a version skew
/// between the running background service and the GUI that owns it.
///
/// Today this is the cargo package version compiled into the GUI binary.
/// The lifecycle compares it against the value the running service
/// advertises via [`VersionProbe`]. The comparison is on whatever string
/// the two binaries agree to emit; it intentionally has no semver shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceBuildIdentity {
    version: String,
}

impl ServiceBuildIdentity {
    /// Build identity of the currently-running GUI process.
    pub fn current_gui() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Synthesise an identity that is guaranteed to mismatch
    /// [`Self::current_gui`] — used by tests to exercise the repair path.
    pub fn mismatch() -> Self {
        Self {
            version: "__test_mismatched_service_build__".to_string(),
        }
    }

    /// Underlying version string.
    pub fn as_str(&self) -> &str {
        &self.version
    }
}

/// Result of probing the running service for its build identity.
///
/// `Ok(None)` means "service did not report an identity" (e.g. it has not
/// finished booting, or it predates the build-identity field); the ladder
/// treats this as compatible so the repair path is not triggered on
/// transient probe failures.
#[cfg(target_os = "macos")]
pub(crate) trait VersionProbe: Send + Sync {
    fn probe_service_build_identity(&self) -> Result<Option<String>>;
}

/// Cleanup result returned by [`cleanup_legacy_launch_agents`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupLegacyResult {
    /// Reverse-DNS plist file names (e.g. `com.busytok.service.plist`) that
    /// were removed from `~/Library/LaunchAgents`.
    pub removed_files: Vec<String>,
}

// ── Pure helpers ───────────────────────────────────────────────────

fn resolve_uid() -> Result<u32> {
    let uid_output = Command::new("id")
        .arg("-u")
        .output()
        .context("resolving current uid")?;
    String::from_utf8_lossy(&uid_output.stdout)
        .trim()
        .parse::<u32>()
        .context("parsing current uid")
}

/// Best-effort cleanup of handwritten `~/Library/LaunchAgents/com.busytok.*.plist`
/// files left behind by older builds.
///
/// Returns the list of files removed. Errors are logged but do not abort
/// the lifecycle — the launchd-backed flow does not depend on these files
/// being absent for correctness; they only confuse users looking at
/// `~/Library/LaunchAgents`.
pub fn cleanup_legacy_launch_agents(
    launch_agents_dir: &Path,
    runner: &dyn CommandRunner,
) -> Result<CleanupLegacyResult> {
    let mut removed_files = Vec::new();

    let entries = match fs::read_dir(launch_agents_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Nothing to clean up; record an empty event and return.
            crate::logging::append_bootstrap_event(
                "INFO",
                "service_lifecycle.smappservice.cleanup_complete",
                "no legacy LaunchAgents directory present",
                Some(serde_json::json!({ "removed_files": [] })),
            );
            return Ok(CleanupLegacyResult { removed_files });
        }
        Err(e) => {
            return Err(e).with_context(|| format!("reading {}", launch_agents_dir.display()));
        }
    };

    // First pass: identify candidate plist files. We bootout any matching
    // job so a still-loaded legacy agent does not race the bootstrap.
    //
    // IMPORTANT: the current managed agent plist (`com.busytok.service.plist`,
    // rendered at runtime by `managed_launch_agent`) MUST be preserved here —
    // it is the file production bootstraps from. A blanket "delete all
    // com.busytok.*.plist" would wipe it and reintroduce the "service can't
    // register" failure. We only remove OTHER legacy com.busytok.* plists.
    let managed_name = format!("{}.plist", SERVICE_LABEL);
    let mut to_remove: Vec<(PathBuf, String)> = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Some(file_name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if !file_name.starts_with("com.busytok.") || !file_name.ends_with(".plist") {
            continue;
        }
        if file_name == managed_name {
            // Preserve the runtime-managed agent plist. If it is stale (e.g.
            // left by an older build), ensure_managed_plist_current() in the
            // upcoming bootstrap overwrites it with the correct content.
            continue;
        }
        let label_stem = file_name.trim_end_matches(".plist");
        let domain_label = launchctl_label(current_uid(), label_stem);
        // Best-effort bootout; ignore failure (job may not be loaded).
        let _ = launchctl_bootout(runner, &domain_label);
        to_remove.push((path, file_name));
    }

    for (path, file_name) in to_remove {
        match fs::remove_file(&path) {
            Ok(()) => {
                removed_files.push(file_name);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!(
                event_code = "service_lifecycle.smappservice.cleanup_file_failed",
                path = %path.display(),
                error = %e,
                "failed to remove legacy LaunchAgents file"
            ),
        }
    }

    crate::logging::append_bootstrap_event(
        "INFO",
        "service_lifecycle.smappservice.cleanup_complete",
        "cleaned legacy LaunchAgents entries",
        Some(serde_json::json!({
            "removed_files": removed_files,
        })),
    );

    Ok(CleanupLegacyResult { removed_files })
}

fn wait_for_socket_ready<F>(socket_ready: &F, attempts: usize, delay: Duration) -> bool
where
    F: Fn() -> bool,
{
    for _ in 0..attempts {
        if socket_ready() {
            return true;
        }
        thread::sleep(delay);
    }
    false
}

// ── SmAppServiceLifecycle ──────────────────────────────────────────

/// macOS service lifecycle backed by the bundled LaunchAgent and `launchctl`.
///
/// Owns the inputs the repair ladder needs:
/// - the bundle layout (resolved at construction from the GUI bundle root)
/// - the main-thread executor, retained for remaining direct SMAppService
///   bridge tests and login-item code paths
/// - a [`VersionProbe`] for the version-skew check
/// - a [`CommandRunner`] for the `launchctl` plumbing (bootstrap, kickstart,
///   bootout, print)
///
/// Construction is **not** zero-argument: the executor must come from the
/// Tauri setup (Task 5). The trait-coordinated constructor
/// [`Self::new_with_executor`] is the entry point; the deprecated
/// [`crate::service_lifecycle::current`] singleton is being phased out.
#[cfg(target_os = "macos")]
pub struct SmAppServiceLifecycle {
    layout: BundleLayout,
    paths: BusytokPaths,
    platform: PlatformPaths,
    executor: Arc<dyn MainThreadExecutor>,
    version_probe: Arc<dyn VersionProbe>,
    runner: Box<dyn CommandRunner>,
}

#[cfg(target_os = "macos")]
impl SmAppServiceLifecycle {
    /// Production constructor.
    ///
    /// `executor` must route closures onto the GUI's main thread (Tauri's
    /// `AppHandle::run_on_main_thread`). `version_probe` is the production
    /// [`ControlClientVersionProbe`]; tests pass a fake.
    pub fn new_with_executor(
        layout: BundleLayout,
        paths: BusytokPaths,
        platform: PlatformPaths,
        executor: Arc<dyn MainThreadExecutor>,
        version_probe: Arc<dyn VersionProbe>,
        runner: Box<dyn CommandRunner>,
    ) -> Self {
        Self {
            layout,
            paths,
            platform,
            executor,
            version_probe,
            runner,
        }
    }

    fn service_handle(&self) -> SMAppServiceHandle {
        SMAppServiceHandle::agent(self.layout.service_plist_name())
    }

    /// Preflight: validate the bundle contains the service binary before we
    /// bootstrap a managed plist that points at it. The production lifecycle
    /// bootstraps from the runtime-rendered user-domain plist (see
    /// [`managed_launch_agent`]), NOT the plist baked into the bundle at
    /// build time — so the only bundle artifact the launchd path depends on
    /// is the `busytok-service` executable.
    ///
    /// (SMAppService throws an NSException, not an NSError, when the bundle
    /// is missing the agent plist / binary or is unsigned/unsealed, and that
    /// foreign exception cannot safely cross the Rust FFI boundary. The
    /// launchd-backed production path does not enter that FFI, but we still
    /// refuse to bootstrap a plist whose target binary is absent.)
    fn preflight_bundle(&self, stage: &str) -> Result<()> {
        let binary_path = self.layout.service_binary_path();

        if !binary_path.exists() {
            warn!(
                event_code = "service_lifecycle.smappservice.bundle_preflight_failed",
                stage = stage,
                reason = "service_binary_missing",
                service_binary_path = %binary_path.display(),
                "service binary not found; refusing to bootstrap"
            );
            anyhow::bail!(
                "bundle preflight failed ({}): service binary missing at {}",
                stage,
                binary_path.display()
            );
        }
        Ok(())
    }

    fn service_label(&self) -> String {
        // launchctl requires `gui/<uid>/<label>`; without the uid the
        // command silently targets the wrong domain or fails.
        launchctl_label(current_uid(), SERVICE_LABEL)
    }

    fn service_domain(&self) -> String {
        launchctl_domain(current_uid())
    }

    fn launchctl_print_job(&self) -> Result<Option<CommandStatus>> {
        let label = self.service_label();
        let status = launchctl_print(&*self.runner, &label)?;
        if status.success {
            Ok(Some(status))
        } else {
            tracing::debug!(
                event_code = "service_lifecycle.smappservice.launchctl_job_absent",
                session_id = %crate::logging::tauri_session_id(),
                label = %label,
                exit_code = ?status.exit_code,
                stderr = %status.stderr.trim(),
                "launchctl print did not find a loaded Busytok service job"
            );
            Ok(None)
        }
    }

    fn bootstrap_via_launchctl(&self) -> Result<()> {
        self.preflight_bundle("bootstrap")?;
        // Bootstrap the runtime-rendered user-domain plist, NOT the plist
        // baked into the bundle at build time. The bundled plist carried
        // build-machine absolute paths and pointed at /Users/runner/... on
        // end-user machines; the managed plist is rendered from the current
        // install location so it always points at the real service binary.
        let plist_path = ensure_managed_plist_current(&self.layout, &self.platform)?;
        launchctl_bootstrap_strict(&*self.runner, &self.service_domain(), &plist_path)
    }

    // SMAppService-backed helpers. Retained for the login-item bridge
    // (DesktopLoginStart) and for preflight tests — the production
    // ServiceLifecycle trait methods use launchctl exclusively.
    #[cfg(test)]
    fn status_via_executor(&self) -> Result<SMServiceStatus> {
        self.preflight_bundle("status")?;
        self.service_handle()
            .status_with_executor(self.executor.as_ref())
    }

    #[cfg(test)]
    fn register_via_executor(&self) -> Result<()> {
        self.preflight_bundle("bootstrap")?;
        self.service_handle()
            .register_with_executor(self.executor.as_ref())
    }

    #[cfg(test)]
    fn unregister_via_executor(&self) -> Result<()> {
        self.preflight_bundle("bootout")?;
        self.service_handle()
            .unregister_with_executor(self.executor.as_ref())
    }

    fn socket_ready(&self) -> Result<bool> {
        // Both the marker file AND the control socket must be present.
        // A stale marker (service crashed without cleanup) passes the
        // marker check alone but the socket file will be gone.
        if !busytok_config::service_marker::exists(self.paths.data_dir()) {
            return Ok(false);
        }
        match self.paths.control_endpoint() {
            Ok(socket_path) => Ok(std::path::Path::new(&socket_path).exists()),
            Err(_) => Ok(false),
        }
    }

    fn wait_for_socket_ready(&self) -> Result<()> {
        // Use the same readiness probe as socket_ready(): marker AND
        // socket file must both be present. A stale marker alone is not
        // sufficient — bootstrap/repair must not return success for a
        // service whose control socket is missing.
        let data_dir = self.paths.data_dir().clone();
        let socket_path = self.paths.control_endpoint().ok();
        let ready_fn = || {
            busytok_config::service_marker::exists(&data_dir)
                && socket_path
                    .as_ref()
                    .map(|p| std::path::Path::new(p).exists())
                    .unwrap_or(false)
        };
        let ready = wait_for_socket_ready(&ready_fn, 100, Duration::from_millis(50));
        if ready {
            Ok(())
        } else {
            error!(
                event_code = "service_lifecycle.smappservice.repair_failed",
                session_id = %crate::logging::tauri_session_id(),
                "control socket did not become ready within timeout"
            );
            crate::logging::append_bootstrap_event(
                "ERROR",
                "service_lifecycle.smappservice.repair_failed",
                "control socket did not become ready within timeout",
                None,
            );
            anyhow::bail!("Busytok service did not become ready within timeout")
        }
    }

    /// Implementation of the ladder step "registered, healthy registration
    /// already running". Returns `Some(outcome)` when the caller should
    /// short-circuit with the given outcome.
    fn ladder_registered_present_with_print(
        &self,
        print_status: Option<CommandStatus>,
    ) -> Result<EnsureRunningOutcome> {
        // Inspect the launchd job snapshot for three identity checks.
        let snapshot: Option<LaunchdJobSnapshot> = print_status
            .as_ref()
            .and_then(|s| LaunchdJobSnapshot::parse(&s.stdout).ok());

        if let Some(ref snapshot) = snapshot {
            // 1. Stale bundle path — launchd's registered program path
            //    differs from the current bundle's service binary.
            let desired = self.layout.service_binary_path();
            if let Some(registered) = snapshot.program_path() {
                if registered != desired {
                    info!(
                        event_code = "service_lifecycle.smappservice.detected_stale_bundle",
                        session_id = %crate::logging::tauri_session_id(),
                        registered = %registered.display(),
                        desired = %desired.display(),
                        "service registered to stale bundle path; repairing"
                    );
                    return self.repair_via_ladder(RepairReason::StaleBundle);
                }
            }

            // 2. Stale live process — launchd metadata says the job is
            //    registered to the current bundle, but the actual running
            //    PID's executable is outside the bundle (e.g. a trashed
            //    old build still holding the control socket). This is the
            //    identity gap identified in the stale-live-service bug.
            if let Some(pid) = snapshot.pid() {
                match executable_path_for_pid(pid) {
                    Ok(Some(live_path)) => {
                        // Canonicalize both for comparison — /Applications
                        // vs /private/var/.../Trash resolve differently
                        // even after symlink resolution.
                        let desired_canon = std::fs::canonicalize(&desired)
                            .unwrap_or_else(|_| desired.clone());
                        let live_canon = std::fs::canonicalize(&live_path)
                            .unwrap_or_else(|_| live_path.clone());
                        if live_canon != desired_canon {
                            info!(
                                event_code = "service_lifecycle.smappservice.detected_stale_live_process",
                                session_id = %crate::logging::tauri_session_id(),
                                pid = pid,
                                live_executable = %live_path.display(),
                                desired_executable = %desired.display(),
                                "live PID executable is outside the current bundle; repairing"
                            );
                            crate::logging::append_bootstrap_event(
                                "INFO",
                                "service_lifecycle.smappservice.detected_stale_live_process",
                                "live PID executable is outside the current bundle; repairing",
                                Some(serde_json::json!({
                                    "pid": pid,
                                    "live_executable": live_path.display().to_string(),
                                    "desired_executable": desired.display().to_string(),
                                })),
                            );
                            return self.repair_via_ladder(RepairReason::StaleLiveProcess);
                        }
                    }
                    Ok(None) => {
                        // PID exited between snapshot and inspection —
                        // treat as missing, fall through to socket check.
                        tracing::debug!(
                            event_code = "service_lifecycle.smappservice.pid_exited",
                            pid = pid,
                            "live PID exited between snapshot and inspection"
                        );
                    }
                    Err(e) => {
                        // proc_pidpath failed unexpectedly — log and
                        // continue; don't block bootstrap on this.
                        warn!(
                            event_code = "service_lifecycle.smappservice.proc_pidpath_failed",
                            pid = pid,
                            error = %e,
                            "proc_pidpath failed; skipping live-PID identity check"
                        );
                    }
                }
            }
        }

        // Probe the service's build identity.
        match self.version_probe.probe_service_build_identity() {
            Ok(Some(service_identity)) => {
                if service_identity != ServiceBuildIdentity::current_gui().as_str() {
                    info!(
                        event_code = "service_lifecycle.smappservice.detected_version_skew",
                        session_id = %crate::logging::tauri_session_id(),
                        service_identity = %service_identity,
                        gui_identity = %ServiceBuildIdentity::current_gui().as_str(),
                        "service build identity mismatches the GUI; repairing"
                    );
                    crate::logging::append_bootstrap_event(
                        "INFO",
                        "service_lifecycle.smappservice.detected_version_skew",
                        "service build identity mismatches the GUI; repairing",
                        Some(serde_json::json!({
                            "service_identity": service_identity,
                            "gui_identity": ServiceBuildIdentity::current_gui().as_str(),
                        })),
                    );
                    return self.repair_via_ladder(RepairReason::VersionSkew);
                }
            }
            Ok(None) => {
                // Probe succeeded but the service did not advertise an
                // identity. Treat as compatible.
            }
            Err(e) => {
                warn!(
                    event_code = "service_lifecycle.smappservice.version_probe_failed",
                    session_id = %crate::logging::tauri_session_id(),
                    error = %e,
                    "version probe failed; skipping version-skew repair step"
                );
            }
        }

        // 3. Socket reachability. If the socket is ready (marker + path
        //    exist), the service is healthy.
        if self.socket_ready()? {
            return Ok(EnsureRunningOutcome::AlreadyRunning);
        }

        // 4. Socket unreachable. If launchd says the job is running, a
        //    plain `kickstart` is a no-op — launchd will not restart an
        //    already-running process. Force a full repair instead so the
        //    stale process (if any) is replaced.
        let job_is_running = snapshot
            .as_ref()
            .and_then(|s| s.state())
            .map(|s| s == "running")
            .unwrap_or(false);
        if job_is_running {
            info!(
                event_code = "service_lifecycle.smappservice.detected_socket_unreachable",
                session_id = %crate::logging::tauri_session_id(),
                "launchd reports job running but control socket is unreachable; repairing"
            );
            crate::logging::append_bootstrap_event(
                "INFO",
                "service_lifecycle.smappservice.detected_socket_unreachable",
                "launchd reports job running but control socket is unreachable; repairing",
                None,
            );
            return self.repair_via_ladder(RepairReason::SocketUnreachable);
        }

        // 5. Job is registered but not running — kickstart is sufficient.
        launchctl_kickstart_strict(&*self.runner, &self.service_label())?;
        self.wait_for_socket_ready()?;
        Ok(EnsureRunningOutcome::Started {
            install_outcome: InstallOutcome::AlreadyPresent,
        })
    }

    fn ladder_registered_present(&self) -> Result<EnsureRunningOutcome> {
        let print_status = self.launchctl_print_job()?;
        self.ladder_registered_present_with_print(print_status)
    }

    /// Repair an unhealthy-but-present registration by running
    /// `bootout -> bootstrap -> kickstart -> wait_for_ready`. Returns
    /// `Started { Upgraded }`.
    fn repair_via_ladder(&self, reason: RepairReason) -> Result<EnsureRunningOutcome> {
        launchctl_bootout_strict(&*self.runner, &self.service_label()).map_err(|e| {
            error!(
                event_code = "service_lifecycle.smappservice.repair_failed",
                session_id = %crate::logging::tauri_session_id(),
                stage = "bootout",
                error = %e,
                "service repair failed during bootout"
            );
            e
        })?;
        self.bootstrap_via_launchctl().map_err(|e| {
            error!(
                event_code = "service_lifecycle.smappservice.repair_failed",
                session_id = %crate::logging::tauri_session_id(),
                stage = "bootstrap",
                error = %e,
                "service repair failed during bootstrap"
            );
            e
        })?;
        launchctl_kickstart_strict(&*self.runner, &self.service_label())?;
        self.wait_for_socket_ready()?;

        let event_code = match reason {
            RepairReason::StaleBundle => {
                "service_lifecycle.smappservice.repaired_after_stale_bundle"
            }
            RepairReason::StaleLiveProcess => {
                "service_lifecycle.smappservice.repaired_after_stale_live_process"
            }
            RepairReason::SocketUnreachable => {
                "service_lifecycle.smappservice.repaired_after_socket_unreachable"
            }
            RepairReason::VersionSkew => {
                "service_lifecycle.smappservice.repaired_after_version_skew"
            }
        };
        info!(
            event_code = event_code,
            session_id = %crate::logging::tauri_session_id(),
            "service repaired"
        );
        crate::logging::append_bootstrap_event("INFO", event_code, "service repaired", None);

        Ok(EnsureRunningOutcome::Started {
            install_outcome: InstallOutcome::Upgraded,
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum RepairReason {
    StaleBundle,
    StaleLiveProcess,
    /// launchd says the job is loaded+running but the control socket is
    /// unreachable — a `kickstart` cannot replace a running process, so
    /// the ladder must `bootout → bootstrap → kickstart` to force a fresh
    /// service instance.
    SocketUnreachable,
    VersionSkew,
}

/// Pure downgrade classifier used by [`SmAppServiceLifecycle::status`].
/// launchd can report a loaded job while the service is still starting,
/// wedged, or crash-looped.
/// Validate via socket readiness so diagnostics don't report Running
/// for an unreachable service. Exposed so tests can exercise the
/// downgrade without touching the FFI.
#[cfg(target_os = "macos")]
pub(crate) fn classify_running_status(
    base: LifecycleStatus,
    socket_ready: bool,
) -> LifecycleStatus {
    if base == LifecycleStatus::Running && !socket_ready {
        LifecycleStatus::RegisteredInactive
    } else {
        base
    }
}

#[cfg(target_os = "macos")]
impl ServiceLifecycle for SmAppServiceLifecycle {
    fn ensure_registered(&self) -> Result<InstallOutcome> {
        let launch_agents_dir = self.platform.service_install_root();
        cleanup_legacy_launch_agents(&launch_agents_dir, &*self.runner)?;

        if self.launchctl_print_job()?.is_some() {
            return Ok(InstallOutcome::AlreadyPresent);
        }

        self.bootstrap_via_launchctl()?;
        info!(
            event_code = "service_lifecycle.smappservice.registered",
            session_id = %crate::logging::tauri_session_id(),
            "service newly registered via launchctl bootstrap"
        );
        crate::logging::append_bootstrap_event(
            "INFO",
            "service_lifecycle.smappservice.registered",
            "service newly registered via launchctl bootstrap",
            None,
        );
        Ok(InstallOutcome::NewlyInstalled)
    }

    fn ensure_running(&self) -> Result<EnsureRunningOutcome> {
        let launch_agents_dir = self.platform.service_install_root();
        cleanup_legacy_launch_agents(&launch_agents_dir, &*self.runner)?;

        if let Some(print_status) = self.launchctl_print_job()? {
            return self.ladder_registered_present_with_print(Some(print_status));
        }

        self.bootstrap_via_launchctl()?;
        info!(
            event_code = "service_lifecycle.smappservice.registered",
            session_id = %crate::logging::tauri_session_id(),
            "service newly registered via launchctl bootstrap"
        );
        crate::logging::append_bootstrap_event(
            "INFO",
            "service_lifecycle.smappservice.registered",
            "service newly registered via launchctl bootstrap",
            None,
        );

        if !self.socket_ready()? {
            launchctl_kickstart_strict(&*self.runner, &self.service_label())?;
            info!(
                event_code = "service_lifecycle.smappservice.kickstarted",
                session_id = %crate::logging::tauri_session_id(),
                "service kickstarted after fresh registration"
            );
            crate::logging::append_bootstrap_event(
                "INFO",
                "service_lifecycle.smappservice.kickstarted",
                "service kickstarted after fresh registration",
                None,
            );
        }
        self.wait_for_socket_ready()?;
        Ok(EnsureRunningOutcome::Started {
            install_outcome: InstallOutcome::NewlyInstalled,
        })
    }

    fn status(&self) -> Result<LifecycleStatus> {
        let socket_ready = self.socket_ready().unwrap_or(false);
        let base = if self.launchctl_print_job()?.is_some() {
            if socket_ready {
                LifecycleStatus::Running
            } else {
                LifecycleStatus::RegisteredInactive
            }
        } else {
            LifecycleStatus::NotRegistered
        };
        Ok(classify_running_status(base, socket_ready))
    }

    fn stop_for_current_session(&self) -> Result<()> {
        // Use non-strict bootout: if the job isn't loaded (fresh launch,
        // bootstrap never ran, or already stopped), the post-condition
        // (job not loaded) is already satisfied — treat as success.
        let bootout_status = launchctl_bootout(&*self.runner, &self.service_label())?;
        if !bootout_status.success {
            // Distinguish "job not loaded" (expected on some paths) from
            // genuine failure. launchctl prints to stderr on real errors.
            let stderr = &bootout_status.stderr;
            if stderr.contains("Could not find") || stderr.contains("No such process") {
                info!(
                    event_code = "service_lifecycle.smappservice.bootout_noop",
                    session_id = %crate::logging::tauri_session_id(),
                    "service job was not loaded; bootout is a no-op"
                );
            } else {
                anyhow::bail!(
                    "launchctl bootout failed (exit {:?}): {}",
                    bootout_status.exit_code,
                    stderr.trim()
                );
            }
        }
        // Drop the marker so status() reports inactive for this session.
        let _ = busytok_config::service_marker::remove(self.paths.data_dir());

        info!(
            event_code = "service_lifecycle.smappservice.booted_out_for_session",
            session_id = %crate::logging::tauri_session_id(),
            "service booted out for current session"
        );
        crate::logging::append_bootstrap_event(
            "INFO",
            "service_lifecycle.smappservice.booted_out_for_session",
            "service booted out for current session",
            None,
        );
        Ok(())
    }

    fn uninstall(&self) -> Result<()> {
        // Idempotent: if the job is not loaded, the desired post-state
        // (job absent) is already satisfied. Use non-strict bootout
        // and treat "not loaded" as success, same as stop_for_current_session.
        let bootout_status = launchctl_bootout(&*self.runner, &self.service_label())?;
        if !bootout_status.success {
            let stderr = &bootout_status.stderr;
            if !(stderr.contains("Could not find") || stderr.contains("No such process")) {
                anyhow::bail!(
                    "launchctl bootout failed during uninstall (exit {:?}): {}",
                    bootout_status.exit_code,
                    stderr.trim()
                );
            }
        }
        let _ = busytok_config::service_marker::remove(self.paths.data_dir());

        // Architecture B: the production registration source is the
        // runtime-rendered user-domain plist.  Remove it so that a
        // future login does not attempt to load a job pointing at a
        // no-longer-present bundle.  NotFound is expected (the user may
        // have already deleted the plist manually, or this is a
        // pre-Architecture-B install that never wrote one).
        let managed_path = managed_plist_path(&self.platform);
        match std::fs::remove_file(&managed_path) {
            Ok(()) => {
                info!(
                    event_code = "service_lifecycle.smappservice.uninstalled.managed_plist_removed",
                    session_id = %crate::logging::tauri_session_id(),
                    path = %managed_path.display(),
                    "removed managed launch agent plist"
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Already absent — post-condition satisfied.
            }
            Err(e) => {
                // Non-fatal: the job is already booted-out.
                warn!(
                    event_code = "service_lifecycle.smappservice.uninstalled.managed_plist_remove_failed",
                    session_id = %crate::logging::tauri_session_id(),
                    path = %managed_path.display(),
                    error = %e,
                    "failed to remove managed launch agent plist during uninstall"
                );
            }
        }

        info!(
            event_code = "service_lifecycle.smappservice.uninstalled",
            session_id = %crate::logging::tauri_session_id(),
            "service uninstalled"
        );
        crate::logging::append_bootstrap_event(
            "INFO",
            "service_lifecycle.smappservice.uninstalled",
            "service uninstalled",
            None,
        );
        Ok(())
    }

    fn probe_service_identity(&self) -> Result<Option<String>> {
        match self.version_probe.probe_service_build_identity() {
            Ok(Some(ident)) => Ok(Some(ident)),
            Ok(None) => Ok(None),
            // Probe failures (socket not ready, etc.) are not actionable
            // for diagnostics — return None rather than propagating.
            Err(e) => {
                tracing::debug!(
                    event_code = "service_lifecycle.smappservice.identity_probe_failed",
                    error = %e,
                    "version probe failed during diagnostics"
                );
                Ok(None)
            }
        }
    }
}

// ── Production VersionProbe (control client) ───────────────────────

/// Production [`VersionProbe`] that asks the running service for its
/// build identity via `busytok_control::ControlClient`.
#[cfg(target_os = "macos")]
pub(crate) struct ControlClientVersionProbe {
    socket_path: String,
}

#[cfg(target_os = "macos")]
impl ControlClientVersionProbe {
    pub fn new(socket_path: String) -> Self {
        Self { socket_path }
    }
}

#[cfg(target_os = "macos")]
impl VersionProbe for ControlClientVersionProbe {
    fn probe_service_build_identity(&self) -> Result<Option<String>> {
        if self.socket_path.is_empty() {
            // No endpoint resolved; skip the probe.
            return Ok(None);
        }
        let socket_path = self.socket_path.clone();

        // This is a sync trait method, and callers may already be executing
        // on a Tauri/Tokio worker. Creating a runtime and block_on-ing on
        // that same thread would panic, so isolate the best-effort RPC on a
        // dedicated OS thread with its own current-thread runtime.
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("building tokio runtime for version probe")?;
            rt.block_on(async move {
                use busytok_protocol::dto::{ControlRequest, ControlResponse, ServiceStatusDto};
                let mut client: busytok_control::ControlClient =
                    busytok_control::ControlClient::connect(&socket_path)
                        .await
                        .map_err(|e| anyhow::anyhow!("connecting to control socket: {e}"))?;
                let request = ControlRequest::new("service.status", serde_json::json!({}));
                let response = client
                    .call(request)
                    .await
                    .map_err(|e| anyhow::anyhow!("calling service.status: {e}"))?;
                match response {
                    ControlResponse::Ok(value) => {
                        let dto: ServiceStatusDto = serde_json::from_value(value).map_err(|e| {
                            anyhow::anyhow!("decoding service.status response: {e}")
                        })?;
                        Ok(Some(dto.version))
                    }
                    ControlResponse::Err(err) => {
                        anyhow::bail!("service.status RPC error [{}]: {}", err.code, err.message)
                    }
                }
            })
        })
        .join()
        .map_err(|_| anyhow::anyhow!("version probe thread panicked"))?
    }
}

// ── ServiceBootstrapState hydration (used by desktop_service_status) ──

/// Determines the shell-hydration [`ServiceBootstrapState`] given a build
/// identity. When the running service's identity is known to mismatch the
/// GUI's, hydration must remain in `Repairing` until the lifecycle ladder
/// has repaired the service.
pub fn hydration_state(
    identity: ServiceBuildIdentity,
) -> crate::desktop_service_status::ServiceBootstrapState {
    use crate::desktop_service_status::ServiceBootstrapState;
    if identity == ServiceBuildIdentity::current_gui() {
        ServiceBootstrapState::Ready
    } else {
        ServiceBootstrapState::Repairing
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service_lifecycle::command_runner::{
        CommandRunner, CommandStatus, SystemCommandRunner,
    };
    use std::sync::Arc;
    use tempfile::tempdir;

    // ── Test doubles ────────────────────────────────────────────────

    /// What the fake's `status()` probe should return.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FakeStatus {
        NotRegistered,
        RegisteredInactive,
        Healthy,
        RequiresApproval,
    }

    /// What the launchd-job snapshot returns when the ladder inspects it.
    #[derive(Debug, Clone)]
    enum FakeBundle {
        /// Snapshot's program_path matches the desired bundle path.
        Valid,
        /// Snapshot's program_path is some stale path that doesn't match.
        Stale,
    }

    /// What the version probe returns.
    #[derive(Debug, Clone)]
    enum FakeVersion {
        Matching,
        Mismatch,
        Unknown,
    }

    /// What `bootstrap` does to the readiness socket.
    #[derive(Debug, Clone, Copy)]
    enum FakeRegisterEffect {
        /// After bootstrap, socket is immediately ready (launchd started
        /// the agent).
        AutoStarts,
        /// After bootstrap, socket stays closed (caller must kickstart).
        DoesNotAutoStart,
    }

    /// What `socket_ready()` returns when the ladder asks.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FakeSocket {
        Closed,
        Open,
    }

    /// What the live PID's executable path resolves to.
    #[derive(Debug, Clone)]
    enum FakeLivePid {
        /// proc_pidpath returns the current bundle's service binary.
        CurrentBundle,
        /// proc_pidpath returns a stale path (e.g. trashed old build).
        StaleInTrash,
        /// PID has exited between snapshot and inspection.
        Exited,
    }

    /// What launchd reports as the job state.
    #[derive(Debug, Clone)]
    enum FakeJobState {
        Running,
        Waiting,
        /// Job not loaded — the print call itself returns None.
        NotLoaded,
    }

    /// Configurable scenario for the fake lifecycle.
    #[derive(Debug, Clone)]
    struct FakeScenario {
        status: FakeStatus,
        bundle: FakeBundle,
        version: FakeVersion,
        register_effect: FakeRegisterEffect,
        socket_when_registered: FakeSocket,
        live_pid: FakeLivePid,
        job_state: FakeJobState,
    }

    impl FakeScenario {
        fn not_registered() -> Self {
            Self {
                status: FakeStatus::NotRegistered,
                bundle: FakeBundle::Valid,
                version: FakeVersion::Matching,
                register_effect: FakeRegisterEffect::DoesNotAutoStart,
                socket_when_registered: FakeSocket::Closed,
                live_pid: FakeLivePid::CurrentBundle,
                job_state: FakeJobState::NotLoaded,
            }
        }

        fn registers_and_auto_starts() -> Self {
            Self {
                status: FakeStatus::NotRegistered,
                bundle: FakeBundle::Valid,
                version: FakeVersion::Matching,
                register_effect: FakeRegisterEffect::AutoStarts,
                socket_when_registered: FakeSocket::Open,
                live_pid: FakeLivePid::CurrentBundle,
                job_state: FakeJobState::NotLoaded,
            }
        }

        fn running() -> Self {
            Self {
                status: FakeStatus::Healthy,
                bundle: FakeBundle::Valid,
                version: FakeVersion::Matching,
                register_effect: FakeRegisterEffect::DoesNotAutoStart,
                socket_when_registered: FakeSocket::Open,
                live_pid: FakeLivePid::CurrentBundle,
                job_state: FakeJobState::Running,
            }
        }

        fn healthy_registered() -> Self {
            Self {
                status: FakeStatus::Healthy,
                bundle: FakeBundle::Valid,
                version: FakeVersion::Matching,
                register_effect: FakeRegisterEffect::DoesNotAutoStart,
                socket_when_registered: FakeSocket::Open,
                live_pid: FakeLivePid::CurrentBundle,
                job_state: FakeJobState::Running,
            }
        }

        fn registered_inactive_with_valid_bundle() -> Self {
            Self {
                status: FakeStatus::RegisteredInactive,
                bundle: FakeBundle::Valid,
                version: FakeVersion::Matching,
                register_effect: FakeRegisterEffect::DoesNotAutoStart,
                socket_when_registered: FakeSocket::Closed,
                live_pid: FakeLivePid::CurrentBundle,
                job_state: FakeJobState::Waiting,
            }
        }

        fn registered_to_stale_bundle() -> Self {
            Self {
                status: FakeStatus::RegisteredInactive,
                bundle: FakeBundle::Stale,
                version: FakeVersion::Matching,
                register_effect: FakeRegisterEffect::DoesNotAutoStart,
                socket_when_registered: FakeSocket::Closed,
                live_pid: FakeLivePid::CurrentBundle,
                job_state: FakeJobState::Running,
            }
        }

        fn running_old_service_version() -> Self {
            Self {
                status: FakeStatus::RegisteredInactive,
                bundle: FakeBundle::Valid,
                version: FakeVersion::Mismatch,
                register_effect: FakeRegisterEffect::DoesNotAutoStart,
                socket_when_registered: FakeSocket::Closed,
                live_pid: FakeLivePid::CurrentBundle,
                job_state: FakeJobState::Running,
            }
        }

        // ── New scenarios for StaleLiveProcess + SocketUnreachable ────

        /// Registered path is correct (current bundle) but the live PID's
        /// executable is in the Trash — the stale-live-process identity gap.
        fn registered_current_but_live_process_stale_in_trash() -> Self {
            Self {
                status: FakeStatus::Healthy,
                bundle: FakeBundle::Valid,
                version: FakeVersion::Matching,
                register_effect: FakeRegisterEffect::DoesNotAutoStart,
                socket_when_registered: FakeSocket::Closed,
                live_pid: FakeLivePid::StaleInTrash,
                job_state: FakeJobState::Running,
            }
        }

        /// Job is running but the socket is unreachable. A kickstart
        /// cannot replace a running process — must repair.
        fn running_but_socket_unreachable() -> Self {
            Self {
                status: FakeStatus::Healthy,
                bundle: FakeBundle::Valid,
                version: FakeVersion::Matching,
                register_effect: FakeRegisterEffect::DoesNotAutoStart,
                socket_when_registered: FakeSocket::Closed,
                live_pid: FakeLivePid::CurrentBundle,
                job_state: FakeJobState::Running,
            }
        }

        /// Job is waiting (not running), socket closed — kickstart is
        /// sufficient, no repair needed.
        fn waiting_with_socket_closed() -> Self {
            Self {
                status: FakeStatus::RegisteredInactive,
                bundle: FakeBundle::Valid,
                version: FakeVersion::Matching,
                register_effect: FakeRegisterEffect::DoesNotAutoStart,
                socket_when_registered: FakeSocket::Closed,
                live_pid: FakeLivePid::CurrentBundle,
                job_state: FakeJobState::Waiting,
            }
        }
    }

    /// Test double for [`SmAppServiceLifecycle`] that records each repair
    /// ladder step as a string and bypasses the FFI entirely.
    struct FakeMacLifecycle {
        scenario: FakeScenario,
        actions: Mutex<Vec<&'static str>>,
        unregister_called: Mutex<bool>,
        /// Mutable readiness flip used by `kickstart` / `register`.
        socket_open: Mutex<bool>,
        /// Layout used to compute the desired bundle path.
        layout: BundleLayout,
    }

    impl FakeMacLifecycle {
        fn new(scenario: FakeScenario) -> Self {
            let socket_open = match scenario.status {
                FakeStatus::Healthy => matches!(scenario.socket_when_registered, FakeSocket::Open),
                _ => false,
            };
            Self {
                scenario,
                actions: Mutex::new(Vec::new()),
                unregister_called: Mutex::new(false),
                socket_open: Mutex::new(socket_open),
                layout: BundleLayout::for_app_root("/Applications/Busytok.app"),
            }
        }

        fn recorded_actions(&self) -> Vec<String> {
            self.actions
                .lock()
                .unwrap()
                .iter()
                .map(|s| s.to_string())
                .collect()
        }

        fn unregister_called(&self) -> bool {
            *self.unregister_called.lock().unwrap()
        }

        fn record(&self, action: &'static str) {
            self.actions.lock().unwrap().push(action);
        }

        fn status_probe(&self) -> FakeStatus {
            self.scenario.status
        }

        fn program_path_probe(&self) -> Option<PathBuf> {
            match self.scenario.bundle {
                FakeBundle::Valid => Some(self.layout.service_binary_path()),
                FakeBundle::Stale => Some(PathBuf::from(
                    "/Old/Busytok.app/Contents/MacOS/busytok-service",
                )),
            }
        }

        fn live_pid_probe(&self) -> Option<PathBuf> {
            match self.scenario.live_pid {
                FakeLivePid::CurrentBundle => Some(self.layout.service_binary_path()),
                FakeLivePid::StaleInTrash => Some(PathBuf::from(
                    "/Users/wsd/.Trash/Busytok.app/Contents/MacOS/busytok-service",
                )),
                FakeLivePid::Exited => None,
            }
        }

        fn job_is_running(&self) -> bool {
            matches!(self.scenario.job_state, FakeJobState::Running)
        }

        fn version_probe(&self) -> Result<Option<String>> {
            Ok(match self.scenario.version {
                FakeVersion::Matching => {
                    Some(ServiceBuildIdentity::current_gui().as_str().to_string())
                }
                FakeVersion::Mismatch => {
                    Some(ServiceBuildIdentity::mismatch().as_str().to_string())
                }
                FakeVersion::Unknown => None,
            })
        }

        fn socket_ready(&self) -> bool {
            *self.socket_open.lock().unwrap()
        }

        /// Implementation of the repair ladder, mirroring
        /// `SmAppServiceLifecycle::ensure_running` step by step.
        fn ensure_running_inner(&self) -> Result<EnsureRunningOutcome> {
            self.record("cleanup_legacy");

            match self.status_probe() {
                FakeStatus::NotRegistered => {
                    self.do_register();
                    if self.socket_ready() {
                        self.record("wait_ready");
                        return Ok(EnsureRunningOutcome::Started {
                            install_outcome: InstallOutcome::NewlyInstalled,
                        });
                    }
                    self.do_kickstart();
                    self.record("wait_ready");
                    return Ok(EnsureRunningOutcome::Started {
                        install_outcome: InstallOutcome::NewlyInstalled,
                    });
                }
                FakeStatus::RequiresApproval => {
                    anyhow::bail!("service requires user approval");
                }
                FakeStatus::RegisteredInactive | FakeStatus::Healthy => {
                    // fall through to the inspect ladder
                }
            }

            self.record("inspect_registration");

            // 1. Stale bundle check — launchd registered path ≠ desired.
            if let Some(registered) = self.program_path_probe() {
                if registered != self.layout.service_binary_path() {
                    self.do_repair();
                    return Ok(EnsureRunningOutcome::Started {
                        install_outcome: InstallOutcome::Upgraded,
                    });
                }
            }

            // 2. Stale live process check — live PID executable path ≠ desired.
            if let Some(live_path) = self.live_pid_probe() {
                if live_path != self.layout.service_binary_path() {
                    self.record("detect_stale_live_process");
                    self.do_repair();
                    return Ok(EnsureRunningOutcome::Started {
                        install_outcome: InstallOutcome::Upgraded,
                    });
                }
            }

            // 3. Version skew check.
            match self.version_probe()? {
                Some(id) if id != ServiceBuildIdentity::current_gui().as_str() => {
                    self.record("detect_version_skew");
                    self.do_repair();
                    return Ok(EnsureRunningOutcome::Started {
                        install_outcome: InstallOutcome::Upgraded,
                    });
                }
                _ => {}
            }

            // 4. Healthy — socket ready.
            if self.socket_ready() {
                self.record("wait_ready");
                return Ok(EnsureRunningOutcome::AlreadyRunning);
            }

            // 5. Socket unreachable + job running — kickstart is a no-op
            //    on a running process; must force repair.
            if self.job_is_running() {
                self.record("detect_socket_unreachable");
                self.do_repair();
                return Ok(EnsureRunningOutcome::Started {
                    install_outcome: InstallOutcome::Upgraded,
                });
            }

            // 6. Job is not running — kickstart is sufficient.
            self.do_kickstart();
            self.record("wait_ready");
            Ok(EnsureRunningOutcome::Started {
                install_outcome: InstallOutcome::AlreadyPresent,
            })
        }

        fn do_register(&self) {
            self.record("bootstrap");
            if matches!(
                self.scenario.register_effect,
                FakeRegisterEffect::AutoStarts
            ) {
                *self.socket_open.lock().unwrap() = true;
            }
        }

        fn do_kickstart(&self) {
            self.record("kickstart");
            *self.socket_open.lock().unwrap() = true;
        }

        fn do_repair(&self) {
            self.do_unregister();
            self.do_register();
            self.do_kickstart();
            self.record("wait_ready");
        }

        fn do_unregister(&self) {
            self.record("bootout");
            *self.unregister_called.lock().unwrap() = true;
            // After unregister, the socket is closed until re-register.
            *self.socket_open.lock().unwrap() = false;
        }
    }

    impl ServiceLifecycle for FakeMacLifecycle {
        fn ensure_registered(&self) -> Result<InstallOutcome> {
            self.record("cleanup_legacy");
            match self.status_probe() {
                FakeStatus::NotRegistered => {
                    self.do_register();
                    Ok(InstallOutcome::NewlyInstalled)
                }
                FakeStatus::RequiresApproval => {
                    anyhow::bail!("service requires user approval")
                }
                _ => Ok(InstallOutcome::AlreadyPresent),
            }
        }

        fn ensure_running(&self) -> Result<EnsureRunningOutcome> {
            self.ensure_running_inner()
        }

        fn status(&self) -> Result<LifecycleStatus> {
            Ok(match self.status_probe() {
                FakeStatus::NotRegistered => LifecycleStatus::NotRegistered,
                FakeStatus::RegisteredInactive => LifecycleStatus::RegisteredInactive,
                FakeStatus::Healthy => LifecycleStatus::Running,
                FakeStatus::RequiresApproval => LifecycleStatus::NeedsAttention,
            })
        }

        fn stop_for_current_session(&self) -> Result<()> {
            self.record("bootout");
            Ok(())
        }

        fn uninstall(&self) -> Result<()> {
            Ok(())
        }
    }

    // ── Recorded-action test helpers ────────────────────────────────

    impl FakeMacLifecycle {
        fn not_registered() -> Self {
            Self::new(FakeScenario::not_registered())
        }
        fn running() -> Self {
            Self::new(FakeScenario::running())
        }
        fn registered_to_stale_bundle() -> Self {
            Self::new(FakeScenario::registered_to_stale_bundle())
        }
        fn registers_and_auto_starts() -> Self {
            Self::new(FakeScenario::registers_and_auto_starts())
        }
        fn registered_inactive_with_valid_bundle() -> Self {
            Self::new(FakeScenario::registered_inactive_with_valid_bundle())
        }
        fn healthy_registered() -> Self {
            Self::new(FakeScenario::healthy_registered())
        }
        fn running_old_service_version() -> Self {
            Self::new(FakeScenario::running_old_service_version())
        }
        fn registered_current_but_live_process_stale_in_trash() -> Self {
            Self::new(FakeScenario::registered_current_but_live_process_stale_in_trash())
        }
        fn running_but_socket_unreachable() -> Self {
            Self::new(FakeScenario::running_but_socket_unreachable())
        }
        fn waiting_with_socket_closed() -> Self {
            Self::new(FakeScenario::waiting_with_socket_closed())
        }
    }

    // ── Spec-required tests ─────────────────────────────────────────

    #[test]
    fn ensure_running_registers_service_then_kickstarts_when_not_registered() {
        let fake = FakeMacLifecycle::not_registered();
        let outcome = fake.ensure_running().unwrap();
        assert_eq!(
            outcome,
            EnsureRunningOutcome::Started {
                install_outcome: InstallOutcome::NewlyInstalled,
            }
        );
        assert_eq!(
            fake.recorded_actions(),
            ["cleanup_legacy", "bootstrap", "kickstart", "wait_ready"]
        );
    }

    #[test]
    fn stop_for_current_session_boots_out_without_unregistering() {
        let fake = FakeMacLifecycle::running();
        fake.stop_for_current_session().unwrap();
        assert_eq!(fake.recorded_actions(), ["bootout"]);
        assert!(!fake.unregister_called());
    }

    #[test]
    fn ensure_running_re_registers_when_bundle_path_is_stale_after_app_move() {
        let fake = FakeMacLifecycle::registered_to_stale_bundle();
        let outcome = fake.ensure_running().unwrap();
        assert_eq!(
            outcome,
            EnsureRunningOutcome::Started {
                install_outcome: InstallOutcome::Upgraded,
            }
        );
        assert_eq!(
            fake.recorded_actions(),
            [
                "cleanup_legacy",
                "inspect_registration",
                "bootout",
                "bootstrap",
                "kickstart",
                "wait_ready",
            ]
        );
    }

    #[test]
    fn ensure_running_returns_started_when_register_auto_launches_service() {
        let fake = FakeMacLifecycle::registers_and_auto_starts();
        let outcome = fake.ensure_running().unwrap();
        assert_eq!(
            outcome,
            EnsureRunningOutcome::Started {
                install_outcome: InstallOutcome::NewlyInstalled,
            }
        );
        assert_eq!(
            fake.recorded_actions(),
            ["cleanup_legacy", "bootstrap", "wait_ready"]
        );
    }

    #[test]
    fn registered_inactive_but_bundle_valid_prefers_kickstart_over_reregister() {
        let fake = FakeMacLifecycle::registered_inactive_with_valid_bundle();
        let outcome = fake.ensure_running().unwrap();
        assert_eq!(
            outcome,
            EnsureRunningOutcome::Started {
                install_outcome: InstallOutcome::AlreadyPresent,
            }
        );
        assert_eq!(
            fake.recorded_actions(),
            [
                "cleanup_legacy",
                "inspect_registration",
                "kickstart",
                "wait_ready",
            ]
        );
        assert!(!fake.unregister_called());
    }

    #[test]
    fn healthy_registration_never_unregisters_on_normal_launch() {
        let fake = FakeMacLifecycle::healthy_registered();
        let outcome = fake.ensure_running().unwrap();
        assert_eq!(outcome, EnsureRunningOutcome::AlreadyRunning);
        assert_eq!(
            fake.recorded_actions(),
            ["cleanup_legacy", "inspect_registration", "wait_ready"]
        );
        assert!(!fake.unregister_called());
    }

    #[test]
    fn ensure_running_repairs_when_service_version_mismatches_gui_bundle() {
        let fake = FakeMacLifecycle::running_old_service_version();
        let outcome = fake.ensure_running().unwrap();
        assert_eq!(
            outcome,
            EnsureRunningOutcome::Started {
                install_outcome: InstallOutcome::Upgraded,
            }
        );
        assert_eq!(
            fake.recorded_actions(),
            [
                "cleanup_legacy",
                "inspect_registration",
                "detect_version_skew",
                "bootout",
                "bootstrap",
                "kickstart",
                "wait_ready",
            ]
        );
    }

    #[test]
    fn shell_hydration_blocks_on_known_version_skew() {
        let state = hydration_state(ServiceBuildIdentity::mismatch());
        assert_eq!(
            state,
            crate::desktop_service_status::ServiceBootstrapState::Repairing
        );
    }

    // ── StaleLiveProcess + SocketUnreachable tests ──────────────────

    #[test]
    fn ensure_running_repairs_stale_live_process_when_registered_path_is_current() {
        // The exact bug: registered path matches the current bundle, but
        // the live PID is running from a trashed old build whose socket
        // is gone. The ladder must detect this and repair.
        let fake = FakeMacLifecycle::registered_current_but_live_process_stale_in_trash();
        let outcome = fake.ensure_running().unwrap();
        assert_eq!(
            outcome,
            EnsureRunningOutcome::Started {
                install_outcome: InstallOutcome::Upgraded,
            }
        );
        assert_eq!(
            fake.recorded_actions(),
            [
                "cleanup_legacy",
                "inspect_registration",
                "detect_stale_live_process",
                "bootout",
                "bootstrap",
                "kickstart",
                "wait_ready",
            ]
        );
    }

    #[test]
    fn ensure_running_repairs_when_socket_unreachable_and_job_is_running() {
        // Job is running (launchd says so) but socket is unreachable.
        // A plain kickstart would be a no-op — must force repair.
        let fake = FakeMacLifecycle::running_but_socket_unreachable();
        let outcome = fake.ensure_running().unwrap();
        assert_eq!(
            outcome,
            EnsureRunningOutcome::Started {
                install_outcome: InstallOutcome::Upgraded,
            }
        );
        assert_eq!(
            fake.recorded_actions(),
            [
                "cleanup_legacy",
                "inspect_registration",
                "detect_socket_unreachable",
                "bootout",
                "bootstrap",
                "kickstart",
                "wait_ready",
            ]
        );
    }

    #[test]
    fn ensure_running_kickstarts_when_socket_closed_but_job_is_waiting() {
        // Job is NOT running (waiting state), socket closed — kickstart
        // is sufficient here since launchd CAN start a non-running job.
        let fake = FakeMacLifecycle::waiting_with_socket_closed();
        let outcome = fake.ensure_running().unwrap();
        assert_eq!(
            outcome,
            EnsureRunningOutcome::Started {
                install_outcome: InstallOutcome::AlreadyPresent,
            }
        );
        assert_eq!(
            fake.recorded_actions(),
            [
                "cleanup_legacy",
                "inspect_registration",
                "kickstart",
                "wait_ready",
            ]
        );
        assert!(!fake.unregister_called());
    }

    #[test]
    fn ensure_running_pid_exited_falls_through_to_socket_unreachable_repair() {
        // PID exits between LaunchdJobSnapshot and proc_pidpath — Ok(None).
        // The ladder must fall through to the socket check, detect the socket
        // is unreachable on a running job, and repair.
        let scenario = FakeScenario {
            live_pid: FakeLivePid::Exited,
            ..FakeScenario::running_but_socket_unreachable()
        };
        let fake = FakeMacLifecycle::new(scenario);
        let outcome = fake.ensure_running().unwrap();
        assert_eq!(
            outcome,
            EnsureRunningOutcome::Started {
                install_outcome: InstallOutcome::Upgraded,
            }
        );
        let actions = fake.recorded_actions();
        assert!(
            actions.contains(&"detect_socket_unreachable".to_string()),
            "PID exited should fall through to socket-unreachable repair, got: {actions:?}"
        );
    }

    #[test]
    fn ensure_running_stale_live_process_takes_priority_over_version_probe() {
        // When both conditions exist, StaleLiveProcess repairs first
        // (it's checked before version probe). The version probe may
        // fail because the stale process's socket is gone, but the
        // ladder should not depend on that — it should detect the
        // stale live PID directly.
        let scenario = FakeScenario {
            version: FakeVersion::Mismatch,
            ..FakeScenario::registered_current_but_live_process_stale_in_trash()
        };
        let fake = FakeMacLifecycle::new(scenario);
        let outcome = fake.ensure_running().unwrap();
        assert_eq!(
            outcome,
            EnsureRunningOutcome::Started {
                install_outcome: InstallOutcome::Upgraded,
            }
        );
        // The StaleLiveProcess check fires before version probe, so
        // we see detect_stale_live_process, not detect_version_skew.
        let actions = fake.recorded_actions();
        assert!(
            actions.contains(&"detect_stale_live_process".to_string()),
            "must detect stale live process, got: {actions:?}"
        );
        assert!(
            !actions.contains(&"detect_version_skew".to_string()),
            "version skew must NOT trigger before stale live process"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn control_client_version_probe_does_not_panic_inside_tokio_runtime() {
        let socket_path = std::env::temp_dir()
            .join(format!(
                "busytok-missing-version-probe-{}.sock",
                std::process::id()
            ))
            .display()
            .to_string();
        let probe = ControlClientVersionProbe::new(socket_path);

        let result = probe.probe_service_build_identity();

        assert!(
            result.is_err(),
            "missing socket should return a normal probe error, not panic from nested Tokio runtime"
        );
    }

    // ── cleanup_legacy_launch_agents pure test ──────────────────────

    /// Command runner used by the cleanup test — accepts all calls and
    /// reports success without touching `launchctl`.
    struct PermissiveRunner;
    impl CommandRunner for PermissiveRunner {
        fn run(&self, _program: &str, _args: &[String]) -> Result<CommandStatus> {
            Ok(CommandStatus {
                success: true,
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    /// Recording runner that captures every arg list for later inspection.
    /// Used when tests need to assert the exact command-line args, not
    /// just that something ran.
    struct CapturingRunner {
        calls: Mutex<Vec<Vec<String>>>,
    }
    impl CapturingRunner {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }
    impl CommandRunner for CapturingRunner {
        fn run(&self, _program: &str, args: &[String]) -> Result<CommandStatus> {
            self.calls.lock().unwrap().push(args.to_vec());
            Ok(CommandStatus {
                success: true,
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn cleanup_legacy_launch_agents_preserves_managed_removes_others() {
        let temp_home = tempdir().unwrap();
        let launch_agents = temp_home.path().join("Library/LaunchAgents");
        fs::create_dir_all(&launch_agents).unwrap();
        // The managed agent plist MUST be preserved — production bootstraps
        // from it. A blanket "delete all com.busytok.*.plist" would wipe it
        // and reintroduce the registration failure.
        fs::write(
            launch_agents.join("com.busytok.service.plist"),
            "managed plist (current)",
        )
        .unwrap();
        // A genuinely-legacy com.busytok.* plist from an older build IS removed.
        fs::write(
            launch_agents.join("com.busytok.legacy.plist"),
            "legacy plist",
        )
        .unwrap();
        // Unrelated file should survive.
        fs::write(launch_agents.join("com.other.dev.plist"), "untouched").unwrap();

        let runner = PermissiveRunner;
        let result = cleanup_legacy_launch_agents(&launch_agents, &runner).unwrap();
        assert!(
            !result
                .removed_files
                .contains(&"com.busytok.service.plist".to_string()),
            "managed plist must be preserved, got removed_files: {:?}",
            result.removed_files
        );
        assert!(launch_agents.join("com.busytok.service.plist").exists());
        assert!(
            result
                .removed_files
                .contains(&"com.busytok.legacy.plist".to_string()),
            "legacy com.busytok.* plist must be removed"
        );
        assert!(!launch_agents.join("com.busytok.legacy.plist").exists());
        // Untouched file is preserved.
        assert!(launch_agents.join("com.other.dev.plist").exists());
    }

    #[test]
    fn cleanup_legacy_uses_uid_in_launchctl_label() {
        let temp_home = tempdir().unwrap();
        let launch_agents = temp_home.path().join("Library/LaunchAgents");
        fs::create_dir_all(&launch_agents).unwrap();
        // Use a NON-managed legacy name so cleanup actually bootouts it
        // (the managed com.busytok.service.plist is now preserved/skipped).
        fs::write(
            launch_agents.join("com.busytok.legacy.plist"),
            "legacy plist",
        )
        .unwrap();

        let runner = CapturingRunner::new();
        let _ = cleanup_legacy_launch_agents(&launch_agents, &runner).unwrap();

        let all_args: Vec<String> = runner
            .calls()
            .iter()
            .flat_map(|c| c.iter().cloned())
            .collect();
        let full = all_args.join(" ");
        // Format must be gui/<uid>/<label>, not gui/<label>.
        let expected_prefix = format!("bootout gui/{}/", current_uid());
        assert!(
            full.contains(&expected_prefix),
            "cleanup bootout label must include uid. Got args: {full}"
        );
    }

    fn cleanup_legacy_launch_agents_handles_missing_dir() {
        let runner = PermissiveRunner;
        let temp = tempdir().unwrap();
        let missing = temp.path().join("does/not/exist");
        let result = cleanup_legacy_launch_agents(&missing, &runner).unwrap();
        assert!(result.removed_files.is_empty());
    }

    // ── Production lifecycle integration coverage ──────────────────
    //
    // The FFI-backed status/register/unregister methods can only be
    // exercised against a real macOS ServiceManagement context, which is
    // not available in unit tests. The tests below exercise the
    // non-FFI production paths (`cleanup_legacy_launch_agents`,
    // `socket_ready`, `wait_for_socket_ready`) by constructing a real
    // `SmAppServiceLifecycle` with stubbed executor + runner. This
    // catches drift between the prod struct's non-FFI methods and the
    // fake used in the ladder tests above.

    /// Stub executor that runs closures inline. Real SMAppService calls
    /// would still assert `pthread_main_np()`, but the lifecycle methods
    /// exercised here do not touch SMAppService — they only do filesystem
    /// + launchctl-shell work.
    struct InlineExecutor;
    impl MainThreadExecutor for InlineExecutor {
        fn run_on_main_thread(&self, f: Box<dyn FnOnce() + Send>) {
            f();
        }
    }

    struct PanicExecutor;
    impl MainThreadExecutor for PanicExecutor {
        fn run_on_main_thread(&self, _f: Box<dyn FnOnce() + Send>) {
            panic!("cold-start ensure_running must not call SMAppService through the executor");
        }
    }

    /// Version probe stub that always returns `None` (no skew signal).
    struct NoIdentityProbe;
    impl VersionProbe for NoIdentityProbe {
        fn probe_service_build_identity(&self) -> Result<Option<String>> {
            Ok(None)
        }
    }

    struct LaunchctlPrintRunner {
        stdout: String,
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl LaunchctlPrintRunner {
        fn loaded_job_for(program_path: &Path) -> Self {
            Self {
                stdout: format!("program = {};\n", program_path.display()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl CommandRunner for LaunchctlPrintRunner {
        fn run(&self, program: &str, args: &[String]) -> Result<CommandStatus> {
            self.calls.lock().unwrap().push(args.to_vec());
            assert_eq!(program, "launchctl");
            assert_eq!(args.first().map(String::as_str), Some("print"));
            Ok(CommandStatus {
                success: true,
                exit_code: Some(0),
                stdout: self.stdout.clone(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn prod_ensure_running_never_dispatches_to_smappservice_executor() {
        let temp = tempdir().unwrap();
        let app_root = temp.path().join("Busytok.app");
        let layout = BundleLayout::for_app_root(&app_root);
        let paths = BusytokPaths::for_test(&temp.path().join("data"));
        paths.ensure_dirs_exist().unwrap();
        busytok_config::service_marker::write(paths.data_dir()).unwrap();
        let socket_path = paths.control_endpoint().unwrap();
        std::fs::create_dir_all(std::path::Path::new(&socket_path).parent().unwrap()).unwrap();
        std::fs::write(&socket_path, "").unwrap();

        let platform = PlatformPaths::with_home_dir(temp.path().join("home"));
        let executor: Arc<dyn MainThreadExecutor> = Arc::new(PanicExecutor);
        let probe: Arc<dyn VersionProbe> = Arc::new(NoIdentityProbe);
        let runner: Box<dyn CommandRunner> = Box::new(LaunchctlPrintRunner::loaded_job_for(
            &layout.service_binary_path(),
        ));
        let lc = SmAppServiceLifecycle::new_with_executor(
            layout, paths, platform, executor, probe, runner,
        );

        let outcome = lc.ensure_running().unwrap();
        assert_eq!(outcome, EnsureRunningOutcome::AlreadyRunning);
    }

    #[test]
    fn prod_lifecycle_cleanup_legacy_launch_agents_works_with_real_struct() {
        let layout = BundleLayout::for_app_root("/Applications/Busytok.app");
        let paths = BusytokPaths::for_test(
            &std::env::temp_dir().join(format!("busytok-prod-test-{}", uuid::Uuid::new_v4())),
        );
        paths.ensure_dirs_exist().unwrap();
        let platform = PlatformPaths::new();
        let executor: Arc<dyn MainThreadExecutor> = Arc::new(InlineExecutor);
        let probe: Arc<dyn VersionProbe> = Arc::new(NoIdentityProbe);
        let runner: Box<dyn CommandRunner> = Box::new(PermissiveRunner);

        let lc = SmAppServiceLifecycle::new_with_executor(
            layout,
            paths.clone(),
            platform,
            executor,
            probe,
            runner,
        );

        // Pre-create the LaunchAgents directory with the managed agent
        // plist (which production bootstraps from and cleanup MUST
        // preserve) plus a genuinely-legacy com.busytok.* plist (which
        // cleanup must remove).
        let launch_agents_dir = lc.platform.service_install_root();
        std::fs::create_dir_all(&launch_agents_dir).unwrap();
        let managed_plist = launch_agents_dir.join("com.busytok.service.plist");
        std::fs::write(&managed_plist, "managed (current)").unwrap();
        let legacy_plist = launch_agents_dir.join("com.busytok.legacy.plist");
        std::fs::write(&legacy_plist, "stale").unwrap();

        let result = cleanup_legacy_launch_agents(&launch_agents_dir, &*lc.runner).unwrap();
        assert!(
            !result
                .removed_files
                .contains(&"com.busytok.service.plist".to_string()),
            "production cleanup must preserve the managed plist"
        );
        assert!(managed_plist.exists(), "managed plist must survive cleanup");
        assert!(
            result
                .removed_files
                .contains(&"com.busytok.legacy.plist".to_string()),
            "production cleanup must remove legacy com.busytok.* plists"
        );
        assert!(!legacy_plist.exists());
    }

    #[test]
    fn prod_lifecycle_socket_ready_reads_real_marker_file() {
        let layout = BundleLayout::for_app_root("/Applications/Busytok.app");
        let paths = BusytokPaths::for_test(
            &std::env::temp_dir()
                .join(format!("busytok-prod-socket-test-{}", uuid::Uuid::new_v4())),
        );
        paths.ensure_dirs_exist().unwrap();
        let platform = PlatformPaths::new();
        let executor: Arc<dyn MainThreadExecutor> = Arc::new(InlineExecutor);
        let probe: Arc<dyn VersionProbe> = Arc::new(NoIdentityProbe);
        let runner: Box<dyn CommandRunner> = Box::new(PermissiveRunner);
        let lc = SmAppServiceLifecycle::new_with_executor(
            layout,
            paths.clone(),
            platform,
            executor,
            probe,
            runner,
        );

        // Marker absent -> socket not ready.
        assert!(!lc.socket_ready().unwrap());

        // Write marker -> not enough; socket file must also exist.
        busytok_config::service_marker::write(paths.data_dir()).unwrap();
        assert!(
            !lc.socket_ready().unwrap(),
            "marker alone must not satisfy socket_ready — socket file must exist"
        );

        // Create the socket file so both checks pass.
        let socket_path = paths.control_endpoint().unwrap();
        std::fs::create_dir_all(std::path::Path::new(&socket_path).parent().unwrap()).unwrap();
        std::fs::write(&socket_path, "").unwrap();
        assert!(lc.socket_ready().unwrap());

        // Remove marker but keep socket -> not ready (marker is primary signal).
        busytok_config::service_marker::remove(paths.data_dir()).unwrap();
        assert!(!lc.socket_ready().unwrap());
    }

    // ── uninstall managed-plist regression ──────────────────────────

    #[test]
    fn uninstall_removes_managed_plist_when_present() {
        let temp_home = tempdir().unwrap();
        let launch_agents = temp_home.path().join("Library/LaunchAgents");
        fs::create_dir_all(&launch_agents).unwrap();
        let managed = launch_agents.join("com.busytok.service.plist");
        fs::write(&managed, "<fake/>").unwrap();
        assert!(managed.exists());

        let layout = BundleLayout::for_app_root("/Applications/Busytok.app");
        let paths = BusytokPaths::for_test(temp_home.path());
        paths.ensure_dirs_exist().unwrap();
        let platform = PlatformPaths::with_home_dir(temp_home.path().to_path_buf());
        let executor: Arc<dyn MainThreadExecutor> = Arc::new(InlineExecutor);
        let probe: Arc<dyn VersionProbe> = Arc::new(NoIdentityProbe);
        let runner: Box<dyn CommandRunner> = Box::new(PermissiveRunner);
        let lc = SmAppServiceLifecycle::new_with_executor(
            layout, paths, platform, executor, probe, runner,
        );
        lc.uninstall().expect("uninstall must succeed");
        assert!(
            !managed.exists(),
            "uninstall must remove the managed user-domain plist"
        );
    }

    #[test]
    fn uninstall_tolerates_missing_managed_plist() {
        let temp_home = tempdir().unwrap();
        let layout = BundleLayout::for_app_root("/Applications/Busytok.app");
        let paths = BusytokPaths::for_test(temp_home.path());
        paths.ensure_dirs_exist().unwrap();
        let platform = PlatformPaths::with_home_dir(temp_home.path().to_path_buf());
        let executor: Arc<dyn MainThreadExecutor> = Arc::new(InlineExecutor);
        let probe: Arc<dyn VersionProbe> = Arc::new(NoIdentityProbe);
        let runner: Box<dyn CommandRunner> = Box::new(PermissiveRunner);
        let lc = SmAppServiceLifecycle::new_with_executor(
            layout, paths, platform, executor, probe, runner,
        );
        // Must not panic or return Err just because the managed plist is
        // absent (fresh install, or user already removed it manually).
        lc.uninstall()
            .expect("uninstall must tolerate missing managed plist");
    }

    // ── ServiceBuildIdentity ────────────────────────────────────────

    #[test]
    fn build_identity_current_gui_is_compile_time_version() {
        let id = ServiceBuildIdentity::current_gui();
        assert_eq!(id.as_str(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn build_identity_mismatch_is_not_current_gui() {
        assert_ne!(
            ServiceBuildIdentity::mismatch(),
            ServiceBuildIdentity::current_gui()
        );
    }

    #[test]
    fn hydration_state_ready_when_identity_matches() {
        let state = hydration_state(ServiceBuildIdentity::current_gui());
        assert_eq!(
            state,
            crate::desktop_service_status::ServiceBootstrapState::Ready
        );
    }

    // ── Readiness classifier (status downgrade) ────────────────────

    #[test]
    fn status_running_downgrades_when_socket_not_ready() {
        // SMAppService says Enabled (mapped to Running), but the control
        // socket is missing — diagnostics must NOT report Running.
        let downgraded = classify_running_status(LifecycleStatus::Running, false);
        assert_eq!(downgraded, LifecycleStatus::RegisteredInactive);
    }

    #[test]
    fn status_running_stays_running_when_socket_ready() {
        let actual = classify_running_status(LifecycleStatus::Running, true);
        assert_eq!(actual, LifecycleStatus::Running);
    }

    #[test]
    fn status_non_running_states_pass_through_unchanged() {
        for s in [
            LifecycleStatus::NotRegistered,
            LifecycleStatus::RegisteredInactive,
            LifecycleStatus::Disabled,
            LifecycleStatus::NeedsAttention,
        ] {
            assert_eq!(
                classify_running_status(s.clone(), false),
                s,
                "{s:?} must not be affected by socket readiness"
            );
            assert_eq!(
                classify_running_status(s.clone(), true),
                s,
                "{s:?} must not be affected by socket readiness"
            );
        }
    }

    // ── Readiness probe rejects stale marker ───────────────────────

    #[test]
    fn socket_ready_rejects_stale_marker() {
        // The readiness probe must require both marker AND socket file.
        // Write a marker WITHOUT creating the socket file — this is
        // the stale-marker state. socket_ready() must return false.
        let layout = BundleLayout::for_app_root("/Applications/Busytok.app");
        let paths = BusytokPaths::for_test(&std::env::temp_dir().join(format!(
            "busytok-stale-marker-test-{}",
            uuid::Uuid::new_v4()
        )));
        paths.ensure_dirs_exist().unwrap();
        let platform = PlatformPaths::new();
        let executor: Arc<dyn MainThreadExecutor> = Arc::new(InlineExecutor);
        let probe: Arc<dyn VersionProbe> = Arc::new(NoIdentityProbe);
        let runner: Box<dyn CommandRunner> = Box::new(PermissiveRunner);
        let lc = SmAppServiceLifecycle::new_with_executor(
            layout,
            paths.clone(),
            platform,
            executor,
            probe,
            runner,
        );

        // Marker present, socket missing -> not ready.
        busytok_config::service_marker::write(paths.data_dir()).unwrap();
        assert!(
            !lc.socket_ready().unwrap(),
            "stale marker must not satisfy socket_ready — socket file must exist too"
        );
    }

    #[test]
    fn wait_for_socket_ready_errors_on_stale_marker_within_timeout() {
        // Directly exercise wait_for_socket_ready() (the function
        // bootstrap/repair uses for readiness polling). A stale marker
        // without the socket file must cause it to error out rather
        // than succeed — otherwise bootstrap could emit Ready for a
        // wedged service.
        let layout = BundleLayout::for_app_root("/Applications/Busytok.app");
        let paths = BusytokPaths::for_test(&std::env::temp_dir().join(format!(
            "busytok-wait-stale-marker-test-{}",
            uuid::Uuid::new_v4()
        )));
        paths.ensure_dirs_exist().unwrap();
        let platform = PlatformPaths::new();
        let executor: Arc<dyn MainThreadExecutor> = Arc::new(InlineExecutor);
        let probe: Arc<dyn VersionProbe> = Arc::new(NoIdentityProbe);
        let runner: Box<dyn CommandRunner> = Box::new(PermissiveRunner);
        let lc = SmAppServiceLifecycle::new_with_executor(
            layout,
            paths.clone(),
            platform,
            executor,
            probe,
            runner,
        );

        // Marker present, socket missing — wait_for_socket_ready polls
        // for 100 * 50ms = 5s then bails with an error. Override the
        // default deadline by using the prod function directly; for
        // tests we accept the 5s timeout as the cost of correctness
        // verification.
        busytok_config::service_marker::write(paths.data_dir()).unwrap();
        let result = lc.wait_for_socket_ready();
        assert!(
            result.is_err(),
            "wait_for_socket_ready must error when socket file is missing \
             even if marker exists; got {result:?}"
        );
    }

    // ── preflight_bundle gate tests ────────────────────────────────

    /// Create a minimal fake lifecycle wired to a tempdir. The lifecycle
    /// exposes preflight_bundle only indirectly via `status_via_executor`
    /// etc., so we call those methods to exercise the preflight gate.
    fn preflight_lifecycle(temp: &std::path::Path) -> SmAppServiceLifecycle {
        use crate::service_lifecycle::bundle_layout::BundleLayout;
        use crate::service_lifecycle::smappservice_bridge::MainThreadExecutor;

        let layout = BundleLayout::for_app_root(temp);
        let paths = busytok_config::BusytokPaths::new();
        let platform = busytok_platform::PlatformPaths::new();

        struct NoopExecutor;
        impl MainThreadExecutor for NoopExecutor {
            fn run_on_main_thread(&self, f: Box<dyn FnOnce() + Send>) {
                f();
            }
        }
        let executor: Arc<dyn MainThreadExecutor> = Arc::new(NoopExecutor);

        struct FakeProbe;
        impl VersionProbe for FakeProbe {
            fn probe_service_build_identity(&self) -> Result<Option<String>> {
                Ok(None)
            }
        }
        let version_probe: Arc<dyn VersionProbe> = Arc::new(FakeProbe);

        SmAppServiceLifecycle::new_with_executor(
            layout,
            paths,
            platform,
            executor,
            version_probe,
            Box::new(SystemCommandRunner),
        )
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn preflight_no_longer_requires_bundled_plist() {
        // Production bootstraps from the runtime-rendered user-domain plist
        // (managed_launch_agent), NOT the plist baked into the bundle. So a
        // missing bundle plist is no longer a preflight failure — only the
        // service binary is required (see preflight_rejects_missing_service_binary).
        let temp = tempdir().unwrap();
        let root = temp.path().join("Busytok.app");
        let macos_dir = root.join("Contents/MacOS");
        fs::create_dir_all(&macos_dir).unwrap();
        // Binary present, bundle plist deliberately absent.
        std::fs::write(macos_dir.join("busytok-service"), "#!/bin/sh\n").unwrap();

        let lc = preflight_lifecycle(&root);
        // Exercise preflight directly (not status_via_executor, which is the
        // test-only SMAppService path that still expects a bundle plist).
        let result = lc.preflight_bundle("status");
        assert!(
            result.is_ok(),
            "preflight must pass when the binary is present even without a bundle plist; got: {:?}",
            result.err()
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn preflight_rejects_missing_service_binary() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("Busytok.app");
        let plist_dir = root.join("Contents/Library/LaunchAgents");
        fs::create_dir_all(&plist_dir).unwrap();
        // Write the plist but leave the binary absent.
        std::fs::write(plist_dir.join("com.busytok.service.plist"), "<fake/>\n").unwrap();

        let lc = preflight_lifecycle(&root);
        let result = lc.status_via_executor();
        assert!(
            result.is_err(),
            "status_via_executor without a service binary must return Err"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("service binary missing"),
            "error must mention the binary, got: {msg}"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn preflight_passes_when_both_plist_and_binary_exist() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("Busytok.app");
        let plist_dir = root.join("Contents/Library/LaunchAgents");
        let macos_dir = root.join("Contents/MacOS");
        fs::create_dir_all(&plist_dir).unwrap();
        fs::create_dir_all(&macos_dir).unwrap();
        std::fs::write(plist_dir.join("com.busytok.service.plist"), "<fake/>\n").unwrap();
        std::fs::write(macos_dir.join("busytok-service"), b"fake-binary").unwrap();

        let lc = preflight_lifecycle(&root);
        lc.preflight_bundle("status")
            .expect("preflight should pass when both bundle artifacts exist");
    }
}
