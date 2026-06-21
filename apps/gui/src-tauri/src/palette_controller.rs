//! Orchestrates the full lifecycle of the Prompt Palette native panel.
//!
//! `PaletteController` creates the native NSPanel (with ObjC handler and bridge
//! callback) on first show, reuses the existing panel on subsequent shows,
//! hides the panel and restores focus on Esc, and destroys the panel on quit.
//! It also pushes `service:status` and `prompts:invalidate` events to the
//! panel webview.

use std::ffi::c_void;
use std::sync::mpsc;

use tauri::{AppHandle, Manager};

use crate::activation_context::{self, ActivationContext};
use crate::host_application_services::HostServices;
use crate::palette_native::{self, PaletteNativeConfig, PaletteNativeWindow};
use crate::panel_bridge::{PaletteEvent, PanelBridge};

// ---------------------------------------------------------------------------
// Main-thread detection
// ---------------------------------------------------------------------------

/// Returns `true` if the current thread is the application main thread.
///
/// Used to avoid deadlocks: if `show()` is already on the main thread,
/// calling `run_on_main_thread` + `recv()` would deadlock because the main
/// thread is blocked waiting on the channel while the queued work can never
/// run.
#[cfg(target_os = "macos")]
fn is_main_thread() -> bool {
    unsafe { libc::pthread_main_np() != 0 }
}

#[cfg(not(target_os = "macos"))]
fn is_main_thread() -> bool {
    false
}

// ---------------------------------------------------------------------------
// State enum
// ---------------------------------------------------------------------------

/// Lifecycle state of the palette panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaletteControllerState {
    #[default]
    Hidden,
    Showing,
    Visible,
    Hiding,
}

impl PaletteControllerState {
    /// Returns `true` for states where the panel is considered on-screen.
    pub fn is_visible(&self) -> bool {
        matches!(self, Self::Showing | Self::Visible)
    }
}

// ---------------------------------------------------------------------------
// SendPtr — raw pointer wrapper that is Send
// ---------------------------------------------------------------------------

/// Wrapper around a raw pointer that implements `Send`.
///
/// The underlying pointer is only dereferenced on the main thread via
/// `run_on_main_thread`.
struct SendPtr(*mut c_void);

unsafe impl Send for SendPtr {}

