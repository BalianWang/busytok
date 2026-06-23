fn main() {
    let manifest = tauri_build::AppManifest::new().commands(&[
        "invoke_busytok",
        "log_frontend_event",
        "flush_frontend_logs",
        "prompt_palette_paste_active_app",
        "prompt_palette_accessibility_status",
        "prompt_palette_open_accessibility_settings",
        "palette_panel_message",
        "desktop_host_shortcut_diagnostics",
        "desktop_host_retry_shortcut_registration",
        "desktop_host_show_gui",
        "desktop_lifecycle_settings_snapshot",
        "desktop_lifecycle_settings_update",
        "desktop_background_service_diagnostics",
        "desktop_background_service_repair",
        "install_version",
    ]);
    let attrs = tauri_build::Attributes::new().app_manifest(manifest);
    // During CI (cargo clippy/test without pre-built sidecar binaries),
    // tauri_build's externalBin resolution may fail. That's safe — the app
    // is never run during these steps. Only panic during actual app builds.
    if let Err(e) = tauri_build::try_build(attrs) {
        if std::env::var_os("CARGO_FEATURE_CUSTOM_PROTOCOL").is_some() {
            panic!("failed to run tauri build: {e}");
        }
        println!("cargo:warning=tauri_build skipped (no sidecar binaries): {e}");
    }
}
