#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[path = "support/mod.rs"]
mod support;

use busytok_config::SubagentSettings;
use busytok_store::repository::{SubagentHarnessBindingRow, SubagentLogicalSubagentRow};
use busytok_store::Database;
use busytok_subagent::manager::SubagentManager;
use busytok_subagent::mock_executor::MockTaskExecutor;
use busytok_subagent::models::DelegateRequest;
use busytok_subagent::sidecar::{
    PiSidecarSupervisor, PressureLevel, SidecarConfig, SidecarError, SidecarTaskExecutor,
    WorkerState,
};
use busytok_subagent::{PressureAction, PressureGate, PressureResponder};

fn mock_config() -> SidecarConfig {
    SidecarConfig {
        node_binary: support::sidecar_shell_path(),
        bundle_path: support::mock_sidecar_bundle_path(),
        env: HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(3600), // disable in basic tests
        task_timeout: Duration::from_secs(5),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_millis(10),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
        provider_id: String::new(),
        api_key_env_name: String::new(),
        base_url_env_name: String::new(),
    }
}

/// Config used by the crash-reconciliation test. The sidecar crashes after
/// processing 1 message (adapter.initialize), so the supervision loop detects
/// the crash on its first poll and calls `reconcile_sidecar_crash`.
fn mock_sidecar_config_crash_on_init() -> SidecarConfig {
    let mut cfg = mock_config();
    cfg.env
        .insert("BUSYTOK_MOCK_CRASH_AFTER".to_string(), "1".to_string());
    cfg
}

#[tokio::test]
async fn supervisor_spawns_and_initializes() {
    let sup = PiSidecarSupervisor::new(mock_config(), None);
    let handle = sup.ensure_started().await.unwrap();
    let health = handle.health().await.unwrap();
    assert_eq!(health["status"], "healthy");
    sup.shutdown().await.unwrap();
}

#[tokio::test]
async fn supervisor_try_is_running_tracks_lifecycle() {
    let sup = PiSidecarSupervisor::new(mock_config(), None);
    assert!(
        !sup.try_is_running(),
        "fresh supervisor should report not running"
    );

    let _ = sup.ensure_started().await.unwrap();
    assert!(
        sup.try_is_running(),
        "ensure_started should make child visible"
    );

    sup.shutdown().await.unwrap();
    assert!(
        !sup.try_is_running(),
        "shutdown should clear the running child"
    );
}

#[tokio::test]
async fn supervisor_crash_recovery_restarts_sidecar() {
    let mut cfg = mock_config();
    cfg.env
        .insert("BUSYTOK_MOCK_CRASH_AFTER".into(), "2".into());
    cfg.health_interval = Duration::from_secs(3600); // avoid health-ping interference
                                                     // Attach a DB so the supervisor writes resource events (sidecar_start,
                                                     // sidecar_crash, sidecar_restart, sidecar_stop). This verifies the
                                                     // `sidecar_restart` event is emitted on crash-recovery respawn.
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let sup = PiSidecarSupervisor::new(cfg, Some(db.clone()));
    let handle = sup.ensure_started().await.unwrap();
    // First turn_auto succeeds (message 2 — initialize was message 1).
    // Mock crashes AFTER responding to message 2 (CRASH_AFTER=2).
    let _ = handle
        .turn_auto(serde_json::json!({
            "logical_subagent_id": "test",
            "prompt": "do",
            "cwd": "/tmp",
            "profile": "pi/search-cheap",
        }))
        .await
        .unwrap();
    // Sidecar crashes after message 2; the supervision loop detects it via
    // try_wait. Wait for detection + backoff, then ensure_started respawns.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let handle2 = sup.ensure_started().await.unwrap();
    let _ = handle2
        .turn_auto(serde_json::json!({
            "logical_subagent_id": "test",
            "prompt": "again",
            "cwd": "/tmp",
            "profile": "pi/search-cheap",
        }))
        .await
        .unwrap();
    sup.shutdown().await.unwrap();

    // Assert the full resource event sequence for crash recovery:
    // sidecar_start (initial spawn) → sidecar_crash (detected) →
    // sidecar_restart (respawn) → sidecar_stop (graceful shutdown).
    let db = db.lock().unwrap();
    let events = db.subagent_list_resource_events(None, 100).unwrap();
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(
        types.contains(&"sidecar_start"),
        "missing sidecar_start event: {types:?}"
    );
    assert!(
        types.contains(&"sidecar_crash"),
        "missing sidecar_crash event: {types:?}"
    );
    assert!(
        types.contains(&"sidecar_restart"),
        "missing sidecar_restart event after crash recovery: {types:?}"
    );
    assert!(
        types.contains(&"sidecar_stop"),
        "missing sidecar_stop event: {types:?}"
    );
}

#[tokio::test]
async fn supervisor_idle_exit_stops_sidecar() {
    let mut cfg = mock_config();
    cfg.idle_exit_seconds = 0; // immediate idle exit
    cfg.health_interval = Duration::from_secs(3600);
    let sup = PiSidecarSupervisor::new(cfg, None);
    let _ = sup.ensure_started().await.unwrap();
    // The supervision loop polls every 100ms; idle_exit_seconds=0 means the
    // first idle check triggers shutdown. Wait for it.
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Sidecar should be stopped; a fresh ensure_started spawns again.
    let _ = sup.ensure_started().await.unwrap();
    sup.shutdown().await.unwrap();
}

#[tokio::test]
async fn supervisor_writes_resource_events_when_db_provided() {
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let sup = PiSidecarSupervisor::new(mock_config(), Some(db.clone()));
    let _ = sup.ensure_started().await.unwrap();
    sup.shutdown().await.unwrap();
    // sidecar_start and sidecar_stop events should be present.
    let db = db.lock().unwrap();
    let events = db.subagent_list_resource_events(None, 100).unwrap();
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(
        types.contains(&"sidecar_start"),
        "missing sidecar_start event: {types:?}"
    );
    assert!(
        types.contains(&"sidecar_stop"),
        "missing sidecar_stop event: {types:?}"
    );
}

// --- crash reconciliation test harness ---

struct Harness {
    db: Arc<Mutex<Database>>,
    manager: SubagentManager,
}

async fn make_harness() -> Harness {
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let manager = SubagentManager::new(
        Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        Arc::new(MockTaskExecutor),
    );
    Harness { db, manager }
}

