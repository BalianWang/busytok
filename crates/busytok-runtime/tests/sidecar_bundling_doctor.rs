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
use busytok_protocol::dto::PiSidecarLocatorUpdateRequestDto;
use busytok_runtime::BusytokSupervisor;
use busytok_store::Database;
use serial_test::serial;
use std::fs;
use std::sync::Arc;
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
    fs::write(runtime_dir.join("manifest.json"), manifest.to_json_string()).unwrap();

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
    for check_name in [
        "bundled_node_arch",
        "bundle_manifest_readable",
        "pi_runtime_installed",
    ] {
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
    fs::write(runtime_dir.join("pi-sidecar.bundle.js"), "// stub").unwrap();
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
    let settings = make_settings_with_runtime_dir(&runtime_dir.to_string_lossy());
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
    let settings = make_settings_with_runtime_dir(&runtime_dir.to_string_lossy());
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
    assert!(check
        .detail
        .as_deref()
        .unwrap_or("")
        .contains("SidecarManifest"));

    supervisor.shutdown_writer().await.unwrap();
}

/// P0 fix: after `pi_sidecar_locator_update` flips `enabled` from `false` to
/// `true` with a valid `runtime_dir`, the sidecar runtime is REBUILT in-flight
/// (not "pending restart"). This closes the spec §472-475 fresh-install
/// closed loop: GUI calls this RPC → service rebuilds sidecar → doctor/delegate
/// work in the CURRENT session.
///
/// We verify the rebuild by checking that `pressure_gate()` and `worker_pool()`
/// transition from `None` (disabled) to `Some` (enabled + config resolved).
/// `sidecar_supervisor` stays `None` because no providers are configured —
/// that's a separate concern (provider_create would make it `Some`).
#[tokio::test]
#[serial]
async fn rebuild_sidecar_runtime_after_locator_update_enables_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = stage_complete_sidecar_dir(&tmp);
    let runtime_dir_str = runtime_dir.to_string_lossy().to_string();

    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    // Start with enabled=false so construct_sidecar returns all-None fields.
    let mut settings = BusytokSettings::default();
    settings.subagent.pi_sidecar.enabled = false;
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    // Pre-condition: sidecar is disabled — no pressure_gate, no worker_pool.
    assert!(supervisor.pressure_gate().is_none());
    assert!(supervisor.worker_pool().is_none());
    assert!(supervisor.sidecar_init_error().is_none());

    // Flip enabled=true via the service-owned RPC (mirrors the GUI startup
    // path). This triggers validate_runtime_dir + rebuild_sidecar_runtime.
    supervisor
        .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
            runtime_dir: runtime_dir_str.clone(),
            enabled: true,
        })
        .await
        .unwrap();

    // Post-condition: sidecar runtime was rebuilt — pressure_gate and
    // worker_pool are now Some (construct_sidecar ran with enabled=true and
    // resolve_base_sidecar_config succeeded). sidecar_init_error is None
    // (config resolved OK). sidecar_supervisor is None (no providers
    // configured — that's expected, not a rebuild failure).
    assert!(
        supervisor.pressure_gate().is_some(),
        "pressure_gate should be Some after rebuild (enabled=true, config OK)"
    );
    assert!(
        supervisor.worker_pool().is_some(),
        "worker_pool should be Some after rebuild (enabled=true, config OK)"
    );
    assert!(
        supervisor.sidecar_init_error().is_none(),
        "sidecar_init_error should be None (config resolved successfully)"
    );

    // The doctor protocol_version check should NOT say "disabled" or
    // "pending restart" — it should say "enabled but no worker running"
    // (correct for enabled=true with no providers configured).
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "protocol_version")
        .unwrap();
    let detail = check.detail.as_deref().unwrap_or("");
    assert!(
        !detail.contains("disabled"),
        "post-rebuild detail must NOT say 'disabled': {detail:?}"
    );
    assert!(
        !detail.contains("pending restart"),
        "post-rebuild detail must NOT say 'pending restart': {detail:?}"
    );
    assert!(
        detail.contains("configure a provider"),
        "post-rebuild detail should mention 'configure a provider': {detail:?}"
    );

    supervisor.shutdown_writer().await.unwrap();
}

