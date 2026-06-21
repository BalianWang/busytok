//! Desktop host window management helpers.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};

/// Label for the main application window.
pub const MAIN_LABEL: &str = "main";

/// Palette dimensions (exposed for reference; the native panel uses these).
pub const PALETTE_WIDTH: u32 = 760;
pub const PALETTE_HEIGHT: u32 = 520;

/// Generation counter used to cancel pending fullscreen-hide tasks.
///
/// `show_gui` increments this on every call; the delayed hide spawned by
/// the fullscreen close path snapshots the value before waiting and checks
/// it again on the main thread.  If the generation has advanced, the
/// window was reshown and the stale hide is dropped.
static PENDING_FULLSCREEN_HIDE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Increment the generation counter, invalidating any pending delayed hide.
/// Returns the pre-increment value (the snapshot that is now stale).
pub(crate) fn cancel_pending_fullscreen_hide() -> u64 {
    PENDING_FULLSCREEN_HIDE_GENERATION.fetch_add(1, Ordering::SeqCst)
}

/// Check whether a previously captured generation snapshot is stale,
/// meaning `cancel_pending_fullscreen_hide` (i.e. `show_gui`) was called
/// after the snapshot was taken.
pub(crate) fn is_pending_hide_stale(gen_snapshot: u64) -> bool {
    PENDING_FULLSCREEN_HIDE_GENERATION.load(Ordering::SeqCst) != gen_snapshot
}

/// Main window configuration — single source of truth.
/// Must stay consistent with `tauri.conf.json` `app.windows[0]`.
pub(crate) struct MainWindowConfig {
    pub label: &'static str,
    pub title: &'static str,
    pub width: f64,
    pub height: f64,
    pub min_width: f64,
    pub min_height: f64,
}

pub(crate) fn main_window_config() -> MainWindowConfig {
    MainWindowConfig {
        label: MAIN_LABEL,
        title: "Busytok",
        width: 1160.0,
        height: 700.0,
        min_width: 700.0,
        min_height: 480.0,
    }
}

/// Minimum delay after requesting macOS native fullscreen exit.
///
/// Fullscreen exit is animated asynchronously, so hiding too early can be
/// dropped by the window server and leave a restored window visible.
pub(crate) const FULLSCREEN_CLOSE_HIDE_MIN_DELAY_MS: u64 = 700;

/// Maximum time to wait for macOS native fullscreen state to clear.
pub(crate) const FULLSCREEN_CLOSE_HIDE_TIMEOUT_MS: u64 = 2_000;