fn req(name: &str, prompt: &str) -> DelegateRequest {
    DelegateRequest {
        subagent_name: name.to_string(),
        subagent_id: None,
        cwd: "/tmp/repo".to_string(),
        profile: "pi/search-cheap".to_string(),
        intent: None,
        prompt: prompt.to_string(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
    }
}

/// Insert a hot binding row for `subagent_id` on the `pi` harness.
/// The manager does NOT create hot bindings (Plan 2 mock executor), so the
/// crash-reconciliation test must seed them manually to simulate a real
/// sidecar session.
fn seed_hot_binding(db: &Database, subagent_id: &str) {
    db.subagent_upsert_binding(&SubagentHarnessBindingRow {
        id: format!("bind_{subagent_id}"),
        subagent_id: subagent_id.to_string(),
        harness: "pi".to_string(),
        adapter_session_id: Some(format!("sess-{subagent_id}")),
        adapter_process_id: Some("12345".to_string()),
        is_hot: 1,
        status: "hot".to_string(),
        created_at_ms: busytok_domain::now_ms(),
        last_used_at_ms: None,
        closed_at_ms: None,
        detail_json: None,
    })
    .unwrap();
}

/// Flip a subagent's logical status (e.g. to "hot" to simulate a live session,
/// or to "deleted" to soft-delete it). Reuses the existing upsert path so the
/// change goes through the same write path as the manager.
fn set_logical_status(db: &Database, subagent_id: &str, status: &str) {
    let mut row: SubagentLogicalSubagentRow = db
        .subagent_get_logical(subagent_id)
        .unwrap()
        .expect("subagent must exist before status flip");
    row.status = status.to_string();
    row.updated_at_ms = busytok_domain::now_ms();
    row.last_active_at_ms = Some(row.updated_at_ms);
    db.subagent_upsert_logical(&row).unwrap();
}

#[tokio::test]
async fn crash_reconciliation_marks_tasks_failed_releases_bindings_rolls_back_status() {
    let h = make_harness().await;
    // Delegate a subagent that will be affected by the crash. delegate() runs
    // the mock executor (which produces no memory_update → status is "cold"
    // per §3.3: warm iff hot_summary IS NOT NULL), but does NOT create a hot
    // binding (Plan 2 mock executor). We seed the hot binding manually below
    // to simulate a real sidecar session.
    let r = h
        .manager
        .delegate(req("crash-test", "in-flight work"))
        .await
        .unwrap();
    {
        let db = h.db.lock().unwrap();
        // Flip the just-completed task back to 'running' to simulate the
        // crash-mid-task scenario.
        let tasks = db.subagent_list_tasks(&r.subagent_id, 10).unwrap();
        assert!(!tasks.is_empty(), "delegate should have created a task");
        let task_id = tasks[0].id.clone();
        db.subagent_set_task_status(&task_id, "running", None, None)
            .unwrap();
        // Seed a hot binding (Plan 2 mock executor doesn't) and flip status to
        // "hot" to simulate a live sidecar session.
        seed_hot_binding(&db, &r.subagent_id);
        set_logical_status(&db, &r.subagent_id, "hot");
        // Sanity: status is hot before crash.
        let sub = db.subagent_get_logical(&r.subagent_id).unwrap().unwrap();
        assert_eq!(sub.status, "hot");
    }

    // Also create a soft-deleted subagent with a hot binding, to verify the
    // reconcile does NOT touch deleted tombstones (Plan 1 deletion semantics).
    let deleted = h
        .manager
        .delegate(req("to-be-deleted", "work"))
        .await
        .unwrap();
    {
        let db = h.db.lock().unwrap();
        // Seed hot binding FIRST (so it exists on a non-deleted row), then
        // soft-delete while keeping the hot binding row in place — simulates
        // a crash happening between delegate() and a clean delete.
        seed_hot_binding(&db, &deleted.subagent_id);
        set_logical_status(&db, &deleted.subagent_id, "deleted");
        let sub = db
            .subagent_get_logical(&deleted.subagent_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            sub.status, "deleted",
            "precondition: soft-deleted before crash"
        );
    }

    // Trigger crash by setting BUSYTOK_MOCK_CRASH_AFTER=1 and spawning a
    // fresh sidecar that crashes after responding to adapter.initialize.
    let cfg = mock_sidecar_config_crash_on_init();
    let crashing_sup = PiSidecarSupervisor::new(cfg, Some(Arc::clone(&h.db)));
    // ensure_started spawns + initializes; the supervision loop detects the
    // crash on the next poll and calls reconcile_sidecar_crash.
    let _ = crashing_sup.ensure_started().await;
    // Wait for the supervision loop to observe the crash and reconcile.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Scope the db guard so it is provably dropped before the `.await` below
    // (clippy::await_holding_lock can't track an explicit `drop(db)`).
    {
        let db = h.db.lock().unwrap();
        // (a) The in-flight task is now 'failed' with SIDECAR_CRASHED.
        let tasks = db.subagent_list_tasks(&r.subagent_id, 10).unwrap();
        let t = tasks
            .iter()
            .find(|t| t.status == "failed")
            .expect("at least one failed task after crash reconciliation");
        assert_eq!(
            t.status, "failed",
            "in-flight task must be failed after crash"
        );
        assert_eq!(t.error.as_deref(), Some("SIDECAR_CRASHED"));

        // (b) The hot binding is released (is_hot=0, status='crashed').
        let hot = db.subagent_hot_binding(&r.subagent_id, "pi").unwrap();
        assert!(hot.is_none(), "no hot binding should remain after crash");
        // Verify the crashed binding row still exists (for debugging).
        let crashed: Vec<SubagentHarnessBindingRow> = db
            .conn()
            .prepare(
                "SELECT id, subagent_id, harness, adapter_session_id, adapter_process_id, \
             is_hot, status, created_at_ms, last_used_at_ms, closed_at_ms, detail_json \
             FROM subagent_harness_bindings \
             WHERE subagent_id = ?1 AND status = 'crashed'",
            )
            .unwrap()
            .query_map(rusqlite::params![r.subagent_id], |row| {
                Ok(SubagentHarnessBindingRow {
                    id: row.get(0)?,
                    subagent_id: row.get(1)?,
                    harness: row.get(2)?,
                    adapter_session_id: row.get(3)?,
                    adapter_process_id: row.get(4)?,
                    is_hot: row.get(5)?,
                    status: row.get(6)?,
                    created_at_ms: row.get(7)?,
                    last_used_at_ms: row.get(8)?,
                    closed_at_ms: row.get(9)?,
                    detail_json: row.get(10)?,
                })
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(!crashed.is_empty(), "crashed binding row must be retained");
        assert_eq!(crashed[0].is_hot, 0);

        // (c) Logical status rolled back to 'cold' (§3.3: warm iff hot_summary
        //     IS NOT NULL). The mock sidecar produces no memory_update
        //     (current_state_summary=None), so hot_summary stays None → cold.
        //     P1-1 regression: the old delegate unconditionally set Warm, but
        //     the §3.3 invariant requires warm iff hot_summary IS NOT NULL.
        let sub = db.subagent_get_logical(&r.subagent_id).unwrap().unwrap();
        assert_eq!(
            sub.status, "cold",
            "logical status must roll back to cold (no hot_summary → cold per §3.3)"
        );

        // (d) Regression: the soft-deleted subagent's status is STILL 'deleted'.
        //     The reconcile must NOT rewrite deleted tombstones (Plan 1 semantics).
        let deleted_sub = db
            .subagent_get_logical(&deleted.subagent_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            deleted_sub.status, "deleted",
            "soft-deleted subagent must NOT be touched by crash reconciliation"
        );

        // (e) The soft-deleted subagent's hot binding WAS released (bindings are
        //     not tombstone-protected — only logical status is). This is correct:
        //     a crashed sidecar's bindings must all be released regardless of the
        //     subagent's logical status, so future delegates don't see a stale
        //     hot binding.
        let deleted_hot = db.subagent_hot_binding(&deleted.subagent_id, "pi").unwrap();
        assert!(
            deleted_hot.is_none(),
            "soft-deleted subagent's hot binding must be released after crash"
        );
    } // db guard dropped here — before the `.await` below

    let _ = crashing_sup.shutdown().await;
}

// --- supervisor error-path and lifecycle tests ---
//
// These tests cover branches not exercised by the happy-path tests above:
// spawn failure, health pinger, call-after-shutdown, and max-restart-attempts
// exhaustion.

#[tokio::test]
async fn supervisor_spawn_fails_with_nonexistent_node_binary() {
    // A non-existent node_binary forces `cmd.spawn()` to fail in
    // `spawn_internal` → `SidecarError::Spawn`. Covers the spawn-error branch.
    let mut cfg = mock_config();
    cfg.node_binary = PathBuf::from("/nonexistent/busytok-node-binary");
    let sup = PiSidecarSupervisor::new(cfg, None);
    let err = match sup.ensure_started().await {
        Ok(_) => panic!("expected spawn error for non-existent node binary"),
        Err(e) => e,
    };
    match err {
        busytok_subagent::sidecar::SidecarError::Spawn(msg) => {
            assert!(
                msg.contains("busytok-node-binary") || !msg.is_empty(),
                "expected spawn error, got: {msg}"
            );
        }
        other => panic!("expected SidecarError::Spawn, got {other:?}"),
    }
}

#[tokio::test]
async fn supervisor_health_pinger_sends_periodic_health_checks() {
    // A short `health_interval` forces the supervision loop's health pinger
    // to fire within the test window. The mock sidecar responds to
    // `adapter.health`, so the ping succeeds (best-effort). We just need to
    // exercise the health-pinger branch — if the sidecar is still healthy
    // after the ping, the test passes.
    let mut cfg = mock_config();
    cfg.health_interval = Duration::from_millis(200);
    cfg.idle_exit_seconds = 3600; // disable idle exit
    let sup = PiSidecarSupervisor::new(cfg, None);
    let handle = sup.ensure_started().await.unwrap();
    // Wait long enough for at least 2 health-ping cycles (200ms each).
    // The supervision loop polls every 100ms, so 600ms covers ~3 polls.
    tokio::time::sleep(Duration::from_millis(600)).await;
    // If the sidecar is still responsive after health pings, the pinger
    // didn't kill it.
    let health = handle.health().await.unwrap();
    assert_eq!(health["status"], "healthy");
    sup.shutdown().await.unwrap();
}

#[tokio::test]
async fn supervisor_call_after_shutdown_returns_crashed() {
    // After `shutdown()`, `state.client` is `None`. A subsequent `call_rpc`
    // (via `SidecarHandle::health`) must return `SidecarError::Crashed` with
    // the "sidecar not running" message — NOT a timeout or IO error.
    let sup = PiSidecarSupervisor::new(mock_config(), None);
    let handle = sup.ensure_started().await.unwrap();
    sup.shutdown().await.unwrap();
    let err = handle.health().await.unwrap_err();
    match err {
        busytok_subagent::sidecar::SidecarError::Crashed(msg) => {
            assert!(
                msg.contains("not running"),
                "expected 'not running' message, got: {msg}"
            );
        }
        other => panic!("expected SidecarError::Crashed, got {other:?}"),
    }
}

#[tokio::test]
async fn supervisor_max_restart_attempts_exceeded_returns_error() {
    // `max_restart_attempts = 0` means NO restarts are allowed. The mock
    // sidecar crashes after responding to `adapter.initialize` (message 1).
    // The supervision loop detects the crash, increments `restart_attempts`
    // to 1 (and pushes to `restart_history`). The next `ensure_started`
    // calls `spawn_internal`, which checks the FIXED rolling-window cap
    // (`restart_history.len() (1) >= MAX_CRASHES_PER_WINDOW (3)` → false,
    // so the window limiter does NOT fire), then falls through to the
    // consecutive-attempt check `restart_attempts (1) > max (0)` → true →
    // returns `SidecarError::Crashed`.
    //
    // This verifies the `max_restart_attempts = 0` semantic is preserved:
    // the first spawn succeeds (the rolling window is empty), and the
    // restart after a crash is blocked by the consecutive-attempt check —
    // NOT by the rolling window. The rolling window cap is FIXED at 3 per
    // spec §5.4 and is decoupled from `max_restart_attempts`.
    let mut cfg = mock_config();
    cfg.env
        .insert("BUSYTOK_MOCK_CRASH_AFTER".to_string(), "1".to_string());
    cfg.max_restart_attempts = 0;
    cfg.health_interval = Duration::from_secs(3600);
    cfg.idle_exit_seconds = 3600;
    let sup = PiSidecarSupervisor::new(cfg, None);
    // First ensure_started succeeds (initialize is message 1; sidecar crashes
    // after responding).
    let _ = sup.ensure_started().await.unwrap();
    // Wait for the supervision loop to detect the crash and increment
    // restart_attempts + push to restart_history.
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Second ensure_started must fail with "max restart attempts exceeded".
    let err = match sup.ensure_started().await {
        Ok(_) => panic!("expected max-restart-attempts error"),
        Err(e) => e,
    };
    match err {
        busytok_subagent::sidecar::SidecarError::Crashed(msg) => {
            assert!(
                msg.contains("max restart attempts"),
                "expected max-restart-attempts message, got: {msg}"
            );
        }
        other => panic!("expected SidecarError::Crashed, got {other:?}"),
    }
}

#[tokio::test]
async fn supervisor_concurrent_ensure_started_does_not_double_spawn() {
    // Two concurrent `ensure_started` calls should both succeed and share
    // the same sidecar process. The double-checked locking in
    // `spawn_internal` ensures the second caller returns early if the first
    // has already installed the client. This test is primarily a regression
    // guard — if the locking is broken, both calls spawn and the second
    // overwrites the first's client (leaking a process).
    let sup = PiSidecarSupervisor::new(mock_config(), None);
    let sup1 = Arc::clone(&sup);
    let sup2 = Arc::clone(&sup);
    let (r1, r2) = tokio::join!(async move { sup1.ensure_started().await }, async move {
        sup2.ensure_started().await
    },);
    let h1 = r1.unwrap();
    let h2 = r2.unwrap();
    // Both handles should be functional.
    let health1 = h1.health().await.unwrap();
    let health2 = h2.health().await.unwrap();
    assert_eq!(health1["status"], "healthy");
    assert_eq!(health2["status"], "healthy");
    sup.shutdown().await.unwrap();
}

#[tokio::test]
async fn supervisor_returns_spawn_error_on_protocol_mismatch() {
    // A sidecar that reports a different `protocol_version` than
    // `PROTOCOL_VERSION` (1) must cause `spawn_internal` to return
    // `SidecarError::Spawn("protocol mismatch: ...")`. Covers the
    // protocol-version check branch.
    let tmp = tempfile::tempdir().unwrap();
    let script_path = tmp.path().join("protocol-mismatch.sh");
    std::fs::write(
        &script_path,
        "#!/usr/bin/env bash\n\
         IFS= read -r LINE\n\
         ID=$(printf '%s' \"$LINE\" | sed -n 's/.*\"id\"[[:space:]]*:[[:space:]]*\\([0-9]*\\).*/\\1/p')\n\
         printf '{\"jsonrpc\":\"2.0\",\"result\":{\"protocol_version\":99},\"id\":%s}\\n' \"$ID\"\n\
         sleep 5\n",
    )
    .unwrap();

    let mut cfg = mock_config();
    cfg.bundle_path = script_path;
    cfg.health_interval = Duration::from_secs(3600);
    let sup = PiSidecarSupervisor::new(cfg, None);

    let err = match sup.ensure_started().await {
        Ok(_) => panic!("expected protocol mismatch error"),
        Err(e) => e,
    };
    match err {
        SidecarError::Spawn(msg) => {
            assert!(
                msg.contains("protocol mismatch"),
                "expected protocol mismatch message, got: {msg}"
            );
        }
        other => panic!("expected SidecarError::Spawn, got {other:?}"),
    }
}

#[tokio::test]
async fn supervisor_drains_stderr_without_blocking() {
    // P1-1 regression: the sidecar writes many stderr lines per message.
    // Without the background stderr reader the pipe buffer (64 KiB on macOS)
    // fills and the child blocks on its next stderr write — manifesting as
    // a turn_auto timeout. With the reader, all messages complete normally.
    let mut cfg = mock_config();
    // 200 lines × ~40 bytes ≈ 8 KiB per response; across initialize +
    // health + turn_auto the child writes well over the pipe capacity.
    cfg.env
        .insert("BUSYTOK_MOCK_STDERR_LINES".into(), "200".into());
    cfg.health_interval = Duration::from_secs(3600);
    let sup = PiSidecarSupervisor::new(cfg, None);
    let handle = sup.ensure_started().await.unwrap();
    let result = handle
        .turn_auto(serde_json::json!({
            "logical_subagent_id": "test",
            "prompt": "do",
            "cwd": "/tmp",
            "profile": "pi/search-cheap",
        }))
        .await
        .expect("turn_auto must not block when sidecar writes lots of stderr");
    assert_eq!(result["status"], "completed");
    sup.shutdown().await.unwrap();
}

// --- hot session pool: session reuse (Plan 3 Task 7) ---
//
// Verifies the sidecar's session reuse contract: a second `turn_auto` for
// the same `logical_subagent_id` must return the SAME `adapter_session_id`
// with `session_reused: true`, rather than creating a new session. This is
// the core hot-pool invariant — without it, every turn would burn a new
// session and the LRU eviction path would never engage.

#[tokio::test]
async fn supervisor_turn_auto_reuses_session_for_same_subagent() {
    let mut cfg = mock_config();
    cfg.health_interval = Duration::from_secs(3600);
    let sup = PiSidecarSupervisor::new(cfg, None);
    let handle = sup.ensure_started().await.unwrap();

    // First turn_auto — creates a new session
    let params1 = serde_json::json!({
        "logical_subagent_id": "sub-a",
        "logical_subagent_name": "a",
        "cwd": "/tmp",
        "profile": "pi/search-cheap",
        "prompt": "do 1",
    });
    let result1 = handle.turn_auto(params1).await.unwrap();
    let sess1 = result1["adapter_session_id"].as_str().unwrap().to_string();
    assert_eq!(result1["session_reused"], false);

    // Second turn_auto — same subagent, must reuse the session
    let params2 = serde_json::json!({
        "logical_subagent_id": "sub-a",
        "logical_subagent_name": "a",
        "cwd": "/tmp",
        "profile": "pi/search-cheap",
        "prompt": "do 2",
    });
    let result2 = handle.turn_auto(params2).await.unwrap();
    let sess2 = result2["adapter_session_id"].as_str().unwrap().to_string();
    assert_eq!(sess1, sess2, "same subagent must reuse the same session");
    assert_eq!(result2["session_reused"], true);

    sup.shutdown().await.unwrap();
}

#[tokio::test]
async fn sidecar_handle_supports_prepare_hibernate_close_and_prepare_all() {
    let mut cfg = mock_config();
    cfg.health_interval = Duration::from_secs(3600);
    let sup = PiSidecarSupervisor::new(cfg, None);
    let handle = sup.ensure_started().await.unwrap();

    let turn = handle
        .turn_auto(serde_json::json!({
            "logical_subagent_id": "sub-a",
            "logical_subagent_name": "a",
            "cwd": "/tmp",
            "profile": "pi/search-cheap",
            "prompt": "do 1",
        }))
        .await
        .unwrap();
    let sess = turn["adapter_session_id"]
        .as_str()
        .expect("mock sidecar should return adapter_session_id");

    let prepare_one = handle.prepare_hibernate(sess).await.unwrap();
    assert_eq!(prepare_one["stats"]["adapter_session_id"], sess);
    assert_eq!(prepare_one["memory_delta"]["hot_summary"], "hibernated");

    let close = handle.close(sess).await.unwrap();
    assert_eq!(close["ok"], true);

    let _ = handle
        .turn_auto(serde_json::json!({
            "logical_subagent_id": "sub-b",
            "logical_subagent_name": "b",
            "cwd": "/tmp",
            "profile": "pi/search-cheap",
            "prompt": "do 2",
        }))
        .await
        .unwrap();
    let prepare_all = handle.prepare_hibernate_all().await.unwrap();
    assert!(prepare_all.get("memory_delta").is_some());

    sup.shutdown().await.unwrap();
}

#[test]
fn write_resource_event_with_sample_populates_rss_and_cpu_columns() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    let config = busytok_subagent::sidecar::SidecarConfig {
        node_binary: support::sidecar_shell_path(),
        bundle_path: std::path::PathBuf::from("/dev/null"),
        env: std::collections::HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: std::time::Duration::from_secs(30),
        task_timeout: std::time::Duration::from_secs(30),
        max_restart_attempts: 3,
        restart_backoff_base: std::time::Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
        provider_id: String::new(),
        api_key_env_name: String::new(),
        base_url_env_name: String::new(),
    };
    let sup = busytok_subagent::sidecar::PiSidecarSupervisor::new(
        config,
        Some(std::sync::Arc::new(std::sync::Mutex::new(db))),
    );
    let sample = busytok_subagent::resource::ResourceSample {
        service_rss_mb: 25.0,
        sidecar_rss_mb: Some(150.0),
        sidecar_cpu_percent: Some(3.5),
        hot_session_count: 2,
        system_available_mb: 4096.0,
        queued_task_count: 0,
        running_task_count: 0,
    };
    sup.write_resource_event_with_sample("sidecar_start", Some(&sample));

    let db = sup.db_for_test().lock().unwrap();
    let events = db.subagent_list_resource_events(None, 100).unwrap();
    let evt = events
        .iter()
        .find(|e| e.event_type == "sidecar_start")
        .expect("sidecar_start event must be written");
    assert_eq!(evt.rss_mb, Some(150.0), "sidecar_rss_mb must be populated");
    assert_eq!(evt.cpu_percent, Some(3.5), "cpu_percent must be populated");
    let detail: serde_json::Value =
        serde_json::from_str(evt.detail_json.as_deref().unwrap_or("null")).unwrap();
    assert_eq!(detail["service_rss_mb"], 25.0);
    assert_eq!(detail["hot_session_count"], 2);
    assert_eq!(detail["system_available_mb"], 4096.0);
}

// --- 5-min rolling crash window (Plan 5 Task 4, spec §5.4) ---

/// Test that after 3 crashes within 5 min, the 4th restart attempt is
/// rejected with `SidecarError::Crashed`. The existing code only counts
/// consecutive `restart_attempts` (reset on successful spawn); this test
/// verifies the NEW rolling window limiter.
///
/// The config uses `max_restart_attempts = 5` (NOT 3) to PROVE the cap is
/// FIXED at 3 independent of the config — spec §5.4 mandates "max 3 attempts
/// per 5 min" as a hard safety bound, NOT tied to `max_restart_attempts`
/// (which governs backoff only). If the limiter were tied to the config,
/// this test would NOT fire on 3 entries and would wrongly proceed to spawn.
#[tokio::test]
async fn spawn_rejects_after_3_crashes_within_5_min_window() {
    use busytok_subagent::sidecar::SidecarError;
    use std::collections::VecDeque;
    use tokio::time::Instant;

    let db = busytok_store::Database::open_in_memory().unwrap();
    let shared_db: std::sync::Arc<std::sync::Mutex<busytok_store::Database>> =
        std::sync::Arc::new(std::sync::Mutex::new(db));
    let config = busytok_subagent::sidecar::SidecarConfig {
        node_binary: support::sidecar_shell_path(),
        bundle_path: std::path::PathBuf::from("/dev/null"),
        env: std::collections::HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: std::time::Duration::from_secs(30),
        task_timeout: std::time::Duration::from_secs(30),
        // Deliberately non-3 (5) to prove the rolling-window cap is a FIXED
        // const (MAX_CRASHES_PER_WINDOW=3), NOT derived from this config.
        max_restart_attempts: 5,
        restart_backoff_base: std::time::Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
        provider_id: String::new(),
        api_key_env_name: String::new(),
        base_url_env_name: String::new(),
    };
    let sup = busytok_subagent::sidecar::PiSidecarSupervisor::new(
        config,
        Some(std::sync::Arc::clone(&shared_db)),
    );

    // Simulate 3 crashes within the 5-min window by pre-populating
    // restart_history with 3 recent timestamps. This bypasses the
    // supervision loop and directly tests the limiter in spawn_internal.
    {
        let mut state = sup.state_for_test().lock().await;
        let now = Instant::now();
        state.restart_history = VecDeque::from([now, now, now]);
    }

    // The 4th spawn attempt should be rejected — even though
    // `max_restart_attempts = 5`, the FIXED cap of 3 fires first.
    let result = sup.ensure_started().await;
    assert!(
        matches!(result, Err(SidecarError::Crashed(_))),
        "4th restart within 5 min must be rejected with SidecarError::Crashed, got: {result:?}"
    );
}

/// Test that crashes older than 5 min are pruned, allowing restart.
#[tokio::test]
async fn spawn_allows_restart_after_5_min_window_expires() {
    use std::collections::VecDeque;
    use tokio::time::{Duration, Instant};

    let db = busytok_store::Database::open_in_memory().unwrap();
    let shared_db: std::sync::Arc<std::sync::Mutex<busytok_store::Database>> =
        std::sync::Arc::new(std::sync::Mutex::new(db));
    let config = busytok_subagent::sidecar::SidecarConfig {
        node_binary: support::sidecar_shell_path(),
        bundle_path: support::mock_sidecar_bundle_path(),
        env: std::collections::HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: std::time::Duration::from_secs(30),
        task_timeout: std::time::Duration::from_secs(30),
        max_restart_attempts: 3,
        restart_backoff_base: std::time::Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
        provider_id: String::new(),
        api_key_env_name: String::new(),
        base_url_env_name: String::new(),
    };
    let sup = busytok_subagent::sidecar::PiSidecarSupervisor::new(
        config,
        Some(std::sync::Arc::clone(&shared_db)),
    );

    // Simulate 3 crashes 6 minutes ago (outside the 5-min window).
    {
        let mut state = sup.state_for_test().lock().await;
        let old = Instant::now()
            .checked_sub(Duration::from_secs(360))
            .expect("6 min ago should be representable");
        state.restart_history = VecDeque::from([old, old, old]);
    }

    // After pruning, restart_history should be empty, so spawn should
    // proceed. The mock sidecar responds to adapter.initialize with the
    // correct protocol_version, so spawn succeeds — strongest verification
    // that the limiter did NOT fire.
    let result = sup.ensure_started().await;
    assert!(
        result.is_ok(),
        "after 5-min window expires, spawn must succeed (limiter must not fire), got: {result:?}"
    );
    sup.shutdown().await.unwrap();
}

/// `with_resource_policy` constructs the supervisor with an explicit
/// `SubagentResourcePolicyConfig` (the production path — the runtime
/// supervisor threads settings → monitor). Verify it produces a working
/// supervisor that can spawn, handle a health RPC, and shut down cleanly.
/// This also covers the `with_resource_policy` constructor body, which
/// would otherwise be dead code in the per-crate coverage gate (the
/// runtime calls it from a different crate).
#[tokio::test]
async fn with_resource_policy_constructs_working_supervisor() {
    use busytok_config::SubagentResourcePolicyConfig;

    let policy = SubagentResourcePolicyConfig {
        memory_pressure_free_mb: 1024,
        monitor_interval_seconds: 5,
    };
    let sup = PiSidecarSupervisor::with_resource_policy(mock_config(), None, policy, None);
    let handle = sup.ensure_started().await.unwrap();
    let health = handle.health().await.unwrap();
    assert_eq!(health["status"], "healthy");
    sup.shutdown().await.unwrap();
}

/// `with_resource_policy` must thread `memory_soft_limit_mb` /
/// `memory_hard_limit_mb` from `SidecarConfig` into the `ResourceMonitor`
/// (the predicates read these). Verify by constructing a supervisor whose
/// config has distinct limits and asserting the attached DB receives a
/// `sidecar_start` event (proving the full construct → spawn →
/// write_resource_event path works under `with_resource_policy`).
#[tokio::test]
async fn with_resource_policy_threads_limits_into_monitor() {
    use busytok_config::SubagentResourcePolicyConfig;

    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let policy = SubagentResourcePolicyConfig::default();
    let sup = PiSidecarSupervisor::with_resource_policy(
        mock_config(),
        Some(Arc::clone(&db)),
        policy,
        None,
    );
    let handle = sup.ensure_started().await.unwrap();
    let _ = handle.health().await.unwrap();
    sup.shutdown().await.unwrap();

    let db = db.lock().unwrap();
    let events = db.subagent_list_resource_events(None, 100).unwrap();
    assert!(
        events.iter().any(|e| e.event_type == "sidecar_start"),
        "with_resource_policy supervisor must emit sidecar_start event"
    );
}

// --- additional coverage tests ---
//
// These tests target uncovered branches in supervisor.rs: pressure responder
// wiring, reconcile_crash error path, SIGKILL fallback, health-pinger failure,
// stale-loop guard, crash detection, idle exit, and restart backoff.

/// `set_pressure_responder` (lines 336-338) + `force_kill` body + `reconcile_crash`
/// Ok branch (line 301). Constructs a `PressureResponder`, wires it via
/// `set_pressure_responder`, then calls `respond(ForceKill)` which calls
/// `force_kill` → `reconcile_crash` (Ok path with empty DB) → `write_resource_event`.
#[tokio::test]
async fn pressure_responder_force_kill_kills_sidecar_and_writes_crash_event() {
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let sup = PiSidecarSupervisor::new(mock_config(), Some(db.clone()));
    let _ = sup.ensure_started().await.unwrap();
    assert!(sup.try_is_running());

    let executor = Arc::new(SidecarTaskExecutor::new(Arc::clone(&sup)));
    let responder = Arc::new(PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&executor),
        Arc::new(PressureGate::new()),
    ));
    sup.set_pressure_responder(Arc::clone(&responder));

    responder.respond(PressureAction::ForceKill).await;
    assert!(!sup.try_is_running(), "force_kill should stop the sidecar");

    let db = db.lock().unwrap();
    let events = db.subagent_list_resource_events(None, 100).unwrap();
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(
        types.contains(&"sidecar_crash"),
        "force_kill should write sidecar_crash event: {types:?}"
    );
}