/// P1: `pi_sidecar_locator_update` must reject non-absolute paths with a
/// `validation_error` BEFORE persisting to settings.
#[tokio::test]
#[serial]
async fn locator_update_rejects_non_absolute_path() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = BusytokSettings::default();
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    let err = supervisor
        .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
            runtime_dir: "relative/path".to_string(),
            enabled: true,
        })
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("validation_error"),
        "should be validation_error: {msg}"
    );
    assert!(
        msg.contains("must be absolute"),
        "should mention 'must be absolute': {msg}"
    );

    supervisor.shutdown_writer().await.unwrap();
}

/// P1: `pi_sidecar_locator_update` must reject non-existent directories.
#[tokio::test]
#[serial]
async fn locator_update_rejects_missing_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = BusytokSettings::default();
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    let missing = tmp.path().join("does-not-exist");
    let err = supervisor
        .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
            runtime_dir: missing.to_string_lossy().to_string(),
            enabled: true,
        })
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("validation_error"),
        "should be validation_error: {msg}"
    );
    assert!(
        msg.contains("does not exist"),
        "should mention 'does not exist': {msg}"
    );

    supervisor.shutdown_writer().await.unwrap();
}

/// P1: `pi_sidecar_locator_update` must reject directories missing
/// `pi-sidecar.bundle.js`.
#[tokio::test]
#[serial]
async fn locator_update_rejects_missing_bundle_file() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = BusytokSettings::default();
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    // Directory exists but has only manifest.json (no bundle).
    let dir = tmp.path().join("incomplete-sidecar");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("manifest.json"), "{}").unwrap();
    let err = supervisor
        .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
            runtime_dir: dir.to_string_lossy().to_string(),
            enabled: true,
        })
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("validation_error"),
        "should be validation_error: {msg}"
    );
    assert!(
        msg.contains("pi-sidecar.bundle.js"),
        "should mention missing bundle file: {msg}"
    );

    supervisor.shutdown_writer().await.unwrap();
}

/// P1: `pi_sidecar_locator_update` must reject directories missing
/// `manifest.json`.
#[tokio::test]
#[serial]
async fn locator_update_rejects_missing_manifest_file() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = BusytokSettings::default();
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    // Directory exists but has only bundle.js (no manifest).
    let dir = tmp.path().join("incomplete-sidecar");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("pi-sidecar.bundle.js"), "// stub").unwrap();
    let err = supervisor
        .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
            runtime_dir: dir.to_string_lossy().to_string(),
            enabled: true,
        })
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("validation_error"),
        "should be validation_error: {msg}"
    );
    assert!(
        msg.contains("manifest.json"),
        "should mention missing manifest file: {msg}"
    );

    supervisor.shutdown_writer().await.unwrap();
}

/// P1: when `enabled=false`, validation is skipped (the path may be stale
/// and we don't care about its contents — the user is disabling the sidecar).
/// The RPC should succeed and persist the settings without rebuilding.
#[tokio::test]
#[serial]
async fn locator_update_skips_validation_when_disabling() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = BusytokSettings::default();
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    // Even a bogus path should be accepted when enabled=false.
    supervisor
        .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
            runtime_dir: "bogus/relative/path".to_string(),
            enabled: false,
        })
        .await
        .expect("disabling should skip validation");

    supervisor.shutdown_writer().await.unwrap();
}

/// P0 edge: calling `pi_sidecar_locator_update` with the SAME runtime_dir
/// and enabled=true (when already enabled) is a no-op — no rebuild, no
/// dispatcher churn. Verifies the `changed=false` branch.
#[tokio::test]
#[serial]
async fn locator_update_no_rebuild_when_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = stage_complete_sidecar_dir(&tmp);
    let runtime_dir_str = runtime_dir.to_string_lossy().to_string();

    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    // Start with enabled=true + the runtime_dir already set.
    let mut settings = BusytokSettings::default();
    settings.subagent.pi_sidecar.enabled = true;
    settings.subagent.pi_sidecar.runtime_dir = Some(runtime_dir_str.clone());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    // Capture the pressure_gate Arc identity before the call.
    let gate_before = supervisor.pressure_gate();

    // Call with the SAME values — should be a no-op (changed=false).
    supervisor
        .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
            runtime_dir: runtime_dir_str,
            enabled: true,
        })
        .await
        .unwrap();

    // The pressure_gate Arc should be the SAME object (no rebuild happened).
    let gate_after = supervisor.pressure_gate();
    assert!(
        gate_before.is_some() && gate_after.is_some(),
        "pressure_gate should be Some (enabled=true, config OK)"
    );
    assert!(
        Arc::ptr_eq(&gate_before.unwrap(), &gate_after.unwrap()),
        "pressure_gate Arc identity should be unchanged (no rebuild)"
    );

    supervisor.shutdown_writer().await.unwrap();
}

