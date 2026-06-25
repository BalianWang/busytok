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
use busytok_subagent::sidecar::{PiSidecarSupervisor, SidecarConfig};

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
    // the mock executor (which writes memory and sets status to "warm"), but
    // does NOT create a hot binding (Plan 2 mock executor). We seed the hot
    // binding manually below to simulate a real sidecar session.
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

        // (c) Logical status rolled back to 'warm' (memory was written by mock
        //     executor during delegate, so the warm invariant holds).
        let sub = db.subagent_get_logical(&r.subagent_id).unwrap().unwrap();
        assert_eq!(
            sub.status, "warm",
            "logical status must roll back to warm (memory exists)"
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
