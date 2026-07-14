#[cfg(windows)]
use anyhow::Context;
use std::path::PathBuf;

const APP_NAME: &str = "busytok";
const DB_NAME: &str = "busytok.db";
const SOCKET_NAME: &str = "busytok.sock";
const PRICE_CATALOG_NAME: &str = "price-catalog.json";
const LOGS_DIR_NAME: &str = "logs";
pub const ARTIFACTS_DIR_NAME: &str = "artifacts";

/// Resolves local filesystem paths for the Busytok service.
///
/// All paths use the "busytok" name and follow XDG Base Directory conventions
/// on Linux, with sensible fallbacks on macOS.
///
/// This struct only resolves paths -- it does NOT contain any proxy, auth,
/// or network configuration.
#[derive(Debug, Clone)]
pub struct BusytokPaths {
    data_dir: PathBuf,
    config_dir: PathBuf,
    runtime_dir: PathBuf,
}

impl BusytokPaths {
    /// Create a new `BusytokPaths` using environment overrides or the
    /// system's standard directories.
    ///
    /// Precedence (per directory):
    /// 1. `BUSYTOK_DATA_DIR` / `BUSYTOK_CONFIG_DIR` / `BUSYTOK_RUNTIME_DIR`
    ///    env vars (if set, used as-is — the caller controls the full path,
    ///    including the app name suffix if desired).
    /// 2. Platform default via `dirs` crate:
    ///    - `data_dir`: `$XDG_DATA_HOME/busytok` (Linux) or
    ///      `~/Library/Application Support/busytok` (macOS)
    ///    - `config_dir`: `$XDG_CONFIG_HOME/busytok` or `~/.config/busytok`
    ///    - `runtime_dir`: `$XDG_RUNTIME_DIR/busytok` or fallback to data dir
    ///
    /// **Bug 2 fix:** on macOS, `dirs` ignores `XDG_DATA_HOME` etc. and
    /// returns `~/Library/Application Support`, so source-built services
    /// would write to the installed app's data directory. The explicit
    /// `BUSYTOK_*` env vars override this on all platforms, enabling
    /// reliable test isolation without relying on XDG compliance.
    pub fn new() -> Self {
        Self::from_env()
    }

    /// Resolve paths from environment variables, falling back to platform
    /// defaults via `dirs`. Each `BUSYTOK_*` variable, when set, is used
    /// as the complete directory path (no app-name suffix appended).
    pub fn from_env() -> Self {
        Self::from_env_values(
            std::env::var("BUSYTOK_DATA_DIR").ok(),
            std::env::var("BUSYTOK_CONFIG_DIR").ok(),
            std::env::var("BUSYTOK_RUNTIME_DIR").ok(),
        )
    }

