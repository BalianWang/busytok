//! macOS AppKit persistent-state controls.
//!
//! Busytok is not a document editor and does not benefit from macOS Resume.
//! After a crash, AppKit can otherwise block launch behind a system restore
//! prompt before the Tauri window is usable.

/// AppKit/NSUserDefaults overrides applied at process startup.
pub(crate) fn persistent_state_default_overrides() -> &'static [(&'static str, bool)] {
    &[
        ("NSQuitAlwaysKeepsWindows", false),
        ("ApplePersistenceIgnoreState", true),
    ]
}

#[cfg(target_os = "macos")]
pub(crate) fn disable_appkit_persistent_state() {
    use std::ffi::CString;

    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}

    unsafe fn nsstring(value: &str) -> Option<*mut Object> {
        let c_value = CString::new(value).ok()?;
        let ns_value: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String: c_value.as_ptr()];
        if ns_value.is_null() {
            None
        } else {
            Some(ns_value)
        }
    }

    let mut failures = 0u32;
    let total = persistent_state_default_overrides().len() as u32;

    unsafe {
        let defaults: *mut Object = msg_send![class!(NSUserDefaults), standardUserDefaults];
        if defaults.is_null() {
            tracing::warn!(
                event_code = "desktop_host.persistent_state_defaults_failed",
                reason = "standard_user_defaults_unavailable",
                "failed to disable AppKit persistent state"
            );
            return;
        }

        for (key, value) in persistent_state_default_overrides() {
            let Some(ns_key) = nsstring(key) else {
                failures += 1;
                tracing::warn!(
                    event_code = "desktop_host.persistent_state_defaults_failed",
                    key = *key,
                    reason = "invalid_key",
                    "failed to build NSUserDefaults key"
                );
                continue;
            };
            let _: () = msg_send![defaults, setBool: *value forKey: ns_key];
        }

        let _: bool = msg_send![defaults, synchronize];
    }

    if failures == 0 {
        tracing::info!(
            event_code = "desktop_host.persistent_state_defaults_applied",
            "disabled AppKit persistent window restoration ({total} keys)"
        );
    } else {
        tracing::warn!(
            event_code = "desktop_host.persistent_state_defaults_partial",
            applied = total - failures,
            failed = failures,
            total = total,
            "AppKit persistent state partially disabled"
        );
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn disable_appkit_persistent_state() {}