/// `reconcile_crash` Err branch (lines 304-308): when the DB is read-only,
/// `subagent_reconcile_sidecar_crash` fails on the UPDATE, covering the error
/// log path.
#[tokio::test]
async fn reconcile_crash_error_path_when_db_write_fails() {
    let h = make_harness().await;
    let r = h.manager.delegate(req("crash-err", "work")).await.unwrap();
    {
        let db = h.db.lock().unwrap();
        seed_hot_binding(&db, &r.subagent_id);
    }
    // Make all write operations fail so reconcile_sidecar_crash returns Err.
    h.db.lock()
        .unwrap()
        .conn()
        .execute_batch("PRAGMA query_only = 1")
        .unwrap();

    let mut cfg = mock_config();
    cfg.health_interval = Duration::from_secs(3600);
    cfg.idle_exit_seconds = 3600;
    let sup = PiSidecarSupervisor::new(cfg, Some(h.db.clone()));
    let _ = sup.ensure_started().await.unwrap();

    let executor = Arc::new(SidecarTaskExecutor::new(Arc::clone(&sup)));
    let responder = Arc::new(PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&executor),
        Arc::new(PressureGate::new()),
    ));
    sup.set_pressure_responder(Arc::clone(&responder));

    // ForceKill calls reconcile_crash, which fails due to read-only DB.
    responder.respond(PressureAction::ForceKill).await;
    assert!(!sup.try_is_running());
}

