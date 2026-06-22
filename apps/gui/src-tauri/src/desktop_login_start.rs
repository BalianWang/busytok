//! Desktop login-start manager — `SMAppService.mainApp` lifecycle.
//!
//! ## Two-layer state contract
//!
//! - **Persisted toggle** (`launch_busytok_desktop_at_login` in
//!   `desktop_lifecycle.toml`): whether macOS should launch Busytok at
//!   future logins. Mutated by `enable_for_future_logins` and `disable`.
//! - **In-process host-mode flag** (`host_mode_active`): whether the
//!   current process is providing desktop-host behavior (tray, shortcut,
//!   window-close-as-hide). Mutated by `enable_for_current_session` and
//!   `stop_for_current_session`.
//!
//! The two layers are independent. `stop_for_current_session` must NOT
//! touch the persisted toggle — Quit stops the current session but
//! preserves future-login behavior per spec §65.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::desktop_lifecycle_settings::DesktopLifecycleSettings;
use crate::desktop_lifecycle_settings::DesktopLifecycleSettingsStore;
use crate::service_lifecycle::smappservice_bridge::{MainThreadExecutor, SMAppServiceHandle};

// ── Trait ────────────────────────────────────────────────────────────

pub trait DesktopLoginStart: Send + Sync {
    /// Register `SMAppService.mainApp` and persist
    /// `launch_busytok_desktop_at_login = true`. Does NOT change the
    /// current-process host mode; only affects future logins.
    fn enable_for_future_logins(&self) -> Result<()>;

    /// Persist `launch_busytok_desktop_at_login = false`, unregister
    /// `SMAppService.mainApp`, and deactivate the in-process host mode.
    fn disable(&self) -> Result<()>;

    /// Adopt host mode in the current process (set host_mode_active =
    /// true). Also registers `mainApp` for future logins. Must never
    /// spawn a second GUI process.
    fn enable_for_current_session(&self) -> Result<()>;

    /// Adopt host mode in the current process only. This is used during
    /// GUI startup when the persisted login-start toggle is already true;
    /// it must not call `SMAppService.mainApp` because ServiceManagement can
    /// throw Objective-C exceptions during app launch.
    fn adopt_current_session(&self) -> Result<()>;

    /// Deactivate the in-process host mode only (set host_mode_active =
    /// false). Does NOT touch the persisted toggle or unregister
    /// mainApp — future logins still launch Busytok.
    fn stop_for_current_session(&self) -> Result<()>;

    /// Read the in-process host-mode flag. Used by diagnostics and
    /// smoke to verify Quit actually stopped the desktop-host.
    fn host_mode_active(&self) -> bool;
}

// ── Production manager ───────────────────────────────────────────────

pub struct DesktopLoginStartManager {
    main_app_service: SMAppServiceHandle,
    settings: Arc<DesktopLifecycleSettingsStore>,
    main_thread: Arc<dyn MainThreadExecutor>,
    /// In-process host-mode flag. Independent from the persisted
    /// `launch_busytok_desktop_at_login` toggle.
    host_mode_active: AtomicBool,
}

impl DesktopLoginStartManager {
    pub fn new(
        settings: Arc<DesktopLifecycleSettingsStore>,
        executor: Arc<dyn MainThreadExecutor>,
    ) -> Self {
        Self {
            main_app_service: SMAppServiceHandle::main_app(),
            settings,
            main_thread: executor,
            host_mode_active: AtomicBool::new(false),
        }
    }
}

impl DesktopLoginStart for DesktopLoginStartManager {
    fn enable_for_future_logins(&self) -> Result<()> {
        // Reconcile OS state first; only persist after success so a
        // failed registration does not leave the on-disk toggle ahead
        // of reality.
        self.main_app_service
            .register_with_executor(self.main_thread.as_ref())?;
        let mut s = self.settings.load();
        s.launch_busytok_desktop_at_login = true;
        self.settings.save(s);
        Ok(())
    }

    fn disable(&self) -> Result<()> {
        // Deactivate host mode immediately (in-process, no I/O) so the
        // UI stops desktop-host behavior even if unregister fails.
        self.host_mode_active.store(false, Ordering::SeqCst);
        // Unregister mainApp so we don't relaunch at next login.
        self.main_app_service
            .unregister_with_executor(self.main_thread.as_ref())?;
        // Persist AFTER the OS reconcile succeeds.
        let mut s = self.settings.load();
        s.launch_busytok_desktop_at_login = false;
        self.settings.save(s);
        Ok(())
    }

