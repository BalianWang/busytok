//! Runtime-rendered, user-domain managed LaunchAgent plist.
//!
//! Production service lifecycle no longer consumes the plist bundled inside
//! `Busytok.app`. That bundled plist was rendered with build-machine absolute
//! paths at packaging time (see `packaging/macos/scripts/_bundle_helpers.sh`),
//! so on an end-user machine its `ProgramArguments[0]` still pointed at
//! `/Users/runner/.../busytok-service` and launchd failed with `EX_CONFIG`.
//!
//! Instead, the GUI renders a minimal plist into the *current user's*
//! `~/Library/LaunchAgents/com.busytok.service.plist` at the moment it
//! bootstraps the service, using the install location resolved from
//! [`BundleLayout::service_binary_path`] (which is derived from the running
//! GUI's own bundle path). Moving the `.app` therefore self-heals on next
//! launch: the stale-bundle repair rewrites this plist before re-bootstrapping.
//!
//! The plist intentionally carries only:
//! - `Label`
//! - `ProgramArguments[0]` = absolute path to the bundled `busytok-service`
//! - `RunAtLoad`, `KeepAlive`
//!
//! Log paths (`StandardOutPath` / `StandardErrorPath`) and the
//! `BUSYTOK_APP_DATA_DIR` env var are deliberately OMITTED. `busytok-service`
//! resolves its own data and log directories at runtime via
//! `BusytokPaths::new()` (see `apps/service/src/main.rs`); baking user or
//! build-machine paths into the plist would reintroduce the exact class of
//! bug this module exists to fix.

use std::path::PathBuf;

use anyhow::{Context, Result};
use busytok_config::atomic_write;
use busytok_platform::PlatformPaths;

use super::bundle_layout::{BundleLayout, SERVICE_LABEL, SERVICE_PLIST_FILENAME};

/// Render the minimal managed-agent plist for the given bundle layout.
///
/// Pure function — no I/O. `ProgramArguments[0]` is the absolute path to the
/// service binary as resolved from the current install location, so the
/// rendered plist tracks the app wherever it is installed or moved.
pub fn render_managed_plist(layout: &BundleLayout) -> String {
    let binary = layout.service_binary_path();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{label}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>{binary}</string>\n\
         \t</array>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>KeepAlive</key>\n\
         \t<true/>\n\
         </dict>\n\
         </plist>\n",
        label = SERVICE_LABEL,
        binary = binary.display(),
    )
}

/// Path to the managed user-domain plist:
/// `~/Library/LaunchAgents/com.busytok.service.plist`.
pub fn managed_plist_path(platform: &PlatformPaths) -> PathBuf {
    platform.service_install_root().join(SERVICE_PLIST_FILENAME)
}

