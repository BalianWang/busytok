//! Unit tests for `resolve_sidecar_config`.
//!
//! `resolve_sidecar_config` was previously covered via the
//! `BUSYTOK_TEST_SIDECAR_BUNDLE` env var, which was removed. These tests
//! rebuild coverage by constructing a `BusytokPaths::for_test` root and
//! materializing the bundle / node binary files the function looks for.

#![allow(clippy::unwrap_used, clippy::field_reassign_with_default)]

use std::path::PathBuf;
use std::time::Duration;

use busytok_config::{BusytokPaths, SubagentPiSidecarConfig};
use busytok_subagent::sidecar::{resolve_sidecar_config, SidecarError};
use tempfile::TempDir;

/// Materialize the sidecar bundle file at `<runtime_dir>/pi-sidecar.bundle.js`.
fn write_bundle(runtime_dir: &std::path::Path) -> PathBuf {
    let bundle = runtime_dir.join("pi-sidecar.bundle.js");
    std::fs::write(&bundle, "// mock bundle\n").unwrap();
    bundle
}

/// Materialize the bundled node binary at
/// `<runtime_dir>/node/<arch>/node`. Returns the path.
fn write_bundled_node(runtime_dir: &std::path::Path) -> PathBuf {
    let node_path = runtime_dir
        .join("node")
        .join(std::env::consts::ARCH)
        .join("node");
    std::fs::create_dir_all(node_path.parent().unwrap()).unwrap();
    // Contents don't matter — `resolve_sidecar_config` only checks `.exists()`.
    std::fs::write(&node_path, b"#!/bin/sh\n").unwrap();
    node_path
}

/// Build paths rooted under `tmp`, with `runtime_dir` resolved to the tmp
/// subdir. We pass `runtime_dir = Some(tmpdir)` to `resolve_sidecar_config`
/// via settings so the bundle/node lookups hit our materialized files instead
/// of the dev fallback path.
fn paths_for(tmp: &TempDir) -> BusytokPaths {
    BusytokPaths::for_test(tmp.path())
}

#[test]
fn resolve_sidecar_config_system_mode_uses_system_node_path() {
    let tmp = TempDir::new().unwrap();
    let paths = paths_for(&tmp);
    let runtime_dir = tmp.path().to_string_lossy().to_string();
    write_bundle(tmp.path());

    let mut settings = SubagentPiSidecarConfig::default();
    settings.node_runtime = "system".to_string();
    settings.system_node_path = "bash".to_string();
    settings.runtime_dir = Some(runtime_dir);

    let cfg = resolve_sidecar_config(&settings, &paths).unwrap();
    assert_eq!(cfg.node_binary, PathBuf::from("bash"));
    // bundle_path points at the materialized file.
    assert!(cfg.bundle_path.exists());
    assert!(cfg.bundle_path.ends_with("pi-sidecar.bundle.js"));
}

#[test]
fn resolve_sidecar_config_system_mode_empty_path_falls_back_to_node_on_path() {
    // An empty `system_node_path` is NOT an error — the spec says "system"
    // mode resolves `node` from PATH. We verify the fallback (no error).
    let tmp = TempDir::new().unwrap();
    let paths = paths_for(&tmp);
    let runtime_dir = tmp.path().to_string_lossy().to_string();
    write_bundle(tmp.path());

    let mut settings = SubagentPiSidecarConfig::default();
    settings.node_runtime = "system".to_string();
    settings.system_node_path = String::new();
    settings.runtime_dir = Some(runtime_dir);

    let cfg = resolve_sidecar_config(&settings, &paths).unwrap();
    assert_eq!(cfg.node_binary, PathBuf::from("node"));
}

#[test]
fn resolve_sidecar_config_bundled_mode_uses_bundled_node_path() {
    let tmp = TempDir::new().unwrap();
    let paths = paths_for(&tmp);
    let runtime_dir = tmp.path().to_string_lossy().to_string();
    let expected_node = write_bundled_node(tmp.path());
    write_bundle(tmp.path());

    let mut settings = SubagentPiSidecarConfig::default();
    settings.node_runtime = "bundled".to_string();
    settings.runtime_dir = Some(runtime_dir);

    let cfg = resolve_sidecar_config(&settings, &paths).unwrap();
    assert_eq!(cfg.node_binary, expected_node);
    assert!(cfg.bundle_path.exists());
}