    /// Resolve paths from explicit environment-like values. Keeping the
    /// resolution pure lets tests verify override/fallback semantics without
    /// mutating process-global environment variables (which would race other
    /// tests running in parallel).
    fn from_env_values(
        data_override: Option<String>,
        config_override: Option<String>,
        runtime_override: Option<String>,
    ) -> Self {
        let data_dir = data_override
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::data_dir()
                    .unwrap_or_else(|| PathBuf::from(".local/share"))
                    .join(APP_NAME)
            });

        let config_dir = config_override
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::config_dir()
                    .unwrap_or_else(|| PathBuf::from(".config"))
                    .join(APP_NAME)
            });

        let runtime_dir = runtime_override
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::runtime_dir()
                    .unwrap_or_else(|| data_dir.clone())
                    .join(APP_NAME)
            });

        Self {
            data_dir,
            config_dir,
            runtime_dir,
        }
    }

    /// Create a `BusytokPaths` for testing, using the given root directory
    /// for all three base directories (data, config, runtime).
    ///
    /// This avoids touching the real user's config/data directories.
    pub fn for_test(root: &std::path::Path) -> Self {
        Self {
            data_dir: root.join("data").join(APP_NAME),
            config_dir: root.join("config").join(APP_NAME),
            runtime_dir: root.join("runtime").join(APP_NAME),
        }
    }

    /// Returns the data directory: `$XDG_DATA_HOME/busytok` or `~/.local/share/busytok`.
    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }

    /// Returns the config directory: `$XDG_CONFIG_HOME/busytok` or `~/.config/busytok`.
    pub fn config_dir(&self) -> &PathBuf {
        &self.config_dir
    }

    /// Returns the runtime directory: `$XDG_RUNTIME_DIR/busytok` or fallback.
    pub fn runtime_dir(&self) -> &PathBuf {
        &self.runtime_dir
    }

    /// Returns the database path: `{data_dir}/busytok.db`.
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join(DB_NAME)
    }

    /// Returns the control socket path: `{runtime_dir}/busytok.sock`.
    ///
    /// Deprecated on Unix+Windows cross-platform paths; prefer
    /// [`BusytokPaths::control_endpoint`] which returns a string endpoint
    /// that is valid on both Unix sockets and Windows named pipes.
    #[deprecated(note = "use control_endpoint()")]
    pub fn control_socket(&self) -> PathBuf {
        self.runtime_dir.join(SOCKET_NAME)
    }

    /// Platform-agnostic IPC endpoint string.
    ///
    /// Unix: filesystem socket path (`{runtime_dir}/busytok.sock`).
    /// Windows: named-pipe name with user SID (`\\.\pipe\busytok-{user-sid}`)
    ///   to isolate per-user pipes on multi-user machines.
    ///
    /// Returns `Result<String>` because Windows SID resolution can fail; Unix
    /// always succeeds.
    pub fn control_endpoint(&self) -> anyhow::Result<String> {
        #[cfg(unix)]
        {
            Ok(self.runtime_dir.join(SOCKET_NAME).display().to_string())
        }
        #[cfg(windows)]
        {
            let sid = crate::platform::current_user_sid_string()
                .context("failed to resolve current user SID for control endpoint")?;
            Ok(format!(r"\\.\pipe\busytok-{sid}"))
        }
        #[cfg(not(any(unix, windows)))]
        {
            anyhow::bail!("unsupported platform for control_endpoint");
        }
    }

    /// Returns the log directory: `{data_dir}/logs`.
    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join(LOGS_DIR_NAME)
    }

    /// Returns the price catalog path: `{data_dir}/price-catalog.json`.
    pub fn price_catalog_path(&self) -> PathBuf {
        self.data_dir.join(PRICE_CATALOG_NAME)
    }

    /// Root for large subagent artifacts (logs, patches, traces).
    /// Full layout: `<artifacts_dir>/<subagent_id>/<task_id>/...`.
    pub fn artifacts_dir(&self) -> PathBuf {
        self.data_dir.join(ARTIFACTS_DIR_NAME)
    }

    /// Ensures all directories exist by creating them if needed.
    ///
    /// Creates `data_dir`, `config_dir`, `runtime_dir`, `log_dir`, and `artifacts_dir`.
    pub fn ensure_dirs_exist(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.config_dir)?;
        std::fs::create_dir_all(&self.runtime_dir)?;
        std::fs::create_dir_all(self.log_dir())?;
        std::fs::create_dir_all(self.artifacts_dir())?;
        Ok(())
    }

    /// Resolve the sidecar runtime directory.
    ///
    /// Precedence (spec §5.1):
    /// 1. `runtime_dir` (from `SubagentPiSidecarConfig`) — packaged app and
    ///    service-only mode both set this via settings.toml or a Tauri-injected
    ///    env var. This is the "runtime locator" the spec calls for.
    /// 2. Dev fallback: `apps/pi-sidecar/dist/` resolved via `CARGO_MANIFEST_DIR`
    ///    of the `busytok-config` crate. Dev-only; brittle if the binary is
    ///    relocated. Packaged builds MUST set `runtime_dir`.
    ///
    /// NO hardcoded `data_dir/pi-sidecar` fallback — that path is neither the
    /// Tauri bundle location nor a service-only config entry, and silently
    /// falling back to it would mask a misconfigured packaged build.
    pub fn sidecar_runtime_dir(&self, runtime_dir: Option<&str>) -> std::path::PathBuf {
        if let Some(dir) = runtime_dir {
            return std::path::PathBuf::from(dir);
        }
        // Dev fallback only.
        let dev = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/pi-sidecar/dist");
        dev
    }

    /// Path to the sidecar JS bundle. Takes the runtime_dir locator.
    pub fn sidecar_bundle_path(&self, runtime_dir: Option<&str>) -> std::path::PathBuf {
        self.sidecar_runtime_dir(runtime_dir)
            .join("pi-sidecar.bundle.js")
    }

    /// Path to the sidecar bundle manifest (spec §5.1 line 549).
    /// `manifest.json` sits alongside `pi-sidecar.bundle.js` in the runtime dir.
    /// Doctor check verifies this file is readable + valid JSON.
    pub fn sidecar_manifest_path(&self, runtime_dir: Option<&str>) -> std::path::PathBuf {
        self.sidecar_runtime_dir(runtime_dir).join("manifest.json")
    }

    /// Path to the bundled Node binary for the current arch. Returns the path
    /// even if it doesn't exist — the caller (`resolve_sidecar_config`) decides
    /// whether to use it (mode `bundled`) or fall back to system `node` (mode
    /// `system`). NO silent fallback here.
    pub fn sidecar_bundled_node_path(&self, runtime_dir: Option<&str>) -> std::path::PathBuf {
        self.sidecar_runtime_dir(runtime_dir)
            .join("node")
            .join(std::env::consts::ARCH)
            .join("node")
    }
}

