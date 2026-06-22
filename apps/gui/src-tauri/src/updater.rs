//! Tauri 2 updater — Rust-side bootstrap.
//!
//! The plugin itself is registered in `lib.rs` via
//! `.plugin(tauri_plugin_updater::Builder::new().build())`.
//!
//! Tauri 2 removed the Tauri 1 auto-poll behavior. The actual check /
//! download / install / relaunch flow is driven from the frontend via
//! `initUpdaterAutoCheck()` in `apps/gui/src/lib/updaterClient.ts`,
//! fired once at module-level from `apps/gui/src/main.tsx` (mirrors
//! `initThemeRuntime`). See Task 27.
//!
//! This module's only responsibility is to emit a startup tracing event
//! so logs confirm the plugin loaded. The plugin's own check/download
//! activity emits its own tracing events, captured by the subscriber
//! initialized in `logging.rs`.

/// Called once from `lib.rs` Tauri setup hook. Emits a tracing::info!
/// so logs confirm the updater plugin loaded at startup.
pub(crate) fn init_updater_logging() {
    tracing::info!(
        "Tauri updater plugin loaded; checks are driven by the frontend initUpdaterAutoCheck"
    );
}