#[test]
fn resolve_sidecar_config_bundled_mode_missing_node_binary_errors() {
    let tmp = TempDir::new().unwrap();
    let paths = paths_for(&tmp);
    let runtime_dir = tmp.path().to_string_lossy().to_string();
    // Bundle exists, but the bundled node binary does NOT.
    write_bundle(tmp.path());

    let mut settings = SubagentPiSidecarConfig::default();
    settings.node_runtime = "bundled".to_string();
    settings.runtime_dir = Some(runtime_dir);

    match resolve_sidecar_config(&settings, &paths) {
        Ok(_) => panic!("expected error for missing bundled node binary"),
        Err(SidecarError::Spawn(msg)) => {
            assert!(
                msg.contains("node_runtime='bundled'"),
                "expected bundled-node-missing message, got: {msg}"
            );
        }
        Err(other) => panic!("expected SidecarError::Spawn, got {other:?}"),
    }
}

#[test]
fn resolve_sidecar_config_invalid_node_runtime_errors() {
    let tmp = TempDir::new().unwrap();
    let paths = paths_for(&tmp);
    let runtime_dir = tmp.path().to_string_lossy().to_string();
    write_bundle(tmp.path());

    let mut settings = SubagentPiSidecarConfig::default();
    settings.node_runtime = "invalid-runtime".to_string();
    settings.runtime_dir = Some(runtime_dir);

    match resolve_sidecar_config(&settings, &paths) {
        Ok(_) => panic!("expected error for invalid node_runtime"),
        Err(SidecarError::Spawn(msg)) => {
            assert!(
                msg.contains("unknown node_runtime") && msg.contains("invalid-runtime"),
                "expected unknown-runtime message, got: {msg}"
            );
        }
        Err(other) => panic!("expected SidecarError::Spawn, got {other:?}"),
    }
}

#[test]
fn resolve_sidecar_config_missing_bundle_errors() {
    let tmp = TempDir::new().unwrap();
    let paths = paths_for(&tmp);
    let runtime_dir = tmp.path().to_string_lossy().to_string();
    // No bundle file materialized.

    let mut settings = SubagentPiSidecarConfig::default();
    settings.node_runtime = "system".to_string();
    settings.system_node_path = "bash".to_string();
    settings.runtime_dir = Some(runtime_dir);

    match resolve_sidecar_config(&settings, &paths) {
        Ok(_) => panic!("expected error for missing bundle"),
        Err(SidecarError::Spawn(msg)) => {
            assert!(
                msg.contains("sidecar bundle not found"),
                "expected bundle-not-found message, got: {msg}"
            );
        }
        Err(other) => panic!("expected SidecarError::Spawn, got {other:?}"),
    }
}

#[test]
fn resolve_sidecar_config_carries_timeouts_and_limits() {
    let tmp = TempDir::new().unwrap();
    let paths = paths_for(&tmp);
    let runtime_dir = tmp.path().to_string_lossy().to_string();
    write_bundle(tmp.path());

    let mut settings = SubagentPiSidecarConfig::default();
    settings.node_runtime = "system".to_string();
    settings.system_node_path = "bash".to_string();
    settings.runtime_dir = Some(runtime_dir);
    settings.idle_exit_seconds = 42;
    settings.task_timeout_seconds = 99;

    let cfg = resolve_sidecar_config(&settings, &paths).unwrap();
    // User-supplied values propagate verbatim.
    assert_eq!(cfg.idle_exit_seconds, 42);
    assert_eq!(cfg.task_timeout, Duration::from_secs(99));
    // Fixed defaults — spec §5.4 mandates these in MVP.
    assert_eq!(cfg.health_interval, Duration::from_secs(30));
    assert_eq!(cfg.max_restart_attempts, 3);
    assert_eq!(cfg.restart_backoff_base, Duration::from_secs(1));
    assert_eq!(cfg.harness_name, "pi");
    assert_eq!(cfg.max_hot_sessions, settings.max_hot_sessions);
    // env now carries the hot session limit (spec §8.2); API keys are added
    // at spawn in Plan 4.
    assert_eq!(
        cfg.env.get("BUSYTOK_SIDECAR_MAX_HOT_SESSIONS"),
        Some(&settings.max_hot_sessions.to_string()),
        "max_hot_sessions must be passed to the sidecar via env var"
    );
}

#[test]
fn resolve_sidecar_config_passes_max_hot_sessions() {
    let tmp = TempDir::new().unwrap();
    let paths = paths_for(&tmp);
    let runtime_dir = tmp.path().to_string_lossy().to_string();
    write_bundle(tmp.path());

    let mut settings = SubagentPiSidecarConfig::default();
    settings.node_runtime = "system".to_string();
    settings.system_node_path = "bash".to_string();
    settings.runtime_dir = Some(runtime_dir);
    settings.max_hot_sessions = 5;

    let cfg = resolve_sidecar_config(&settings, &paths).unwrap();
    assert_eq!(cfg.max_hot_sessions, 5);
    assert_eq!(
        cfg.env.get("BUSYTOK_SIDECAR_MAX_HOT_SESSIONS"),
        Some(&"5".to_string()),
        "max_hot_sessions must be passed to sidecar via env var"
    );
}
