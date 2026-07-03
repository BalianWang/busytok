#![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
//! Phase 5 tests: path resolver (the GUI-side pure function).
//!
//! Tests the REAL production free function (resolve_sidecar_dir_from_exe),
//! NOT a mirror — so modifying production code without updating tests will
//! correctly fail. The service-owned update path is tested in
//! subagent_e2e_sidecar.rs (integration test).

use super::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn resolves_sidecar_dir_when_app_bundle_contains_resources() {
    let tmp = TempDir::new().unwrap();
    let app_root = tmp.path().join("Busytok.app");
    let sidecar = app_root.join("Contents/Resources/pi-sidecar");
    fs::create_dir_all(&sidecar).unwrap();
    fs::write(sidecar.join("pi-sidecar.bundle.js"), "// stub").unwrap();
    let exe = app_root.join("Contents/MacOS/Busytok");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    fs::write(&exe, b"stub").unwrap();

    // Call the REAL production function.
    let resolved = resolve_sidecar_dir_from_exe(&exe);
    assert_eq!(resolved, Some(sidecar));
}

#[test]
fn returns_none_when_no_app_bundle_ancestor() {
    let tmp = TempDir::new().unwrap();
    let exe = tmp.path().join("busytok-gui");
    fs::write(&exe, b"stub").unwrap();
    let resolved = resolve_sidecar_dir_from_exe(&exe);
    assert_eq!(resolved, None);
}

#[test]
fn returns_none_when_app_bundle_has_no_sidecar_resources() {
    let tmp = TempDir::new().unwrap();
    let app_root = tmp.path().join("Busytok.app");
    fs::create_dir_all(app_root.join("Contents/MacOS")).unwrap();
    // No Contents/Resources/pi-sidecar directory.
    let exe = app_root.join("Contents/MacOS/Busytok");
    fs::write(&exe, b"stub").unwrap();
    let resolved = resolve_sidecar_dir_from_exe(&exe);
    assert_eq!(resolved, None);
}

#[test]
fn classifies_transport_vs_business_errors() {
    // P1 guard: the file fallback must fire ONLY on transport-unreachable
    // errors (cold-start), NOT on service business errors. A business error
    // indicates a real bug; bypassing it with a file write would mask the
    // bug and re-introduce in-memory/disk state drift.

    // Transport-unreachable → file fallback (true).
    assert!(is_transport_unreachable(
        "connect/bootstrap phase timed out"
    ));
    assert!(is_transport_unreachable(
        "service unavailable: connection refused"
    ));
    assert!(is_transport_unreachable(
        "service bootstrap failed: launchctl timeout"
    ));
    assert!(is_transport_unreachable(
        "call to 'pi_sidecar_locator_update' timed out"
    ));
    assert!(is_transport_unreachable(
        "service bootstrap unavailable (coordinator not initialized)"
    ));

    // Service business errors → log + surface (false, NO fallback).
    assert!(!is_transport_unreachable(
        "[validation_error] runtime_dir must be absolute"
    ));
    assert!(!is_transport_unreachable(
        "[internal_error] failed to serialize response"
    ));
    assert!(!is_transport_unreachable(
        "dispatch error: method not found"
    ));
    // Edge: empty string is NOT transport-unreachable.
    assert!(!is_transport_unreachable(""));
}

/// Canary: assert the source files still produce the exact error-string
/// prefixes that `is_transport_unreachable` matches. If anyone changes a
/// format string in `host_application_services.rs`, `service_recovery.rs`,
/// or `commands.rs`, this test breaks and forces them to update
/// `is_transport_unreachable` (or refactor to a typed error — the proper
/// fix). Without this canary, a format drift would silently break the
/// cold-start file fallback path.
///
/// `include_str!` embeds the source at compile time, so the assertion runs
/// against the EXACT source compiled into the test binary — no path
/// resolution, no runtime file read.
#[test]
fn canary_transport_error_prefixes_match_source_files() {
    let has = include_str!("host_application_services.rs");
    let srec = include_str!("service_recovery.rs");
    let cmds = include_str!("commands.rs");

    // "connect/bootstrap phase timed out" — host_application_services.rs:67
    assert!(
        has.contains("\"connect/bootstrap phase timed out\""),
        "host_application_services.rs no longer emits the 'connect/bootstrap phase timed out' prefix"
    );
    // "call to '{method}' timed out" — host_application_services.rs:74,134
    assert!(
        has.contains("\"call to '{method}' timed out\""),
        "host_application_services.rs no longer emits the 'call to {{method}} timed out' prefix"
    );
    // "service unavailable: {e}" — host_application_services.rs:129 + service_recovery.rs:45
    assert!(
        has.contains("\"service unavailable: {e}\""),
        "host_application_services.rs no longer emits the 'service unavailable: {{e}}' prefix"
    );
    assert!(
        srec.contains("\"service unavailable: {e}\""),
        "service_recovery.rs no longer emits the 'service unavailable: {{e}}' prefix"
    );
    // "service bootstrap failed: {e}" — service_recovery.rs:85 + commands.rs:57
    assert!(
        srec.contains("\"service bootstrap failed: {e}\""),
        "service_recovery.rs no longer emits the 'service bootstrap failed: {{e}}' prefix"
    );
    assert!(
        cmds.contains("\"service bootstrap failed: {e}\""),
        "commands.rs no longer emits the 'service bootstrap failed: {{e}}' prefix"
    );
    // "service bootstrap unavailable (coordinator not initialized)" — commands.rs:60
    assert!(
        cmds.contains("\"service bootstrap unavailable (coordinator not initialized)\""),
        "commands.rs no longer emits the 'service bootstrap unavailable' prefix"
    );
}
