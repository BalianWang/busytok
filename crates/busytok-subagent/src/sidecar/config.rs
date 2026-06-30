//! Sidecar runtime configuration.
//!
//! `SidecarConfig` is the resolved, ready-to-spawn configuration produced from
//! `SubagentPiSidecarConfig` (settings) + `BusytokPaths` (filesystem locators).
//! `resolve_sidecar_config` is the single entry point that turns settings into
//! a spawnable config — including the explicit `bundled` vs `system` node
//! runtime selection (no silent fallback, spec §10.1/§5.1).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use busytok_config::{BusytokPaths, SubagentPiSidecarConfig};

use crate::sidecar::SidecarError;

/// Resolved sidecar configuration — everything needed to spawn and supervise.
///
/// `Clone` so the WorkerPool (Task 2) can clone the base config produced by
/// `resolve_base_sidecar_config` and override `provider_id` +
/// `api_key_env_name` / `base_url_env_name` per provider before spawning
/// (Phase 3 multi-provider routing). All fields are `Clone`-able.
#[derive(Clone)]
pub struct SidecarConfig {
    pub node_binary: PathBuf,
    pub bundle_path: PathBuf,
    pub env: HashMap<String, String>,
    pub idle_exit_seconds: u64,
    pub health_interval: Duration,
    pub task_timeout: Duration,
    pub max_restart_attempts: u32,
    /// Base delay for exponential backoff on crash-restart (1s → 2s → 4s → 8s).
    pub restart_backoff_base: Duration,
    /// Harness name scopes crash reconciliation (spec §5.4). "pi" for Plan 2;
    /// future harnesses (Claude Code, Codex) set their own.
    pub harness_name: String,
    /// Maximum concurrent hot sessions the sidecar will hold before evicting
    /// the LRU (spec §4.4). Mirrored to the sidecar via the
    /// `BUSYTOK_SIDECAR_MAX_HOT_SESSIONS` env var (spec §8.2).
    pub max_hot_sessions: u32,
    /// Soft RSS limit (MB) — at/above this, log warning + plan graceful
    /// restart (spec §8.3 step 3).
    pub memory_soft_limit_mb: u32,
    /// Hard RSS limit (MB) — at/above this, write `rss_limit_exceeded`
    /// event; existing crash path will restart (spec §8.3 step 5).
    pub memory_hard_limit_mb: u32,
    /// Phase 3: provider this supervisor runs tasks for. Empty in the base
    /// config produced by `resolve_base_sidecar_config`; the WorkerPool
    /// (Task 2) clones the base and sets this per provider before spawning.
    /// Empty string means "unbound base" — never spawn directly with this.
    pub provider_id: String,
    /// Phase 3: name of the env var holding the provider API key. Empty in
    /// the base config; WorkerPool sets it (e.g. `OPENAI_API_KEY`) so the
    /// supervisor can inject the env var at spawn without re-resolving
    /// provider config. Kept on the config for observability — log/metric
    /// labels can reference it without re-reading provider settings.
    pub api_key_env_name: String,
    /// Phase 3: name of the env var holding the provider base URL. Same
    /// lifecycle as `api_key_env_name` — empty in base, set per-provider.
    pub base_url_env_name: String,
}

/// Resolve a base `SidecarConfig` from settings + paths.
///
/// Produces a config with empty `provider_id` / env-name placeholders.
/// The WorkerPool (Task 2) clones this and overrides `provider_id` +
/// `api_key_env_name` / `base_url_env_name` per provider before spawning.
/// Existing callers that don't care about provider binding (Plan 1/2 tests,
/// single-supervisor paths) can use this directly or via
/// `resolve_sidecar_config` (which delegates here).
///
/// Explicit mode selection — NO silent fallback. Spec §10.1/§5.1.
/// `node_runtime = "bundled"` requires the bundled node binary to exist;
/// `node_runtime = "system"` uses `system_node_path` (or PATH `node` if empty).
pub fn resolve_base_sidecar_config(
    settings: &SubagentPiSidecarConfig,
    paths: &BusytokPaths,
) -> Result<SidecarConfig, SidecarError> {
    let runtime_dir = settings.runtime_dir.as_deref();
    // Explicit mode selection — NO silent fallback. Spec §10.1/§5.1.
    let node_binary = match settings.node_runtime.as_str() {
        "system" => {
            if settings.system_node_path.is_empty() {
                PathBuf::from("node") // rely on PATH (explicit system mode)
            } else {
                PathBuf::from(&settings.system_node_path)
            }
        }
        "bundled" => {
            let bundled = paths.sidecar_bundled_node_path(runtime_dir);
            if !bundled.exists() {
                return Err(SidecarError::Spawn(format!(
                    "node_runtime='bundled' but bundled node not found at {}; \
                     set node_runtime='system' or install the bundled runtime",
                    bundled.display()
                )));
            }
            bundled
        }
        other => {
            return Err(SidecarError::Spawn(format!(
                "unknown node_runtime: '{other}' (expected 'bundled' or 'system')"
            )));
        }
    };
    let bundle_path = paths.sidecar_bundle_path(runtime_dir);
    if !bundle_path.exists() {
        return Err(SidecarError::Spawn(format!(
            "sidecar bundle not found at {}",
            bundle_path.display()
        )));
    }
    let mut env = HashMap::new();
    env.insert(
        "BUSYTOK_SIDECAR_MAX_HOT_SESSIONS".to_string(),
        settings.max_hot_sessions.to_string(),
    );
    Ok(SidecarConfig {
        node_binary,
        bundle_path,
        env,
        idle_exit_seconds: settings.idle_exit_seconds,
        // Spec §5.4: health ping every 30s. Fixed in MVP (no config knob).
        health_interval: Duration::from_secs(30),
        task_timeout: Duration::from_secs(settings.task_timeout_seconds),
        // Spec §5.4: max 3 backoff attempts. The rolling 5-min crash window
        // (restart_history + MAX_CRASHES_PER_WINDOW=3) is implemented in
        // supervisor.rs and is independent of this backoff-only counter.
        // restart_attempts resets on successful spawn; restart_history does not.
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: settings.max_hot_sessions,
        memory_soft_limit_mb: settings.memory_soft_limit_mb,
        memory_hard_limit_mb: settings.memory_hard_limit_mb,
        // Phase 3: provider binding is set per-supervisor by the WorkerPool
        // (Task 2). The base config ships unbound — `provider_id` empty,
        // env names empty placeholders. WorkerPool clones this, then sets
        // `provider_id` + the provider-specific env names before spawning.
        provider_id: String::new(),
        api_key_env_name: String::new(),
        base_url_env_name: String::new(),
    })
}