/// P0 edge: when `construct_sidecar` fails during rebuild (e.g., missing
/// node binary → `resolve_base_sidecar_config` error), the rebuild
/// "succeeds" but produces a degraded `FailingTaskExecutor` +
/// `sidecar_init_error`. The RPC still returns Ok — the error is
/// surfaced via `sidecar_init_error()` / doctor checks, not propagated.
#[tokio::test]
#[serial]
async fn rebuild_degraded_when_node_binary_missing() {
    let tmp = tempfile::tempdir().unwrap();
    // Stage bundle.js + manifest.json but NO node binary.
    let runtime_dir = tmp.path().join("pi-sidecar");
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::write(runtime_dir.join("pi-sidecar.bundle.js"), "// stub bundle").unwrap();
    let manifest = SidecarManifest {
        version: "1".to_string(),
        protocol_version: busytok_subagent::sidecar::protocol::PROTOCOL_VERSION,
        bundle: "pi-sidecar.bundle.js".to_string(),
        node_runtime_version: "22.6.0".to_string(),
    };
    fs::write(runtime_dir.join("manifest.json"), manifest.to_json_string()).unwrap();
    let runtime_dir_str = runtime_dir.to_string_lossy().to_string();

    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = BusytokSettings::default();
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    // Validation passes (bundle.js + manifest.json exist), but
    // resolve_base_sidecar_config fails (no node binary) → degraded mode.
    supervisor
        .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
            runtime_dir: runtime_dir_str,
            enabled: true,
        })
        .await
        .unwrap();

    // The rebuild produced a FailingTaskExecutor + sidecar_init_error.
    assert!(
        supervisor.sidecar_init_error().is_some(),
        "sidecar_init_error should be Some (node binary missing)"
    );
    // worker_pool is None (construct_sidecar's Err branch).
    assert!(
        supervisor.worker_pool().is_none(),
        "worker_pool should be None (config resolution failed)"
    );

    supervisor.shutdown_writer().await.unwrap();
}

/// P0 edge: disabling (enabled=true → false) via `pi_sidecar_locator_update`
/// triggers a rebuild that swaps in a MockTaskExecutor runtime and shuts
/// down the old sidecar supervisor. Without this, the old Node subprocess
/// would stay alive and the doctor would misleadingly report "ok".
#[tokio::test]
#[serial]
async fn rebuild_on_disable_shuts_down_supervisor() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = stage_complete_sidecar_dir(&tmp);
    let runtime_dir_str = runtime_dir.to_string_lossy().to_string();

    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    // Start with enabled=true.
    let mut settings = BusytokSettings::default();
    settings.subagent.pi_sidecar.enabled = true;
    settings.subagent.pi_sidecar.runtime_dir = Some(runtime_dir_str.clone());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    // Pre-condition: sidecar is enabled — pressure_gate is Some.
    assert!(supervisor.pressure_gate().is_some());

    // Disable via the RPC — triggers rebuild with enabled=false.
    supervisor
        .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
            runtime_dir: runtime_dir_str,
            enabled: false,
        })
        .await
        .unwrap();

    // Post-condition: sidecar is disabled — pressure_gate is None
    // (MockTaskExecutor runtime, no pressure gate).
    assert!(
        supervisor.pressure_gate().is_none(),
        "pressure_gate should be None after disable (MockTaskExecutor runtime)"
    );
    assert!(
        supervisor.worker_pool().is_none(),
        "worker_pool should be None after disable"
    );
    assert!(
        supervisor.sidecar_init_error().is_none(),
        "sidecar_init_error should be None (disabled, not broken)"
    );

    // The doctor protocol_version check should say "disabled" (not "ok"
    // from a stale supervisor).
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "protocol_version")
        .unwrap();
    let detail = check.detail.as_deref().unwrap_or("");
    assert!(
        detail.contains("disabled"),
        "post-disable detail should say 'disabled': {detail:?}"
    );

    supervisor.shutdown_writer().await.unwrap();
}
