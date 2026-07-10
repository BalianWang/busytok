//! Auto-install the CLI shim on first app launch (and auto-uninstall on
//! `--uninstall-self`).
//!
//! ## Why
//!
//! DMG distribution has no `postinstall` hook — the user drags
//! `Busytok.app` to `/Applications` and the CLI binary stays hidden inside
//! the app bundle. To make `busytok` available on PATH without manual
//! action, the GUI spawns the bundled `busytok cli install` on first
//! launch, guarded by a version-stamped marker file so it only runs once
//! per installation (and re-runs on version upgrade).
//!
//! ## How
//!
//! - [`ensure_cli_installed`] resolves the enclosing `.app` bundle from
//!   `current_exe()`, locates the bundled CLI binary at
//!   `Contents/MacOS/busytok`, and spawns `busytok cli install
//!   --app-bundle-path <bundle>`.
//! - [`uninstall_cli`] removes the marker and spawns
//!   `busytok cli uninstall`. Called from `--uninstall-self`.
//!
//! All operations are best-effort: failures are logged but never block
//! app startup or uninstall.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Marker file placed in the app's data directory to indicate the CLI
/// shim has been auto-installed for a specific app version.
const MARKER_FILE_NAME: &str = "cli-auto-installed";

/// Name of the bundled CLI binary inside `Contents/MacOS/`.
const BUNDLED_CLI_NAME: &str = "busytok";

// ── Pure helpers (fully unit-testable) ─────────────────────────────────

/// Return the path to the bundled CLI binary inside a `.app` bundle.
fn resolve_bundled_cli_from_bundle(bundle: &Path) -> PathBuf {
    bundle.join("Contents/MacOS").join(BUNDLED_CLI_NAME)
}

/// Return the marker file path: `{data_dir}/cli-auto-installed`.
fn marker_path(data_dir: &Path) -> PathBuf {
    data_dir.join(MARKER_FILE_NAME)
}

/// Decide whether auto-install should run based on the current marker
/// content and the app version.
///
/// - `None` → marker doesn't exist → install.
/// - `Some(v)` where `v == current_version` → already installed → skip.
/// - `Some(v)` where `v != current_version` → version upgrade → re-install.
fn should_install(marker_content: Option<&str>, current_version: &str) -> bool {
    match marker_content {
        None => true,
        Some(v) => v.trim() != current_version,
    }
}

// ── I/O helpers (testable with TempDir) ────────────────────────────────

/// Read the marker file. Returns `None` if the file doesn't exist or
/// can't be read.
fn read_marker(data_dir: &Path) -> Option<String> {
    std::fs::read_to_string(marker_path(data_dir)).ok()
}

/// Write the marker file with the current app version.
fn write_marker(data_dir: &Path, version: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(marker_path(data_dir), version)
}

// ── Public API ─────────────────────────────────────────────────────────

/// Ensure the CLI shim is installed. On first launch (or version upgrade),
/// spawns the bundled `busytok cli install` subprocess. Best-effort:
/// failures are logged but never block app startup.
pub fn ensure_cli_installed(data_dir: &Path, current_version: &str) {
    let marker = read_marker(data_dir);
    if !should_install(marker.as_deref(), current_version) {
        tracing::debug!(
            event_code = "cli_auto_install.already_installed",
            version = current_version,
            "CLI shim already auto-installed for this version"
        );
        return;
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            tracing::warn!(
                event_code = "cli_auto_install.current_exe_failed",
                error = %e,
                "cannot resolve current_exe; skipping CLI auto-install"
            );
            return;
        }
    };

    let bundle = match crate::find_enclosing_app_bundle(&exe) {
        Some(bundle) => bundle,
        None => {
            tracing::info!(
                event_code = "cli_auto_install.no_bundle",
                exe = %exe.display(),
                "not running inside a .app bundle; skipping CLI auto-install (likely dev build)"
            );
            return;
        }
    };

    let cli_binary = resolve_bundled_cli_from_bundle(&bundle);
    if !cli_binary.is_file() {
        tracing::info!(
            event_code = "cli_auto_install.no_bundled_cli",
            cli_binary = %cli_binary.display(),
            "bundled CLI binary not found; skipping auto-install"
        );
        return;
    }

    let bundle_str = bundle.to_string_lossy().to_string();
    tracing::info!(
        event_code = "cli_auto_install.starting",
        cli_binary = %cli_binary.display(),
        app_bundle = %bundle.display(),
        version = current_version,
        "auto-installing CLI shim"
    );

    let result = Command::new(&cli_binary)
        .arg("cli")
        .arg("install")
        .arg("--app-bundle-path")
        .arg(&bundle_str)
        .output();

    match result {
        Ok(output) if output.status.success() => {
            if let Err(e) = write_marker(data_dir, current_version) {
                tracing::warn!(
                    event_code = "cli_auto_install.marker_write_failed",
                    error = %e,
                    "CLI shim installed but failed to write marker; will retry on next launch"
                );
            }
            tracing::info!(
                event_code = "cli_auto_install.completed",
                stdout = %String::from_utf8_lossy(&output.stdout).trim(),
                "CLI shim auto-installed successfully"
            );
        }
        Ok(output) => {
            tracing::warn!(
                event_code = "cli_auto_install.install_failed",
                cli_binary = %cli_binary.display(),
                exit_code = ?output.status.code(),
                stderr = %String::from_utf8_lossy(&output.stderr),
                "CLI shim auto-install subprocess exited non-zero"
            );
        }
        Err(e) => {
            tracing::warn!(
                event_code = "cli_auto_install.spawn_failed",
                cli_binary = %cli_binary.display(),
                error = %e,
                "failed to spawn bundled CLI for auto-install"
            );
        }
    }
}

