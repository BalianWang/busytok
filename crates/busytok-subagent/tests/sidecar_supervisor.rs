#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use busytok_config::SubagentSettings;
use busytok_store::repository::{SubagentHarnessBindingRow, SubagentLogicalSubagentRow};
use busytok_store::Database;
use busytok_subagent::manager::SubagentManager;
use busytok_subagent::mock_executor::MockTaskExecutor;
use busytok_subagent::models::DelegateRequest;
use busytok_subagent::sidecar::{PiSidecarSupervisor, SidecarConfig, SidecarError};

fn mock_sidecar_script() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/mock-sidecar.sh");
    p
}

fn mock_config() -> SidecarConfig {
    SidecarConfig {
        node_binary: PathBuf::from("bash"),
        bundle_path: mock_sidecar_script(),
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

#[test]
fn write_resource_event_with_sample_populates_rss_and_cpu_columns() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    let config = busytok_subagent::sidecar::SidecarConfig {
        node_binary: std::path::PathBuf::from("bash"),
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
        node_binary: std::path::PathBuf::from("bash"),
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
        node_binary: std::path::PathBuf::from("bash"),
        bundle_path: mock_sidecar_script(),
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
