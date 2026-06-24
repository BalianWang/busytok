#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
#![allow(clippy::all, unstable_name_collisions)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod activation_context;
mod bootstrap;
mod commands;
mod desktop_host;
mod desktop_lifecycle_settings;
mod desktop_login_start;
mod desktop_menu;
mod desktop_runtime;
mod desktop_service_status;
mod desktop_shortcut;
mod desktop_windows;
mod host_application_services;
mod lifecycle_coordinator;
mod logging;
mod macos_persistent_state;
mod palette_controller;
mod palette_native;
mod panel_bridge;
mod prompt_palette;
mod prompt_palette_native;
mod service_lifecycle;
mod service_recovery;
mod subscribe;
mod updater;

#[cfg(test)]
mod activation_context_tests;
mod bootstrap_lock;
#[cfg(test)]
mod bootstrap_lock_tests;
#[cfg(test)]
mod commands_tests;
#[cfg(test)]
mod desktop_host_tests;
#[cfg(test)]
mod desktop_runtime_tests;
#[cfg(test)]
mod desktop_shortcut_tests;
#[cfg(test)]
mod desktop_windows_tests;
#[cfg(test)]
mod host_application_services_tests;
#[cfg(test)]
mod macos_persistent_state_tests;
#[cfg(test)]
mod palette_controller_tests;
#[cfg(test)]
mod palette_native_tests;
#[cfg(test)]
mod panel_bridge_tests;
#[cfg(test)]
mod prompt_palette_tests;
#[cfg(test)]
mod updater_tests;

use busytok_config::BusytokPaths;
use desktop_service_status::{ServiceBootstrapState, ServiceStatusEvent};
use host_application_services::{BusytokState, HostServices};
use palette_controller::PaletteController;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
use tokio::sync::watch;

/// Holds the subscription bridge shutdown sender so the bridge lives
/// for the entire app lifetime. Dropped when the Tauri app exits, which
/// signals the bridge to shut down gracefully.
struct SubscriptionGuard {
    _shutdown: watch::Sender<bool>,
}

fn emit_service_status(
    app: &tauri::AppHandle,
    status: ServiceBootstrapState,
    reason: Option<String>,
) {
    let since_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let event = ServiceStatusEvent {
        status,
        since_ms,
        reason,
    };
    tracing::info!(
        event_code = "desktop_host.service_status_emitted",
        status = event.status.as_str(),
        "service status emitted"
    );
    if let Err(e) = app.emit("busytok:service-status", &event) {
        tracing::warn!("failed to emit service status: {e}");
    }

    // Update PaletteController with current status — stores it for replay
    // when panel opens, and pushes immediately if panel is visible.
    let palette_state = app.try_state::<Mutex<PaletteController>>();
    if let Some(ctrl) = palette_state {
        if let Ok(mut ctrl) = ctrl.lock() {
            ctrl.update_service_status(status.as_str());
        }
    }
}

/// Upper bound on bootstrap `ensure_running` attempts for a retryable failure.
/// Covers the stale-live-process repair race on fresh-install-over-old-app,
/// where the first attempt fails and the service recovers within ~hundreds of
/// ms. See docs/bugs/2026-06-24-startup-status-stale-on-fresh-install.md.
const BOOTSTRAP_MAX_ATTEMPTS: u32 = 3;
/// Delay between bootstrap retries. Sized to the observed repair-recovery
/// window (~hundreds of ms) with margin.
const BOOTSTRAP_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// Whether a bootstrap `ensure_running` failure should be retried before the
/// one-shot bootstrap task declares the service unavailable.
///
/// Mirrors the `LifecycleCoordinator`'s internal NonRetryable/RetryableFailure
/// split (the `ensure_running` Err arm): only an SMAppService "requires user
/// approval" condition is non-retryable, because the user must act in System
/// Settings and no retry will help. Everything else — launchctl timeout,
/// version-probe socket not ready, the stale-live-process repair race — may
/// resolve within ~hundreds of ms and is worth a bounded retry so the bootstrap
/// task emits `Ready` (not `Unavailable`) once the service is actually up.
pub(crate) fn bootstrap_failure_is_retryable(err: &str) -> bool {
    !err.contains("requires user approval")
}