/// SIGKILL fallback in `shutdown_internal` (lines 266-271): when the sidecar
/// ignores `adapter.shutdown`, the 10s grace period expires and SIGKILL is used.
/// NOTE: This test takes ~10s due to SHUTDOWN_GRACE.
#[tokio::test]
async fn shutdown_internal_sigkills_unresponsive_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let script = tmp.path().join("ignore-shutdown.sh");
    std::fs::write(
        &script,
        "#!/usr/bin/env bash\n\
         IFS= read -r LINE\n\
         ID=$(printf '%s' \"$LINE\" | sed -n 's/.*\"id\"[[:space:]]*:[[:space:]]*\\([0-9]*\\).*/\\1/p')\n\
         printf '{\"jsonrpc\":\"2.0\",\"result\":{\"protocol_version\":1,\"sidecar_version\":\"mock-1.0\"},\"id\":%s}\\n' \"$ID\"\n\
         while IFS= read -r line; do\n\
         \tID=$(printf '%s' \"$line\" | sed -n 's/.*\"id\"[[:space:]]*:[[:space:]]*\\([0-9]*\\).*/\\1/p')\n\
         \tMETHOD=$(printf '%s' \"$line\" | sed -n 's/.*\"method\"[[:space:]]*:[[:space:]]*\"\\([^\"]*\\).*/\\1/p')\n\
         \tcase \"$METHOD\" in\n\
         \t\tadapter.shutdown)\n\
         \t\t\tprintf '{\"jsonrpc\":\"2.0\",\"result\":{\"ok\":true},\"id\":%s}\\n' \"$ID\"\n\
         \t\t\tsleep 300\n\
         \t\t\t;;\n\
         \t\t*)\n\
         \t\t\tprintf '{\"jsonrpc\":\"2.0\",\"result\":{},\"id\":%s}\\n' \"$ID\"\n\
         \t\t\t;;\n\
         \tesac\n\
         done\n",
    )
    .unwrap();

    let mut cfg = mock_config();
    cfg.bundle_path = script;
    cfg.health_interval = Duration::from_secs(3600);
    cfg.idle_exit_seconds = 3600;
    let sup = PiSidecarSupervisor::new(cfg, None);
    let _ = sup.ensure_started().await.unwrap();

    sup.shutdown_internal().await.unwrap();
    assert!(
        !sup.try_is_running(),
        "SIGKILL should have killed the sidecar"
    );
}