/// Poll cadence while waiting for macOS native fullscreen state to clear.
pub(crate) const FULLSCREEN_CLOSE_HIDE_POLL_INTERVAL_MS: u64 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseHideStrategy {
    Immediate,
    AfterFullscreenExit {
        min_delay_ms: u64,
        timeout_ms: u64,
        poll_interval_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FullscreenExitWaitOutcome {
    Exited { elapsed_ms: u64 },
    TimedOut { elapsed_ms: u64 },
    StateUnavailable { elapsed_ms: u64 },
}

pub(crate) fn close_hide_strategy(is_fullscreen: bool) -> CloseHideStrategy {
    if is_fullscreen {
        CloseHideStrategy::AfterFullscreenExit {
            min_delay_ms: FULLSCREEN_CLOSE_HIDE_MIN_DELAY_MS,
            timeout_ms: FULLSCREEN_CLOSE_HIDE_TIMEOUT_MS,
            poll_interval_ms: FULLSCREEN_CLOSE_HIDE_POLL_INTERVAL_MS,
        }
    } else {
        CloseHideStrategy::Immediate
    }
}

/// Enter menu-bar-only desktop host mode.
///
/// Hides the main window, hides the Dock, and switches activation policy to
/// [`tauri::ActivationPolicy::Accessory`]. This is the target state for the
/// red close button — every close path (normal and fullscreen-after-exit)
/// converges here so the mode transition is traced through a single stable
/// event code instead of scattered per-path logging.
fn enter_menu_bar_only_mode(app: &AppHandle) {
    let mut ok = true;
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        if window.is_visible().unwrap_or(false) {
            if let Err(e) = window.hide() {
                tracing::warn!(
                    event_code = "desktop_host.menu_bar_only.window_hide_failed",
                    error = %e,
                    "failed to hide main window in menu-bar-only transition"
                );
                ok = false;
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = app.set_activation_policy(tauri::ActivationPolicy::Accessory) {
            tracing::warn!(
                event_code = "desktop_host.menu_bar_only.activation_policy_failed",
                error = %e,
                "failed to set Accessory activation policy in menu-bar-only transition"
            );
            ok = false;
        }
        if let Err(e) = app.set_dock_visibility(false) {
            tracing::warn!(
                event_code = "desktop_host.menu_bar_only.dock_hide_failed",
                error = %e,
                "failed to hide Dock in menu-bar-only transition"
            );
            ok = false;
        }
        // Deactivate the app so the previously-active application regains
        // focus. Without NSApp.hide(), the app remains the frontmost process
        // after hiding its only window — keyboard input goes to an invisible
        // Busytok instead of the user's editor / terminal.
        unsafe {
            let ns_app: *mut objc::runtime::Object =
                msg_send![class!(NSApplication), sharedApplication];
            let _: () =
                msg_send![ns_app, hide: std::ptr::null_mut::<objc::runtime::Object>()];
        }
    }
    if ok {
        tracing::info!(
            event_code = "desktop_host.menu_bar_only.entered",
            "entered menu-bar-only mode (window hidden, Dock hidden, Accessory)"
        );
    } else {
        tracing::warn!(
            event_code = "desktop_host.menu_bar_only.entered_degraded",
            "entered menu-bar-only mode with partial failures (see prior warnings)"
        );
    }
}

async fn wait_for_fullscreen_exit(
    window: &tauri::WebviewWindow,
    min_delay_ms: u64,
    timeout_ms: u64,
    poll_interval_ms: u64,
) -> FullscreenExitWaitOutcome {
    let started_at = Instant::now();
    tokio::time::sleep(Duration::from_millis(min_delay_ms)).await;
    let timeout = Duration::from_millis(timeout_ms);
    let poll_interval = Duration::from_millis(poll_interval_ms);

    loop {
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        match window.is_fullscreen() {
            Ok(false) => return FullscreenExitWaitOutcome::Exited { elapsed_ms },
            Ok(true) => {}
            Err(e) => {
                tracing::warn!(
                    event_code = "desktop_host.fullscreen_exit_state_unavailable",
                    error = %e,
                    elapsed_ms,
                    "failed to read fullscreen state while waiting to hide window"
                );
                return FullscreenExitWaitOutcome::StateUnavailable { elapsed_ms };
            }
        }

        if started_at.elapsed() >= timeout {
            return FullscreenExitWaitOutcome::TimedOut { elapsed_ms };
        }

        tokio::time::sleep(poll_interval).await;
    }
}

pub(crate) fn hide_window_after_close(
    window: &tauri::WebviewWindow,
    event_code: &'static str,
    visible_name: &'static str,
) {
    let label = window.label().to_string();
    let is_fullscreen_result = window.is_fullscreen();
    let (is_fullscreen, fullscreen_error) = match is_fullscreen_result {
        Ok(is_fullscreen) => (is_fullscreen, None),
        Err(e) => (false, Some(e)),
    };
    let is_visible = window.is_visible().ok();

    if let Some(e) = fullscreen_error {
        tracing::warn!(
            event_code,
            window = visible_name,
            label = %label,
            visible = ?is_visible,
            error = %e,
            "window close intercepted, failed to read fullscreen state"
        );
    } else {
        tracing::info!(
            event_code,
            window = visible_name,
            label = %label,
            fullscreen = is_fullscreen,
            visible = ?is_visible,
            "window close intercepted"
        );
    }

    let close_strategy = close_hide_strategy(is_fullscreen);

    match close_strategy {
        CloseHideStrategy::Immediate => {
            enter_menu_bar_only_mode(&window.app_handle());
        }
        CloseHideStrategy::AfterFullscreenExit {
            min_delay_ms,
            timeout_ms,
            poll_interval_ms,
        } => {
            let set_fullscreen_result = window.set_fullscreen(false);
            if let Err(e) = &set_fullscreen_result {
                tracing::warn!(
                    event_code,
                    window = visible_name,
                    label = %label,
                    error = %e,
                    "failed to request fullscreen exit before hiding window"
                );
            }

            let gen_snapshot = PENDING_FULLSCREEN_HIDE_GENERATION.load(Ordering::SeqCst);
            let app = window.app_handle().clone();
            let wait_window = window.clone();
            tauri::async_runtime::spawn(async move {
                let wait_outcome = wait_for_fullscreen_exit(
                    &wait_window,
                    min_delay_ms,
                    timeout_ms,
                    poll_interval_ms,
                )
                .await;
                let fullscreen_before_hide = wait_window.is_fullscreen().ok();
                let visible_before_hide = wait_window.is_visible().ok();
                let app_for_mode = app.clone();
                let schedule_result = app.run_on_main_thread(move || {
                    if is_pending_hide_stale(gen_snapshot) {
                        tracing::info!(
                            event_code,
                            window = visible_name,
                            label = %label,
                            gen_snapshot,
                            "pending fullscreen close cancelled by ShowGui"
                        );
                        return;
                    }
                    // The fullscreen has exited; now enter menu-bar-only mode
                    // — identical end state to the normal-window close path.
                    enter_menu_bar_only_mode(&app_for_mode);
                });

                if let Err(e) = schedule_result {
                    tracing::warn!(
                        event_code,
                        window = visible_name,
                        fullscreen_before_hide = ?fullscreen_before_hide,
                        visible_before_hide = ?visible_before_hide,
                        wait_outcome = ?wait_outcome,
                        min_delay_ms,
                        timeout_ms,
                        poll_interval_ms,
                        error = %e,
                        "failed to schedule fullscreen close hide on main thread"
                    );
                }
            });
        }
    }
}

pub(crate) fn install_hide_on_close(
    window: &tauri::WebviewWindow,
    event_code: &'static str,
    visible_name: &'static str,
) {
    let hide_window = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            hide_window_after_close(&hide_window, event_code, visible_name);
        }
    });
}

