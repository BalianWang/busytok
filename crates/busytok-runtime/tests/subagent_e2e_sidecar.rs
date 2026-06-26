#![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
//! End-to-end subagent lifecycle through the real Pi sidecar subprocess.
//!
//! Constructs a `BusytokSupervisor` with `pi_sidecar.enabled = true` and
//! injects mock-sidecar.sh as the sidecar bundle via a pre-resolved
//! `SidecarConfig` passed to `BusytokSupervisor::new_with_sidecar_config`
//! (no env-var escape hatch in production code). Exercises the full
//! delegate → list → show → hibernate → delete lifecycle through the
//! `RuntimeControl` dispatch path — the same path the control server uses.
//!
//! Regression value: catches integration bugs that unit tests miss —
//! supervisor constructs the sidecar incorrectly, settings don't propagate,
//! the shutdown sequence doesn't cleanly stop the sidecar, etc.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use busytok_config::{BusytokPaths, BusytokSettings};
use busytok_control::dispatch::RuntimeControl;
use busytok_protocol::dto::*;
use busytok_runtime::BusytokSupervisor;
use busytok_subagent::sidecar::SidecarConfig;

/// Path to the mock-sidecar.sh fixture, resolved relative to
/// CARGO_MANIFEST_DIR (crates/busytok-runtime). The fixture lives in
/// busytok-subagent/tests/fixtures/.
fn mock_sidecar_path() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(format!(
        "{manifest}/../busytok-subagent/tests/fixtures/mock-sidecar.sh"
    ))
}

/// Settings with pi_sidecar enabled, using system bash as the "node"
/// binary (mock-sidecar.sh is a bash script, not a Node bundle).
fn make_sidecar_settings() -> BusytokSettings {
    let mut settings = BusytokSettings::default();
    settings.subagent.pi_sidecar.enabled = true;
    settings.subagent.pi_sidecar.node_runtime = "system".to_string();
    settings.subagent.pi_sidecar.system_node_path = "/bin/bash".to_string();
    settings.subagent.pi_sidecar.idle_exit_seconds = 300;
    settings.subagent.pi_sidecar.task_timeout_seconds = 30;
    settings
}

/// Build a `SidecarConfig` that points at mock-sidecar.sh. Mirrors the
/// fields `resolve_sidecar_config` would produce for the test settings,
/// but with `bundle_path` set to the mock fixture (no env var needed).
fn make_sidecar_config() -> SidecarConfig {
    SidecarConfig {
        node_binary: PathBuf::from("bash"),
        bundle_path: mock_sidecar_path(),
        env: HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(30),
        task_timeout: Duration::from_secs(30),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_secs(1),
        harness_name: "pi".to_string(),
    }
}

/// Construct a supervisor that loads sidecar-enabled settings from the
/// config file in `tmp` and injects the mock sidecar bundle via
/// `new_with_sidecar_config`. The settings file must still have
/// `pi_sidecar.enabled = true` so the sidecar wiring path is taken.
fn make_sidecar_supervisor(
    db: busytok_store::Database,
    tmp: &tempfile::TempDir,
    settings: BusytokSettings,
) -> BusytokSupervisor {
    let paths = BusytokPaths::for_test(tmp.path());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");
    BusytokSupervisor::new_with_sidecar_config(db, paths, make_sidecar_config())
}