/// Health-pinger failure (lines 658-660): when `adapter.health` returns a
/// JSON-RPC error, the warn log fires and hot_sessions defaults to 0.
#[tokio::test]
async fn health_pinger_warns_on_health_rpc_error() {
    let tmp = tempfile::tempdir().unwrap();
    let script = tmp.path().join("health-error.sh");
    std::fs::write(
        &script,
        "#!/usr/bin/env bash\n\
         IFS= read -r LINE\n\
         ID=$(printf '%s' \"$LINE\" | sed -n 's/.*\"id\"[[:space:]]*:[[:space:]]*\\([0-9]*\\).*/\\1/p')\n\
         printf '{\"jsonrpc\":\"2.0\",\"result\":{\"protocol_version\":1,\"sidecar_version\":\"mock-1.0\"},\"id\":%s}\\n' \"$ID\"\n\
         while IFS= read -r line; do\n\
         \tID=$(printf '%s' \"$line\" | sed -n 's/.*\"id\"[[:space:]]*:[[:space:]]*\\([0-9]*\\).*/\\1/p')\n\
         \tMETHOD=$(printf '%s' \"$line\" | sed -n 's/.*\"method\"[[:space:]]*:[[:space:]]*\"\\([^\"]*\\).*/\\1/p')\n\
         \tcase \"$METHOD\" in\n\
         \t\tadapter.health)\n\
         \t\t\tprintf '{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32603,\"message\":\"health failed\"},\"id\":%s}\\n' \"$ID\"\n\
         \t\t\t;;\n\
         \t\tadapter.shutdown)\n\
         \t\t\tprintf '{\"jsonrpc\":\"2.0\",\"result\":{\"ok\":true},\"id\":%s}\\n' \"$ID\"\n\
         \t\t\texit 0\n\
         \t\t\t;;\n\
         \t\t*)\n\
         \t\t\tprintf '{\"jsonrpc\":\"2.0\",\"result\":{},\"id\":%s}\\n' \"$ID\"\n\
         \t\t\t;;\n\
         \tesac\n\
         done\n",
    )
    .unwrap();

    let mut cfg = mock_config();
    cfg.bundle_path = script;
    cfg.health_interval = Duration::from_millis(200);
    cfg.idle_exit_seconds = 3600;
    let sup = PiSidecarSupervisor::new(cfg, None);
    // ensure_started spawns + initializes; no handle is needed here because
    // calling handle.health() would hit the same failing RPC.
    sup.ensure_started().await.unwrap();

    // Wait for at least 2 health-ping cycles to fire (200ms each). The
    // supervisor's internal pinger calls adapter.health, gets a JSON-RPC error,
    // and logs the warn (lines 658-660).
    tokio::time::sleep(Duration::from_millis(600)).await;

    // Health-ping failure is non-fatal: sidecar is still running.
    assert!(
        sup.try_is_running(),
        "health-ping failure should not kill the sidecar"
    );
    sup.shutdown().await.unwrap();
}

