//! Desktop host shortcut registration, diagnostics, and retry logic.
#![allow(dead_code)]

use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

use crate::desktop_windows;

/// Shortcut key combination for the prompt palette.
pub const PROMPT_PALETTE_SHORTCUT: &str = "CommandOrControl+Option+K";

/// Diagnostic state for the global shortcut.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ShortcutDiagnostics {
    pub state: String,
    pub shortcut: String,
    pub failure_reason: Option<String>,
    pub retry_count: u32,
}

/// Managed state holding the current shortcut diagnostics.
pub struct ShortcutState {
    pub diagnostics: Mutex<ShortcutDiagnostics>,
}

/// Calculate the retry delay in milliseconds using bounded exponential backoff.
///
/// - retry 0: 1s
/// - retry 1: 2s
/// - retry 2: 4s
/// - ...
/// - capped at 30s
pub fn next_retry_delay_ms(retry_count: u32) -> u64 {
    (1_000_u64.saturating_mul(2_u64.saturating_pow(retry_count))).min(30_000)
}

/// Record a shortcut registration failure into the diagnostics struct.
pub fn record_shortcut_failure(
    mut diagnostics: ShortcutDiagnostics,
    shortcut: &str,
    reason: &str,
) -> ShortcutDiagnostics {
    diagnostics.state = "failed".into();
    diagnostics.shortcut = shortcut.into();
    diagnostics.failure_reason = Some(reason.into());
    diagnostics
}

/// Register the global prompt palette shortcut with the Tauri global-shortcut plugin.
///
/// On success, updates the managed `ShortcutState` to `"registered"`.
/// On failure, records diagnostics and emits a `busytok:shortcut-failed` event.
pub fn register_prompt_palette_shortcut(app: &AppHandle) {
    let state = app.state::<ShortcutState>();
    let mut diag = state.diagnostics.lock().unwrap().clone();

    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let shortcut_result = app.global_shortcut().on_shortcut(
        PROMPT_PALETTE_SHORTCUT,
        move |app_handle: &AppHandle,
              _shortcut: &tauri_plugin_global_shortcut::Shortcut,
              event: tauri_plugin_global_shortcut::ShortcutEvent| {
            if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                tracing::info!(
                    event_code = "desktop_host.shortcut_pressed",
                    "prompt palette shortcut pressed"
                );
                // This callback runs in a macOS Carbon Event extern "C" context.
                // Two constraints:
                // 1. Any Rust panic cannot unwind through FFI and would abort
                //    the process → catch_unwind as safety net.
                // 2. Creating WKWebView / NSPanel inside the Carbon event
                //    handler throws an ObjC exception that crosses the FFI
                //    boundary as a foreign exception → abort. ObjC exceptions
                //    are NOT caught by catch_unwind.
                //
                // Defer to the next run-loop iteration so UI work runs outside
                // the Carbon handler. Keep catch_unwind as a second layer.
                let app = AppHandle::clone(app_handle);
                let app_for_closure = app.clone();
                if let Err(e) = app.run_on_main_thread(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        desktop_windows::show_prompt_palette(&app_for_closure);
                    }));
                    if let Err(panic) = result {
                        let cause = panic
                            .downcast_ref::<String>()
                            .cloned()
                            .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                            .unwrap_or_else(|| "unknown panic".to_string());
                        tracing::error!(
                            event_code = "desktop_host.shortcut_handler_panic",
                            cause = %cause,
                            "show_prompt_palette panicked inside run_on_main_thread"
                        );
                    }
                }) {
                    tracing::error!(
                        event_code = "desktop_host.run_on_main_thread_failed",
                        error = %e,
                        "failed to schedule show_prompt_palette on main thread"
                    );
                }
            }
        },
    );

    match shortcut_result {
        Ok(()) => {
            diag.state = "registered".into();
            diag.shortcut = PROMPT_PALETTE_SHORTCUT.into();
            diag.failure_reason = None;
            tracing::info!(
                event_code = "desktop_host.shortcut_registered",
                shortcut = PROMPT_PALETTE_SHORTCUT,
                "global shortcut registered"
            );
        }
        Err(e) => {
            let reason = e.to_string();
            diag = record_shortcut_failure(diag, PROMPT_PALETTE_SHORTCUT, &reason);
            tracing::warn!(
                event_code = "desktop_host.shortcut_registration_failed",
                shortcut = PROMPT_PALETTE_SHORTCUT,
                reason = %reason,
                "global shortcut registration failed"
            );
            let _ = app.emit("busytok:shortcut-failed", &diag);
        }
    }

    *state.diagnostics.lock().unwrap() = diag;
}

/// Retry shortcut registration. Resets diagnostics to idle first.
pub fn retry_shortcut_registration(app: &AppHandle) {
    let state = app.state::<ShortcutState>();
    let mut diag = state.diagnostics.lock().unwrap();
    diag.retry_count += 1;
    diag.state = "idle".into();
    diag.failure_reason = None;
    drop(diag);

    register_prompt_palette_shortcut(app);
}

/// Tauri command: return current shortcut diagnostics.
#[tauri::command]
pub(crate) fn desktop_host_shortcut_diagnostics(app: AppHandle) -> ShortcutDiagnostics {
    let state = app.state::<ShortcutState>();
    let diag = state.diagnostics.lock().unwrap().clone();
    diag
}

/// Tauri command: retry shortcut registration.
#[tauri::command]
pub(crate) fn desktop_host_retry_shortcut_registration(app: AppHandle) -> Result<(), String> {
    retry_shortcut_registration(&app);
    Ok(())
}
