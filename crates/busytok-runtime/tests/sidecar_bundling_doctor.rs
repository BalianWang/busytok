#![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
//! Phase 5 Task 5: verifies that when a complete sidecar bundle
//! (`pi-sidecar.bundle.js` + `manifest.json` + node binary for the current
//! arch) is staged at the persisted `runtime_dir`, the doctor checks
//! `bundled_node_arch`, `bundle_manifest_readable`, and `pi_runtime_installed`
//! all return `"ok"`.
//!
//! These are the 3 previously-failing checks (they error in dev because the
//! dev-fallback path `apps/pi-sidecar/dist` has no node/manifest/bundle files).
//! The `protocol_version` check is NOT verified here — it requires a real
//! Node binary + running sidecar handshake, which CI cannot provide. Its
//! pass state is verified by the manual smoke checklist (Step 4 of the plan).
//!
//! `pi_sidecar.enabled = false` in the test settings: we exercise the
//! filesystem doctor checks, not the protocol probe — the stub node binary
//! is an empty file with `+x`, not a real Node runtime.

use busytok_config::{BusytokPaths, BusytokSettings, SidecarManifest};
use busytok_control::dispatch::RuntimeControl;
use busytok_runtime::BusytokSupervisor;
use busytok_store::Database;
use serial_test::serial;
use std::fs;
use tempfile::TempDir;

/// Stage a complete packaged sidecar directory at `<tmp>/pi-sidecar/`:
/// - `pi-sidecar.bundle.js` (stub — the doctor doesn't execute it)
/// - `manifest.json` (valid `SidecarManifest` via `to_json_string`)
/// - `node/<std::env::consts::ARCH>/node` (stub empty file with `0o755`)
///
/// Returns the staged `runtime_dir` path. The node arch dir uses the host's
/// `std::env::consts::ARCH` (so on aarch64 macOS it stages `node/aarch64/node`,
/// on x86_64 it stages `node/x86_64/node`), matching the doctor check's
/// expected-arch derivation at `supervisor.rs` (`let expected_arch =
/// std::env::consts::ARCH`).
fn stage_complete_sidecar_dir(tmp: &TempDir) -> std::path::PathBuf {
    let runtime_dir = tmp.path().join("pi-sidecar");
    let node_arch_dir = runtime_dir.join("node").join(std::env::consts::ARCH);
    fs::create_dir_all(&node_arch_dir).unwrap();

    // Write a stub bundle.js — the doctor's `bundle_manifest_readable` check
    // only parses `manifest.json`; `pi_runtime_installed` only checks existence
    // of bundle + node. Neither executes the bundle. The protocol probe
    // (which WOULD execute it) is verified by the manual smoke checklist.
    fs::write(
        runtime_dir.join("pi-sidecar.bundle.js"),
        "// stub bundle for doctor filesystem checks",
    )
    .unwrap();

    // Write a valid manifest.json conforming to the SidecarManifest schema
    // (Task 1: version/protocol_version/bundle/node_runtime_version). The
    // `protocol_version` field is `u32` to match `PROTOCOL_VERSION: u32` at
    // `protocol.rs:28` — direct assignment, NO cast.
    let manifest = SidecarManifest {
        version: "1".to_string(),
        protocol_version: busytok_subagent::sidecar::protocol::PROTOCOL_VERSION,
        bundle: "pi-sidecar.bundle.js".to_string(),
        node_runtime_version: "22.6.0".to_string(),
    };
    fs::write(
        runtime_dir.join("manifest.json"),
        manifest.to_json_string(),
    )
    .unwrap();

    // Write a stub node binary (empty file with +x). The doctor checks
    // existence + arch-dir-name match, not that it's a real executable.
    let node_path = node_arch_dir.join("node");
    fs::write(&node_path, b"#!/bin/sh\n# stub node\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&node_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&node_path, perms).unwrap();
    }

    runtime_dir
}

/// Build a `BusytokSettings` pointing at the given `runtime_dir` with
/// `pi_sidecar.enabled = false` (we test filesystem checks, NOT the protocol
/// probe — the stub node binary can't actually run a sidecar). This deviates
/// intentionally from `make_sidecar_settings` in `subagent_e2e_sidecar.rs`,
/// which sets `enabled = true` for the real-sidecar e2e tests.
fn make_settings_with_runtime_dir(runtime_dir: &str) -> BusytokSettings {
    let mut settings = BusytokSettings::default();
    settings.subagent.pi_sidecar.enabled = false;
    settings.subagent.pi_sidecar.runtime_dir = Some(runtime_dir.to_string());
    settings
}

#[tokio::test]
#[serial]
async fn doctor_passes_all_bundle_checks_when_resources_present() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = stage_complete_sidecar_dir(&tmp);
    let runtime_dir_str = runtime_dir.to_string_lossy().to_string();

    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = make_settings_with_runtime_dir(&runtime_dir_str);
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();

    // The 3 checks that previously failed in dev must now pass.
    for check_name in ["bundled_node_arch", "bundle_manifest_readable", "pi_runtime_installed"] {
        let check = sub
            .checks
            .iter()
            .find(|c| c.name == check_name)
            .unwrap_or_else(|| panic!("missing check: {check_name}"));
        assert_eq!(
            check.status, "ok",
            "check {} should be ok with complete resources: {:?}",
            check_name, check.detail
        );
    }

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_bundled_node_arch_fails_when_arch_dir_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = tmp.path().join("pi-sidecar");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::write(
        runtime_dir.join("pi-sidecar.bundle.js"),
        "// stub",
    )
    .unwrap();
    fs::write(
        runtime_dir.join("manifest.json"),
        SidecarManifest {
            version: "1".to_string(),
            protocol_version: 1,
            bundle: "pi-sidecar.bundle.js".to_string(),
            node_runtime_version: "22.6.0".to_string(),
        }
        .to_json_string(),
    )
    .unwrap();
    // NO node/ directory — the bundled_node_arch check must fail.

    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = make_settings_with_runtime_dir(
        &runtime_dir.to_string_lossy(),
    );
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "bundled_node_arch")
        .unwrap();
    assert_eq!(check.status, "error");
    assert!(check.detail.as_deref().unwrap_or("").contains("not found"));

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_manifest_rejects_missing_node_runtime_version_field() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = tmp.path().join("pi-sidecar");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::write(runtime_dir.join("pi-sidecar.bundle.js"), "// stub").unwrap();
    // Manifest missing `node_runtime_version` — must be rejected by the
    // typed `SidecarManifest` deserialize (Task 1). The doctor's
    // `bundle_manifest_readable` check surfaces the typed deserialize error
    // in its `detail`, so we assert the detail mentions `SidecarManifest`.
    fs::write(
        runtime_dir.join("manifest.json"),
        r#"{"version":"1","protocol_version":1,"bundle":"pi-sidecar.bundle.js"}"#,
    )
    .unwrap();

    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = make_settings_with_runtime_dir(
        &runtime_dir.to_string_lossy(),
    );
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "bundle_manifest_readable")
        .unwrap();
    assert_eq!(check.status, "error");
    assert!(check.detail.as_deref().unwrap_or("").contains("SidecarManifest"));

    supervisor.shutdown_writer().await.unwrap();
}
