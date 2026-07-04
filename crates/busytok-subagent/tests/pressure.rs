#![allow(
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    clippy::type_complexity
)]
use busytok_config::SubagentResourcePolicyConfig;
use busytok_store::Database;
use busytok_subagent::pressure::{PressureAction, PressureGate, PressureResponder};
use busytok_subagent::sidecar::{
    PiSidecarSupervisor, ProviderRuntimeEntry, ResponderFactory, SidecarConfig,
    SidecarTaskExecutor, WorkerPool,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

#[path = "support/mod.rs"]
mod support;

/// The test provider ID used by pool-based tests.
const TEST_PROVIDER_ID: &str = "test-prov";

/// Build the provider runtime entries map (Task 7: replaces the old
/// `ProviderLookup` + `CredentialReader` closures).
fn make_providers() -> HashMap<String, ProviderRuntimeEntry> {
    let mut map = HashMap::new();
    map.insert(
        TEST_PROVIDER_ID.to_string(),
        ProviderRuntimeEntry {
            provider_id: TEST_PROVIDER_ID.to_string(),
            api_key: "test-key".to_string(),
            base_url: "https://test.example.com/v1".to_string(),
        },
    );
    map
}

/// Build a responder factory that keeps responders alive in a shared holder
/// (mirrors production wiring).
fn make_responder_factory(
    gate: Arc<PressureGate>,
    executor_weak: Weak<SidecarTaskExecutor>,
) -> (ResponderFactory, Arc<Mutex<Vec<Arc<PressureResponder>>>>) {
    let holder: Arc<Mutex<Vec<Arc<PressureResponder>>>> = Arc::new(Mutex::new(Vec::new()));
    let holder_for_closure = Arc::clone(&holder);
    let factory: ResponderFactory = Arc::new(
        move |sup_weak: Weak<PiSidecarSupervisor>| -> Arc<PressureResponder> {
            let responder = Arc::new(PressureResponder::new(
                sup_weak,
                executor_weak.clone(),
                Arc::clone(&gate),
            ));
            holder_for_closure
                .lock()
                .unwrap()
                .push(Arc::clone(&responder));
            responder
        },
    );
    (factory, holder)
}

/// Build a pool + executor + supervisor from the given config + DB.
/// The executor is strong-owned by the caller so the `Weak<SidecarTaskExecutor>`
/// in each responder stays upgradeable (mirrors production wiring).
fn make_pool_executor_sup(
    config: SidecarConfig,
    db: Arc<Mutex<Database>>,
) -> (
    Arc<WorkerPool>,
    Arc<SidecarTaskExecutor>,
    Arc<PiSidecarSupervisor>,
    Arc<Mutex<Vec<Arc<PressureResponder>>>>,
) {
    let gate = Arc::new(PressureGate::new());
    let pool = Arc::new(WorkerPool::new(
        config,
        Some(Arc::clone(&db)),
        make_providers(),
        Some(Arc::clone(&gate)),
        SubagentResourcePolicyConfig::default(),
    ));
    let executor = Arc::new(SidecarTaskExecutor::with_pool(
        Arc::clone(&pool),
        Some(Arc::clone(&db)),
    ));
    let (factory, holder) = make_responder_factory(Arc::clone(&gate), Arc::downgrade(&executor));
    pool.set_responder_factory(factory);
    let supervisor = pool
        .ensure_worker(TEST_PROVIDER_ID)
        .expect("ensure_worker test-prov");
    (pool, executor, supervisor, holder)
}

/// Build a dummy executor (pool with `/usr/bin/true` config) for tests that
/// construct a supervisor directly with a custom policy. The executor just
/// needs to exist so the `Weak<SidecarTaskExecutor>` in the responder stays
/// upgradeable — `execute()` / `evict_lru()` are never called on it.
fn make_dummy_executor(db: Arc<Mutex<Database>>) -> Arc<SidecarTaskExecutor> {
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
    let (_pool, executor, _sup, _holder) = make_pool_executor_sup(config, db);
    executor
}

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

    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
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
    let _sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let _exec = make_dummy_executor(Arc::clone(&db));

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
                    bound_provider_id: "test-provider".into(),
                    bound_model_id: "test-model".into(),
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

// --- PressureResponder::respond() branch coverage (spec §8.3) ---
//
// These tests construct a real `PressureResponder` with a mock `SidecarConfig`
// (`/usr/bin/true` as node_binary) and exercise each `respond()` branch.
// `ensure_started` fails fast (child exits → stdout EOF → Crashed error);
// `shutdown_internal` and `force_kill` are no-ops when no child is running.

/// Build a mock config + in-memory DB (shared setup for responder tests).
fn mock_config_and_db() -> (SidecarConfig, Arc<Mutex<Database>>) {
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
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
    (config, db)
}

fn live_sidecar_config_and_db() -> (SidecarConfig, Arc<Mutex<Database>>) {
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let config = SidecarConfig {
        node_binary: support::sidecar_shell_path(),
        bundle_path: support::mock_sidecar_bundle_path(),
        env: HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(3600),
        task_timeout: Duration::from_secs(5),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_millis(10),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
    };
    (config, db)
}

fn missing_node_config_and_db() -> (SidecarConfig, Arc<Mutex<Database>>) {
    let (mut config, db) = mock_config_and_db();
    config.node_binary = std::path::PathBuf::from("/definitely/missing/binary");
    (config, db)
}

/// Build a responder + gate + strong Arcs (supervisor, executor) for tests
/// that keep both alive. Returns the strong Arcs so the caller can drop them
/// (for the "dropped" branch tests).
fn mock_responder() -> (
    Arc<PressureGate>,
    PressureResponder,
    Arc<PiSidecarSupervisor>,
    Arc<SidecarTaskExecutor>,
) {
    let (config, db) = mock_config_and_db();
    let (_pool, exec, sup, _holder) = make_pool_executor_sup(config, db);
    let gate = Arc::new(PressureGate::new());
    let responder = PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&exec),
        Arc::clone(&gate),
    );
    (gate, responder, sup, exec)
}

