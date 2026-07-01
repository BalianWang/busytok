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