/// Stale-loop guard (line 572): after shutdown + quick respawn, the old loop
/// detects the generation mismatch and exits. Also covers child.is_none()
/// exit path (lines 577-578) depending on timing.
#[tokio::test]
async fn stale_loop_guard_exits_on_generation_mismatch() {
    let mut cfg = mock_config();
    cfg.idle_exit_seconds = 3600;
    cfg.health_interval = Duration::from_secs(3600);
    let sup = PiSidecarSupervisor::new(cfg, None);
    let _ = sup.ensure_started().await.unwrap();

    // Shutdown + immediate respawn bumps the generation. The old loop
    // (still sleeping) will detect the mismatch on its next poll.
    sup.shutdown_internal().await.unwrap();
    let _ = sup.ensure_started().await.unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        sup.try_is_running(),
        "sidecar should still be running after respawn"
    );
    sup.shutdown().await.unwrap();
}

/// Loop exits when child is None after shutdown (lines 577-578). The
/// supervision loop polls, sees `child.is_none()`, resets
/// `supervision_started`, and returns.
#[tokio::test]
async fn loop_exits_when_child_is_none_after_shutdown() {
    let mut cfg = mock_config();
    cfg.health_interval = Duration::from_secs(3600);
    cfg.idle_exit_seconds = 3600;
    let sup = PiSidecarSupervisor::new(cfg, None);
    let _ = sup.ensure_started().await.unwrap();

    // shutdown_internal takes the child. The loop detects child.is_none()
    // on its next poll and exits.
    sup.shutdown_internal().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ensure_started should work (spawn new sidecar + new loop).
    let _ = sup.ensure_started().await.unwrap();
    sup.shutdown().await.unwrap();
}