/// Uninstall the CLI shim and remove the marker. Called from
/// `--uninstall-self`. Best-effort: failures are logged but never block
/// uninstall.
pub fn uninstall_cli(data_dir: &Path) {
    // Remove the marker first so a concurrent GUI launch doesn't see a
    // stale marker while the shim is being removed.
    let marker = marker_path(data_dir);
    if marker.exists() {
        let _ = std::fs::remove_file(&marker);
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            tracing::warn!(
                event_code = "cli_auto_uninstall.current_exe_failed",
                error = %e,
                "cannot resolve current_exe; skipping CLI auto-uninstall"
            );
            return;
        }
    };

    let bundle = match crate::find_enclosing_app_bundle(&exe) {
        Some(bundle) => bundle,
        None => {
            tracing::info!(
                event_code = "cli_auto_uninstall.no_bundle",
                exe = %exe.display(),
                "not running inside a .app bundle; skipping CLI auto-uninstall (likely dev build)"
            );
            return;
        }
    };

    let cli_binary = resolve_bundled_cli_from_bundle(&bundle);
    if !cli_binary.is_file() {
        tracing::info!(
            event_code = "cli_auto_uninstall.no_bundled_cli",
            cli_binary = %cli_binary.display(),
            "bundled CLI binary not found; skipping auto-uninstall"
        );
        return;
    }

    tracing::info!(
        event_code = "cli_auto_uninstall.starting",
        cli_binary = %cli_binary.display(),
        "auto-uninstalling CLI shim"
    );

    let result = Command::new(&cli_binary)
        .arg("cli")
        .arg("uninstall")
        .output();

    match result {
        Ok(output) if output.status.success() => {
            tracing::info!(
                event_code = "cli_auto_uninstall.completed",
                "CLI shim auto-uninstalled successfully"
            );
        }
        Ok(output) => {
            tracing::warn!(
                event_code = "cli_auto_uninstall.uninstall_failed",
                cli_binary = %cli_binary.display(),
                exit_code = ?output.status.code(),
                stderr = %String::from_utf8_lossy(&output.stderr),
                "CLI shim auto-uninstall subprocess exited non-zero"
            );
        }
        Err(e) => {
            tracing::warn!(
                event_code = "cli_auto_uninstall.spawn_failed",
                cli_binary = %cli_binary.display(),
                error = %e,
                "failed to spawn bundled CLI for auto-uninstall"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── find_enclosing_app_bundle (shared helper in lib.rs) ───────────

    #[test]
    fn resolve_app_bundle_finds_app_ancestor() {
        let tmp = TempDir::new().unwrap();
        let bundle = tmp.path().join("Busytok.app");
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let exe = macos.join("busytok-gui");
        std::fs::write(&exe, "fake").unwrap();

        let result = crate::find_enclosing_app_bundle(&exe).unwrap();
        assert_eq!(result, bundle);
    }

    #[test]
    fn resolve_app_bundle_returns_none_for_dev_binary() {
        let tmp = TempDir::new().unwrap();
        let exe = tmp.path().join("busytok-gui");
        std::fs::write(&exe, "fake").unwrap();

        assert!(crate::find_enclosing_app_bundle(&exe).is_none());
    }

    #[test]
    fn resolve_app_bundle_finds_nested_app_with_subdir_name() {
        let tmp = TempDir::new().unwrap();
        // Bundle name has spaces (common on macOS).
        let bundle = tmp.path().join("My App.app");
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let exe = macos.join("busytok-gui");
        std::fs::write(&exe, "fake").unwrap();

        let result = crate::find_enclosing_app_bundle(&exe).unwrap();
        assert_eq!(result, bundle);
    }

    // ── resolve_bundled_cli_from_bundle ────────────────────────────────

    #[test]
    fn resolve_bundled_cli_returns_correct_path() {
        let bundle = PathBuf::from("/Applications/Busytok.app");
        let cli = resolve_bundled_cli_from_bundle(&bundle);
        assert_eq!(
            cli,
            PathBuf::from("/Applications/Busytok.app/Contents/MacOS/busytok")
        );
    }

    // ── marker_path ─────────────────────────────────────────────────────

    #[test]
    fn marker_path_joins_data_dir() {
        let path = marker_path(Path::new("/data"));
        assert_eq!(path, PathBuf::from("/data/cli-auto-installed"));
    }

    // ── should_install ─────────────────────────────────────────────────

    #[test]
    fn should_install_returns_true_when_no_marker() {
        assert!(should_install(None, "1.0.0"));
    }

    #[test]
    fn should_install_returns_false_when_version_matches() {
        assert!(!should_install(Some("1.0.0"), "1.0.0"));
    }

    #[test]
    fn should_install_returns_true_when_version_differs() {
        assert!(should_install(Some("0.9.0"), "1.0.0"));
    }

    #[test]
    fn should_install_handles_whitespace_in_marker() {
        assert!(!should_install(Some("1.0.0\n"), "1.0.0"));
    }

    // ── read_marker / write_marker ─────────────────────────────────────

    #[test]
    fn write_and_read_marker_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");

        write_marker(&data_dir, "1.2.3").unwrap();
        assert_eq!(read_marker(&data_dir).as_deref(), Some("1.2.3"));
    }

    #[test]
    fn read_marker_returns_none_when_file_missing() {
        let tmp = TempDir::new().unwrap();
        assert!(read_marker(tmp.path()).is_none());
    }

    #[test]
    fn write_marker_creates_data_dir_if_missing() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().join("nested/dir/data");
        write_marker(&data_dir, "1.0.0").unwrap();
        assert!(marker_path(&data_dir).exists());
    }

    #[test]
    fn write_marker_overwrites_previous_version() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");

        write_marker(&data_dir, "1.0.0").unwrap();
        write_marker(&data_dir, "2.0.0").unwrap();
        assert_eq!(read_marker(&data_dir).as_deref(), Some("2.0.0"));
    }

    #[test]
    fn read_marker_returns_none_when_file_is_not_valid_utf8() {
        // P2-5: A corrupted marker file (invalid UTF-8) must be treated as
        // "no marker" so the next launch re-installs rather than skipping.
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        // Write invalid UTF-8 bytes (0xFF 0xFE is a BOM-like prefix that is
        // not valid UTF-8 on its own).
        std::fs::write(marker_path(&data_dir), b"\xff\xfe\x00invalid").unwrap();

        assert!(read_marker(&data_dir).is_none());
        // should_install should treat corrupted marker as "install needed".
        assert!(should_install(read_marker(&data_dir).as_deref(), "1.0.0"));
    }

    // ── ensure_cli_installed (integration-level) ───────────────────────

    #[test]
    fn ensure_cli_installed_skips_when_marker_matches_version() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");

        // Write a marker with the current version.
        write_marker(&data_dir, "0.0.10").unwrap();

        // ensure_cli_installed should skip — no bundle resolution, no subprocess.
        ensure_cli_installed(&data_dir, "0.0.10");

        // Marker should be unchanged.
        assert_eq!(read_marker(&data_dir).as_deref(), Some("0.0.10"));
    }

    #[test]
    fn ensure_cli_installed_skips_in_dev_build_without_writing_marker() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");

        // No marker — would try to install. But current_exe() in the test
        // binary doesn't point inside a .app, so it skips.
        ensure_cli_installed(&data_dir, "0.0.10");

        // Marker should NOT be written (install was skipped).
        assert!(!marker_path(&data_dir).exists());
    }

    // ── uninstall_cli (integration-level) ──────────────────────────────

    #[test]
    fn uninstall_cli_removes_marker_and_skips_subprocess_in_dev_build() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");

        // Write a marker.
        write_marker(&data_dir, "0.0.10").unwrap();
        assert!(marker_path(&data_dir).exists());

        // uninstall_cli should remove the marker, then skip the subprocess
        // (no .app bundle in dev/test).
        uninstall_cli(&data_dir);

        assert!(!marker_path(&data_dir).exists());
    }

    #[test]
    fn uninstall_cli_is_safe_when_no_marker_exists() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");

        // No marker — should not panic or error.
        uninstall_cli(&data_dir);

        assert!(!marker_path(&data_dir).exists());
    }
}
