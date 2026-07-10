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

/// Delimited block markers for shell rc PATH setup. Everything between
/// (and including) these lines is managed by `ShimManager` and removed
/// cleanly on uninstall.
const PATH_BLOCK_BEGIN: &str = "# BEGIN busytok-cli-path (auto-generated, do not edit)";
const PATH_BLOCK_END: &str = "# END busytok-cli-path";

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
    if let Some(path) = saved_bundle_path.filter(|p| p.join("Contents/MacOS/busytok").is_file()) {
        return Ok(path.to_path_buf());
    }
    fallback_roots
        .iter()
        .find(|root| root.join("Busytok.app/Contents/MacOS/busytok").is_file())
        .map(|root| root.join("Busytok.app"))
        .with_context(|| "Open Busytok.app once, then reinstall the CLI shim.".to_string())
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
    /// Also ensures `bin_dir` is on the user's PATH by appending an
    /// idempotent delimited block to shell rc files (`~/.zshrc`,
    /// `~/.bash_profile`) if the directory is not already on PATH.
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

        // Ensure `bin_dir` is on the user's PATH via shell rc files.
        // Best-effort: individual file failures are logged internally and
        // do not fail the install — the shim is already in place.
        self.ensure_path_setup(bin_dir);

        tracing::info!(
            event_code = "shim.installed",
            path = %shim_path.display(),
            bundle = %app_bundle_path.display(),
        );

        eprintln!("Busytok CLI shim installed at {}", shim_path.display());

        Ok(())
    }

    /// Report whether the shim is installed and functional.
    pub fn status(&self, bin_dir: &Path) -> Result<()> {
        let shim_path = bin_dir.join(SHIM_SCRIPT_NAME);

        if !shim_path.exists() {
            anyhow::bail!("CLI shim is not installed. Run `busytok cli install` to install it.");
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

    /// Remove the shim script, its config, and the PATH setup block from
    /// shell rc files.
    pub fn uninstall(&self, bin_dir: &Path) -> Result<()> {
        let shim_path = bin_dir.join(SHIM_SCRIPT_NAME);

        if shim_path.exists() {
            fs::remove_file(&shim_path)
                .with_context(|| format!("failed to remove shim at {}", shim_path.display()))?;
        }

        // Clean up the config directory.
        if self.shim_config_dir.exists() {
            if let Err(e) = fs::remove_dir_all(&self.shim_config_dir) {
                tracing::warn!(
                    event_code = "shim.config_dir_remove_failed",
                    path = %self.shim_config_dir.display(),
                    error = %e,
                    "failed to remove shim config directory"
                );
            }
        }

        // Remove the PATH setup block from shell rc files. Best-effort:
        // individual file failures are logged internally.
        self.remove_path_setup();

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
        let bundle_path_str = escape_bash_double_quoted(&app_bundle_path.display().to_string());

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

    /// Ensure `bin_dir` is on the user's PATH by appending an idempotent
    /// delimited block to shell rc files. If the block already exists in
    /// a file, that file is skipped. If a file doesn't exist, it's created.
    ///
    /// On macOS, targets `~/.zshrc` (default shell) and `~/.bash_profile`.
    /// On other Unix, targets `~/.bashrc` and `~/.profile`.
    ///
    /// Best-effort: individual file failures are logged and skipped so that
    /// one unwritable rc file doesn't prevent the other from being updated.
    #[cfg(unix)]
    fn ensure_path_setup(&self, bin_dir: &Path) {
        let block = format_path_block(bin_dir);
        for rc_file in shell_rc_files() {
            // Read existing contents. NotFound → treat as empty (file will be
            // created). Other errors (invalid UTF-8, permission denied) → skip
            // to avoid overwriting a file we couldn't read.
            let contents = match fs::read_to_string(&rc_file) {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => {
                    tracing::warn!(
                        event_code = "shim.path_setup.read_failed",
                        file = %rc_file.display(),
                        error = %e,
                        "failed to read rc file; skipping PATH setup for this file"
                    );
                    continue;
                }
            };
            if contents.contains(PATH_BLOCK_BEGIN) {
                // Block already present — skip (idempotent).
                continue;
            }
            // Append the block, ensuring a trailing newline before it.
            let mut new_contents = contents;
            if !new_contents.is_empty() && !new_contents.ends_with('\n') {
                new_contents.push('\n');
            }
            new_contents.push_str(&block);
            // Atomically replace the rc file (temp-file + rename) so a crash
            // or disk-full mid-write can't truncate the user's dotfile.
            if let Err(e) = busytok_config::atomic_write(&rc_file, &new_contents) {
                tracing::warn!(
                    event_code = "shim.path_setup.write_failed",
                    file = %rc_file.display(),
                    error = %e,
                    "failed to write PATH block to rc file; skipping"
                );
                continue;
            }
            tracing::info!(
                event_code = "shim.path_setup.added",
                file = %rc_file.display(),
                bin_dir = %bin_dir.display(),
            );
        }
    }

    /// No-op on non-Unix (Windows has no shell rc files).
    #[cfg(not(unix))]
    #[allow(unused_variables)]
    fn ensure_path_setup(&self, bin_dir: &Path) {}

    /// Remove the PATH setup block from all shell rc files.
    /// Removes ALL blocks (even if the bin_dir differs from the install-time
    /// value) to ensure clean teardown.
    ///
    /// Best-effort: individual file failures are logged and skipped so that
    /// one unwritable rc file doesn't prevent the other from being cleaned.
    #[cfg(unix)]
    fn remove_path_setup(&self) {
        for rc_file in shell_rc_files() {
            let contents = match fs::read_to_string(&rc_file) {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    tracing::warn!(
                        event_code = "shim.path_teardown.read_failed",
                        file = %rc_file.display(),
                        error = %e,
                        "failed to read rc file; skipping PATH teardown for this file"
                    );
                    continue;
                }
            };
            if !contents.contains(PATH_BLOCK_BEGIN) {
                continue;
            }
            let new_contents = strip_path_block(&contents);
            // Atomically replace the rc file (temp-file + rename) so a crash
            // or disk-full mid-write can't truncate the user's dotfile.
            if let Err(e) = busytok_config::atomic_write(&rc_file, &new_contents) {
                tracing::warn!(
                    event_code = "shim.path_teardown.write_failed",
                    file = %rc_file.display(),
                    error = %e,
                    "failed to write cleaned PATH block to rc file; skipping"
                );
                continue;
            }
            tracing::info!(
                event_code = "shim.path_setup.removed",
                file = %rc_file.display(),
            );
        }
    }

    /// No-op on non-Unix (Windows has no shell rc files).
    #[cfg(not(unix))]
    fn remove_path_setup(&self) {}
}

/// Escape a path for safe embedding in a bash double-quoted string.
/// In bash double quotes, the characters `"`, `$`, `` ` ``, and `\` are
/// special and must be escaped to prevent command substitution or variable
/// expansion.
fn escape_bash_double_quoted(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
}

/// Format the PATH setup block for a given `bin_dir`.
fn format_path_block(bin_dir: &Path) -> String {
    let dir = escape_bash_double_quoted(&bin_dir.display().to_string());
    format!(
        "{begin}\nexport PATH=\"{dir}:$PATH\"\n{end}\n",
        begin = PATH_BLOCK_BEGIN,
        dir = dir,
        end = PATH_BLOCK_END,
    )
}

/// Strip the PATH setup block (and any surrounding blank lines it introduced)
/// from `contents`. Removes ALL blocks if multiple exist.
///
/// **Safety:** If any BEGIN marker lacks a matching END marker (corrupted
/// or truncated rc file), returns the original content unchanged — never
/// silently discard user content that might follow an unterminated block.
fn strip_path_block(contents: &str) -> String {
    let begin_count = contents.matches(PATH_BLOCK_BEGIN).count();
    let end_count = contents.matches(PATH_BLOCK_END).count();

    // Unbalanced markers → corrupted block. Don't risk data loss.
    if begin_count != end_count {
        tracing::warn!(
            event_code = "shim.path_strip.unbalanced_markers",
            begin_count,
            end_count,
            "rc file has unbalanced PATH block markers; leaving file unchanged"
        );
        return contents.to_string();
    }

    let mut result = String::with_capacity(contents.len());
    let mut in_block = false;
    for line in contents.lines() {
        if line == PATH_BLOCK_BEGIN {
            in_block = true;
            continue;
        }
        if line == PATH_BLOCK_END {
            in_block = false;
            continue;
        }
        if !in_block {
            result.push_str(line);
            result.push('\n');
        }
    }
    // Trim trailing whitespace that may have been left by block removal.
    result.trim_end().to_string() + "\n"
}

/// Return the list of shell rc files to manage for PATH setup.
fn shell_rc_files() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    #[cfg(target_os = "macos")]
    {
        vec![home.join(".zshrc"), home.join(".bash_profile")]
    }
    #[cfg(not(target_os = "macos"))]
    {
        vec![home.join(".bashrc"), home.join(".profile")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    fn make_fake_bundle(root: &Path, name: &str) -> PathBuf {
        let bundle = root.join(name);
        let macos_dir = bundle.join("Contents/MacOS");
        fs::create_dir_all(&macos_dir).unwrap();
        let binary = macos_dir.join("busytok");
        fs::write(&binary, "fake binary").unwrap();
        bundle
    }

    /// RAII guard that temporarily sets `HOME` to `tmp` and restores the
    /// original value on drop. Must be used with `#[serial]` since
    /// `std::env::set_var` is process-global.
    struct HomeGuard {
        original: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn new(tmp: &Path) -> Self {
            let original = std::env::var_os("HOME");
            std::env::set_var("HOME", tmp);
            HomeGuard { original }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(val) => std::env::set_var("HOME", val),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    /// Isolate `HOME` so tests that call `install()`/`uninstall()` (which
    /// internally touch `~/.zshrc` via PATH setup) don't modify the real
    /// user's shell rc files. The returned guard restores `HOME` on drop.
    fn isolate_home(tmp: &Path) -> HomeGuard {
        HomeGuard::new(tmp)
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
    #[serial]
    fn install_creates_executable_shim() {
        let tmp = TempDir::new().unwrap();
        let _guard = isolate_home(tmp.path());
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
    #[serial]
    fn uninstall_removes_shim() {
        let tmp = TempDir::new().unwrap();
        let _guard = isolate_home(tmp.path());
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
    #[serial]
    fn status_reports_installed_shim() {
        let tmp = TempDir::new().unwrap();
        let _guard = isolate_home(tmp.path());
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

    // ── PATH setup/teardown tests ──────────────────────────────────────

    #[test]
    fn format_path_block_contains_markers_and_export() {
        let bin_dir = PathBuf::from("/home/user/.local/bin");
        let block = format_path_block(&bin_dir);
        assert!(block.starts_with(PATH_BLOCK_BEGIN));
        assert!(block.ends_with(&format!("{}\n", PATH_BLOCK_END)));
        assert!(block.contains("export PATH=\"/home/user/.local/bin:$PATH\""));
    }

    #[test]
    fn format_path_block_with_spaces_in_path_quotes_correctly() {
        let bin_dir = PathBuf::from("/Users/My User/.local/bin");
        let block = format_path_block(&bin_dir);
        // The path is embedded as-is; spaces are acceptable inside the
        // double-quoted export assignment.
        assert!(block.contains("export PATH=\"/Users/My User/.local/bin:$PATH\""));
    }

    #[test]
    fn strip_path_block_removes_single_block() {
        let contents = format!(
            "alias foo='bar'\n{}\nexport PATH=\"/x:$PATH\"\n{}\necho hi\n",
            PATH_BLOCK_BEGIN, PATH_BLOCK_END,
        );
        let result = strip_path_block(&contents);
        assert!(!result.contains(PATH_BLOCK_BEGIN));
        assert!(!result.contains(PATH_BLOCK_END));
        assert!(!result.contains("export PATH=\"/x:$PATH\""));
        assert!(result.contains("alias foo='bar'"));
        assert!(result.contains("echo hi"));
    }

    #[test]
    fn strip_path_block_removes_multiple_blocks() {
        let contents = format!(
            "{}\nexport PATH=\"/a:$PATH\"\n{}\nother line\n{}\nexport PATH=\"/b:$PATH\"\n{}\n",
            PATH_BLOCK_BEGIN, PATH_BLOCK_END, PATH_BLOCK_BEGIN, PATH_BLOCK_END,
        );
        let result = strip_path_block(&contents);
        assert!(!result.contains(PATH_BLOCK_BEGIN));
        assert!(!result.contains(PATH_BLOCK_END));
        assert!(!result.contains("export PATH=\"/a:$PATH\""));
        assert!(!result.contains("export PATH=\"/b:$PATH\""));
        assert!(result.contains("other line"));
    }

    #[test]
    fn strip_path_block_preserves_content_without_blocks() {
        let contents = "alias foo='bar'\necho hi\n";
        let result = strip_path_block(&contents);
        assert_eq!(result, "alias foo='bar'\necho hi\n");
    }

    #[test]
    fn strip_path_block_handles_empty_string() {
        let result = strip_path_block("");
        assert_eq!(result, "\n");
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn ensure_path_setup_appends_block_to_existing_rc_file() {
        let tmp = TempDir::new().unwrap();
        let _guard = isolate_home(tmp.path());
        let config_dir = tmp.path().join("config");
        let bin_dir = tmp.path().join("bin");

        // Pre-create the first rc file with some content. Use
        // shell_rc_files()[0] so the test works on both macOS (.zshrc) and
        // Linux (.bashrc).
        let rc_file = shell_rc_files().into_iter().next().unwrap();
        fs::write(&rc_file, "alias ls='ls --color'\n").unwrap();

        let manager = ShimManager::new(&config_dir);
        manager
            .install(&bin_dir, &make_fake_bundle(tmp.path(), "Busytok.app"))
            .unwrap();

        let contents = fs::read_to_string(&rc_file).unwrap();
        assert!(contents.contains(PATH_BLOCK_BEGIN));
        assert!(contents.contains(PATH_BLOCK_END));
        assert!(contents.contains("export PATH="));
        assert!(contents.contains("alias ls='ls --color'"));
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn ensure_path_setup_creates_rc_file_if_missing() {
        let tmp = TempDir::new().unwrap();
        let _guard = isolate_home(tmp.path());
        let config_dir = tmp.path().join("config");
        let bin_dir = tmp.path().join("bin");

        let rc_file = shell_rc_files().into_iter().next().unwrap();
        assert!(!rc_file.exists());

        let manager = ShimManager::new(&config_dir);
        manager
            .install(&bin_dir, &make_fake_bundle(tmp.path(), "Busytok.app"))
            .unwrap();

        assert!(rc_file.exists());
        let contents = fs::read_to_string(&rc_file).unwrap();
        assert!(contents.contains(PATH_BLOCK_BEGIN));
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn ensure_path_setup_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let _guard = isolate_home(tmp.path());
        let config_dir = tmp.path().join("config");
        let bin_dir = tmp.path().join("bin");

        let rc_file = shell_rc_files().into_iter().next().unwrap();
        fs::write(&rc_file, "existing content\n").unwrap();

        let manager = ShimManager::new(&config_dir);

        // Install twice — the block should appear exactly once.
        manager
            .install(&bin_dir, &make_fake_bundle(tmp.path(), "Busytok.app"))
            .unwrap();
        let after_first = fs::read_to_string(&rc_file).unwrap();
        let count_first = after_first.matches(PATH_BLOCK_BEGIN).count();
        assert_eq!(count_first, 1);

        // Second install — shim needs bundle to exist; reinstall.
        manager
            .install(&bin_dir, &make_fake_bundle(tmp.path(), "Busytok.app"))
            .unwrap();
        let after_second = fs::read_to_string(&rc_file).unwrap();
        let count_second = after_second.matches(PATH_BLOCK_BEGIN).count();
        assert_eq!(count_second, 1, "block should not be duplicated");
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn uninstall_removes_path_block_from_rc_files() {
        let tmp = TempDir::new().unwrap();
        let _guard = isolate_home(tmp.path());
        let config_dir = tmp.path().join("config");
        let bin_dir = tmp.path().join("bin");

        let rc_file = shell_rc_files().into_iter().next().unwrap();
        fs::write(&rc_file, "user content line\n").unwrap();

        let manager = ShimManager::new(&config_dir);
        manager
            .install(&bin_dir, &make_fake_bundle(tmp.path(), "Busytok.app"))
            .unwrap();
        assert!(fs::read_to_string(&rc_file)
            .unwrap()
            .contains(PATH_BLOCK_BEGIN));

        manager.uninstall(&bin_dir).unwrap();

        let contents = fs::read_to_string(&rc_file).unwrap();
        assert!(!contents.contains(PATH_BLOCK_BEGIN));
        assert!(!contents.contains(PATH_BLOCK_END));
        assert!(contents.contains("user content line"));
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn uninstall_cleans_up_when_rc_file_has_only_the_block() {
        let tmp = TempDir::new().unwrap();
        let _guard = isolate_home(tmp.path());
        let config_dir = tmp.path().join("config");
        let bin_dir = tmp.path().join("bin");

        let rc_file = shell_rc_files().into_iter().next().unwrap();
        // Start with only the block.
        fs::write(&rc_file, &format_path_block(&bin_dir)).unwrap();

        let manager = ShimManager::new(&config_dir);
        manager.uninstall(&bin_dir).unwrap();

        let contents = fs::read_to_string(&rc_file).unwrap();
        assert!(!contents.contains(PATH_BLOCK_BEGIN));
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn remove_path_setup_is_safe_when_no_block_exists() {
        let tmp = TempDir::new().unwrap();
        let _guard = isolate_home(tmp.path());
        let config_dir = tmp.path().join("config");
        let bin_dir = tmp.path().join("bin");

        let rc_file = shell_rc_files().into_iter().next().unwrap();
        fs::write(&rc_file, "user content\n").unwrap();

        let manager = ShimManager::new(&config_dir);
        // Uninstall without prior install — should not corrupt the rc file.
        manager.uninstall(&bin_dir).unwrap();

        let contents = fs::read_to_string(&rc_file).unwrap();
        assert_eq!(contents, "user content\n");
    }

    #[test]
    fn shell_rc_files_returns_macos_paths_on_macos() {
        // This test verifies the platform-correct files are returned.
        // On macOS (where CI runs), it checks for .zshrc + .bash_profile.
        let files = shell_rc_files();
        #[cfg(target_os = "macos")]
        {
            assert!(files.iter().any(|p| p.ends_with(".zshrc")));
            assert!(files.iter().any(|p| p.ends_with(".bash_profile")));
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert!(files.iter().any(|p| p.ends_with(".bashrc")));
            assert!(files.iter().any(|p| p.ends_with(".profile")));
        }
    }

    // ── P1-1 regression: strip_path_block with unbalanced markers ──────

    #[test]
    fn strip_path_block_preserves_content_when_end_marker_is_missing() {
        // BEGIN present, END missing — must NOT discard content after BEGIN.
        let contents = format!(
            "line before\n{}\nexport PATH=\"/x:$PATH\"\nuser content after\n",
            PATH_BLOCK_BEGIN,
        );
        let result = strip_path_block(&contents);
        // Should return the original content unchanged (no data loss).
        assert_eq!(result, contents);
    }

    #[test]
    fn strip_path_block_preserves_content_when_begin_marker_is_missing() {
        // END present, BEGIN missing — also unbalanced.
        let contents = format!("line before\n{}\nuser content after\n", PATH_BLOCK_END,);
        let result = strip_path_block(&contents);
        assert_eq!(result, contents);
    }

    // ── P2-3: rc file without trailing newline ──────────────────────────

    #[cfg(unix)]
    #[test]
    #[serial]
    fn ensure_path_setup_handles_rc_file_without_trailing_newline() {
        let tmp = TempDir::new().unwrap();
        let _guard = isolate_home(tmp.path());
        let config_dir = tmp.path().join("config");
        let bin_dir = tmp.path().join("bin");

        // Pre-create rc file with content that has NO trailing newline.
        let rc_file = shell_rc_files().into_iter().next().unwrap();
        fs::write(&rc_file, "alias ls='ls --color'").unwrap(); // no \n

        let manager = ShimManager::new(&config_dir);
        manager
            .install(&bin_dir, &make_fake_bundle(tmp.path(), "Busytok.app"))
            .unwrap();

        let contents = fs::read_to_string(&rc_file).unwrap();
        // The original content and the block must be separated by a newline.
        assert!(
            contents.contains("alias ls='ls --color'\n"),
            "original content must have a trailing newline before the block"
        );
        assert!(contents.contains(PATH_BLOCK_BEGIN));
        assert!(contents.contains(PATH_BLOCK_END));
    }

    // ── P2-4: second rc file is also written ───────────────────────────

    #[cfg(unix)]
    #[test]
    #[serial]
    fn ensure_path_setup_writes_second_rc_file() {
        let tmp = TempDir::new().unwrap();
        let _guard = isolate_home(tmp.path());
        let config_dir = tmp.path().join("config");
        let bin_dir = tmp.path().join("bin");

        let files = shell_rc_files();
        let second = files.get(1).unwrap();
        assert!(!second.exists());

        let manager = ShimManager::new(&config_dir);
        manager
            .install(&bin_dir, &make_fake_bundle(tmp.path(), "Busytok.app"))
            .unwrap();

        let contents = fs::read_to_string(second).unwrap();
        assert!(contents.contains(PATH_BLOCK_BEGIN));
        assert!(contents.contains(PATH_BLOCK_END));
    }

    // ── P2-6: partial failure in ensure_path_setup ─────────────────────

    #[cfg(unix)]
    #[test]
    #[serial]
    fn install_succeeds_when_one_rc_file_is_immutable() {
        let tmp = TempDir::new().unwrap();
        let _guard = isolate_home(tmp.path());
        let config_dir = tmp.path().join("config");
        let bin_dir = tmp.path().join("bin");

        // Pre-create both rc files with content.
        let files = shell_rc_files();
        let mutable_file = files.first().unwrap();
        let immutable_file = files.get(1).unwrap();
        fs::write(mutable_file, "existing content\n").unwrap();
        fs::write(immutable_file, "existing content\n").unwrap();

        // Make the second rc file immutable via `chflags uchg` (macOS user
        // immutable flag). This blocks rename(2) with EPERM — unlike
        // chmod 0444 which only blocks open(O_WRONLY) but NOT rename.
        // On non-macOS, skip the immutability assertion (no portable
        // equivalent without root).
        #[cfg(target_os = "macos")]
        {
            use std::process::Command;
            let chflags = Command::new("chflags")
                .arg("uchg")
                .arg(immutable_file)
                .status();
            // chflags may fail if not file owner; skip assertion in that case.
            if !matches!(chflags, Ok(s) if s.success()) {
                eprintln!("chflags uchg failed; skipping immutability test");
                return;
            }
        }

        let manager = ShimManager::new(&config_dir);
        // install() should succeed even though the second rc file is immutable.
        manager
            .install(&bin_dir, &make_fake_bundle(tmp.path(), "Busytok.app"))
            .unwrap();

        // The mutable rc file should have the PATH block.
        let mutable_contents = fs::read_to_string(mutable_file).unwrap();
        assert!(mutable_contents.contains(PATH_BLOCK_BEGIN));

        // The immutable rc file: verify it was NOT modified.
        #[cfg(target_os = "macos")]
        {
            let immutable_contents = fs::read_to_string(immutable_file).unwrap();
            assert_eq!(
                immutable_contents, "existing content\n",
                "immutable rc file should be unchanged"
            );
        }

        // Remove the immutable flag so TempDir cleanup works.
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("chflags")
                .arg("nouchg")
                .arg(immutable_file)
                .status();
        }
    }

    // ── Shim script content tests (IMPORTANT-4) ─────────────────────────

    #[test]
    fn generate_shim_script_contains_required_elements() {
        let tmp = TempDir::new().unwrap();
        let config_dir = tmp.path().join("config");
        let manager = ShimManager::new(&config_dir);
        let bundle = PathBuf::from("/Applications/Busytok.app");

        let script = manager.generate_shim_script(&bundle).unwrap();

        // Shebang and safety directives.
        assert!(script.starts_with("#!/usr/bin/env bash"));
        assert!(script.contains("set -euo pipefail"));

        // Bundle path is embedded correctly.
        assert!(script.contains("/Applications/Busytok.app/Contents/MacOS/busytok"));

        // Fallback search roots are present.
        assert!(script.contains("/Applications"));
        assert!(script.contains("~/Applications"));
        assert!(script.contains("$HOME/Applications"));

        // Exec delegation line is correct.
        assert!(script.contains("exec \"$BUNDLE_PATH/Contents/MacOS/busytok\" \"$@\""));
    }

    #[test]
    fn generate_shim_script_escapes_dollar_in_bundle_path() {
        let tmp = TempDir::new().unwrap();
        let config_dir = tmp.path().join("config");
        let manager = ShimManager::new(&config_dir);
        // Path with a dollar sign that could trigger variable expansion.
        let bundle = PathBuf::from("/Users/test/$evil.app");

        let script = manager.generate_shim_script(&bundle).unwrap();

        // The dollar sign must be escaped to prevent variable expansion.
        assert!(script.contains("\\$evil"));
        // The unescaped path must NOT appear in a double-quoted context.
        assert!(!script.contains("\"/$evil"));
    }

    #[test]
    fn format_path_block_escapes_dollar_in_bin_dir() {
        let bin_dir = PathBuf::from("/Users/test/$evil/bin");
        let block = format_path_block(&bin_dir);
        // The dollar sign must be escaped.
        assert!(block.contains("\\$evil"));
        // The unescaped path must NOT appear.
        assert!(!block.contains("/$evil/"));
    }

    #[test]
    fn escape_bash_double_quoted_escapes_all_special_chars() {
        let escaped = escape_bash_double_quoted("a\"$`\\b");
        assert_eq!(escaped, "a\\\"\\$\\`\\\\b");
    }
}
