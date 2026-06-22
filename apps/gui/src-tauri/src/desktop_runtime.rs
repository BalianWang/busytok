#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::Manager;

use crate::desktop_lifecycle_settings::DesktopLifecycleSettings;

/// How the app was launched, as best we can determine.
///
/// macOS `SMAppService.mainApp` does not pass a clean `--login-start`
/// flag when launching the app at login. Detection is therefore
/// limited to the explicit `--desktop-host-login-start` CLI opt-in
/// (used by scripts / advanced users who want hidden-mode startup).
/// `SMAppService.mainApp` login launches fall through to `Unknown`,
/// which `from_launch_reason` maps to visible GUI.
///
/// **Known product limitation:** because we cannot reliably detect
/// `SMAppService.mainApp` login launches, an enabled "Launch Busytok
/// Desktop at login" toggle will result in a visible GUI at login
/// rather than a hidden desktop-host mode. The spec records this
/// trade-off explicitly; a future revision may revisit if Apple
/// exposes a reliable login-item launch signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchReason {
    /// Explicit `--desktop-host-login-start` CLI flag.
    LoginItem,
    /// Detection is inconclusive; default to visible GUI.
    Unknown,
    /// User explicitly launched the app (Dock click, Finder, terminal).
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupContext {
    InteractiveAppLaunch,
    LoginStart,
    SingletonReveal,
}

impl StartupContext {
    /// Map a launch reason and lifecycle settings to the startup context.
    ///
    /// Rule: `LoginItem` + `launch_busytok_desktop_at_login == true`
    ///   → `LoginStart` (hidden GUI, tray/menu/shortcut still created).
    /// Everything else (including `Unknown`) → `InteractiveAppLaunch`.
    pub fn from_launch_reason(reason: LaunchReason, settings: DesktopLifecycleSettings) -> Self {
        match reason {
            LaunchReason::LoginItem if settings.launch_busytok_desktop_at_login => {
                StartupContext::LoginStart
            }
            _ => StartupContext::InteractiveAppLaunch,
        }
    }
}

/// Determine the launch reason for the current process.
///
/// Returns `LoginItem` only when the caller passed the explicit
/// `--desktop-host-login-start` CLI flag (scriptable opt-in for
/// hidden-mode startup). Returns `Unknown` for all other launches,
/// including `SMAppService.mainApp` login launches (which macOS does
/// not flag in a way the app can detect). `Unknown` maps to visible
/// GUI via `from_launch_reason`.
pub fn detect_launch_reason() -> LaunchReason {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--desktop-host-login-start") {
        return LaunchReason::LoginItem;
    }
    LaunchReason::Unknown
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupStep {
    InitLogging,
    ConfigureHostPresentation,
    CreateMenuBar,
    RegisterShortcut,
    ShowGui,
    KeepGuiHidden,
    StartServiceBootstrapAsync,
    StartSubscriptionBridge,
}

/// Ordered startup steps for each context.
///
/// This function serves as a documentation and test scaffold. `lib.rs::run()`
/// implements the startup sequence inline. If the startup order changes,
/// update both this function and the runtime.
pub fn startup_steps(context: StartupContext) -> Vec<StartupStep> {
    let visibility = match context {
        StartupContext::InteractiveAppLaunch | StartupContext::SingletonReveal => {
            StartupStep::ShowGui
        }
        StartupContext::LoginStart => StartupStep::KeepGuiHidden,
    };
    vec![
        StartupStep::InitLogging,
        StartupStep::ConfigureHostPresentation,
        StartupStep::CreateMenuBar,
        StartupStep::RegisterShortcut,
        visibility,
        StartupStep::StartServiceBootstrapAsync,
        StartupStep::StartSubscriptionBridge,
    ]
}

pub fn singleton_relaunch_action() -> crate::desktop_host::DesktopHostAction {
    crate::desktop_host::DesktopHostAction::ShowGui
}

pub fn dock_reopen_action() -> crate::desktop_host::DesktopHostAction {
    crate::desktop_host::DesktopHostAction::ShowGui
}

pub struct HostPresentationConfig {
    pub menu_bar_only: bool,
    pub dock_visible: bool,
}

pub fn host_presentation_config() -> HostPresentationConfig {
    HostPresentationConfig {
        menu_bar_only: false,
        dock_visible: true,
    }
}