#[tokio::test]
async fn respond_resume_clears_pause() {
    let (gate, responder, _sup, _exec) = mock_responder();
    gate.set_action(PressureAction::PauseNewTasks);
    assert!(gate.is_paused());
    responder.respond(PressureAction::Resume).await;
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
}

#[tokio::test]
async fn respond_hibernate_lru_does_not_pause() {
    let (gate, responder, _sup, _exec) = mock_responder();
    // evict_lru is a no-op on an empty DB.
    responder.respond(PressureAction::HibernateLru).await;
    assert!(!gate.is_paused());
    assert!(matches!(
        gate.last_action(),
        Some(PressureAction::HibernateLru)
    ));
}

#[tokio::test]
async fn respond_hibernate_lru_failure_still_sets_action() {
    let (config, db) = mock_config_and_db();
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = make_dummy_executor(Arc::clone(&db));
    let gate = Arc::new(PressureGate::new());
    let responder = PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&exec),
        Arc::clone(&gate),
    );

    responder.respond(PressureAction::HibernateLru).await;

    assert!(!gate.is_paused());
    assert!(matches!(
        gate.last_action(),
        Some(PressureAction::HibernateLru)
    ));
}

#[tokio::test]
async fn respond_pause_new_tasks_sets_pause() {
    let (gate, responder, _sup, _exec) = mock_responder();
    responder.respond(PressureAction::PauseNewTasks).await;
    assert!(gate.is_paused());
    assert!(matches!(
        gate.last_action(),
        Some(PressureAction::PauseNewTasks)
    ));
}

#[tokio::test]
async fn respond_force_kill_clears_gate() {
    let (gate, responder, _sup, _exec) = mock_responder();
    // force_kill is a no-op when no child is running (reconcile_crash +
    // write_resource_event only). Gate is cleared to Resume afterward.
    responder.respond(PressureAction::ForceKill).await;
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
}

#[tokio::test]
async fn respond_force_kill_with_live_sidecar_writes_crash_event() {
    let (config, db) = live_sidecar_config_and_db();
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = make_dummy_executor(Arc::clone(&db));
    let gate = Arc::new(PressureGate::new());
    let responder = PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&exec),
        Arc::clone(&gate),
    );

    let _ = sup.ensure_started().await.unwrap();
    assert!(
        sup.try_is_running(),
        "precondition: sidecar should be running"
    );

    responder.respond(PressureAction::ForceKill).await;

    assert!(
        !sup.try_is_running(),
        "force kill should leave the sidecar stopped"
    );
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));

    let db = db.lock().unwrap();
    let events = db.subagent_list_resource_events(None, 100).unwrap();
    assert!(
        events.iter().any(|e| e.event_type == "sidecar_crash"),
        "force-kill path must persist a sidecar_crash resource event"
    );
}