pub fn run() {
    let paths = BusytokPaths::new();
    if let Err(e) = paths.ensure_dirs_exist() {
        tracing::error!("Failed to create Busytok directories: {e}");
    }

    // ── 1. Initialize logging ────────────────────────────────────
    let tauri_session_id = uuid::Uuid::new_v4().to_string();
    let _logging_guards = logging::init_gui_logging(&paths.log_dir(), &tauri_session_id);
    macos_persistent_state::disable_appkit_persistent_state();

    // ── 2. Handle --uninstall-self BEFORE control_endpoint resolution ─
    // On Windows, control_endpoint() resolves the user SID; if that fails,
    // gating --uninstall-self behind it would crash the uninstall path on
    // broken SID configurations. Uninstall doesn't need the endpoint at all.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--uninstall-self") {
        fn run_uninstall_self() -> anyhow::Result<()> {
            let paths = busytok_config::BusytokPaths::new();
            paths.ensure_dirs_exist()?;
            // Remove marker before uninstall() so supervisor status readers in any
            // concurrently-running GUI process correctly observe service_offline while
            // the uninstall removes the task definition. If a stale marker lingers
            // after crash, the next service boot clears it (see ServiceApp::boot).
            let _ = busytok_config::service_marker::remove(paths.data_dir());
            #[cfg(target_os = "macos")]
            {
                crate::service_lifecycle::macos_lifecycle_for_uninstall_self()?.uninstall()?;
            }
            #[cfg(not(target_os = "macos"))]
            {
                // Non-macOS uninstall: construct platform-default lifecycle.
                // On Windows the task-scheduler lifecycle has no executor
                // dependency and can be safely constructed ad-hoc here.
                #[cfg(target_os = "windows")]
                {
                    use crate::service_lifecycle::task_scheduler::TaskSchedulerLifecycle;
                    TaskSchedulerLifecycle::new().uninstall()?;
                }
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                {
                    anyhow::bail!("uninstall-self is not supported on this platform");
                }
            }
            Ok(())
        }
        match run_uninstall_self() {
            Ok(()) => println!("uninstall-self completed"),
            Err(e) => {
                eprintln!("uninstall-self failed: {e}");
                std::process::exit(1);
            }
        }
        std::process::exit(0);
    }

    // ── 3. Detect launch reason (before control_endpoint) ─────────
    let launch_reason = desktop_runtime::detect_launch_reason();
    // Load the persisted lifecycle settings so `LoginItem` + enabled
    // login-start correctly resolves to hidden-at-login startup. The
    // store wraps a default; load() prefers the on-disk TOML.
    let lifecycle_settings_store = std::sync::Arc::new(
        desktop_lifecycle_settings::DesktopLifecycleSettingsStore::new(
            desktop_lifecycle_settings::DesktopLifecycleSettings::default(),
            busytok_config::BusytokPaths::new(),
        ),
    );
    let lifecycle_settings = lifecycle_settings_store.load();
    let startup_context = desktop_runtime::StartupContext::from_launch_reason(
        launch_reason,
        lifecycle_settings.clone(),
    );
    tracing::info!(
        event_code = "desktop_host.launch_reason_detected",
        reason = ?launch_reason,
        context = ?startup_context,
        "startup context resolved"
    );

    // ── 4. Resolve control endpoint (graceful on failure) ────────
    let control_endpoint = match paths.control_endpoint() {
        Ok(ep) => ep,
        Err(e) => {
            tracing::error!(
                event_code = "gui.control_endpoint_failed",
                error = %e,
                "failed to resolve control endpoint; GUI will start in degraded mode"
            );
            // Fallback: empty string; IPC calls will fail gracefully
            String::new()
        }
    };
    let state = BusytokState {
        control_endpoint: control_endpoint.clone(),
    };

    tauri::Builder::default()
        .manage::<BusytokState>(state)
        .manage(desktop_runtime::HostExitState {
            allow_exit: std::sync::atomic::AtomicBool::new(false),
        })
        // ── Single-instance plugin FIRST ─────────────────────────
        .plugin(tauri_plugin_single_instance::init(
            move |app_handle, _args, _cwd| {
                tracing::info!(
                    event_code = "desktop_host.singleton_reveal",
                    "singleton relaunch detected, showing GUI"
                );
                // A singleton relaunch is an explicit user action (Dock
                // click, Finder double-click, etc.). It clears any
                // persisted session suppression so auto-ensure resumes
                // immediately for the current session. Collect owned
                // references first, then spawn.
                let coordinator_arc: Option<std::sync::Arc<lifecycle_coordinator::LifecycleCoordinator>> =
                    app_handle
                        .try_state::<std::sync::Arc<lifecycle_coordinator::LifecycleCoordinator>>()
                        .map(|s| s.inner().clone());
                let store_arc: Option<std::sync::Arc<desktop_lifecycle_settings::DesktopLifecycleSettingsStore>> =
                    app_handle
                        .try_state::<std::sync::Arc<desktop_lifecycle_settings::DesktopLifecycleSettingsStore>>()
                        .map(|s| s.inner().clone());
                if let (Some(coordinator), Some(store)) = (coordinator_arc, store_arc) {
                    tauri::async_runtime::spawn(async move {
                        coordinator
                            .clear_suppression_and_persist(
                                lifecycle_coordinator::LifecycleCause::ManualReopen,
                                &store,
                            )
                            .await;
                        // Nudge the service ensure path so the GUI hydrates.
                        let _ = coordinator
                            .ensure_running(lifecycle_coordinator::LifecycleCause::ManualReopen)
                            .await;
                    });
                }
                let action = desktop_runtime::singleton_relaunch_action();
                desktop_runtime::dispatch_host_action(app_handle, action);
            },
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(desktop_shortcut::ShortcutState {
            diagnostics: std::sync::Mutex::new(desktop_shortcut::ShortcutDiagnostics::default()),
        })
        .setup(move |app| {
            // ── Updater plugin bootstrap ────────────────────────────
            crate::updater::init_updater_logging();

            // ── Host presentation FIRST (Regular app / Dock visible) ─────
            desktop_runtime::configure_host_presentation(app.handle());

            // ── Lifecycle settings store + coordinator ──────────────
            // Reuse the store constructed before Tauri setup so the
            // disk-loaded lifecycle settings (including suppression +
            // login-start toggle) are available to the coordinator.
            let settings_store = lifecycle_settings_store.clone();
            app.handle().manage(settings_store.clone());

            #[cfg(target_os = "macos")]
            {
                use anyhow::Context;
                use crate::service_lifecycle::bundle_layout::BundleLayout;
                use crate::service_lifecycle::command_runner::SystemCommandRunner;
                use crate::service_lifecycle::smappservice::{
                    ControlClientVersionProbe, SmAppServiceLifecycle,
                };
                use crate::service_lifecycle::smappservice_bridge::MainThreadExecutor;
                use busytok_platform::PlatformPaths;

                struct TauriMainThreadExecutor {
                    handle: tauri::AppHandle,
                }
                impl MainThreadExecutor for TauriMainThreadExecutor {
                    fn run_on_main_thread(&self, f: Box<dyn FnOnce() + Send>) {
                        let _ = self.handle.run_on_main_thread(move || {
                            f();
                        });
                    }
                }

                let executor: Arc<dyn MainThreadExecutor> =
                    Arc::new(TauriMainThreadExecutor {
                        handle: app.handle().clone(),
                    });

                // Resolve bundle layout by walking up from current_exe.
                let exe = std::env::current_exe()
                    .context("resolving current executable")?;
                let mut cursor = exe.parent();
                let bundle_root = loop {
                    let dir = cursor.context("could not locate enclosing .app bundle")?;
                    if dir
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.ends_with(".app"))
                        .unwrap_or(false)
                    {
                        break dir.to_path_buf();
                    }
                    cursor = dir.parent();
                };
                let layout = BundleLayout::for_app_root(&bundle_root);

                let paths_for_lc = busytok_config::BusytokPaths::new();
                let platform = PlatformPaths::new();
                let socket_path = paths_for_lc
                    .control_endpoint()
                    .unwrap_or_default();
                let version_probe: std::sync::Arc<dyn crate::service_lifecycle::smappservice::VersionProbe> =
                    std::sync::Arc::new(ControlClientVersionProbe::new(socket_path));
                let runner: Box<dyn crate::service_lifecycle::command_runner::CommandRunner> =
                    Box::new(SystemCommandRunner);

                let lifecycle: Arc<dyn crate::service_lifecycle::ServiceLifecycle> =
                    Arc::new(SmAppServiceLifecycle::new_with_executor(
                        layout,
                        paths_for_lc,
                        platform,
                        Arc::clone(&executor) as Arc<dyn MainThreadExecutor>,
                        version_probe,
                        runner,
                    ));

                let manager: Arc<dyn desktop_login_start::DesktopLoginStart> = Arc::new(
                    desktop_login_start::DesktopLoginStartManager::new(
                        settings_store,
                        executor,
                    ),
                );

                // Adopt in-process host mode when the persisted login-start
                // toggle is enabled, BEFORE the manager moves into the
                // coordinator. Startup adopt must not touch SMAppService:
                // ServiceManagement can throw Objective-C exceptions during
                // app launch, and those abort Rust across the FFI boundary.
                if let Err(e) = desktop_login_start::adopt_current_session_if_enabled(
                    manager.as_ref(),
                    &lifecycle_settings,
                ) {
                    tracing::warn!(
                        event_code = "desktop_host.startup_adopt_failed",
                        error = %e,
                        "desktop host login-start adopt failed; host mode will be degraded for this session"
                    );
                }

                let coordinator = std::sync::Arc::new(
                    lifecycle_coordinator::LifecycleCoordinator::new(
                        Arc::clone(&lifecycle) as Arc<dyn crate::service_lifecycle::ServiceLifecycle>,
                        manager,
                    ),
                );
                app.handle().manage(coordinator);
            };

            // ── Menu bar FIRST (before async bootstrap) ──────────
            if let Err(e) = desktop_menu::install_menu_bar(app.handle()) {
                tracing::error!("Failed to install menu bar: {e}");
            } else {
                tracing::info!(
                    event_code = "desktop_host.menu_bar_created",
                    "menu bar created"
                );
            }

            // ── Register global shortcut ────────────────────────
            desktop_shortcut::register_prompt_palette_shortcut(app.handle());

            // ── PaletteController ────────────────────────────────
            let resource_dir = app
                .path()
                .resource_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".into());
            let services = HostServices::new(control_endpoint.clone());
            let palette = PaletteController::new(services, resource_dir);
            app.handle().manage(Mutex::new(palette));

            // ── Main window close intercept: hide instead of close ─
            if let Some(window) = app.get_webview_window("main") {
                desktop_windows::install_hide_on_close(
                    &window,
                    "desktop_host.gui_close_hidden",
                    "main",
                );
            }

            // ── Service bootstrap ASYNC (non-blocking) ───────────
            let bootstrap_handle = app.handle().clone();
            emit_service_status(&bootstrap_handle, ServiceBootstrapState::Starting, None);
            tracing::info!(
                event_code = "desktop_host.service_bootstrap_async_started",
                "async service bootstrap started"
            );

            tauri::async_runtime::spawn(async move {
                // Prefer the coordinator path when available (macOS). The
                // coordinator handles suppression, ensure-coalescing, and
                // quit-over-ensure priority; bypassing it would defeat the
                // session-suppression contract.
                let coordinator_arc = bootstrap_handle
                    .try_state::<std::sync::Arc<lifecycle_coordinator::LifecycleCoordinator>>()
                    .map(|s| s.inner().clone());
                if let Some(coordinator) = coordinator_arc {
                    // Restore + clear persisted suppression on explicit
                    // reopen. A fresh launch after Quit is a ManualReopen
                    // action that must reactivate helpers and persist the
                    // cleared state to disk.
                    let cause;
                    match bootstrap_handle
                        .try_state::<std::sync::Arc<desktop_lifecycle_settings::DesktopLifecycleSettingsStore>>()
                        .map(|s| s.inner().clone())
                    {
                        Some(store) => {
                            coordinator.restore_from_settings(&store).await;
                            if coordinator.is_suppressed().await {
                                coordinator
                                    .clear_suppression_and_persist(
                                        lifecycle_coordinator::LifecycleCause::ManualReopen,
                                        &store,
                                    )
                                    .await;
                                cause = lifecycle_coordinator::LifecycleCause::ManualReopen;
                            } else {
                                cause = lifecycle_coordinator::LifecycleCause::Startup;
                            }
                        }
                        None => {
                            cause = lifecycle_coordinator::LifecycleCause::Startup;
                        }
                    }
                    // Bounded retry for retryable failures. The first
                    // ensure_running can fail transiently (e.g. the
                    // stale-live-process repair race on fresh-install-over-old-
                    // app) while the service recovers within ~hundreds of ms.
                    // Retry retryable failures a bounded number of times so the
                    // bootstrap task emits Ready once the service is actually
                    // up, instead of emitting Unavailable and stranding the GUI
                    // until the subscription bridge reconnects. A non-retryable
                    // failure (SMAppService approval) is declared unavailable
                    // immediately. See
                    // docs/bugs/2026-06-24-startup-status-stale-on-fresh-install.md (Layer A).
                    let mut attempt: u32 = 0;
                    loop {
                        attempt += 1;
                        match coordinator.ensure_running(cause).await {
                            Ok(_) => {
                                tracing::info!(
                                    event_code = "desktop_host.service_bootstrap_async_finished",
                                    "async service bootstrap finished successfully"
                                );
                                // Follow-up recovery through the coordinator so
                                // suppression + quit-priority are honored.
                                if let Err(e) = coordinator
                                    .repair(lifecycle_coordinator::LifecycleCause::Repair)
                                    .await
                                {
                                    tracing::warn!(
                                        event_code = "desktop_host.service_recovery_followup_failed",
                                        error = %e,
                                        "post-bootstrap service recovery check failed"
                                    );
                                }
                                emit_service_status(
                                    &bootstrap_handle,
                                    ServiceBootstrapState::Ready,
                                    None,
                                );
                                break;
                            }
                            Err(e) => {
                                let err_str = e.to_string();
                                if bootstrap_failure_is_retryable(&err_str)
                                    && attempt < BOOTSTRAP_MAX_ATTEMPTS
                                {
                                    tracing::warn!(
                                        event_code =
                                            "desktop_host.service_bootstrap_retryable_failure_retrying",
                                        attempt,
                                        max_attempts = BOOTSTRAP_MAX_ATTEMPTS,
                                        error = %e,
                                        "retryable bootstrap failure; retrying ensure_running"
                                    );
                                    // Note: this sleep is not interruptible on
                                    // quit, but each ensure_running entry re-
                                    // checks quit_requested, so a Quit during the
                                    // ~500ms sleep costs at most one extra retry
                                    // delay before the next attempt aborts.
                                    tokio::time::sleep(BOOTSTRAP_RETRY_DELAY).await;
                                    continue;
                                }
                                tracing::warn!(
                                    event_code = "desktop_host.service_bootstrap_async_failed",
                                    error = %e,
                                    "async service bootstrap failed"
                                );
                                emit_service_status(
                                    &bootstrap_handle,
                                    ServiceBootstrapState::Unavailable,
                                    Some(err_str),
                                );
                                break;
                            }
                        }
                    }
                    return;
                }

                // Non-macOS fallback: construct a Windows task-scheduler
                // lifecycle inline (it has no executor dependency) and use
                // the serialized ensure path. The unsupported-platform
                // case bails before reaching this branch.
                #[cfg(target_os = "windows")]
                {
                    use crate::service_lifecycle::task_scheduler::TaskSchedulerLifecycle;
                    let recovery_lc: Arc<dyn crate::service_lifecycle::ServiceLifecycle> =
                        Arc::new(TaskSchedulerLifecycle::new());
                    match service_recovery::ensure_service_running_serialized_with(Arc::clone(&recovery_lc)).await {
                        Ok(()) => {
                            tracing::info!(
                                event_code = "desktop_host.service_bootstrap_async_finished",
                                "async service bootstrap finished successfully"
                            );
                            if let Err(e) = service_recovery::run_service_recovery(&*recovery_lc) {
                                tracing::warn!(
                                    event_code = "desktop_host.service_recovery_followup_failed",
                                    error = %e,
                                    "post-bootstrap service recovery check failed"
                                );
                            }
                            emit_service_status(&bootstrap_handle, ServiceBootstrapState::Ready, None);
                        }
                        Err(e) => {
                            tracing::warn!(
                                event_code = "desktop_host.service_bootstrap_async_failed",
                                error = %e,
                                "async service bootstrap failed"
                            );
                            emit_service_status(
                                &bootstrap_handle,
                                ServiceBootstrapState::Unavailable,
                                Some(e.to_string()),
                            );
                        }
                    }
                }
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                {
                    let msg = "service bootstrap not supported on this platform";
                    tracing::warn!(
                        event_code = "desktop_host.service_bootstrap_async_failed",
                        error = %msg,
                    );
                    emit_service_status(
                        &bootstrap_handle,
                        ServiceBootstrapState::Unavailable,
                        Some(msg.to_string()),
                    );
                }
            });

            // ── Subscription bridge ──────────────────────────────
            let handle = app.handle().clone();
            let guard = SubscriptionGuard {
                _shutdown: subscribe::start_subscription_bridge(handle, control_endpoint.clone()),
            };
            app.handle().manage(guard);

            // ── Show or hide main window based on startup context ──
            match startup_context {
                desktop_runtime::StartupContext::LoginStart => {
                    tracing::info!(
                        event_code = "desktop_host.login_start_gui_hidden",
                        "login-start: keeping GUI hidden, tray and shortcut active"
                    );
                }
                _ => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::invoke_busytok,
            commands::desktop_lifecycle_settings_snapshot,
            commands::desktop_lifecycle_settings_update,
            commands::desktop_background_service_diagnostics,
            commands::desktop_background_service_repair,
            logging::log_frontend_event,
            logging::flush_frontend_logs,
            prompt_palette::prompt_palette_paste_active_app,
            prompt_palette::prompt_palette_accessibility_status,
            prompt_palette::prompt_palette_open_accessibility_settings,
            prompt_palette::palette_panel_message,
            desktop_shortcut::desktop_host_shortcut_diagnostics,
            desktop_shortcut::desktop_host_retry_shortcut_registration,
            desktop_windows::desktop_host_show_gui,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Busytok GUI")
        .run(|app, event| match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                let allow_exit = app
                    .state::<desktop_runtime::HostExitState>()
                    .allow_exit
                    .load(std::sync::atomic::Ordering::SeqCst);

                // Thin adapter: delegate to the factored-out handler so the
                // routing composition is unit-tested in desktop_runtime_tests.
                // A real terminate request (Cmd+Q, Apple-menu Quit, Dock Quit,
                // logout) routes through the same full shutdown pipeline as
                // the "Quit Busytok Desktop" menu item. quit_desktop_host sets
                // allow_exit=true and calls app.exit(0), re-triggering
                // ExitRequested; the second pass routes to AllowThrough and
                // lets the exit through.
                desktop_runtime::handle_exit_requested(
                    allow_exit,
                    || api.prevent_exit(),
                    || desktop_runtime::quit_desktop_host(app),
                );
            }
            // Safety net: when the event loop is exiting and allow_exit is
            // still false, the shutdown pipeline was never run (Dock Quit on
            // macOS can bypass ExitRequested).  Run the stop operations
            // directly — do NOT call app.exit(0), we are already inside Exit.
            // Decision extracted as desktop_runtime::handle_exit so
            // both branches are unit-tested (parallels handle_exit_requested).
            tauri::RunEvent::Exit => {
                if desktop_runtime::handle_exit(
                    app.state::<desktop_runtime::HostExitState>()
                        .allow_exit
                        .load(std::sync::atomic::Ordering::SeqCst),
                ) {
                    tracing::warn!(
                        event_code = "desktop_host.exit_without_shutdown",
                        "process exiting without full shutdown pipeline; running stop operations"
                    );
                    desktop_runtime::run_stop_operations(app);
                }
            }
            tauri::RunEvent::Reopen {
                has_visible_windows,
                ..
            } => {
                tracing::info!(
                    event_code = "desktop_host.reopen_requested",
                    has_visible_windows,
                    "macOS reopen requested; ensuring GUI is visible"
                );
                let action = desktop_runtime::dock_reopen_action();
                desktop_runtime::dispatch_host_action(app, action);
            }
            _ => {}
        });
}