pub fn configure_host_presentation(app: &tauri::AppHandle) {
    let config = host_presentation_config();
    #[cfg(target_os = "macos")]
    {
        let activation_policy = if config.menu_bar_only {
            tauri::ActivationPolicy::Accessory
        } else {
            tauri::ActivationPolicy::Regular
        };
        let policy_name = if config.menu_bar_only {
            "accessory"
        } else {
            "regular"
        };

        if let Err(e) = app.set_activation_policy(activation_policy) {
            tracing::warn!(
                event_code = "desktop_host.activation_policy_failed",
                activation_policy = policy_name,
                error = %e,
                "failed to update desktop host activation policy"
            );
        } else {
            tracing::info!(
                event_code = "desktop_host.activation_policy_updated",
                activation_policy = policy_name,
                "desktop host activation policy updated"
            );
        }

        if let Err(e) = app.set_dock_visibility(config.dock_visible) {
            tracing::warn!(
                event_code = "desktop_host.dock_visibility_failed",
                visible = config.dock_visible,
                error = %e,
                "failed to update dock visibility"
            );
        } else {
            tracing::info!(
                event_code = "desktop_host.dock_visibility_updated",
                visible = config.dock_visible,
                "desktop host dock visibility updated"
            );
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

pub struct HostExitState {
    pub allow_exit: AtomicBool,
}

/// How the `RunEvent::ExitRequested` handler should respond, decided by
/// [`route_exit_request`].
///
/// On macOS, `ExitRequested` corresponds to a real application terminate
/// request (Cmd+Q, Apple-menu `Quit`, Dock `Quit`, system logout/shutdown)
/// — **not** a window close. Closing the main window is intercepted earlier
/// by [`crate::desktop_windows::install_hide_on_close`], which prevents the
/// close and hides the window, so it never reaches `ExitRequested`. Per the
/// product contract, every such terminate request must funnel through the
/// full shutdown pipeline so the background service is stopped for the
/// session. The only exception is a self-triggered exit from
/// [`quit_desktop_host`] (`allow_exit == true`), which is let through to
/// complete the shutdown it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitRouting {
    /// `allow_exit` was already set by [`quit_desktop_host`]; let the
    /// terminate request proceed so the process exits.
    AllowThrough,
    /// A real terminate request arrived. The caller must `api.prevent_exit()`
    /// and route through [`quit_desktop_host`] for a full session shutdown.
    RouteToFullShutdown,
}

/// Decide how to handle a `RunEvent::ExitRequested`.
///
/// Host mode is intentionally **not** consulted: window-close-as-hide is
/// handled separately by [`crate::desktop_windows::install_hide_on_close`],
/// and an explicit Quit must always fully quit regardless of host mode.
pub(crate) fn route_exit_request(allow_exit: bool) -> ExitRouting {
    if allow_exit {
        ExitRouting::AllowThrough
    } else {
        ExitRouting::RouteToFullShutdown
    }
}

/// The body of the `RunEvent::ExitRequested` handler, factored out so the
/// composition — read `allow_exit`, route, then invoke the side effects — is
/// unit-testable without a live Tauri event loop.
///
/// `prevent_exit` wraps `RunEvent::ExitRequested`'s `api.prevent_exit()`;
/// `quit` wraps [`quit_desktop_host`]. The handler in `lib.rs` passes these
/// as closures capturing the real Tauri handles, keeping the run-loop closure
/// itself a thin adapter. This is the seam that pins the wiring a pure
/// `route_exit_request` test cannot reach: that a real terminate request
/// invokes *both* `prevent_exit` and `quit`, in that order.
pub(crate) fn handle_exit_requested<F1, F2>(allow_exit: bool, prevent_exit: F1, quit: F2)
where
    F1: FnOnce(),
    F2: FnOnce(),
{
    match route_exit_request(allow_exit) {
        ExitRouting::AllowThrough => {}
        ExitRouting::RouteToFullShutdown => {
            tracing::info!(
                event_code = "desktop_host.system_quit_intercepted",
                "system quit intercepted; routing through full shutdown pipeline"
            );
            prevent_exit();
            quit();
        }
    }
}

pub(crate) fn quit_desktop_host(app: &tauri::AppHandle) {
    tracing::info!(event_code = "desktop_host.quit_requested", "quit requested");

    app.state::<HostExitState>()
        .allow_exit
        .store(true, Ordering::SeqCst);

    // Destroy the palette panel (native NSPanel, not a Tauri window).
    if let Some(mutex) =
        app.try_state::<std::sync::Mutex<crate::palette_controller::PaletteController>>()
    {
        if let Ok(mut ctrl) = mutex.lock() {
            ctrl.destroy(app);
        }
    }

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.close();
    }

    // Route through the coordinator so suppression state is updated and
    // persisted before the process exits. The coordinator's
    // `request_quit()` also prevents racing `ensure_running` calls from
    // undoing the stop. Falls back to direct lifecycle stop + exit when
    // the coordinator isn't available (e.g. very early in startup).
    let coordinator = app
        .try_state::<std::sync::Arc<crate::lifecycle_coordinator::LifecycleCoordinator>>()
        .map(|s| s.inner().clone());
    let settings_store = app
        .try_state::<std::sync::Arc<crate::desktop_lifecycle_settings::DesktopLifecycleSettingsStore>>()
        .map(|s| s.inner().clone());

    if let (Some(coordinator), Some(settings_store)) = (coordinator, settings_store) {
        tauri::async_runtime::block_on(async move {
            // 1. Mark quit-in-flight so racing ensure calls bail out.
            coordinator.request_quit().await;
            // 2. Persist suppression state for the remainder of this login
            //    session; later CLI invocations or relaunches observe it.
            coordinator
                .suppress_and_persist(
                    crate::lifecycle_coordinator::LifecycleCause::Quit,
                    &settings_store,
                )
                .await;
            // 3. Best-effort stop both helpers via the shared quit helper,
            //    which unconditionally calls app.exit(0) via the QuitContext.
            crate::lifecycle_coordinator::quit_desktop_host_with(
                &**coordinator.lifecycle(),
                &**coordinator.login_start(),
                app,
            )
            .ok();
        });
    } else {
        app.exit(0);
    }
}

pub fn dispatch_host_action(
    app: &tauri::AppHandle,
    action: crate::desktop_host::DesktopHostAction,
) {
    match action {
        crate::desktop_host::DesktopHostAction::ShowGui => crate::desktop_windows::show_gui(app),
        crate::desktop_host::DesktopHostAction::OpenMenu => {}
        crate::desktop_host::DesktopHostAction::ShowPromptPalette => {
            crate::desktop_windows::show_prompt_palette(app)
        }
        crate::desktop_host::DesktopHostAction::HidePromptPalette => {
            crate::desktop_windows::hide_prompt_palette(app)
        }
        crate::desktop_host::DesktopHostAction::RetryShortcutRegistration => {
            crate::desktop_shortcut::retry_shortcut_registration(app)
        }
        crate::desktop_host::DesktopHostAction::QuitDesktopHost => quit_desktop_host(app),
    }
}
