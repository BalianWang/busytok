//! Windows prompt palette native implementation — paste via SendInput and a
//! UAC integrity boundary check.
//!
//! When the user picks a snippet in the palette, we hide the palette window
//! (which was created with `WS_EX_NOACTIVATE`, so it never stole keyboard
//! focus), read the current foreground window, verify it isn't running at a
//! higher integrity level than this process (otherwise UIPI would silently
//! drop our injected keystrokes), and finally `SendInput` a Ctrl+V sequence.
//!
//! The `AttachThreadInputGuard` temporarily attaches our thread's input
//! processing to the foreground thread so `SendInput` is delivered to the
//! correct message queue even when our process is in the background.

use anyhow::{Context, Result};
use serde::Serialize;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Debug, Serialize)]
pub struct PasteResult {
    pub ok: bool,
    pub failure: Option<String>,
}

pub async fn paste_active_app() -> Result<serde_json::Value> {
    // paste_blocking() does OpenProcess / SendInput which is synchronous
    // Win32 work that can take tens of ms. Run it on the blocking pool so
    // we don't stall the tokio executor (matches macOS, which itself
    // yields to the runtime via tokio::time::sleep before injecting).
    let result = tauri::async_runtime::spawn_blocking(paste_blocking)
        .await
        .context("paste_blocking task join failed")?;
    Ok(serde_json::to_value(result)?)
}

fn paste_blocking() -> PasteResult {
    // KNOWN DEVIATION: captures foreground at paste-time, not show-time (per spec §6
    // activation_context should capture at palette show). In NoActivate default
    // mode this is safe — WS_EX_NOACTIVATE prevents the palette from becoming
    // foreground between show and paste, so the captured window is still the
    // correct target. If FocusedWindowFallback mode is ever enabled, this MUST
    // be updated to use a context captured at show-time (stored on PaletteController
    // or passed through the JS bridge), otherwise paste may inject into the
    // palette window itself. Tracked in acceptance doc §E.
    let Some(ctx) = crate::activation_context::capture_foreground() else {
        tracing::warn!(event_code = "prompt_palette.no_foreground_window");
        return PasteResult {
            ok: false,
            failure: Some("no_foreground_window".into()),
        };
    };
    let ctx = if unsafe { IsWindow(ctx.hwnd) } == 0 {
        tracing::debug!(event_code = "prompt_palette.foreground_lost_recover");
        match crate::activation_context::capture_foreground() {
            Some(fresh) => fresh,
            None => {
                tracing::warn!(event_code = "prompt_palette.foreground_lost");
                return PasteResult {
                    ok: false,
                    failure: Some("foreground_lost".into()),
                };
            }
        }
    } else {
        ctx
    };

    if let Some(failure) = check_elevation_mismatch(ctx.process_id) {
        return PasteResult {
            ok: false,
            failure: Some(failure),
        };
    }
    let _guard = crate::activation_context::AttachThreadInputGuard::attach(ctx.thread_id);
    match send_ctrl_v() {
        Ok(()) => {
            tracing::info!(event_code = "prompt_palette.paste_injected");
            PasteResult {
                ok: true,
                failure: None,
            }
        }
        Err(e) => {
            tracing::warn!(
                event_code = "prompt_palette.paste_injection_failed",
                error = %e
            );
            PasteResult {
                ok: false,
                failure: Some(format!("injection_failed: {e}")),
            }
        }
    }
}

fn check_elevation_mismatch(target_pid: u32) -> Option<String> {
    unsafe {
        let target_handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, target_pid);
        if target_handle.is_null() {
            return None;
        }
        let result = (|| {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(target_handle, TOKEN_QUERY, &mut token) == 0 {
                return None;
            }
            let target_il = read_integrity_level(token);
            CloseHandle(token);
            target_il
        })();
        CloseHandle(target_handle);
        let target_il = match result {
            Some(il) => il,
            None => return None,
        };
        let my_il = match read_own_integrity_level() {
            Some(il) => il,
            None => return None,
        };
        if target_il > my_il {
            tracing::warn!(
                event_code = "prompt_palette.elevation_mismatch",
                target_il, my_il, target_pid,
                "target window is elevated; SendInput will be blocked by UIPI"
            );
            Some("target_elevated".into())
        } else {
            None
        }
    }
}

unsafe fn read_integrity_level(token: HANDLE) -> Option<u32> {
    let mut len = 0u32;
    GetTokenInformation(
        token,
        TokenIntegrityLevel,
        std::ptr::null_mut(),
        0,
        &mut len,
    );
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    if GetTokenInformation(
        token,
        TokenIntegrityLevel,
        buf.as_mut_ptr() as *mut _,
        len,
        &mut len,
    ) == 0
    {
        return None;
    }
    let il = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
    let sid = il.Label.Sid;
    if sid.is_null() {
        return None;
    }
    let rid_count = GetSidSubAuthorityCount(sid);
    if rid_count.is_null() || *rid_count == 0 {
        return None;
    }
    let rid_ptr = GetSidSubAuthority(sid, (*rid_count - 1) as u32);
    if rid_ptr.is_null() {
        return None;
    }
    Some(*rid_ptr)
}

unsafe fn read_own_integrity_level() -> Option<u32> {
    let mut token: HANDLE = std::ptr::null_mut();
    if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
        return None;
    }
    let il = read_integrity_level(token);
    CloseHandle(token);
    il
}

fn send_ctrl_v() -> std::result::Result<(), std::io::Error> {
    let mut inputs: [KEYBDINPUT; 4] = unsafe { std::mem::zeroed() };
    inputs[0].wVk = VK_CONTROL;
    inputs[0].dwFlags = 0;
    inputs[1].wVk = 0x56; // V
    inputs[1].dwFlags = 0;
    inputs[2].wVk = 0x56; // V
    inputs[2].dwFlags = KEYEVENTF_KEYUP;
    inputs[3].wVk = VK_CONTROL;
    inputs[3].dwFlags = KEYEVENTF_KEYUP;

    let mut input_structs: [INPUT; 4] = unsafe { std::mem::zeroed() };
    for (s, d) in inputs.iter().zip(input_structs.iter_mut()) {
        d.r#type = INPUT_KEYBOARD;
        unsafe {
            std::ptr::copy_nonoverlapping(s, &mut d.Anonymous.ki, 1);
        }
    }
    let expected = input_structs.len() as u32;
    let sent = unsafe {
        SendInput(
            expected,
            input_structs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if sent != expected {
        // Partial success -- SendInput injected only some of the events.
        // Treat as failure so callers see injection_failed and the user
        // gets a retry instead of a half-fired keystroke sequence.
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("SendInput injected only {sent} of {expected} events"),
        ));
    }
    Ok(())
}

pub fn accessibility_status() -> serde_json::Value {
    tracing::debug!(
        event_code = "prompt_palette.accessibility_status_checked",
        "Windows does not require an accessibility permission for SendInput"
    );
    serde_json::json!({ "ok": true, "platform": "windows" })
}

/// Windows has no equivalent of the macOS Accessibility pane to deep-link to.
/// Returns an error so the frontend can fall back to a generic notice.
pub fn open_accessibility_settings() -> Result<(), String> {
    Err("unsupported_platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn own_integrity_level_is_readable() {
        unsafe {
            assert!(read_own_integrity_level().is_some());
        }
    }
    #[test]
    fn self_vs_self_has_no_mismatch() {
        let own_pid = std::process::id();
        assert!(check_elevation_mismatch(own_pid).is_none());
    }
}