/// Ensure the managed plist on disk matches the currently-desired render.
///
/// Atomically writes the plist if it is missing or its contents differ from
/// [`render_managed_plist`]. Returns the path to the managed plist (always
/// `managed_plist_path(platform)`). Call this immediately before
/// `launchctl bootstrap` so the loaded job always points at the current
/// install location.
pub fn ensure_managed_plist_current(
    layout: &BundleLayout,
    platform: &PlatformPaths,
) -> Result<PathBuf> {
    let path = managed_plist_path(platform);
    let desired = render_managed_plist(layout);
    let needs_write = match std::fs::read_to_string(&path) {
        Ok(existing) => existing != desired,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => {
            return Err(e).with_context(|| {
                format!("reading managed launch agent plist at {}", path.display())
            });
        }
    };
    if needs_write {
        atomic_write(&path, &desired)
            .with_context(|| format!("writing managed launch agent plist at {}", path.display()))?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use busytok_platform::PlatformPaths;

    fn layout_at(root: &str) -> BundleLayout {
        BundleLayout::for_app_root(root)
    }

    /// Acceptance #1: ProgramArguments[0] equals the current bundle's
    /// service-binary path.
    #[test]
    fn program_arguments_points_at_current_service_binary() {
        let layout = layout_at("/Applications/Busytok.app");
        let plist = render_managed_plist(&layout);
        let expected = layout.service_binary_path().display().to_string();
        assert!(
            plist.contains(&expected),
            "rendered plist must contain the current service binary path\n{plist}"
        );
        // And it must appear inside ProgramArguments.
        assert!(
            plist.contains("<key>ProgramArguments</key>"),
            "plist must declare ProgramArguments"
        );
    }

    /// Acceptance #1: the rendered plist must never reference the build
    /// machine. This is the regression guard for the original bug.
    #[test]
    fn never_references_build_machine() {
        let layout = layout_at("/Applications/Busytok.app");
        let plist = render_managed_plist(&layout);
        assert!(
            !plist.contains("/Users/runner"),
            "plist must not contain build-machine paths\n{plist}"
        );
        assert!(
            !plist.contains("runner/work"),
            "plist must not contain build-machine paths\n{plist}"
        );
    }

    /// Acceptance #1: no env-var injection of data dir (the service
    /// self-resolves via BusytokPaths::new()).
    #[test]
    fn omits_app_data_dir_env() {
        let layout = layout_at("/Applications/Busytok.app");
        let plist = render_managed_plist(&layout);
        assert!(
            !plist.contains("BUSYTOK_APP_DATA_DIR"),
            "plist must not inject BUSYTOK_APP_DATA_DIR\n{plist}"
        );
        assert!(
            !plist.contains("<key>EnvironmentVariables</key>"),
            "plist must not declare EnvironmentVariables\n{plist}"
        );
    }

    /// Acceptance #1: no log/stdout paths baked in (service self-resolves).
    #[test]
    fn omits_log_and_stdout_paths() {
        let layout = layout_at("/Applications/Busytok.app");
        let plist = render_managed_plist(&layout);
        assert!(
            !plist.contains("StandardOutPath"),
            "plist must not set StandardOutPath\n{plist}"
        );
        assert!(
            !plist.contains("StandardErrorPath"),
            "plist must not set StandardErrorPath\n{plist}"
        );
        assert!(
            !plist.contains("Library/Logs"),
            "plist must not bake log paths\n{plist}"
        );
    }

    /// Acceptance #1: minimal but valid — Label, RunAtLoad, KeepAlive present.
    #[test]
    fn contains_required_keys() {
        let layout = layout_at("/Applications/Busytok.app");
        let plist = render_managed_plist(&layout);
        assert!(plist.contains("<key>Label</key>"));
        assert!(plist.contains("com.busytok.service"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("<true/>"));
    }

    /// `managed_plist_path` lands under the user LaunchAgents dir with the
    /// canonical filename.
    #[test]
    fn managed_plist_path_under_launch_agents() {
        let platform = PlatformPaths::with_home_dir(std::path::PathBuf::from("/Users/test"));
        let path = managed_plist_path(&platform);
        assert!(path.starts_with("/Users/test/Library/LaunchAgents"));
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("com.busytok.service.plist")
        );
    }

    /// Acceptance #2: ensure writes when missing, and rewrites when the
    /// install location changes (stale-bundle self-heal).
    #[test]
    fn ensure_writes_when_missing_and_rewrites_on_move() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let platform = PlatformPaths::with_home_dir(tmp.path().to_path_buf());

        // Use non-overlapping roots so substring checks are unambiguous
        // (a moved-from "/Users/x/Applications/..." would otherwise still
        // contain the "/Applications/..." suffix of the old path).
        let layout_a = layout_at("/Volumes/A/Busytok.app");
        let path = ensure_managed_plist_current(&layout_a, &platform).expect("ensure");
        let written = std::fs::read_to_string(&path).expect("read");
        assert!(written.contains("/Volumes/A/Busytok.app/Contents/MacOS/busytok-service"));

        // App moved. ensure must rewrite to the new path.
        let layout_b = layout_at("/Volumes/B/Busytok.app");
        let path_b = ensure_managed_plist_current(&layout_b, &platform).expect("ensure moved");
        assert_eq!(path, path_b, "managed plist path is stable across moves");
        let rewritten = std::fs::read_to_string(&path).expect("read rewritten");
        assert!(
            rewritten.contains("/Volumes/B/Busytok.app/Contents/MacOS/busytok-service"),
            "plist must track the new install location\n{rewritten}"
        );
        assert!(
            !rewritten.contains("/Volumes/A/"),
            "stale install path must be gone after rewrite\n{rewritten}"
        );
    }

    /// `ensure` is a no-op write when the plist already matches (idempotent).
    #[test]
    fn ensure_is_idempotent_when_current() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let platform = PlatformPaths::with_home_dir(tmp.path().to_path_buf());
        let layout = layout_at("/Applications/Busytok.app");

        let path = ensure_managed_plist_current(&layout, &platform).expect("first ensure");
        let mtime_before = std::fs::metadata(&path).expect("meta").modified().unwrap();

        // Spin briefly so a rewrite (if it happened) would change mtime.
        std::thread::sleep(std::time::Duration::from_millis(20));

        ensure_managed_plist_current(&layout, &platform).expect("second ensure");
        let mtime_after = std::fs::metadata(&path).expect("meta").modified().unwrap();
        assert_eq!(
            mtime_before, mtime_after,
            "idempotent ensure must not rewrite an already-current plist"
        );
    }
}
