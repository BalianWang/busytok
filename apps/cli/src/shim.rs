//! CLI shim -- resolve+bind the `busytok` shell shim so `busytok` on PATH
//! invokes the bundled CLI inside `Busytok.app`.
//!
//! The shim is a small bash script installed at `{bin_dir}/busytok` that
//! resolves the app bundle path (saved at install time, verified on each
//! invocation) and delegates to `Busytok.app/Contents/MacOS/busytok`.
//!
//! ## Relocation support
//!
//! If the user moves `Busytok.app`, the saved bundle path becomes stale.
//! The shim script falls back to searching standard macOS app locations
//! (`/Applications`, `~/Applications`) so it continues to work after a
//! simple drag-and-drop.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const SHIM_SCRIPT_NAME: &str = "busytok";
const SHIM_CONFIG_DIR: &str = "busytok-shim";
const BUNDLE_PATH_FILE: &str = "app-bundle-path";

/// Resolve the app bundle path for shim installation.
///
/// 1. If a `saved_bundle_path` is provided and the bundle binary exists at
///    `{path}/Contents/MacOS/busytok`, use it.
/// 2. Otherwise, search `fallback_roots` for `Busytok.app/Contents/MacOS/busytok`.
/// 3. If nothing is found, return an error with a user-friendly message.
pub fn resolve_app_bundle_for_shim(
    saved_bundle_path: Option<&Path>,
    fallback_roots: &[PathBuf],
) -> Result<PathBuf> {
    if let Some(path) = saved_bundle_path
        .filter(|p| p.join("Contents/MacOS/busytok").is_file())
    {
        return Ok(path.to_path_buf());
    }
    fallback_roots
        .iter()
        .find(|root| root.join("Busytok.app/Contents/MacOS/busytok").is_file())
        .map(|root| root.join("Busytok.app"))
        .with_context(|| {
            "Open Busytok.app once, then reinstall the CLI shim.".to_string()
        })
}

/// Manages the lifecycle of the `busytok` CLI shim script.
///
/// The shim is a small shell script placed in a directory on `PATH` that
/// locates the `Busytok.app` bundle and invokes the bundled CLI binary.
pub struct ShimManager {
    shim_config_dir: PathBuf,
}

impl ShimManager {
    /// Create a new `ShimManager` that stores its config under `config_dir`.
    pub fn new(config_dir: &Path) -> Self {
        Self {
            shim_config_dir: config_dir.join(SHIM_CONFIG_DIR),
        }
    }

