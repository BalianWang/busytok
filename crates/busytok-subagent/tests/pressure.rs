#![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
use busytok_subagent::pressure::{PressureAction, PressureGate};

#[test]
fn gate_starts_unpaused_with_resume_action() {
    let gate = PressureGate::new();
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
}

#[test]
fn pause_new_tasks_sets_paused_flag() {
    let gate = PressureGate::new();
    gate.set_action(PressureAction::PauseNewTasks);
    assert!(gate.is_paused());
    assert!(matches!(
        gate.last_action(),
        Some(PressureAction::PauseNewTasks)
    ));
}

#[test]
fn resume_clears_paused_flag() {
    let gate = PressureGate::new();
    gate.set_action(PressureAction::PauseNewTasks);
    assert!(gate.is_paused());
    gate.set_action(PressureAction::Resume);
    assert!(!gate.is_paused());
}

#[test]
fn hibernate_lru_does_not_pause() {
    let gate = PressureGate::new();
    gate.set_action(PressureAction::HibernateLru);
    assert!(!gate.is_paused());
}

#[test]
fn force_kill_sets_paused() {
    let gate = PressureGate::new();
    gate.set_action(PressureAction::ForceKill);
    assert!(gate.is_paused());
}

#[test]
fn graceful_restart_sets_paused() {
    let gate = PressureGate::new();
    gate.set_action(PressureAction::GracefulRestart);
    assert!(gate.is_paused());
}

// --- evict_lru LRU picker (spec §8.3 step 1) ---
//
// Verifies `find_lru_hot_binding` returns the oldest hot binding by
// `last_used_at_ms`. The full `evict_lru` flow (which calls the sidecar via
// `evict_session`) is covered by the pressure-response e2e test in
// subagent_e2e_sidecar.rs. This unit test verifies the LRU picker query only
// — it cannot run the sidecar (no mock-sidecar.sh in this unit-test context).

#[tokio::test]
async fn evict_lru_hibernates_oldest_hot_binding() {
    use busytok_store::repository::{SubagentHarnessBindingRow, SubagentLogicalSubagentRow};
    use busytok_store::Database;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use busytok_subagent::sidecar::{PiSidecarSupervisor, SidecarConfig, SidecarTaskExecutor};

    let db = Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    let config = SidecarConfig {
        node_binary: std::path::PathBuf::from("/usr/bin/true"),
        bundle_path: std::path::PathBuf::from("/dev/null"),
        env: HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(30),
        task_timeout: Duration::from_secs(300),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
    };
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let _exec = Arc::new(SidecarTaskExecutor::with_db(
        Arc::clone(&sup),
        Arc::clone(&db),
    ));

    // Seed 2 subagents with hot bindings, oldest first.
    {
        let db_guard = db.lock().unwrap();
        for name in ["old-sub", "new-sub"] {
            let sub_id = format!("sub-{name}");
            db_guard
                .subagent_upsert_logical(&SubagentLogicalSubagentRow {
                    id: sub_id.clone(),
                    name: name.to_string(),
                    project_id: "p".into(),
                    repo_path: "/r".into(),
                    repo_hash: "h".into(),
                    branch: None,
                    intent: None,
                    default_profile: "pi/search-cheap".into(),
                    default_model: None,
                    status: "warm".into(),
                    created_at_ms: 0,
                    updated_at_ms: 0,
                    last_active_at_ms: Some(0),
                })
                .unwrap();
        }
        // old-sub binding: last_used_at_ms = 1000
        db_guard
            .subagent_commit_hot_binding_and_status(
                &SubagentHarnessBindingRow {
                    id: "bind-old".to_string(),
                    subagent_id: "sub-old-sub".to_string(),
                    harness: "pi".to_string(),
                    adapter_session_id: Some("sess-old".to_string()),
                    adapter_process_id: None,
                    is_hot: 1,
                    status: "hot".to_string(),
                    created_at_ms: 0,
                    last_used_at_ms: Some(1000),
                    closed_at_ms: None,
                    detail_json: None,
                },
                "sub-old-sub",
            )
            .unwrap();
        // new-sub binding: last_used_at_ms = 2000
        db_guard
            .subagent_commit_hot_binding_and_status(
                &SubagentHarnessBindingRow {
                    id: "bind-new".to_string(),
                    subagent_id: "sub-new-sub".to_string(),
                    harness: "pi".to_string(),
                    adapter_session_id: Some("sess-new".to_string()),
                    adapter_process_id: None,
                    is_hot: 1,
                    status: "hot".to_string(),
                    created_at_ms: 0,
                    last_used_at_ms: Some(2000),
                    closed_at_ms: None,
                    detail_json: None,
                },
                "sub-new-sub",
            )
            .unwrap();
    }

    // LRU picker must return the oldest binding (last_used_at_ms=1000).
    let lru = db
        .lock()
        .unwrap()
        .subagent_find_lru_hot_binding("pi")
        .unwrap();
    assert!(lru.is_some(), "must find an LRU binding");
    let lru = lru.unwrap();
    assert_eq!(
        lru.id, "bind-old",
        "LRU must be the oldest binding (last_used_at_ms=1000)"
    );
}
