//! macOS prompt palette native implementation — accessibility checks and
//! paste injection via CoreGraphics events.

const FAILURE_PERMISSION_MISSING: &str = "permission_missing";
const FAILURE_INJECTION_FAILED: &str = "injection_failed";
const ACCESSIBILITY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

/// Delay after hide completes before injecting Cmd+V. Must be long enough for
/// the panel to fully disappear and `activateApp(byPid:)` to bring the
/// previous app to the foreground. 100ms is empirically sufficient on macOS.
const PASTE_SETTLE_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

/// Check whether the process has accessibility permissions.
pub fn accessibility_status() -> serde_json::Value {
    if is_process_trusted_for_accessibility() {
        serde_json::json!({"ok": true})
    } else {
        serde_json::json!({"ok": false, "failure_reason": FAILURE_PERMISSION_MISSING})
    }
}

/// Hide the palette, wait for the settle delay, then inject Cmd+V into the
/// previously active app.
pub async fn paste_active_app() -> anyhow::Result<serde_json::Value> {
    let status = accessibility_status();
    if !status["ok"].as_bool().unwrap_or(false) {
        tracing::warn!(
            event_code = "gui.prompt_palette.paste_permission_missing",
            ok = false,
            failure_reason = FAILURE_PERMISSION_MISSING,
        );
        return Ok(status);
    }

    tokio::time::sleep(PASTE_SETTLE_DELAY).await;

    match post_command_v() {
        Ok(()) => {
            tracing::info!(event_code = "gui.prompt_palette.paste_injected", ok = true,);
            Ok(serde_json::json!({"ok": true}))
        }
        Err(()) => {
            tracing::warn!(
                event_code = "gui.prompt_palette.paste_injection_failed",
                ok = false,
                failure_reason = FAILURE_INJECTION_FAILED,
            );
            Ok(serde_json::json!({"ok": false, "failure_reason": FAILURE_INJECTION_FAILED}))
        }
    }
}

/// Open the macOS Accessibility settings pane.
pub fn open_accessibility_settings() -> Result<(), String> {
    let status = std::process::Command::new("open")
        .arg(ACCESSIBILITY_SETTINGS_URL)
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open exited with status {status}"))
    }
}

// ---------------------------------------------------------------------------
// macOS accessibility / keyboard event injection
// ---------------------------------------------------------------------------

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> std::os::raw::c_uchar;
}

fn is_process_trusted_for_accessibility() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

fn post_command_v() -> Result<(), ()> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, KeyCode};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)?;
    let flags = CGEventFlags::CGEventFlagCommand;
    let key_down = CGEvent::new_keyboard_event(source.clone(), KeyCode::ANSI_V, true)?;
    let key_up = CGEvent::new_keyboard_event(source, KeyCode::ANSI_V, false)?;

    key_down.set_flags(flags);
    key_up.set_flags(flags);
    key_down.post(CGEventTapLocation::HID);
    key_up.post(CGEventTapLocation::HID);

    Ok(())
}