impl Default for BusytokPaths {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_contains_busytok() {
        let paths = BusytokPaths::new();
        let s = paths.data_dir().to_string_lossy();
        assert!(
            s.contains("busytok"),
            "data_dir should contain 'busytok': {s}"
        );
        assert!(
            !s.contains("autoken"),
            "data_dir must not contain 'autoken': {s}"
        );
    }

    #[test]
    fn config_dir_contains_busytok() {
        let paths = BusytokPaths::new();
        let s = paths.config_dir().to_string_lossy();
        assert!(
            s.contains("busytok"),
            "config_dir should contain 'busytok': {s}"
        );
        assert!(
            !s.contains("autoken"),
            "config_dir must not contain 'autoken': {s}"
        );
    }

    #[test]
    fn db_path_is_under_data_dir() {
        let paths = BusytokPaths::new();
        let db = paths.db_path();
        assert!(db.to_string_lossy().contains("busytok"));
        assert!(db.to_string_lossy().ends_with(".db"));
        assert_eq!(db.parent(), Some(paths.data_dir()).map(|v| &**v));
    }

    #[test]
    #[allow(deprecated)]
    fn control_socket_is_under_runtime_dir() {
        let paths = BusytokPaths::new();
        let sock = paths.control_socket();
        assert!(sock.to_string_lossy().contains("busytok"));
        assert!(sock.to_string_lossy().ends_with(".sock"));
        assert_eq!(sock.parent(), Some(paths.runtime_dir()).map(|v| &**v));
    }

    #[test]
    fn log_dir_is_under_data_dir() {
        let paths = BusytokPaths::new();
        let log = paths.log_dir();
        assert!(log.to_string_lossy().contains("busytok"));
        assert!(log.to_string_lossy().ends_with("logs"));
        assert_eq!(log.parent(), Some(paths.data_dir()).map(|v| &**v));
    }

    #[test]
    fn price_catalog_is_under_data_dir() {
        let paths = BusytokPaths::new();
        let catalog = paths.price_catalog_path();
        assert!(catalog.to_string_lossy().contains("busytok"));
        assert!(catalog.to_string_lossy().ends_with("price-catalog.json"));
        assert_eq!(catalog.parent(), Some(paths.data_dir()).map(|v| &**v));
    }

    #[test]
    fn default_impl_matches_new() {
        let from_new = BusytokPaths::new();
        let from_default = BusytokPaths::default();
        assert_eq!(from_new.data_dir(), from_default.data_dir());
        assert_eq!(from_new.config_dir(), from_default.config_dir());
        assert_eq!(from_new.runtime_dir(), from_default.runtime_dir());
    }

    // --- Bug 2 fix: BUSYTOK_* env var overrides ---
    // Combined into a single test to avoid parallel env-var races.

    #[test]
    fn from_env_uses_busytk_overrides_and_falls_back() {
        // Test 1: all three overrides set.
        let data = std::env::temp_dir().join("busytok-test-all-data");
        let config = std::env::temp_dir().join("busytok-test-all-config");
        let runtime = std::env::temp_dir().join("busytok-test-all-runtime");
        let paths = BusytokPaths::from_env_values(
            Some(data.to_string_lossy().into_owned()),
            Some(config.to_string_lossy().into_owned()),
            Some(runtime.to_string_lossy().into_owned()),
        );
        assert_eq!(paths.data_dir(), &data, "data_dir override");
        assert_eq!(paths.config_dir(), &config, "config_dir override");
        assert_eq!(paths.runtime_dir(), &runtime, "runtime_dir override");
        assert_eq!(paths.db_path(), data.join("busytok.db"));
        assert_eq!(paths.log_dir(), data.join("logs"));

        // Test 2: override is used as-is (no app-name suffix appended).
        assert!(
            !paths
                .data_dir()
                .to_string_lossy()
                .ends_with("busytok/busytok-test-all-data"),
            "override should be used as-is, not appended with app name"
        );

        // Test 3: empty/unset values fall back to dirs crate.
        let paths = BusytokPaths::from_env_values(None, None, None);
        let s = paths.data_dir().to_string_lossy();
        assert!(
            s.contains("busytok"),
            "fallback data_dir should contain 'busytok': {s}"
        );
    }
}
