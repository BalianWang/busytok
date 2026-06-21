//! Local desktop lifecycle settings owned by the GUI.
//!
//! These settings are independent of `busytok-service` and are persisted
//! to `{config_dir}/desktop_lifecycle.toml` via the shared
//! [`busytok_config::atomic_write`] helper. The in-memory store acts as a
//! cache; `load()` prefers the on-disk copy and `save()` writes through to
//! both memory and disk.
//!
//! Beyond the user-visible `launch_busytok_desktop_at_login` toggle, this
//! file also persists session-suppression state so that a `Quit Busytok
//! Desktop` action survives app relaunches within the same macOS login
//! session. The `suppressed_at_boot_secs` field records the system boot
//! time observed when suppression was set; on the next app launch, if the
//! current boot time differs, suppression is treated as stale and cleared
//! (the user logged out / rebooted, starting a new login session).

use std::sync::Mutex;

use busytok_config::{atomic_write, BusytokPaths};

const SETTINGS_FILE_NAME: &str = "desktop_lifecycle.toml";

/// User-facing desktop lifecycle preference plus session-suppression state.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DesktopLifecycleSettings {
    /// Whether to launch the Busytok desktop host automatically at login
    /// (via `SMAppService.mainApp`).
    pub launch_busytok_desktop_at_login: bool,
    /// Set to `true` by `Quit Busytok Desktop`. While `true` and
    /// `suppressed_at_boot_secs` matches the current system boot time,
    /// auto-ensure / auto-repair are suppressed for the current login
    /// session. Cleared by an explicit GUI reopen or a new login session.
    #[serde(default)]
    pub suppressed_for_session: bool,
    /// System boot-time seconds observed when `suppressed_for_session` was
    /// last set. Used to detect "new login session" (logout/reboot clears
    /// suppression). `None` when no suppression has been recorded.
    #[serde(default)]
    pub suppressed_at_boot_secs: Option<u64>,
}

impl Default for DesktopLifecycleSettings {
    fn default() -> Self {
        Self {
            launch_busytok_desktop_at_login: false,
            suppressed_for_session: false,
            suppressed_at_boot_secs: None,
        }
    }
}