/// Restore the normal-window presentation: show the Dock and switch to
/// a Regular activation policy. Called by [`show_gui`] on every open so
/// that menu-bar clicks and Dock reopens restore the full desktop window
/// mode, not just the window itself. Intentionally separate from the
/// one-shot startup [`crate::desktop_runtime::configure_host_presentation`].
fn restore_normal_window_presentation(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let mut ok = true;
        if let Err(e) = app.set_activation_policy(tauri::ActivationPolicy::Regular) {
            tracing::warn!(
                event_code = "desktop_host.normal_window.activation_policy_failed",
                error = %e,
                "failed to set Regular activation policy when restoring normal window mode"
            );
            ok = false;
        }
        if let Err(e) = app.set_dock_visibility(true) {
            tracing::warn!(
                event_code = "desktop_host.normal_window.dock_show_failed",
                error = %e,
                "failed to show Dock when restoring normal window mode"
            );
            ok = false;
        }
        if ok {
            tracing::info!(
                event_code = "desktop_host.normal_window.restored",
                "restored normal window mode (window shown, Dock visible, Regular)"
            );
        } else {
            tracing::warn!(
                event_code = "desktop_host.normal_window.restored_degraded",
                "restored normal window mode with partial failures (see prior warnings)"
            );
        }
    }
}

/// Show and focus the main window, recreating it if necessary.
///
/// Matches the product semantics of `ShowGui`: "ensure GUI is visible".
/// Restores the normal-window presentation (Dock visible, Regular policy)
/// so that opening from the menu bar or Dock always exits menu-bar-only mode.
/// If the window was destroyed (e.g. the webview crashed), this recreates it
/// with the close-to-menu-bar-only intercept installed.
pub fn show_gui(app: &AppHandle) {
    // Cancel any pending fullscreen-hide so it won't hide the window
    // we are about to show.
    cancel_pending_fullscreen_hide();

    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        // Restore presentation BEFORE showing the window, so the app is
        // already in Regular activation policy when the window appears.
        // If we show first while still in Accessory, the window may not
        // acquire proper foreground focus (see Steinberger 2025).
        restore_normal_window_presentation(app);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        tracing::info!(
            event_code = "desktop_host.gui_shown",
            "main window shown and focused"
        );
        return;
    }

    // Main window was destroyed; recreate it.
    let config = main_window_config();
    match WebviewWindowBuilder::new(app, config.label, WebviewUrl::App("/".into()))
        .title(config.title)
        .inner_size(config.width, config.height)
        .min_inner_size(config.min_width, config.min_height)
        .build()
    {
        Ok(window) => {
            install_hide_on_close(&window, "desktop_host.gui_close_hidden", "main");
            // Same ordering as above: restore presentation before showing.
            restore_normal_window_presentation(app);
            let _ = window.show();
            let _ = window.set_focus();
            tracing::info!(
                event_code = "desktop_host.gui_recreated",
                "main window recreated and shown"
            );
        }
        Err(e) => {
            tracing::warn!(
                event_code = "desktop_host.gui_recreation_failed",
                error = %e,
                "failed to recreate main window"
            );
        }
    }
}

/// Show the prompt palette via PaletteController.
pub fn show_prompt_palette(app: &AppHandle) {
    let Some(controller) =
        app.try_state::<std::sync::Mutex<crate::palette_controller::PaletteController>>()
    else {
        tracing::error!(
            event_code = "desktop_windows.show_palette_no_state",
            "PaletteController not found in Tauri state"
        );
        return;
    };
    let Ok(mut ctrl) = controller.try_lock() else {
        tracing::warn!(
            event_code = "desktop_windows.show_palette_lock_contended",
            "PaletteController Mutex contended, skipping show"
        );
        return;
    };
    tracing::info!(
        event_code = "desktop_windows.show_palette_calling",
        "calling PaletteController::show"
    );
    ctrl.show(app);
}

/// Hide the prompt palette via PaletteController.
pub fn hide_prompt_palette(app: &AppHandle) {
    let Some(controller) =
        app.try_state::<std::sync::Mutex<crate::palette_controller::PaletteController>>()
    else {
        tracing::error!(
            event_code = "desktop_windows.hide_palette_no_state",
            "PaletteController not found in Tauri state"
        );
        return;
    };
    let Ok(mut ctrl) = controller.try_lock() else {
        tracing::warn!(
            event_code = "desktop_windows.hide_palette_lock_contended",
            "PaletteController Mutex contended, skipping hide"
        );
        return;
    };
    ctrl.hide(app);
}

/// Tauri command: show and focus the main window.
#[tauri::command]
pub(crate) fn desktop_host_show_gui(app: AppHandle) -> Result<(), String> {
    show_gui(&app);
    Ok(())
}