/// Resolve a `SidecarConfig` from settings + paths.
///
/// Backward-compat wrapper: delegates to `resolve_base_sidecar_config`.
/// Existing callers (Plan 1/2 paths that don't yet do per-provider binding)
/// continue to work; Task 2's WorkerPool calls `resolve_base_sidecar_config`
/// directly so it's clear at the call site that the result is unbound.
pub fn resolve_sidecar_config(
    settings: &SubagentPiSidecarConfig,
    paths: &BusytokPaths,
) -> Result<SidecarConfig, SidecarError> {
    resolve_base_sidecar_config(settings, paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Materialize the sidecar bundle file at `<runtime_dir>/pi-sidecar.bundle.js`.
    fn write_bundle(runtime_dir: &std::path::Path) -> PathBuf {
        let bundle = runtime_dir.join("pi-sidecar.bundle.js");
        std::fs::write(&bundle, "// mock bundle\n").unwrap();
        bundle
    }

    /// Build a settings struct configured for `system` mode with a materialized
    /// bundle so `resolve_base_sidecar_config` succeeds without needing the
    /// bundled node binary on disk.
    fn system_settings(tmp: &TempDir) -> SubagentPiSidecarConfig {
        write_bundle(tmp.path());
        SubagentPiSidecarConfig {
            node_runtime: "system".to_string(),
            system_node_path: "bash".to_string(),
            runtime_dir: Some(tmp.path().to_string_lossy().to_string()),
            ..Default::default()
        }
    }

    /// `resolve_base_sidecar_config` produces a base config with empty
    /// `provider_id` and empty env-name placeholders. This is the contract
    /// the WorkerPool (Task 2) relies on: clone → override → spawn.
    #[test]
    fn resolve_base_sidecar_config_produces_empty_provider_fields() {
        let tmp = TempDir::new().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        let settings = system_settings(&tmp);

        let cfg = resolve_base_sidecar_config(&settings, &paths).unwrap();
        assert_eq!(cfg.provider_id, "", "base config provider_id must be empty");
        assert_eq!(
            cfg.api_key_env_name, "",
            "base config api_key_env_name must be empty"
        );
        assert_eq!(
            cfg.base_url_env_name, "",
            "base config base_url_env_name must be empty"
        );
        // harness_name still set so the WorkerPool can clone+override without
        // re-resolving the runtime pieces.
        assert_eq!(cfg.harness_name, "pi");
    }

    /// `resolve_sidecar_config` delegates to `resolve_base_sidecar_config`
    /// (backward-compat for existing callers). The base fields match.
    #[test]
    fn resolve_sidecar_config_delegates_to_base() {
        let tmp = TempDir::new().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        let settings = system_settings(&tmp);

        let via_resolve = resolve_sidecar_config(&settings, &paths).unwrap();
        let via_base = resolve_base_sidecar_config(&settings, &paths).unwrap();
        assert_eq!(via_resolve.provider_id, via_base.provider_id);
        assert_eq!(via_resolve.api_key_env_name, via_base.api_key_env_name);
        assert_eq!(via_resolve.base_url_env_name, via_base.base_url_env_name);
        assert_eq!(via_resolve.harness_name, via_base.harness_name);
        assert_eq!(via_resolve.bundle_path, via_base.bundle_path);
        assert_eq!(via_resolve.node_binary, via_base.node_binary);
    }
}