/// Best-effort system boot-time in seconds since UNIX epoch.
///
/// Used as a session-boundary proxy: if the persisted suppression was
/// recorded under a different boot time, the user has logged out / rebooted
/// and suppression no longer applies. Returns `None` when the value can't
/// be read (in which case suppression persistence is conservatively
/// ignored — the app starts in the `Active` phase).
///
/// **Approximation caveat:** boot time is a coarse proxy for "same login
/// session". On a single-user Mac with FileVault off, boot == login, so
/// the approximation is accurate. On multi-user systems or systems with
/// fast user switching, the boundary is less precise: a logout/login
/// without a reboot will not invalidate the suppression. A future
/// revision may track `loginwindow` / `CGSession` notifications for a
/// true session-boundary signal.
#[cfg(target_os = "macos")]
pub fn current_boot_secs() -> Option<u64> {
    use libc::{c_int, sysctl};
    use std::mem;

    // MIB for `kern.boottime`: CTL_KERN (1) → KERN_BOOTTIME (7).
    // libc does not export these constants on all targets, so define
    // them inline.  The previous implementation incorrectly used ASCII
    // bytes ('k', 'v', 'm') which sysctl interpreted as garbage.
    const CTL_KERN: c_int = 1;
    const KERN_BOOTTIME: c_int = 7;
    let mut name: [c_int; 2] = [CTL_KERN, KERN_BOOTTIME];
    let mut size = mem::size_of::<libc::timeval>();
    let mut tv: libc::timeval = unsafe { mem::zeroed() };
    let rc = unsafe {
        sysctl(
            name.as_mut_ptr(),
            name.len() as u32,
            &mut tv as *mut _ as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc == 0 {
        Some(tv.tv_sec.max(0) as u64)
    } else {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub fn current_boot_secs() -> Option<u64> {
    None
}

// ── Store ─────────────────────────────────────────────────────────────

/// Thread-safe, in-memory store for [`DesktopLifecycleSettings`] that
/// persists to `{config_dir}/desktop_lifecycle.toml` on every save.
///
/// Held in Tauri state so the runtime, login-start manager, coordinator,
/// and commands modules can read and write it.
pub struct DesktopLifecycleSettingsStore {
    settings: Mutex<DesktopLifecycleSettings>,
    paths: BusytokPaths,
    boot_secs_fn: Box<dyn Fn() -> Option<u64> + Send + Sync>,
}

impl std::fmt::Debug for DesktopLifecycleSettingsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DesktopLifecycleSettingsStore")
            .field("settings", &self.settings)
            .field("paths", &self.paths)
            .finish_non_exhaustive()
    }
}

impl DesktopLifecycleSettingsStore {
    /// Create a new store. `settings` provides the initial value (used as
    /// the default when no on-disk file exists yet).
    pub fn new(settings: DesktopLifecycleSettings, paths: BusytokPaths) -> Self {
        Self {
            settings: Mutex::new(settings),
            paths,
            boot_secs_fn: Box::new(current_boot_secs),
        }
    }

    /// Test constructor that overrides the boot-time source. Lets tests
    /// deterministically construct "same session" / "different session"
    /// conditions without depending on the host OS.
    #[cfg(test)]
    pub fn with_boot_secs_fn<F>(settings: DesktopLifecycleSettings, paths: BusytokPaths, f: F) -> Self
    where
        F: Fn() -> Option<u64> + Send + Sync + 'static,
    {
        Self {
            settings: Mutex::new(settings),
            paths,
            boot_secs_fn: Box::new(f),
        }
    }

    /// Load settings, preferring the on-disk TOML file. Falls back to the
    /// in-memory default when the file is absent or corrupt.
    pub fn load(&self) -> DesktopLifecycleSettings {
        let file_path = self.paths.config_dir().join(SETTINGS_FILE_NAME);
        if file_path.exists() {
            match std::fs::read_to_string(&file_path) {
                Ok(contents) => match toml::from_str::<DesktopLifecycleSettings>(&contents) {
                    Ok(settings) => return settings,
                    Err(e) => {
                        tracing::warn!(
                            event_code = "desktop_lifecycle.corrupt_file",
                            path = %file_path.display(),
                            error = %e,
                            "corrupt desktop lifecycle settings file; falling back to in-memory"
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        event_code = "desktop_lifecycle.read_failed",
                        path = %file_path.display(),
                        error = %e,
                        "failed to read desktop lifecycle settings file"
                    );
                }
            }
        }
        self.settings.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Replace the stored settings in both memory and the on-disk TOML
    /// file. Writes the file atomically via the shared
                    /// [`busytok_config::atomic_write`] helper.
    pub fn save(&self, settings: DesktopLifecycleSettings) {
        // Update in-memory cache.
        *self.settings.lock().unwrap_or_else(|e| e.into_inner()) = settings.clone();

        // Persist to disk atomically.
        let config_dir = self.paths.config_dir().clone();
        let file_path = config_dir.join(SETTINGS_FILE_NAME);

        match toml::to_string_pretty(&settings) {
            Ok(toml_str) => {
                if let Err(e) = atomic_write(&file_path, &toml_str) {
                    tracing::warn!(
                        event_code = "desktop_lifecycle.save_failed",
                        path = %file_path.display(),
                        error = %e,
                        "failed to persist desktop lifecycle settings"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    event_code = "desktop_lifecycle.serialize_failed",
                    error = %e,
                    "failed to serialize desktop lifecycle settings"
                );
            }
        }
    }

    /// Convenience: mark suppression active for the current session and
    /// stamp the current boot time. Returns the new in-memory settings.
    pub fn record_suppression(&self) -> DesktopLifecycleSettings {
        let mut s = self.load();
        s.suppressed_for_session = true;
        s.suppressed_at_boot_secs = (self.boot_secs_fn)();
        self.save(s.clone());
        s
    }

    /// Convenience: clear session suppression. Used when an explicit GUI
    /// reopen or new login session is detected.
    pub fn clear_suppression(&self) -> DesktopLifecycleSettings {
        let mut s = self.load();
        s.suppressed_for_session = false;
        s.suppressed_at_boot_secs = None;
        self.save(s.clone());
        s
    }

    /// Returns `true` when the persisted suppression is still valid for
    /// the current login session. `false` when suppression was never set,
    /// when the boot time differs (new login session), or when the boot
    /// time can't be read.
    pub fn suppression_active_for_current_session(&self) -> bool {
        let s = self.load();
        if !s.suppressed_for_session {
            return false;
        }
        match (s.suppressed_at_boot_secs, (self.boot_secs_fn)()) {
            (Some(stored), Some(current)) => stored == current,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_settings_disable_login_start() {
        let s = DesktopLifecycleSettings::default();
        assert!(!s.launch_busytok_desktop_at_login);
        assert!(!s.suppressed_for_session);
        assert!(s.suppressed_at_boot_secs.is_none());
    }

    #[test]
    fn store_round_trips_settings() {
        let tmp = TempDir::new().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        let store = DesktopLifecycleSettingsStore::new(
            DesktopLifecycleSettings::default(),
            paths,
        );
        assert!(!store.load().launch_busytok_desktop_at_login);

        store.save(DesktopLifecycleSettings {
            launch_busytok_desktop_at_login: true,
            suppressed_for_session: true,
            suppressed_at_boot_secs: Some(42),
        });
        let loaded = store.load();
        assert!(loaded.launch_busytok_desktop_at_login);
        assert!(loaded.suppressed_for_session);
        assert_eq!(loaded.suppressed_at_boot_secs, Some(42));
    }

    #[test]
    fn load_returns_defaults_when_file_missing() {
        let tmp = TempDir::new().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        let store = DesktopLifecycleSettingsStore::new(
            DesktopLifecycleSettings::default(),
            paths,
        );
        let s = store.load();
        assert!(!s.launch_busytok_desktop_at_login);
        assert!(!s.suppressed_for_session);
    }

    #[test]
    fn load_reads_from_disk_when_file_present() {
        let tmp = TempDir::new().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        let store = DesktopLifecycleSettingsStore::new(
            DesktopLifecycleSettings {
                launch_busytok_desktop_at_login: false,
                suppressed_for_session: false,
                suppressed_at_boot_secs: None,
            },
            paths.clone(),
        );
        // Save a value that DIFFERS from the default (false) to prove
        // the disk file is being read, not the default fallback.
        store.save(DesktopLifecycleSettings {
            launch_busytok_desktop_at_login: true,
            suppressed_for_session: false,
            suppressed_at_boot_secs: None,
        });

        let store2 = DesktopLifecycleSettingsStore::new(
            DesktopLifecycleSettings::default(),
            paths,
        );
        assert!(store2.load().launch_busytok_desktop_at_login);
    }

    #[test]
    fn load_falls_back_on_corrupt_file() {
        let tmp = TempDir::new().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let file_path = paths.config_dir().join(SETTINGS_FILE_NAME);
        std::fs::write(&file_path, "this is not valid toml {{{}").unwrap();

        let store = DesktopLifecycleSettingsStore::new(
            DesktopLifecycleSettings::default(),
            paths,
        );
        // Corrupt TOML must fall back to the constructed default. Assert
        // against the default rather than a hard-coded bool so this tracks
        // the default automatically if it ever flips again.
        assert_eq!(
            store.load().launch_busytok_desktop_at_login,
            DesktopLifecycleSettings::default().launch_busytok_desktop_at_login,
        );
    }

    #[test]
    fn saved_toml_is_human_readable() {
        let tmp = TempDir::new().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        let store = DesktopLifecycleSettingsStore::new(
            DesktopLifecycleSettings::default(),
            paths.clone(),
        );

        store.save(DesktopLifecycleSettings {
            launch_busytok_desktop_at_login: false,
            suppressed_for_session: false,
            suppressed_at_boot_secs: None,
        });

        let file_path = paths.config_dir().join(SETTINGS_FILE_NAME);
        let contents = std::fs::read_to_string(&file_path).unwrap();
        assert!(contents.contains("launch_busytok_desktop_at_login = false"));
    }

    #[test]
    fn record_and_clear_suppression_round_trip() {
        let tmp = TempDir::new().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        let store = DesktopLifecycleSettingsStore::new(
            DesktopLifecycleSettings::default(),
            paths,
        );
        assert!(!store.suppression_active_for_current_session());

        store.record_suppression();
        // suppression_active_for_current_session depends on current_boot_secs();
        // on systems where boot time is readable, suppression should be active.
        // Either way, the persisted flag should be true.
        assert!(store.load().suppressed_for_session);

        store.clear_suppression();
        assert!(!store.load().suppressed_for_session);
        assert!(store.load().suppressed_at_boot_secs.is_none());
    }
}
