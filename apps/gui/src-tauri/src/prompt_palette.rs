//! Tauri command facade for prompt palette operations.
//!
//! All platform-specific implementations live in `prompt_palette_native/`;
//! this module only registers `#[tauri::command]` handlers that delegate
//! to the native layer. The Windows palette webview additionally routes
//! bridge messages through `palette_panel_message`, which delegates to
//! `PaletteController::handle_panel_message` so both macOS and Windows
//! converge on `PanelBridge::create_message_callback`.

use std::sync::Mutex;

use crate::palette_controller::PaletteController;
use crate::prompt_palette_native;

#[tauri::command]
pub(crate) async fn prompt_palette_paste_active_app() -> Result<serde_json::Value, String> {
    tracing::info!(
        event_code = "gui.prompt_palette.paste_started",
        "prompt palette paste attempt started"
    );
    prompt_palette_native::paste_active_app()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn prompt_palette_accessibility_status() -> Result<serde_json::Value, String> {
    Ok(prompt_palette_native::accessibility_status())
}

#[tauri::command]
pub(crate) fn prompt_palette_open_accessibility_settings() -> Result<(), String> {
    prompt_palette_native::open_accessibility_settings()
}

/// Receive a panel-bridge JSON envelope from the Windows palette webview.
///
/// The webview invokes this command via
/// `window.__TAURI__.core.invoke('palette_panel_message', { body })` (see
/// `palette_native::windows::BRIDGE_INIT_SCRIPT`). macOS routes through its
/// ObjC message handler instead, so this command is only called on Windows;
/// the body is handed to `PaletteController::handle_panel_message`, which
/// rebuilds the same `MessageCallback` used on macOS.
#[tauri::command]
pub async fn palette_panel_message(
    body: String,
    controller: tauri::State<'_, Mutex<PaletteController>>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    PaletteController::handle_panel_message(&controller, &app, body).map_err(|e| e.to_string())
}