#[tokio::test]
async fn sidecar_e2e_delegate_list_show_hibernate_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let settings = make_sidecar_settings();
    let supervisor = make_sidecar_supervisor(db, &tmp, settings);

    // 1. delegate — must go through the sidecar subprocess.
    //    adapter_session_id being set proves the sidecar was used
    //    (the mock executor returns None for this field).
    let delegate_resp = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "e2e-reviewer".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "find the bug".to_string(),
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .unwrap();

    let sub_id = delegate_resp.subagent_id.clone();
    assert_eq!(delegate_resp.status, "completed");
    assert!(
        delegate_resp.adapter_session_id.is_some(),
        "adapter_session_id must be set — proves the sidecar subprocess was used"
    );
    assert!(
        delegate_resp
            .adapter_session_id
            .as_ref()
            .unwrap()
            .starts_with("pi_sess_mock_"),
        "adapter_session_id should come from mock-sidecar.sh, got: {:?}",
        delegate_resp.adapter_session_id
    );

    // 2. list — the just-created subagent must appear.
    let list = supervisor
        .subagent_list(SubagentListRequestDto {
            status: None,
            project: None,
            include_deleted: Some(false),
        })
        .await
        .unwrap();
    assert!(
        list.subagents.iter().any(|s| s.id == sub_id),
        "delegated subagent must appear in list"
    );

    // 3. show by UUID — verify detail.
    let shown = supervisor
        .subagent_show(SubagentResolveRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
        })
        .await
        .unwrap();
    assert_eq!(shown.name, "e2e-reviewer");
    assert_eq!(shown.status, "hot", "subagent should be hot after delegate");

    // 4. hibernate — releases the hot session.
    supervisor
        .subagent_hibernate(SubagentResolveRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
        })
        .await
        .unwrap();

    // After hibernate, status should transition away from hot.
    let after_hibernate = supervisor
        .subagent_show(SubagentResolveRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
        })
        .await
        .unwrap();
    assert_ne!(
        after_hibernate.status, "hot",
        "subagent should not be hot after hibernate"
    );

    // 5. soft delete — removes from active list.
    supervisor
        .subagent_delete(SubagentDeleteRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
            hard: Some(false),
        })
        .await
        .unwrap();
    let after_list = supervisor
        .subagent_list(SubagentListRequestDto {
            status: None,
            project: None,
            include_deleted: Some(false),
        })
        .await
        .unwrap();
    assert!(
        after_list.subagents.iter().all(|s| s.id != sub_id),
        "soft-deleted subagent must not appear in active list"
    );

    // 6. verify resource events were written (sidecar_start at minimum).
    //    Scoped block ensures the MutexGuard is dropped before the await
    //    points in step 7 (clippy::await_holding_lock).
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        let events = db_guard.subagent_list_resource_events(None, 100).unwrap();
        assert!(
            events.iter().any(|e| e.event_type == "sidecar_start"),
            "sidecar_start resource event must be written"
        );
    }

    // 7. graceful shutdown — kills the sidecar subprocess and reconciles
    //    DB state (releases hot bindings, rolls back logical status).
    supervisor.shutdown_sidecar().await;

    // Post-shutdown DB assertion (spec §3.3 end-to-end): after graceful
    // shutdown, no hot bindings may remain for the harness, and the
    // previously-hot subagent's status must NOT be 'hot'. This guards the
    // shutdown reconciliation added to `shutdown_internal` — a dead sidecar
    // process must never leave a `status='hot'` row or an `is_hot=1`
    // binding in the store.
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        let hot_count: i64 = db_guard
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM subagent_harness_bindings \
                 WHERE is_hot = 1 AND harness = 'pi'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            hot_count, 0,
            "no hot bindings should exist for the pi harness after shutdown"
        );
        let sub = db_guard.subagent_get_logical(&sub_id).unwrap();
        let sub = sub.expect("logical subagent row must still exist after shutdown");
        assert_ne!(
            sub.status, "hot",
            "previously-hot subagent must not be 'hot' after shutdown reconciliation"
        );
    }

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn sidecar_e2e_delegate_then_shutdown_releases_hot_binding() {
    // Delegate creates a hot binding (is_hot=1, status='hot').
    // Graceful shutdown must release it (is_hot=0, status='closed')
    // and roll back logical status to warm/cold — WITHOUT hibernate
    // or delete first. This is the only test that genuinely exercises
    // the shutdown reconciliation path: the sibling lifecycle test
    // hibernates + deletes before shutdown, which drains hot bindings
    // first and makes `release_hot_bindings_for_shutdown` hit its
    // early-return (vacuous assertion).
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let settings = make_sidecar_settings();
    let supervisor = make_sidecar_supervisor(db, &tmp, settings);

    // 1. delegate — must go through the sidecar subprocess.
    let delegate_resp = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "shutdown-test".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "find the bug".to_string(),
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .unwrap();

    let sub_id = delegate_resp.subagent_id.clone();
    assert_eq!(delegate_resp.status, "completed");
    assert!(
        delegate_resp.adapter_session_id.is_some(),
        "adapter_session_id must be set — proves the sidecar subprocess was used"
    );
    assert!(
        delegate_resp
            .adapter_session_id
            .as_ref()
            .unwrap()
            .starts_with("pi_sess_mock_"),
        "adapter_session_id should come from mock-sidecar.sh, got: {:?}",
        delegate_resp.adapter_session_id
    );

    // 2. verify the subagent is hot (proves a hot binding exists pre-shutdown).
    let shown = supervisor
        .subagent_show(SubagentResolveRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
        })
        .await
        .unwrap();
    assert_eq!(
        shown.status, "hot",
        "subagent must be hot after delegate (precondition for the shutdown assertion)"
    );

    // 3. graceful shutdown — WITHOUT hibernate or delete first. This is the
    //    only path that leaves a hot binding in place for
    //    `release_hot_bindings_for_shutdown` to reconcile.
    supervisor.shutdown_sidecar().await;

    // 4. post-shutdown DB assertions (spec §3.3 end-to-end). Scoped block
    //    ensures the MutexGuard is dropped before the cleanup `.await`
    //    (clippy::await_holding_lock).
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        // (a) No hot bindings may remain for the pi harness.
        let hot_count: i64 = db_guard
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM subagent_harness_bindings \
                 WHERE is_hot = 1 AND harness = ?1",
                rusqlite::params!["pi"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            hot_count, 0,
            "no hot bindings should exist for the pi harness after shutdown"
        );
        // (b) The previously-hot subagent's status must roll back to warm/cold.
        //     STRICT assertion (not `!= "hot"`) — a dead sidecar process must
        //     never leave a `status='hot'` row, and the rollback target is
        //     specifically warm (memory exists) or cold (no memory).
        let sub = db_guard
            .subagent_get_logical(&sub_id)
            .unwrap()
            .expect("logical subagent row must still exist after shutdown");
        assert!(
            sub.status == "warm" || sub.status == "cold",
            "previously-hot subagent must roll back to 'warm' or 'cold' after shutdown, got: {:?}",
            sub.status
        );
    }

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn sidecar_e2e_misconfigured_sidecar_fails_delegate_not_silently_mock() {
    // P1-2 regression: when pi_sidecar.enabled=true but the sidecar config
    // cannot be resolved (e.g. runtime_dir points at a nonexistent bundle),
    // the supervisor must NOT silently fall back to MockTaskExecutor —
    // that would mask a deployment misconfiguration as "functional".
    // Instead, delegate must fail with a clear error, and
    // sidecar_init_error() must return Some(reason).
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());

    // Settings with enabled=true but a runtime_dir that has no bundle —
    // resolve_sidecar_config will fail with "sidecar bundle not found".
    let mut settings = BusytokSettings::default();
    settings.subagent.pi_sidecar.enabled = true;
    settings.subagent.pi_sidecar.node_runtime = "system".to_string();
    settings.subagent.pi_sidecar.system_node_path = "bash".to_string();
    settings.subagent.pi_sidecar.runtime_dir = Some(tmp.path().to_string_lossy().to_string());

    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    let supervisor = BusytokSupervisor::with_adapters_and_settings(
        db,
        paths,
        vec![], // no adapters needed for this test
        settings,
    );

    // sidecar_init_error must be populated — the config resolve failure
    // must be surfaced, not silently swallowed.
    assert!(
        supervisor.sidecar_init_error().is_some(),
        "sidecar_init_error must be set when config resolve fails"
    );
    let init_err = supervisor.sidecar_init_error().unwrap();
    assert!(
        init_err.contains("bundle") || init_err.contains("not found"),
        "error should mention the bundle issue, got: {init_err}"
    );

    // delegate MUST fail — never return mock output. The error must carry
    // the semantic code `subagent.sidecar_spawn_failed` (not the generic
    // `subagent.store_error`), proving FailingTaskExecutor's error
    // downcasts to SubagentError::SidecarSpawn through the manager.
    let result = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "misconfigured-test".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "find the bug".to_string(),
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await;

    let err = match result {
        Ok(_) => panic!(
            "delegate must fail when sidecar is enabled but misconfigured — got success (silent mock fallback)"
        ),
        Err(e) => e,
    };
    // The error string from the runtime layer is formatted as "{code}: {message}".
    // Assert the code is `subagent.sidecar_spawn_failed` — NOT `subagent.store_error`.
    let err_str = err.to_string();
    assert!(
        err_str.contains("subagent.sidecar_spawn_failed"),
        "delegate error must carry code 'subagent.sidecar_spawn_failed' \
         (not generic store_error), got: {err_str}"
    );

    supervisor.shutdown_writer().await.unwrap();
}
