/// Captures the pid of the macOS frontmost application so it can be
/// restored (reactivated) later. Used by the prompt-palette to give
/// focus back to the user's previous app when the palette closes.
pub struct ActivationContext {
    previous_frontmost: Option<i32>,
}

impl ActivationContext {
    pub fn new() -> Self {
        Self {
            previous_frontmost: None,
        }
    }

    /// Store the pid that should be restored later.
    pub fn set_captured(&mut self, pid: i32) {
        self.previous_frontmost = Some(pid);
    }

    /// Attempt to activate the previously captured app and clear internal
    /// state. Returns `true` if a pid was captured **and** the activation
    /// call succeeded. Returns `false` if nothing was captured or the
    /// activation failed (e.g. the process no longer exists).
    pub fn restore_and_clear(&mut self) -> bool {
        let pid = match self.previous_frontmost.take() {
            Some(p) => p,
            None => {
                tracing::debug!(
                    event_code = "activation_context.no_pid_to_restore",
                    "no captured PID to restore"
                );
                return false;
            }
        };
        tracing::debug!(
            event_code = "activation_context.restoring",
            pid,
            "restoring previously captured frontmost app"
        );
        let result = activate_app_by_pid(pid);
        if !result {
            tracing::warn!(
                event_code = "activation_context.restore_failed",
                pid,
                "failed to activate previously captured app (process may have exited)"
            );
        }
        result
    }
}

impl Default for ActivationContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Capture the pid of the current frontmost application.
///
/// Returns `None` on non-macOS platforms or if the query fails.
pub fn capture_frontmost_app_pid() -> Option<i32> {
    #[cfg(target_os = "macos")]
    {
        macos_ffi::get_frontmost_app_pid()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

// ---------------------------------------------------------------------------
// Windows implementation — foreground HWND + AttachThreadInput guard
// ---------------------------------------------------------------------------
#[cfg(target_os = "windows")]
pub mod windows {
    use std::time::Instant;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    #[derive(Debug, Clone, Copy)]
    pub struct WindowsActivationContext {
        pub hwnd: HWND,
        pub thread_id: u32,
        pub process_id: u32,
        pub captured_at: Instant,
    }

    pub fn capture_foreground() -> Option<WindowsActivationContext> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_null() {
                return None;
            }
            let mut pid: u32 = 0;
            let tid = GetWindowThreadProcessId(hwnd, &mut pid);
            Some(WindowsActivationContext {
                hwnd,
                thread_id: tid,
                process_id: pid,
                captured_at: Instant::now(),
            })
        }
    }

    pub fn activate(ctx: &WindowsActivationContext) -> bool {
        let guard = AttachThreadInputGuard::attach(ctx.thread_id);
        let ok = unsafe { SetForegroundWindow(ctx.hwnd) };
        drop(guard);
        ok != 0
    }

    pub struct AttachThreadInputGuard {
        target_tid: u32,
    }
    impl AttachThreadInputGuard {
        pub fn attach(target_tid: u32) -> Self {
            unsafe {
                let me = GetCurrentThreadId();
                AttachThreadInput(me, target_tid, 1);
            }
            Self { target_tid }
        }
    }
    impl Drop for AttachThreadInputGuard {
        fn drop(&mut self) {
            unsafe {
                let me = GetCurrentThreadId();
                AttachThreadInput(me, self.target_tid, 0);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn attach_guard_drops_without_panic() {
            if let Some(ctx) = capture_foreground() {
                let _g = AttachThreadInputGuard::attach(ctx.thread_id);
            }
        }
        #[test]
        fn capture_returns_some_when_foreground_exists() {
            // On Windows test runners there's always a foreground window.
            // If this fails in headless env, adjust accordingly.
            assert!(capture_foreground().is_some());
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows::{activate, capture_foreground, AttachThreadInputGuard, WindowsActivationContext};

// ---------------------------------------------------------------------------
// macOS implementation — Objective-C runtime FFI
// ---------------------------------------------------------------------------
#[cfg(target_os = "macos")]
fn activate_app_by_pid(pid: i32) -> bool {
    macos_ffi::activate_app_by_pid(pid)
}

#[cfg(not(target_os = "macos"))]
fn activate_app_by_pid(_pid: i32) -> bool {
    false
}

#[cfg(target_os = "macos")]
mod macos_ffi {
    use std::os::raw::c_int;

    use objc::{class, msg_send, sel, sel_impl};

    extern "C" {
        /// send signal 0 to check if a process exists.
        fn kill(pid: c_int, sig: c_int) -> c_int;
    }

    /// Safety: must only be called on the main thread with valid ObjC
    /// selectors.
    pub fn get_frontmost_app_pid() -> Option<i32> {
        unsafe {
            let workspace: *mut objc::runtime::Object =
                msg_send![class!(NSWorkspace), sharedWorkspace];
            if workspace.is_null() {
                tracing::warn!(
                    event_code = "activation_context.workspace_null",
                    "NSWorkspace.sharedWorkspace returned null"
                );
                return None;
            }

            let app: *mut objc::runtime::Object = msg_send![workspace, frontmostApplication];
            if app.is_null() {
                tracing::debug!(
                    event_code = "activation_context.frontmost_app_null",
                    "NSWorkspace.frontmostApplication returned null"
                );
                return None;
            }

            let pid: isize = msg_send![app, processIdentifier];
            Some(pid as i32)
        }
    }

    /// Activate the application with the given pid via
    /// `NSRunningApplication.activateWithOptions:`.
    ///
    /// `NSApplicationActivateIgnoringOtherApps = 1 << 1 = 2`.
    pub fn activate_app_by_pid(pid: i32) -> bool {
        // Quick POSIX check: does a process with this pid exist?
        unsafe {
            if kill(pid, 0) != 0 {
                return false;
            }
        }

        unsafe {
            let app: *mut objc::runtime::Object = msg_send![
                class!(NSRunningApplication),
                runningApplicationWithProcessIdentifier: pid as c_int
            ];
            if app.is_null() {
                return false;
            }

            // NSApplicationActivateIgnoringOtherApps = 2
            let result: bool = msg_send![app, activateWithOptions: 2u64];
            result
        }
    }
}