    /// Install the shim script at `bin_dir/busytok`.
    ///
    /// `app_bundle_path` is recorded in the shim config directory so the
    /// script can find the bundle even after relocation (with fallback search).
    pub fn install(&self, bin_dir: &Path, app_bundle_path: &Path) -> Result<()> {
        // Record the app bundle path for relocation support.
        self.save_bundle_path(app_bundle_path)?;

        let shim_path = bin_dir.join(SHIM_SCRIPT_NAME);
        let shim_contents = self.generate_shim_script(app_bundle_path)?;

        fs::create_dir_all(bin_dir)
            .with_context(|| format!("failed to create bin directory {}", bin_dir.display()))?;

        fs::write(&shim_path, &shim_contents)
            .with_context(|| format!("failed to write shim script to {}", shim_path.display()))?;

        // Make the shim executable (rwxr-xr-x).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&shim_path)
                .with_context(|| format!("failed to read metadata for {}", shim_path.display()))?
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&shim_path, perms)
                .with_context(|| format!("failed to set permissions on {}", shim_path.display()))?;
        }

        tracing::info!(
            event_code = "shim.installed",
            path = %shim_path.display(),
            bundle = %app_bundle_path.display(),
        );

        eprintln!("Busytok CLI shim installed at {}", shim_path.display());
        eprintln!("Make sure {} is on your PATH.", bin_dir.display());

        Ok(())
    }

    /// Report whether the shim is installed and functional.
    pub fn status(&self, bin_dir: &Path) -> Result<()> {
        let shim_path = bin_dir.join(SHIM_SCRIPT_NAME);

        if !shim_path.exists() {
            anyhow::bail!(
                "CLI shim is not installed. Run `busytok cli install` to install it."
            );
        }

        println!("CLI shim: {}", shim_path.display());

        // Verify the recorded bundle path is valid.
        match self.load_bundle_path() {
            Ok(saved) => {
                let binary = saved.join("Contents/MacOS/busytok");
                if binary.is_file() {
                    println!("App bundle: {} (valid)", saved.display());
                } else {
                    println!(
                        "App bundle: {} (stale — Busytok.app has moved; the shim will search standard locations)",
                        saved.display()
                    );
                }
            }
            Err(_) => {
                println!("App bundle: not recorded (the shim will search standard locations)");
            }
        }

        Ok(())
    }

    /// Remove the shim script and its config.
    pub fn uninstall(&self, bin_dir: &Path) -> Result<()> {
        let shim_path = bin_dir.join(SHIM_SCRIPT_NAME);

        if shim_path.exists() {
            fs::remove_file(&shim_path)
                .with_context(|| format!("failed to remove shim at {}", shim_path.display()))?;
        }

        // Clean up the config directory.
        if self.shim_config_dir.exists() {
            let _ = fs::remove_dir_all(&self.shim_config_dir);
        }

        tracing::info!(
            event_code = "shim.uninstalled",
            path = %shim_path.display(),
        );

        eprintln!("Busytok CLI shim uninstalled.");

        Ok(())
    }

    // ── Internal helpers ────────────────────────────────────────────────

    fn save_bundle_path(&self, app_bundle_path: &Path) -> Result<()> {
        fs::create_dir_all(&self.shim_config_dir).with_context(|| {
            format!(
                "failed to create shim config dir {}",
                self.shim_config_dir.display()
            )
        })?;

        let path_file = self.shim_config_dir.join(BUNDLE_PATH_FILE);
        fs::write(&path_file, app_bundle_path.display().to_string())
            .with_context(|| format!("failed to write bundle path to {}", path_file.display()))?;

        Ok(())
    }

    fn load_bundle_path(&self) -> Result<PathBuf> {
        let path_file = self.shim_config_dir.join(BUNDLE_PATH_FILE);
        let contents = fs::read_to_string(&path_file)
            .with_context(|| format!("failed to read bundle path from {}", path_file.display()))?;
        Ok(PathBuf::from(contents.trim()))
    }

    fn generate_shim_script(&self, app_bundle_path: &Path) -> Result<String> {
        let bundle_path_str = app_bundle_path.display().to_string();

        Ok(format!(
            r#"#!/usr/bin/env bash
# Busytok CLI shim -- generated by `busytok cli install`.
# Do not edit by hand; re-run `busytok cli install` after moving Busytok.app.
set -euo pipefail

BUNDLE_PATH=""

# 1. Try the recorded bundle path.
if [[ -x "{bundle_path}/Contents/MacOS/busytok" ]]; then
    BUNDLE_PATH="{bundle_path}"
fi

# 2. Search standard locations.
if [[ -z "$BUNDLE_PATH" ]]; then
    for root in /Applications ~/Applications "$HOME/Applications"; do
        if [[ -x "$root/Busytok.app/Contents/MacOS/busytok" ]]; then
            BUNDLE_PATH="$root/Busytok.app"
            break
        fi
    done
fi

if [[ -z "$BUNDLE_PATH" ]]; then
    echo "busytok: cannot find Busytok.app. Open Busytok.app once, then reinstall the CLI shim." >&2
    echo "  Run: busytok cli install" >&2
    exit 1
fi

exec "$BUNDLE_PATH/Contents/MacOS/busytok" "$@"
"#,
            bundle_path = bundle_path_str,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_fake_bundle(root: &Path, name: &str) -> PathBuf {
        let bundle = root.join(name);
        let macos_dir = bundle.join("Contents/MacOS");
        fs::create_dir_all(&macos_dir).unwrap();
        let binary = macos_dir.join("busytok");
        fs::write(&binary, "fake binary").unwrap();
        bundle
    }

    #[test]
    fn resolve_uses_saved_path_when_valid() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_fake_bundle(tmp.path(), "Busytok.app");

        let result = resolve_app_bundle_for_shim(Some(&bundle), &[]).unwrap();
        assert_eq!(result, bundle);
    }

    #[test]
    fn resolve_falls_back_to_fallback_roots_when_saved_is_stale() {
        let tmp = TempDir::new().unwrap();
        let fake_root = tmp.path().join("Applications");
        let bundle = make_fake_bundle(&fake_root, "Busytok.app");

        let result = resolve_app_bundle_for_shim(
            Some(&PathBuf::from("/nonexistent/Busytok.app")),
            &[fake_root.clone()],
        )
        .unwrap();
        assert_eq!(result, bundle);
    }

    #[test]
    fn resolve_fails_with_user_friendly_message_when_nothing_found() {
        let result = resolve_app_bundle_for_shim(None, &[]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Open Busytok.app once"));
    }

    #[test]
    fn install_creates_executable_shim() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_fake_bundle(tmp.path(), "Busytok.app");
        let bin_dir = tmp.path().join("bin");
        let config_dir = tmp.path().join("config");

        let manager = ShimManager::new(&config_dir);
        manager.install(&bin_dir, &bundle).unwrap();

        let shim = bin_dir.join("busytok");
        assert!(shim.is_file());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = shim.metadata().unwrap().permissions().mode();
            assert!(mode & 0o111 != 0, "shim should be executable");
        }
    }

    #[test]
    fn uninstall_removes_shim() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_fake_bundle(tmp.path(), "Busytok.app");
        let bin_dir = tmp.path().join("bin");
        let config_dir = tmp.path().join("config");

        let manager = ShimManager::new(&config_dir);
        manager.install(&bin_dir, &bundle).unwrap();
        assert!(bin_dir.join("busytok").is_file());

        manager.uninstall(&bin_dir).unwrap();
        assert!(!bin_dir.join("busytok").exists());
    }

    #[test]
    fn status_reports_installed_shim() {
        let tmp = TempDir::new().unwrap();
        let bundle = make_fake_bundle(tmp.path(), "Busytok.app");
        let bin_dir = tmp.path().join("bin");
        let config_dir = tmp.path().join("config");

        let manager = ShimManager::new(&config_dir);
        manager.install(&bin_dir, &bundle).unwrap();

        // status() should succeed without error.
        manager.status(&bin_dir).unwrap();
    }

    #[test]
    fn status_reports_missing_shim() {
        let tmp = TempDir::new().unwrap();
        let bin_dir = tmp.path().join("nonexistent_bin");
        let config_dir = tmp.path().join("config");

        let manager = ShimManager::new(&config_dir);
        let result = manager.status(&bin_dir);
        assert!(result.is_err());
    }
}
