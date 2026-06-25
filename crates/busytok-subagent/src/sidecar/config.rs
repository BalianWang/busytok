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
}

/// Resolve a `SidecarConfig` from settings + paths.
///
/// Explicit mode selection — NO silent fallback. Spec §10.1/§5.1.
/// `node_runtime = "bundled"` requires the bundled node binary to exist;
/// `node_runtime = "system"` uses `system_node_path` (or PATH `node` if empty).
pub fn resolve_sidecar_config(
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
    // Test escape hatch: when BUSYTOK_TEST_SIDECAR_BUNDLE is set, use that
    // path instead of the resolved bundle. This allows the busytok-runtime
    // e2e test (Task 7) to substitute mock-sidecar.sh without a test-only
    // BusytokSupervisor constructor. The env var is only set by a single
    // integration test, so parallel-test safety is not a concern.
    let bundle_path = std::env::var("BUSYTOK_TEST_SIDECAR_BUNDLE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| paths.sidecar_bundle_path(runtime_dir));
    if !bundle_path.exists() {
        return Err(SidecarError::Spawn(format!(
            "sidecar bundle not found at {}",
            bundle_path.display()
        )));
    }
    Ok(SidecarConfig {
        node_binary,
        bundle_path,
        env: HashMap::new(), // API keys added at spawn in Plan 4
        idle_exit_seconds: settings.idle_exit_seconds,
        // Spec §5.4: health ping every 30s. Fixed in MVP (no config knob).
        health_interval: Duration::from_secs(30),
        task_timeout: Duration::from_secs(settings.task_timeout_seconds),
        // Spec §5.4: max 3 attempts. The sliding 5-min window is NOT
        // implemented in MVP — restart_attempts resets on successful spawn.
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_secs(1),
        harness_name: "pi".to_string(),
    })
}