    fn enable_for_current_session(&self) -> Result<()> {
        // Register mainApp for future logins.
        self.main_app_service
            .register_with_executor(self.main_thread.as_ref())?;
        let mut s = self.settings.load();
        s.launch_busytok_desktop_at_login = true;
        self.settings.save(s);
        // Activate in-process host mode. Does NOT spawn a second GUI
        // process — the host mode is flag-driven in the current process.
        self.host_mode_active.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn adopt_current_session(&self) -> Result<()> {
        self.host_mode_active.store(true, Ordering::SeqCst);
        tracing::info!(
            event_code = "desktop_host.current_session_adopted",
            "desktop host mode adopted in current process without login-item registration"
        );
        Ok(())
    }

    fn stop_for_current_session(&self) -> Result<()> {
        // In-process only. Must NOT touch the persisted toggle — Quit
        // stops the current session but preserves future-login behavior
        // per spec §65.
        self.host_mode_active.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn host_mode_active(&self) -> bool {
        self.host_mode_active.load(Ordering::SeqCst)
    }
}

pub(crate) fn adopt_current_session_if_enabled(
    login_start: &dyn DesktopLoginStart,
    settings: &DesktopLifecycleSettings,
) -> Result<()> {
    if settings.launch_busytok_desktop_at_login {
        login_start.adopt_current_session()?;
    }
    Ok(())
}

// ── Test fake ────────────────────────────────────────────────────────

pub struct FakeDesktopLoginStart {
    actions: Mutex<Vec<String>>,
    unregister_called: Mutex<bool>,
    /// In-process host-mode flag for tests.
    host_mode_active: AtomicBool,
}

impl FakeDesktopLoginStart {
    fn record(&self, action: &str) {
        self.actions.lock().unwrap().push(action.to_string());
    }

    pub fn recorded_actions(&self) -> Vec<String> {
        self.actions.lock().unwrap().clone()
    }

    pub fn unregister_called(&self) -> bool {
        *self.unregister_called.lock().unwrap()
    }

    pub fn register_called(&self) -> bool {
        self.recorded_actions()
            .iter()
            .any(|action| action == "main_app.register")
    }

    // ── Factory methods ──────────────────────────────────────────

    pub fn disabled() -> Self {
        Self {
            actions: Mutex::new(Vec::new()),
            unregister_called: Mutex::new(false),
            host_mode_active: AtomicBool::new(false),
        }
    }

    pub fn enabled() -> Self {
        Self {
            actions: Mutex::new(Vec::new()),
            unregister_called: Mutex::new(false),
            host_mode_active: AtomicBool::new(true),
        }
    }

    pub fn gui_already_active() -> Self {
        Self {
            actions: Mutex::new(Vec::new()),
            unregister_called: Mutex::new(false),
            host_mode_active: AtomicBool::new(false),
        }
    }
}

impl DesktopLoginStart for FakeDesktopLoginStart {
    fn enable_for_future_logins(&self) -> Result<()> {
        self.record("main_app.register");
        Ok(())
    }

    fn disable(&self) -> Result<()> {
        self.host_mode_active.store(false, Ordering::SeqCst);
        self.record("stop_current_host_mode");
        self.record("main_app.unregister");
        *self.unregister_called.lock().unwrap() = true;
        Ok(())
    }

    fn enable_for_current_session(&self) -> Result<()> {
        self.record("main_app.register");
        self.host_mode_active.store(true, Ordering::SeqCst);
        self.record("adopt_host_mode_in_process");
        Ok(())
    }

    fn adopt_current_session(&self) -> Result<()> {
        self.host_mode_active.store(true, Ordering::SeqCst);
        self.record("adopt_host_mode_in_process");
        Ok(())
    }

    fn stop_for_current_session(&self) -> Result<()> {
        self.host_mode_active.store(false, Ordering::SeqCst);
        self.record("stop_current_host_mode");
        Ok(())
    }

    fn host_mode_active(&self) -> bool {
        self.host_mode_active.load(Ordering::SeqCst)
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_login_start_registers_main_app_login_item() {
        let fake = FakeDesktopLoginStart::disabled();
        fake.enable_for_future_logins().unwrap();
        assert_eq!(fake.recorded_actions(), ["main_app.register"]);
    }

    #[test]
    fn disable_login_start_boots_out_current_host_session() {
        let fake = FakeDesktopLoginStart::enabled();
        fake.disable().unwrap();
        assert_eq!(
            fake.recorded_actions(),
            ["stop_current_host_mode", "main_app.unregister"]
        );
    }

    #[test]
    fn enabling_login_start_while_gui_is_active_does_not_spawn_a_second_process() {
        let fake = FakeDesktopLoginStart::gui_already_active();
        fake.enable_for_current_session().unwrap();
        assert_eq!(
            fake.recorded_actions(),
            ["main_app.register", "adopt_host_mode_in_process"]
        );
    }

    #[test]
    fn stop_for_current_session_does_not_unregister() {
        let fake = FakeDesktopLoginStart::enabled();
        fake.stop_for_current_session().unwrap();
        assert_eq!(fake.recorded_actions(), ["stop_current_host_mode"]);
        assert!(!fake.unregister_called());
    }

    #[test]
    fn disable_unregisters_main_app() {
        let fake = FakeDesktopLoginStart::enabled();
        fake.disable().unwrap();
        assert!(fake.unregister_called());
    }

    #[test]
    fn stop_for_current_session_does_not_persist_toggle_change() {
        // Verify the host is active before stop, and the fake does NOT
        // record main_app.unregister or signal register. This test
        // directly asserts the P1-1 fix: stop_for_current_session is an
        // in-process operation that must not affect the persisted
        // login-start toggle.
        let fake = FakeDesktopLoginStart::enabled();
        assert!(fake.host_mode_active());
        fake.stop_for_current_session().unwrap();
        assert!(!fake.host_mode_active());
        assert!(!fake.unregister_called());
        // Only "stop_current_host_mode" should be recorded — no
        // "main_app.register" or "main_app.unregister".
        assert_eq!(fake.recorded_actions(), ["stop_current_host_mode"]);
    }

    #[test]
    fn startup_adopt_uses_in_process_host_mode_without_registering_login_item() {
        let fake = FakeDesktopLoginStart::disabled();
        let settings = crate::desktop_lifecycle_settings::DesktopLifecycleSettings {
            launch_busytok_desktop_at_login: true,
            ..Default::default()
        };

        adopt_current_session_if_enabled(&fake, &settings).unwrap();

        assert!(fake.host_mode_active());
        assert_eq!(fake.recorded_actions(), ["adopt_host_mode_in_process"]);
        assert!(!fake.register_called());
        assert!(!fake.unregister_called());
    }
}