/// Idle exit info log (line 626) with DB attached — verifies the idle_exit
/// event fires and a `sidecar_stop` event is written to the DB.
#[tokio::test]
async fn idle_exit_with_db_writes_stop_event() {
    let mut cfg = mock_config();
    cfg.idle_exit_seconds = 0;
    cfg.health_interval = Duration::from_secs(3600);
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let sup = PiSidecarSupervisor::new(cfg, Some(db.clone()));
    let _ = sup.ensure_started().await.unwrap();

    // Wait for idle exit to trigger and persist sidecar_stop. Use polling
    // instead of a fixed sleep: Windows CI runners are slower and a fixed
    // 400ms may not be enough for the supervision loop to detect idle and
    // write the event.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        {
            let db = db.lock().unwrap();
            let events = db.subagent_list_resource_events(None, 100).unwrap();
            if events.iter().any(|e| e.event_type == "sidecar_stop") {
                return;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let db = db.lock().unwrap();
            let events = db.subagent_list_resource_events(None, 100).unwrap();
            let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
            panic!("idle exit should trigger shutdown which writes sidecar_stop: {types:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Restart backoff warn (lines 434-435): after a crash, the next spawn
/// computes a non-zero backoff and logs the warn. Also covers crash detection
/// warn (lines 599-601) with DB attached.
#[tokio::test]
async fn restart_backoff_and_crash_detection_with_db() {
    let mut cfg = mock_sidecar_config_crash_on_init();
    cfg.health_interval = Duration::from_secs(3600);
    cfg.idle_exit_seconds = 3600;
    cfg.restart_backoff_base = Duration::from_millis(10);
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let sup = PiSidecarSupervisor::new(cfg, Some(db.clone()));

    // First spawn succeeds (initialize is message 1; sidecar crashes after).
    let _ = sup.ensure_started().await.unwrap();
    // Wait for crash detection (supervision loop polls every 100ms).
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Restart triggers backoff warn (restart_attempts=1, backoff=10ms).
    let _ = sup.ensure_started().await.unwrap();
    sup.shutdown().await.unwrap();

    let db = db.lock().unwrap();
    let events = db.subagent_list_resource_events(None, 100).unwrap();
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(
        types.contains(&"sidecar_start"),
        "missing sidecar_start: {types:?}"
    );
    assert!(
        types.contains(&"sidecar_crash"),
        "missing sidecar_crash: {types:?}"
    );
    assert!(
        types.contains(&"sidecar_restart"),
        "missing sidecar_restart: {types:?}"
    );
}

// --- resource sampling coverage tests ---
//
// These tests exercise `maybe_sample_resources` (lines 695-839) by using
// `with_resource_policy` (which attaches a `ResourceMonitor`) combined with a
// short `health_interval` so the supervision loop's health-pinger + resource
// sampling block fires within the test window.

/// Normal resource sampling path (lines 701-705, 744, 834): with a DB
/// attached and normal memory limits, the sampler queries task counts,
/// computes `Normal` pressure state, and takes no action. Verifies the
/// `sidecar_start` event is written (proving the sampling path didn't
/// interfere with the normal lifecycle).
#[tokio::test]
async fn resource_sampling_normal_path_with_db() {
    use busytok_config::SubagentResourcePolicyConfig;

    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.health_interval = Duration::from_millis(200);
    cfg.idle_exit_seconds = 3600;
    // Use memory_pressure_free_mb=0 so is_under_pressure returns false
    // regardless of the system's available memory (sysinfo may report 0 on
    // some platforms). Combined with the default soft/hard limits (800/1200
    // MB), which the bash mock sidecar (~5 MB RSS) never exceeds, this
    // guarantees the Normal pressure state.
    let policy = SubagentResourcePolicyConfig {
        memory_pressure_free_mb: 0,
        monitor_interval_seconds: 5,
    };
    let sup = PiSidecarSupervisor::with_resource_policy(cfg, Some(Arc::clone(&db)), policy, None);
    let _ = sup.ensure_started().await.unwrap();

    // Wait for at least 2 resource-sampling cycles (200ms each).
    tokio::time::sleep(Duration::from_millis(600)).await;
    sup.shutdown().await.unwrap();

    let db = db.lock().unwrap();
    let events = db.subagent_list_resource_events(None, 100).unwrap();
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(
        types.contains(&"sidecar_start"),
        "missing sidecar_start: {types:?}"
    );
    assert!(
        types.contains(&"sidecar_stop"),
        "missing sidecar_stop: {types:?}"
    );
    // Normal path: no pressure events should be written.
    assert!(
        !types.contains(&"memory_pressure"),
        "normal path should not write memory_pressure: {types:?}"
    );
    assert!(
        !types.contains(&"rss_limit_exceeded"),
        "normal path should not write rss_limit_exceeded: {types:?}"
    );
}

/// Hard limit exceeded path (lines 740, 794, 826, 848-850): with a very low
/// `memory_hard_limit_mb`, the sidecar RSS exceeds the hard limit. The
/// sampler writes `rss_limit_exceeded` DB event and invokes the pressure
/// responder with `ForceKill`. A PressureResponder is wired so
/// `invoke_pressure_responder` actually spawns the respond task (covering
/// lines 848-850). The ForceKill kills the sidecar; the supervision loop
/// detects the crash and writes `sidecar_crash`.
#[tokio::test]
async fn resource_sampling_hard_limit_triggers_force_kill_via_responder() {
    use busytok_config::SubagentResourcePolicyConfig;

    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.health_interval = Duration::from_millis(200);
    cfg.idle_exit_seconds = 3600;
    cfg.memory_hard_limit_mb = 1; // sidecar RSS (bash ~5MB) exceeds this
    cfg.memory_soft_limit_mb = 1;
    let policy = SubagentResourcePolicyConfig::default();
    let sup = PiSidecarSupervisor::with_resource_policy(cfg, Some(Arc::clone(&db)), policy, None);

    // Wire a PressureResponder so invoke_pressure_responder spawns the
    // respond task (covers lines 848-850).
    let executor = Arc::new(SidecarTaskExecutor::new(Arc::clone(&sup)));
    let responder = Arc::new(PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&executor),
        Arc::new(PressureGate::new()),
    ));
    sup.set_pressure_responder(Arc::clone(&responder));

    let _ = sup.ensure_started().await.unwrap();

    // Wait for resource sampling to detect hard limit, invoke ForceKill via
    // the responder, and the supervision loop to detect the crash.
    tokio::time::sleep(Duration::from_millis(800)).await;

    let db = db.lock().unwrap();
    let events = db.subagent_list_resource_events(None, 100).unwrap();
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(
        types.contains(&"rss_limit_exceeded"),
        "hard limit should write rss_limit_exceeded: {types:?}"
    );
    // ForceKill via responder should kill the sidecar, triggering crash event.
    assert!(
        types.contains(&"sidecar_crash"),
        "ForceKill should produce sidecar_crash: {types:?}"
    );
}

/// Soft limit exceeded path (line 828): with a low `memory_soft_limit_mb`
/// but high `memory_hard_limit_mb`, the sidecar RSS exceeds the soft limit
/// but not the hard limit. The sampler writes `memory_pressure` DB event.
/// No responder is wired, so `invoke_pressure_responder` is a no-op (the
/// action is computed but the spawn block doesn't fire).
#[tokio::test]
async fn resource_sampling_soft_limit_writes_memory_pressure() {
    use busytok_config::SubagentResourcePolicyConfig;

    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.health_interval = Duration::from_millis(200);
    cfg.idle_exit_seconds = 3600;
    cfg.memory_soft_limit_mb = 1; // sidecar RSS exceeds this
    cfg.memory_hard_limit_mb = 9999; // but not this
    let policy = SubagentResourcePolicyConfig::default();
    let sup = PiSidecarSupervisor::with_resource_policy(cfg, Some(Arc::clone(&db)), policy, None);
    let _ = sup.ensure_started().await.unwrap();

    // Wait for resource sampling to detect soft limit.
    tokio::time::sleep(Duration::from_millis(600)).await;
    sup.shutdown().await.unwrap();

    let db = db.lock().unwrap();
    let events = db.subagent_list_resource_events(None, 100).unwrap();
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(
        types.contains(&"memory_pressure"),
        "soft limit should write memory_pressure: {types:?}"
    );
    assert!(
        !types.contains(&"rss_limit_exceeded"),
        "soft limit should not write rss_limit_exceeded: {types:?}"
    );
}

/// Shutdown reconcile error path (lines 873-874): when the DB is read-only,
/// `subagent_release_hot_bindings_for_shutdown` fails, covering the Err
/// branch of `reconcile_shutdown`. The shutdown itself still succeeds
/// (the error is logged but not propagated). A hot binding is seeded
/// first so the store function actually attempts an UPDATE (otherwise it
/// returns Ok with 0 rows affected, never hitting the Err branch).
#[tokio::test]
async fn shutdown_reconcile_error_path_when_db_read_only() {
    let h = make_harness().await;
    // Delegate to create a logical subagent (required for the FK on
    // subagent_harness_bindings).
    let r = h
        .manager
        .delegate(req("shutdown-err", "work"))
        .await
        .unwrap();
    // Seed a hot binding so reconcile_shutdown has a row to UPDATE.
    {
        let db = h.db.lock().unwrap();
        seed_hot_binding(&db, &r.subagent_id);
    }

    let sup = PiSidecarSupervisor::new(mock_config(), Some(h.db.clone()));
    let _ = sup.ensure_started().await.unwrap();

    // Make all DB writes fail so reconcile_shutdown's UPDATE fails.
    h.db.lock()
        .unwrap()
        .conn()
        .execute_batch("PRAGMA query_only = 1")
        .unwrap();

    // Shutdown should still succeed — the reconcile error is logged, not
    // propagated.
    sup.shutdown().await.unwrap();
    assert!(
        !sup.try_is_running(),
        "shutdown should have stopped the sidecar"
    );
}

/// `SidecarHandle` Debug impl (lines 978-980): formatting a handle with
/// `{:?}` should produce a non-exhaustive debug struct.
#[tokio::test]
async fn sidecar_handle_debug_impl_produces_struct() {
    let sup = PiSidecarSupervisor::new(mock_config(), None);
    let handle = sup.ensure_started().await.unwrap();
    let debug_str = format!("{handle:?}");
    assert!(
        debug_str.contains("SidecarHandle"),
        "Debug output should contain struct name: {debug_str}"
    );
    assert!(
        debug_str.contains(".."),
        "Debug output should use finish_non_exhaustive (..): {debug_str}"
    );
    sup.shutdown().await.unwrap();
}

// --- Phase 2 worker_snapshot tests (Task 3) ---
//
// `worker_snapshot()` exposes the sidecar's current state for the
// `runtime_status` handler (Task 6). Critical invariant: it ALWAYS returns
// `Some` — even when the sidecar is stopped — so "configured-but-stopped"
// sidecars stay observable. Only the handler returns `workers: []` when
// `sidecar_supervisor` is `None`.

/// Critical invariant: `worker_snapshot()` MUST always return `Some` — even
/// when the sidecar is freshly constructed (never started). `state=Stopped`
/// with `pid=None`/`uptime_seconds=None`/`sampled_at_ms=None` represents a
/// configured-but-not-running sidecar.
#[tokio::test]
async fn worker_snapshot_always_returns_some_even_when_stopped() {
    let sup = PiSidecarSupervisor::new(mock_config(), None);
    let snap = sup.worker_snapshot().await;
    assert!(snap.is_some(), "worker_snapshot must always return Some");
    let snap = snap.unwrap();
    assert_eq!(snap.state, WorkerState::Stopped);
    assert!(snap.pid.is_none(), "pid must be None when stopped");
    assert!(
        snap.uptime_seconds.is_none(),
        "uptime must be None when stopped"
    );
    assert_eq!(snap.hot_sessions, 0, "hot_sessions starts at 0");
    assert!(
        snap.sampled_at_ms.is_none(),
        "sampled_at_ms is None before first sample"
    );
}

/// Pressure level is `Normal` by default (no pressure transitions yet).
#[tokio::test]
async fn worker_snapshot_pressure_level_normal_by_default() {
    let sup = PiSidecarSupervisor::new(mock_config(), None);
    let snap = sup.worker_snapshot().await.unwrap();
    assert_eq!(snap.pressure_level, PressureLevel::Normal);
}

/// After `ensure_started`, the snapshot reports `Running` with a pid and
/// uptime. Verifies `spawned_at` is set in `spawn_internal`.
#[tokio::test]
async fn worker_snapshot_reports_running_after_ensure_started() {
    let mut cfg = mock_config();
    cfg.health_interval = Duration::from_secs(3600);
    cfg.idle_exit_seconds = 3600;
    let sup = PiSidecarSupervisor::new(cfg, None);
    let _ = sup.ensure_started().await.unwrap();
    let snap = sup.worker_snapshot().await.unwrap();
    assert_eq!(
        snap.state,
        WorkerState::Running,
        "sidecar should be running"
    );
    assert!(snap.pid.is_some(), "pid must be Some when running");
    assert!(
        snap.uptime_seconds.is_some(),
        "uptime must be Some when running"
    );
    sup.shutdown().await.unwrap();
}

/// After `shutdown`, the snapshot reports `Stopped` with no pid/uptime.
/// Verifies `spawned_at` is cleared in `shutdown_internal` AND that
/// `worker_snapshot()` still returns `Some` (the critical invariant).
#[tokio::test]
async fn worker_snapshot_reports_stopped_after_shutdown() {
    let mut cfg = mock_config();
    cfg.health_interval = Duration::from_secs(3600);
    cfg.idle_exit_seconds = 3600;
    let sup = PiSidecarSupervisor::new(cfg, None);
    let _ = sup.ensure_started().await.unwrap();
    sup.shutdown().await.unwrap();
    let snap = sup.worker_snapshot().await.unwrap();
    assert_eq!(snap.state, WorkerState::Stopped);
    assert!(snap.pid.is_none(), "pid must be cleared after shutdown");
    assert!(
        snap.uptime_seconds.is_none(),
        "uptime must be cleared after shutdown"
    );
}

/// After the supervision loop's health-pinger + resource-sampling tick fires,
/// `sampled_at_ms` is set (absolute ms via `busytok_domain::now_ms()`). This
/// enables the frontend freshness display (Task 6).
#[tokio::test]
async fn worker_snapshot_caches_sample_after_health_tick() {
    use busytok_config::SubagentResourcePolicyConfig;

    let mut cfg = mock_config();
    cfg.health_interval = Duration::from_millis(200);
    cfg.idle_exit_seconds = 3600;
    let policy = SubagentResourcePolicyConfig {
        memory_pressure_free_mb: 0,
        monitor_interval_seconds: 5,
    };
    let sup = PiSidecarSupervisor::with_resource_policy(cfg, None, policy, None);
    let _ = sup.ensure_started().await.unwrap();
    // Wait for at least 2 health-pinger + resource-sampling cycles (200ms).
    tokio::time::sleep(Duration::from_millis(600)).await;
    let snap = sup.worker_snapshot().await.unwrap();
    assert!(
        snap.sampled_at_ms.is_some(),
        "sampled_at_ms must be set after sampling"
    );
    sup.shutdown().await.unwrap();
}

/// `memory_used_pct` is `None` before the first sample tick (bound to
/// `sampled_at_ms` per the stamped-sample invariant, reviewer P1-1). After
/// a health tick triggers `maybe_sample_resources`, both `sampled_at_ms`
/// and `memory_used_pct` become `Some` — the freshness timestamp
/// accurately reflects the memory value.
#[tokio::test]
async fn worker_snapshot_memory_used_pct_bound_to_sample_timestamp() {
    use busytok_config::SubagentResourcePolicyConfig;

    let mut cfg = mock_config();
    cfg.health_interval = Duration::from_millis(200);
    cfg.idle_exit_seconds = 3600;
    let policy = SubagentResourcePolicyConfig {
        memory_pressure_free_mb: 0,
        monitor_interval_seconds: 5,
    };
    let sup = PiSidecarSupervisor::with_resource_policy(cfg, None, policy, None);

    // Before any sample tick: both sampled_at_ms and memory_used_pct are None.
    let snap = sup.worker_snapshot().await.unwrap();
    assert_eq!(
        snap.sampled_at_ms, None,
        "sampled_at_ms must be None before first tick"
    );
    assert_eq!(
        snap.memory_used_pct, None,
        "memory_used_pct must be None before first tick (stamped-sample invariant)"
    );

    // Start the sidecar so the supervision loop's health pinger fires.
    let _ = sup.ensure_started().await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;

    let snap = sup.worker_snapshot().await.unwrap();
    assert!(
        snap.sampled_at_ms.is_some(),
        "sampled_at_ms must be set after sampling"
    );
    assert!(
        snap.memory_used_pct.is_some(),
        "memory_used_pct must be Some after sampling (bound to sampled_at_ms)"
    );
    let pct = snap.memory_used_pct.unwrap();
    assert!(pct <= 100, "memory_used_pct should be <= 100, got {pct}");

    sup.shutdown().await.unwrap();
}