impl SendPtr {
    fn get(self) -> *mut c_void {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Thin wrappers around palette_native that accept *mut c_void
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn show_panel_cv(panel: *mut c_void, webview: *mut c_void) {
    palette_native::show_panel(panel as _, webview as _);
}

#[cfg(not(target_os = "macos"))]
fn show_panel_cv(panel: *mut c_void, webview: *mut c_void) {
    palette_native::show_panel(panel, webview);
}

#[cfg(target_os = "macos")]
fn hide_panel_cv(panel: *mut c_void) {
    palette_native::hide_panel(panel as _);
}

#[cfg(not(target_os = "macos"))]
fn hide_panel_cv(panel: *mut c_void) {
    palette_native::hide_panel(panel);
}

#[cfg(target_os = "macos")]
fn destroy_panel_cv(panel: *mut c_void) {
    palette_native::destroy_panel(panel as _);
}

#[cfg(target_os = "macos")]
fn eval_js_cv(webview: *mut c_void, script: &str) {
    palette_native::eval_js(webview as _, script);
}

#[cfg(target_os = "macos")]
fn cleanup_handler_cv(handler: *mut c_void) {
    palette_native::cleanup_handler(handler as _);
}

#[cfg(not(target_os = "macos"))]
fn destroy_panel_cv(panel: *mut c_void) {
    palette_native::destroy_panel(panel);
}

#[cfg(not(target_os = "macos"))]
fn eval_js_cv(webview: *mut c_void, script: &str) {
    palette_native::eval_js(webview, script);
}

#[cfg(not(target_os = "macos"))]
fn cleanup_handler_cv(handler: *mut c_void) {
    palette_native::cleanup_handler(handler);
}

/// Create the native panel and return a `PaletteNativeWindow` with all
/// pointers stored as `*mut c_void`.
///
/// On Windows the underlying `WebviewWindowBuilder` requires an `AppHandle`,
/// so callers must always pass `app` even though the macOS path ignores it.
fn create_panel_cv(
    app: &AppHandle,
    config: &PaletteNativeConfig,
    handler: *mut c_void,
) -> PaletteNativeWindow {
    #[cfg(target_os = "macos")]
    {
        let _ = app; // macOS NSPanel does not need the AppHandle.
        let (panel, webview) = palette_native::create_panel(config, handler as _);
        PaletteNativeWindow {
            panel: panel as _,
            webview: webview as _,
            handler,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        palette_native::create_panel(app, config, handler)
    }
}

/// Create the ObjC message handler, returning `*mut c_void`.
fn create_handler_cv(callback: palette_native::MessageCallback) -> *mut c_void {
    #[cfg(target_os = "macos")]
    {
        palette_native::create_message_handler(callback) as _
    }
    #[cfg(not(target_os = "macos"))]
    {
        palette_native::create_message_handler(callback)
    }
}

// ---------------------------------------------------------------------------
// PaletteController
// ---------------------------------------------------------------------------

/// Orchestrates creation, show/hide, destruction, and event-pushing for the
/// Prompt Palette native panel.
pub struct PaletteController {
    native: Option<PaletteNativeWindow>,
    bridge: PanelBridge,
    activation_ctx: ActivationContext,
    state: PaletteControllerState,
    services: HostServices,
    bundle_resource_dir: String,
    latest_service_status: Option<String>,
}

// Safety: The native window pointers are only accessed on the main thread
// via `run_on_main_thread`. The controller itself is guarded by a Tauri
// `Mutex<PaletteController>` state.
unsafe impl Send for PaletteController {}

impl PaletteController {
    /// Create a new controller in `Hidden` state with no native window.
    pub fn new(services: HostServices, bundle_resource_dir: String) -> Self {
        let bridge = PanelBridge::new();
        bridge.set_task_spawner(|task| {
            tauri::async_runtime::spawn(task);
        });

        Self {
            native: None,
            bridge,
            activation_ctx: ActivationContext::new(),
            state: PaletteControllerState::Hidden,
            services,
            bundle_resource_dir,
            latest_service_status: None,
        }
    }

    /// Current lifecycle state.
    pub fn state(&self) -> PaletteControllerState {
        self.state
    }

    /// Whether the panel is currently visible (Showing or Visible).
    pub fn is_panel_visible(&self) -> bool {
        self.state.is_visible()
    }

    /// Get the webview pointer (for tests or advanced usage).
    pub fn webview_ptr(&self) -> Option<*mut c_void> {
        self.native.as_ref().map(|n| n.webview)
    }

    /// Get a reference to the underlying `HostServices`.
    pub fn services(&self) -> &HostServices {
        &self.services
    }

    /// Store the latest service status so it can be replayed when the panel
    /// opens. Called from `emit_service_status` regardless of panel visibility.
    pub fn update_service_status(&mut self, status: &str) {
        self.latest_service_status = Some(status.to_string());
        if self.state.is_visible() {
            self.push_service_status(status);
        }
    }

    // -----------------------------------------------------------------------
    // Show
    // -----------------------------------------------------------------------

    /// Show the palette panel.
    ///
    /// On the first call this creates the native NSPanel + WKWebView via
    /// `run_on_main_thread`. On subsequent calls it reuses the existing
    /// panel. A close handler is registered that calls `self.hide()` through
    /// the Tauri state `Mutex<PaletteController>`.
    pub fn show(&mut self, app: &AppHandle) {
        tracing::info!(
            event_code = "palette_controller.show_called",
            has_native = self.native.is_some(),
            state = ?self.state,
            "PaletteController::show called"
        );

        if self.state.is_visible() {
            tracing::debug!(
                event_code = "palette_controller.show_already_visible",
                "palette already visible, ignoring duplicate show"
            );
            return;
        }

        // Capture the foreground app so we can restore focus later.
        if let Some(pid) = activation_context::capture_frontmost_app_pid() {
            self.activation_ctx.set_captured(pid);
        }

        self.state = PaletteControllerState::Showing;

        if self.native.is_some() {
            self.show_existing(app);
        } else {
            self.create_and_show(app);
        }
    }

    /// Show an already-created panel (just orderFront + makeKey).
    fn show_existing(&mut self, app: &AppHandle) {
        let Some(native) = self.native.as_ref() else {
            tracing::error!(
                event_code = "palette_controller.show_existing_no_window",
                "show_existing called with no native window"
            );
            return;
        };

        if is_main_thread() {
            // Already on main thread — call directly, no channel needed.
            show_panel_cv(native.panel, native.webview);
        } else {
            let panel = SendPtr(native.panel);
            let webview = SendPtr(native.webview);

            let result = app.run_on_main_thread(move || {
                show_panel_cv(panel.get(), webview.get());
            });

            if let Err(e) = result {
                tracing::warn!(
                    event_code = "palette_controller.show_existing_failed",
                    error = %e,
                    "failed to run show_panel on main thread"
                );
                return;
            }
        }

        self.state = PaletteControllerState::Visible;

        // Replay stored service status so the panel knows the current state.
        self.replay_service_status();
    }

    /// Create the native panel for the first time and show it.
    fn create_and_show(&mut self, app: &AppHandle) {
        tracing::info!(
            event_code = "palette_controller.create_and_show",
            is_main_thread = is_main_thread(),
            bundle_resource_dir = %self.bundle_resource_dir,
            "PaletteController::create_and_show called"
        );
        let config = PaletteNativeConfig {
            is_debug: cfg!(debug_assertions),
            bundle_resource_dir: self.bundle_resource_dir.clone(),
            ..PaletteNativeConfig::default()
        };

        // Build the bridge message callback.
        let msg_callback = self.bridge.create_message_callback(self.services.clone());

        // Create the ObjC message handler.
        let handler_cv = create_handler_cv(msg_callback);

        // Register a close handler that acquires the PaletteController from
        // Tauri state and calls hide().
        let app_handle = app.clone();
        self.bridge.register_close_handler(Box::new(move || {
            let state = app_handle.try_state::<std::sync::Mutex<PaletteController>>();
            if let Some(ctrl_lock) = state {
                // Use try_lock to avoid deadlock: if another thread holds the
                // Mutex and is waiting on run_on_main_thread (which needs the
                // main thread we're currently on), lock() would deadlock.
                if let Ok(mut ctrl) = ctrl_lock.try_lock() {
                    ctrl.hide(&app_handle);
                } else {
                    tracing::warn!(
                        event_code = "palette_controller.close_handler_lock_contention",
                        "close handler skipped — PaletteController Mutex is contended"
                    );
                }
            } else {
                tracing::warn!(
                    event_code = "palette_controller.close_handler_no_state",
                    "PaletteController not found in Tauri state"
                );
            }
        }));

        if is_main_thread() {
            // Already on main thread — call directly, no channel needed.
            let native_window = create_panel_cv(app, &config, handler_cv);
            show_panel_cv(native_window.panel, native_window.webview);

            // Store the webview pointer in the bridge for event pushing.
            self.bridge.set_webview(native_window.webview);

            // Set the eval_fn: on the main thread we can call eval_js directly.
            self.setup_eval_fn(app);

            self.native = Some(native_window);
            self.state = PaletteControllerState::Visible;

            // Replay stored service status so the panel knows the current state.
            self.replay_service_status();
        } else {
            // Off main thread — use run_on_main_thread + channel to avoid deadlock.
            let (tx, rx) = mpsc::channel::<PaletteNativeWindow>();
            let handler_send = SendPtr(handler_cv);
            let app_clone = app.clone();

            let result = app.run_on_main_thread(move || {
                let native_window = create_panel_cv(&app_clone, &config, handler_send.get());
                show_panel_cv(native_window.panel, native_window.webview);

                // Send the window back. If the receiver is gone, that's fine --
                // the panel was still created and shown.
                let _ = tx.send(native_window);
            });

            if let Err(e) = result {
                tracing::warn!(
                    event_code = "palette_controller.create_failed",
                    error = %e,
                    "failed to run create_panel on main thread"
                );
                self.state = PaletteControllerState::Hidden;
                return;
            }

            // Block until the main thread finishes creating the panel.
            // Timeout guards against a stuck main thread — if panel creation
            // doesn't complete within 5s the state resets to Hidden.
            match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(native_window) => {
                    self.bridge.set_webview(native_window.webview);
                    self.setup_eval_fn(app);
                    self.native = Some(native_window);
                    self.state = PaletteControllerState::Visible;

                    // Replay stored service status so the panel knows the current state.
                    self.replay_service_status();
                }
                Err(e) => {
                    tracing::warn!(
                        event_code = "palette_controller.create_channel_error",
                        error = %e,
                        "failed to receive native window from main thread"
                    );
                    self.state = PaletteControllerState::Hidden;
                }
            }
        }
    }

    /// Configure the eval_fn callback on the bridge.
    ///
    /// The closure checks whether it is on the main thread. If so, it calls
    /// `eval_js` directly. Otherwise it dispatches via `app.run_on_main_thread`.
    fn setup_eval_fn(&self, app: &AppHandle) {
        let app = app.clone();
        self.bridge.set_eval_fn(Box::new(move |wv, script| {
            if is_main_thread() {
                eval_js_cv(wv, script);
            } else {
                let wv = SendPtr(wv);
                let script = script.to_string();
                let _ = app.run_on_main_thread(move || {
                    eval_js_cv(wv.get(), &script);
                });
            }
        }));
    }

    // -----------------------------------------------------------------------
    // Hide
    // -----------------------------------------------------------------------

    /// Hide the palette panel and restore focus to the previously active app.
    pub fn hide(&mut self, app: &AppHandle) {
        self.state = PaletteControllerState::Hiding;

        if let Some(ref native) = self.native {
            if is_main_thread() {
                hide_panel_cv(native.panel);
            } else {
                let panel = SendPtr(native.panel);

                let result = app.run_on_main_thread(move || {
                    hide_panel_cv(panel.get());
                });

                if let Err(e) = result {
                    tracing::warn!(
                        event_code = "palette_controller.hide_failed",
                        error = %e,
                        "failed to run hide_panel on main thread"
                    );
                }
            }
        }

        self.activation_ctx.restore_and_clear();
        self.state = PaletteControllerState::Hidden;
    }

    // -----------------------------------------------------------------------
    // Destroy
    // -----------------------------------------------------------------------

    /// Destroy the native panel (for app shutdown).
    pub fn destroy(&mut self, app: &AppHandle) {
        if let Some(native) = self.native.take() {
            // Clear the webview pointer so any in-flight bridge events
            // don't try to evaluate JS on a destroyed webview.
            self.bridge.set_webview(std::ptr::null_mut());

            let panel = native.panel;
            let handler = native.handler;
            if is_main_thread() {
                destroy_panel_cv(panel);
                cleanup_handler_cv(handler);
            } else {
                let panel = SendPtr(panel);
                let handler = SendPtr(handler);

                let result = app.run_on_main_thread(move || {
                    destroy_panel_cv(panel.get());
                    cleanup_handler_cv(handler.get());
                });

                if let Err(e) = result {
                    tracing::warn!(
                        event_code = "palette_controller.destroy_failed",
                        error = %e,
                        "failed to run destroy_panel on main thread"
                    );
                }
            }
        }

        self.state = PaletteControllerState::Hidden;
    }

    // -----------------------------------------------------------------------
    // Event pushing
    // -----------------------------------------------------------------------

    /// Push a `service:status` event to the palette webview.
    pub fn push_service_status(&self, status: &str) {
        let event = PaletteEvent {
            request_id: None,
            event_type: "service:status".to_string(),
            payload: serde_json::json!({ "status": status }),
        };
        self.bridge.push_event_to_webview(&event);
    }

    /// Push a `prompts:invalidate` event to the palette webview.
    pub fn push_prompts_invalidate(&self) {
        let event = PaletteEvent {
            request_id: None,
            event_type: "prompts:invalidate".to_string(),
            payload: serde_json::json!({}),
        };
        self.bridge.push_event_to_webview(&event);
    }

    /// Replay the stored service status to the panel webview.
    fn replay_service_status(&self) {
        if let Some(ref status) = self.latest_service_status {
            self.push_service_status(status);
        }
    }
}

// ---------------------------------------------------------------------------
// Tauri command entry point
// ---------------------------------------------------------------------------

use std::sync::Mutex;

impl PaletteController {
    /// Single entry point for the Windows Tauri command `palette_panel_message`.
    ///
    /// macOS routes panel messages through its ObjC message handler; both
    /// paths converge on `PanelBridge::create_message_callback`. This method
    /// rebuilds the callback using the same `HostServices` stored on the
    /// controller (sourced from `BusytokState.control_endpoint` at app
    /// startup) so the dispatcher behaves identically cross-platform.
    pub fn handle_panel_message(
        state: &Mutex<PaletteController>,
        app: &tauri::AppHandle,
        body: String,
    ) -> anyhow::Result<()> {
        let ctrl = state
            .lock()
            .map_err(|e| anyhow::anyhow!("PaletteController Mutex poisoned: {e}"))?;
        let services = ctrl.services.clone();
        let callback = ctrl.bridge.create_message_callback(services);
        drop(ctrl);
        let _ = app; // reserved for future main-thread marshaling; no-op today.
        callback(&body);
        Ok(())
    }
}