#[tokio::test]
async fn respond_graceful_restart_clears_gate() {
    let (gate, responder, _sup, _exec) = mock_responder();
    // ensure_started fails (mock binary exits → protocol init fails), logs
    // warning but does NOT return early. shutdown_internal is a no-op (no
    // child stored). Gate is cleared to Resume.
    responder.respond(PressureAction::GracefulRestart).await;
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
}

#[tokio::test]
async fn respond_graceful_restart_with_live_sidecar_restarts_cleanly() {
    let (config, db) = live_sidecar_config_and_db();
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = make_dummy_executor(Arc::clone(&db));
    let gate = Arc::new(PressureGate::new());
    let responder = PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&exec),
        Arc::clone(&gate),
    );

    let _ = sup.ensure_started().await.unwrap();
    assert!(
        sup.try_is_running(),
        "precondition: sidecar should be running"
    );

    responder.respond(PressureAction::GracefulRestart).await;

    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
    assert!(
        !sup.try_is_running(),
        "graceful restart path should stop the current sidecar before lazy restart"
    );

    let handle = sup.ensure_started().await.unwrap();
    let health = handle.health().await.unwrap();
    assert_eq!(health["status"], "healthy");
}

#[tokio::test]
async fn respond_graceful_restart_when_spawn_fails_still_resumes() {
    let (config, db) = missing_node_config_and_db();
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = make_dummy_executor(Arc::clone(&db));
    let gate = Arc::new(PressureGate::new());
    let responder = PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&exec),
        Arc::clone(&gate),
    );

    responder.respond(PressureAction::GracefulRestart).await;

    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
}

#[tokio::test]
async fn respond_both_dropped_is_noop() {
    let (gate, responder, sup, exec) = mock_responder();
    // Drop both strong Arcs — both weak refs fail to upgrade. The supervisor
    // check fires first (returns early). Gate must be unchanged (Resume).
    drop(sup);
    drop(exec);
    assert!(!gate.is_paused());
    responder.respond(PressureAction::ForceKill).await;
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
}

#[tokio::test]
async fn respond_executor_dropped_is_noop() {
    // Supervisor alive, executor dropped: the executor weak fails to upgrade
    // while the supervisor weak still succeeds. respond(HibernateLru) returns
    // early at the executor check, leaving the gate unchanged (Resume).
    let (config, db) = mock_config_and_db();
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = make_dummy_executor(Arc::clone(&db));
    let gate = Arc::new(PressureGate::new());
    let sup_weak = Arc::downgrade(&sup);
    let exec_weak = Arc::downgrade(&exec);
    drop(exec);
    let responder = PressureResponder::new(sup_weak, exec_weak, Arc::clone(&gate));
    responder.respond(PressureAction::HibernateLru).await;
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
}

#[tokio::test]
async fn respond_supervisor_dropped_is_noop() {
    let (dead_config, dead_db) = mock_config_and_db();
    let dead_sup = PiSidecarSupervisor::new(dead_config, Some(dead_db));
    let dead_sup_weak = Arc::downgrade(&dead_sup);
    drop(dead_sup);

    let (_live_config, live_db) = mock_config_and_db();
    let live_exec = make_dummy_executor(Arc::clone(&live_db));

    let gate = Arc::new(PressureGate::new());
    let responder =
        PressureResponder::new(dead_sup_weak, Arc::downgrade(&live_exec), Arc::clone(&gate));
    responder.respond(PressureAction::ForceKill).await;

    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
}

#[tokio::test]
async fn responder_accessors_return_alive_refs() {
    let (gate, responder, sup, exec) = mock_responder();
    // gate() returns the same Arc.
    assert!(Arc::ptr_eq(responder.gate(), &gate));
    // supervisor() / executor() upgrade successfully while strong Arcs alive.
    assert!(responder.supervisor().is_some());
    assert!(responder.executor().is_some());
    assert!(Arc::ptr_eq(&responder.supervisor().unwrap(), &sup));
    assert!(Arc::ptr_eq(&responder.executor().unwrap(), &exec));
    // Default impl == new().
    let default_gate = PressureGate::default();
    assert!(!default_gate.is_paused());
}
