#![allow(clippy::unwrap_used, clippy::type_complexity, dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

#[path = "support/mod.rs"]
mod support;

use busytok_config::{SubagentResourcePolicyConfig, SubagentSettings};
use busytok_store::repository::{
    SubagentHarnessBindingRow, SubagentLogicalSubagentRow, SubagentMemoryRow,
};
use busytok_store::Database;
use busytok_subagent::context::{CompactContext, MemorySnapshot};
use busytok_subagent::error::SubagentError;
use busytok_subagent::mock_executor::{ExecutorInput, TaskExecutor};
use busytok_subagent::models::{DelegateRequest, TaskStatus};
use busytok_subagent::pressure::{PressureGate, PressureResponder};
use busytok_subagent::sidecar::config::SidecarConfig;
use busytok_subagent::sidecar::executor::SidecarTaskExecutor;
use busytok_subagent::sidecar::{
    PiSidecarSupervisor, ProviderRuntimeEntry, ResponderFactory, WorkerPool,
};
use busytok_subagent::SubagentManager;

type SharedDb = Arc<std::sync::Mutex<Database>>;

/// The test provider ID used by all pool-based tests.
const TEST_PROVIDER_ID: &str = "test-prov";

/// Wait for a task to reach a terminal status (completed/failed/cancelled).
/// Polls the DB every 5ms with a 5-second timeout. Bridges the async
/// execution gap: `delegate()` returns `Running` immediately and spawns
/// execution in the background (Bug #1/#2 fix).
async fn await_task_done(m: &std::sync::Arc<SubagentManager>, task_id: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(status) = m.task_status(task_id).unwrap() {
            if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
                return status;
            }
        }
        if std::time::Instant::now() > deadline {
            panic!("task {task_id} did not reach terminal status within 5s");
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

fn empty_memory_snapshot() -> MemorySnapshot {
    MemorySnapshot {
        hot_summary: None,
        long_summary: None,
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![],
    }
}

fn empty_compact_context() -> CompactContext {
    CompactContext {
        compact_context: String::new(),
        budget_tokens: 0,
        source: String::new(),
    }
}

fn mock_sidecar_config_with_env(env: HashMap<String, String>) -> SidecarConfig {
    SidecarConfig {
        node_binary: support::sidecar_shell_path(),
        bundle_path: support::mock_sidecar_bundle_path(),
        env,
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(3600),
        task_timeout: Duration::from_secs(10),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_millis(10),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
    }
}

fn mock_config() -> SidecarConfig {
    mock_sidecar_config_with_env(HashMap::new())
}

// --- Pool-based test helpers (Phase 3 Task 3) ---
//
// `SidecarTaskExecutor` now holds `Arc<WorkerPool>` instead of
// `Arc<PiSidecarSupervisor>`. These helpers construct a pool with a single
// "test-prov" provider, wire the two-phase bootstrap (pool → executor →
// factory → pool), and return the supervisor via `ensure_worker`.

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

/// Build a fully-wired test pool + executor + supervisor for the mock
/// sidecar. The pool is configured with a single `TEST_PROVIDER_ID`
/// provider and the given base config (env vars preserved).
///
/// The DB is threaded to BOTH the pool (for supervisors) and the executor
/// (for eviction persistence). Returns `(pool, executor, supervisor,
/// responder_holder)` — the executor must be kept alive by the caller so
/// the `Weak<SidecarTaskExecutor>` in each responder stays upgradeable.
fn make_pool_with_config(
    config: SidecarConfig,
    db: Option<SharedDb>,
) -> (
    Arc<WorkerPool>,
    Arc<SidecarTaskExecutor>,
    Arc<PiSidecarSupervisor>,
    Arc<Mutex<Vec<Arc<PressureResponder>>>>,
) {
    let gate = Arc::new(PressureGate::new());
    let pool = Arc::new(WorkerPool::new(
        config,
        db.clone(),
        make_providers(),
        Some(Arc::clone(&gate)),
        SubagentResourcePolicyConfig::default(),
    ));
    let executor = Arc::new(SidecarTaskExecutor::with_pool(Arc::clone(&pool), db));
    let (factory, holder) = make_responder_factory(Arc::clone(&gate), Arc::downgrade(&executor));
    pool.set_responder_factory(factory);
    let supervisor = pool
        .ensure_worker(TEST_PROVIDER_ID)
        .expect("ensure_worker test-prov");
    (pool, executor, supervisor, holder)
}

/// Build default `SubagentSettings`. Task 3 removed `provider_id` / `model`
/// from `SubagentProfileConfig` — provider/model binding is now per-subagent
/// via `bound_provider_id` / `bound_model_id` on the delegate request (see
/// `make_delegate_request`). Default settings are sufficient because the
/// `SubagentManager::execute_task` reads `provider_id` from
/// `subagent.bound_provider_id`, not from the profile config.
fn settings_with_test_provider() -> SubagentSettings {
    SubagentSettings::default()
}

struct TestHarness {
    manager: std::sync::Arc<SubagentManager>,
    db: SharedDb,
    pool: Arc<WorkerPool>,
    supervisor: Arc<PiSidecarSupervisor>,
    /// Keep the executor alive so the `Weak<SidecarTaskExecutor>` in each
    /// responder stays upgradeable (mirrors production wiring where
    /// `BusytokSupervisor` holds the strong ref).
    _executor: Arc<SidecarTaskExecutor>,
    _responder_holder: Arc<Mutex<Vec<Arc<PressureResponder>>>>,
}

fn make_harness() -> TestHarness {
    make_harness_with_env(HashMap::new())
}

fn make_harness_with_env(env: HashMap<String, String>) -> TestHarness {
    let db: SharedDb = Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    seed_sidecar_test_provider(&db);
    let (pool, executor, supervisor, holder) =
        make_pool_with_config(mock_sidecar_config_with_env(env), Some(Arc::clone(&db)));
    let exec_dyn: Arc<dyn TaskExecutor> = executor.clone();
    let manager = std::sync::Arc::new(SubagentManager::new(
        Arc::clone(&db),
        settings_with_test_provider(),
        "pi",
        exec_dyn,
    ));
    TestHarness {
        manager,
        db,
        pool,
        supervisor,
        _executor: executor,
        _responder_holder: holder,
    }
}

/// Seed the DB with a provider + model matching `TEST_PROVIDER_ID` so
/// `delegate()` can create subagents with valid bound fields.
fn seed_sidecar_test_provider(db: &SharedDb) {
    let db_guard = db.lock().unwrap();
    let now = busytok_domain::now_ms();
    db_guard.conn().execute(
        "INSERT INTO providers (id, name, provider_kind, base_url, enabled, api_key, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        rusqlite::params![
            TEST_PROVIDER_ID,
            "Test Provider",
            serde_json::to_string(&busytok_domain::ProviderKind::OpenAiCompatible).unwrap(),
            "https://test.example.com/v1",
            1i64,
            "test-key",
            now,
        ],
    ).unwrap();
    db_guard.conn().execute(
        "INSERT INTO models (id, provider_id, model_id, enabled, created_at_ms, updated_at_ms, display_name, reasoning, context_window, max_tokens)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, NULL, 0, 128000, 16384)",
        rusqlite::params![
            "test-model-row",
            TEST_PROVIDER_ID,
            "test-model",
            1i64,
            now,
        ],
    ).unwrap();
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
        timeout_seconds: Some(10),
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: Some(TEST_PROVIDER_ID.to_string()),
        bound_model_id: Some("test-model".to_string()),
        reuse_policy: None,
    }
}

#[tokio::test]
async fn delegate_via_sidecar_writes_binding_and_sets_hot() {
    let h = make_harness();
    let r = h
        .manager
        .delegate(req("reviewer", "review the code"))
        .await
        .unwrap();

    // delegate() now returns Running immediately; the executor runs in a
    // background tokio::spawn. Wait for terminal status before checking
    // post-completion state (Bug #1/#2 fix).
    assert_eq!(r.status, TaskStatus::Running);
    let final_status = await_task_done(&h.manager, &r.task_id).await;
    assert_eq!(final_status, "completed");

    // Hot binding was upserted. The adapter_session_id is now read from the
    // DB (the immediate DelegateResult has it as None because delegate
    // returns Running before the executor runs).
    let adapter_session_id = {
        let db = h.db.lock().unwrap();
        let binding = db.subagent_hot_binding(&r.subagent_id, "pi").unwrap();
        assert!(binding.is_some(), "hot binding not found");
        let binding = binding.unwrap();
        assert_eq!(binding.is_hot, 1);
        assert_eq!(binding.status, "hot");
        binding.adapter_session_id
    };
    assert!(adapter_session_id.is_some(), "expected adapter_session_id");
    assert_eq!(r.adapter, "pi");

    // Subagent status is Hot (not Warm).
    {
        let db = h.db.lock().unwrap();
        let sub = db.subagent_get_logical(&r.subagent_id).unwrap();
        assert!(sub.is_some());
        assert_eq!(sub.unwrap().status, "hot");
    }
    h.supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn delegate_via_sidecar_reuses_hot_binding_on_redelegate() {
    let h = make_harness();
    let r1 = h
        .manager
        .delegate(req("reviewer", "first turn"))
        .await
        .unwrap();
    // delegate() now returns Running immediately; wait for terminal status
    // before re-delegating (Bug #1/#2 fix).
    assert_eq!(r1.status, TaskStatus::Running);
    await_task_done(&h.manager, &r1.task_id).await;
    let r2 = h
        .manager
        .delegate(req("reviewer", "second turn"))
        .await
        .unwrap();
    assert_eq!(r2.status, TaskStatus::Running);
    await_task_done(&h.manager, &r2.task_id).await;

    // Same subagent (resolved by name+cwd).
    assert_eq!(r1.subagent_id, r2.subagent_id);

    // Only one hot binding row (upsert, not duplicate insert).
    {
        let db = h.db.lock().unwrap();
        let binding = db.subagent_hot_binding(&r1.subagent_id, "pi").unwrap();
        assert!(binding.is_some());
    }
    h.supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn delegate_via_sidecar_empty_session_id_falls_to_cold() {
    // Regression: an empty `adapter_session_id` from the sidecar must NOT
    // trigger the hot path. Spec §3.3 requires a real backing session for a
    // hot binding; an empty id has no real session, so the delegate takes the
    // warm/cold path. Per §3.3 (P1-1 fix): warm iff hot_summary IS NOT NULL.
    // The sidecar mock produces no memory_update (current_state_summary=None),
    // so hot_summary stays None → status is Cold (not Warm).
    let mut env = HashMap::new();
    env.insert("BUSYTOK_MOCK_EMPTY_SESSION".to_string(), "1".to_string());
    let h = make_harness_with_env(env);
    let r = h
        .manager
        .delegate(req("reviewer", "empty session turn"))
        .await
        .unwrap();

    // delegate() now returns Running immediately; the executor runs in a
    // background tokio::spawn. Wait for terminal status before checking
    // post-completion state (Bug #1/#2 fix).
    assert_eq!(r.status, TaskStatus::Running);
    let final_status = await_task_done(&h.manager, &r.task_id).await;
    assert_eq!(final_status, "completed");

    {
        let db = h.db.lock().unwrap();

        // No hot binding row — spec §3.3 (no hot binding without a real session).
        let binding = db.subagent_hot_binding(&r.subagent_id, "pi").unwrap();
        assert!(
            binding.is_none(),
            "empty adapter_session_id must not create a hot binding"
        );

        // Logical subagent status is Cold: no memory_update → hot_summary is
        // None → §3.3 says cold (not warm). P1-1 regression: the old code
        // unconditionally set Warm, violating "warm iff hot_summary IS NOT NULL".
        let sub = db.subagent_get_logical(&r.subagent_id).unwrap();
        assert!(sub.is_some());
        assert_eq!(sub.unwrap().status, "cold");
    }
    h.supervisor.shutdown().await.unwrap();
}

// --- executor error-path tests ---
//
// `SidecarTaskExecutor::execute` has two `map_err(sidecar_to_anyhow)` sites
// (ensure_started failure, turn_auto failure) plus the `sidecar_to_anyhow`
// helper itself. The happy-path tests above cover neither; these tests
// exercise both error paths so the error mapping + `SubagentError` downcast
// contract is honored.

fn executor_input() -> ExecutorInput {
    ExecutorInput {
        task_id: "task-fixture".to_string(),
        subagent_id: "sub-test".to_string(),
        subagent_name: "reviewer".to_string(),
        cwd: "/tmp/repo".to_string(),
        profile: "pi/search-cheap".to_string(),
        model: "test-model".to_string(),
        prompt: "do something".to_string(),
        prompt_artifact_ref: None,
        timeout_seconds: Some(5),
        tools: vec![],
        memory: empty_memory_snapshot(),
        context: empty_compact_context(),
        write_access: false,
        provider_id: TEST_PROVIDER_ID.to_string(),
        provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
        provider_base_url: "https://test".to_string(),
        provider_api_key: "sk-test".to_string(),
        model_reasoning: false,
        model_context_window: 8000,
        model_max_tokens: 1000,
        model_display_name: None,
    }
}

#[tokio::test]
async fn execute_returns_error_when_supervisor_cannot_spawn() {
    // A non-existent node_binary forces `cmd.spawn()` to fail in
    // `spawn_internal`, so `ensure_started` returns `SidecarError::Spawn`.
    // `execute` must propagate that via `sidecar_to_anyhow` →
    // `SubagentError::SidecarSpawn`.
    let mut cfg = mock_sidecar_config_with_env(HashMap::new());
    cfg.node_binary = PathBuf::from("/nonexistent/busytok-node-binary");
    cfg.bundle_path = PathBuf::from("/nonexistent/bundle.js");
    let (_pool, executor, _supervisor, _holder) = make_pool_with_config(cfg, None);

    let err = match executor.execute(&executor_input()).await {
        Ok(_) => panic!("expected error when supervisor cannot spawn"),
        Err(e) => e,
    };
    // The error round-trips through SubagentError → anyhow, so downcasting
    // back to SubagentError must yield SidecarSpawn (spec: control contract
    // preserves the sidecar spawn failure code).
    let subagent_err = err
        .downcast_ref::<busytok_subagent::SubagentError>()
        .expect("error should downcast to SubagentError");
    assert!(
        matches!(
            subagent_err,
            busytok_subagent::SubagentError::SidecarSpawn(_)
        ),
        "expected SidecarSpawn, got {subagent_err:?}"
    );
    assert_eq!(subagent_err.code(), "subagent.sidecar_spawn_failed");
}

#[tokio::test]
async fn execute_returns_error_when_sidecar_crashes_during_turn_auto() {
    // BUSYTOK_MOCK_CRASH_AFTER=1 → sidecar crashes after responding to
    // adapter.initialize (message 1). `ensure_started` succeeds (it got the
    // initialize response), but the sidecar exits before `turn_auto` can
    // get a response. Depending on timing, `turn_auto` fails with one of:
    //   - SidecarError::Io("Broken pipe") — write to stdin fails because the
    //     sidecar process has exited and the pipe is closed
    //   - SidecarError::Crashed("sidecar stdout closed") — write succeeds
    //     (buffered) but read returns EOF
    //   - SidecarError::Crashed("sidecar not running") — supervision loop
    //     detected the crash and cleared the client before turn_auto ran
    // All three are valid crash-detection outcomes; the test verifies that
    // `execute` propagates the error via `sidecar_to_anyhow` and the
    // downcast yields a sidecar-domain SubagentError variant.
    let mut env = HashMap::new();
    env.insert("BUSYTOK_MOCK_CRASH_AFTER".to_string(), "1".to_string());
    let mut cfg = mock_sidecar_config_with_env(env);
    // Keep the health interval long so the health pinger doesn't race the
    // crash detection.
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, _supervisor, _holder) = make_pool_with_config(cfg, None);

    let err = match executor.execute(&executor_input()).await {
        Ok(_) => panic!("expected error when sidecar crashes during turn_auto"),
        Err(e) => e,
    };
    let subagent_err = err
        .downcast_ref::<busytok_subagent::SubagentError>()
        .expect("error should downcast to SubagentError");
    match subagent_err {
        busytok_subagent::SubagentError::SidecarCrashed(_) => {
            // Crash detected via EOF or client-None path.
        }
        busytok_subagent::SubagentError::SidecarIo(msg) => {
            // Crash detected via broken-pipe on stdin write.
            assert!(
                msg.contains("Broken pipe") || msg.contains("pipe"),
                "expected pipe-related IO error, got: {msg}"
            );
        }
        other => panic!(
            "expected SidecarCrashed or SidecarIo, got {other:?} (code: {})",
            other.code()
        ),
    }
}

// --- auth-fail kill (Phase 3 Task 3) ---
//
// `SidecarTaskExecutor::execute` classifies sidecar errors via
// `classify_sidecar_error`. When the error is `TaskErrorKind::Auth`
// (sidecar RPC code -32010 AUTH_FAILURE), it calls
// `pool.remove_worker_and_kill(provider_id)` to hard-kill the worker child
// and drop it from the pool map. The 5-min restart window is bypassed
// because the credential is bad — restarting with the same key won't help.
// This test drives that path end-to-end via `BUSYTOK_MOCK_AUTH_FAIL=1`.

#[tokio::test]
async fn executor_auth_failure_kills_worker_from_pool() {
    // BUSYTOK_MOCK_AUTH_FAIL=1 makes the mock sidecar return
    // {"error":{"code":-32010,"message":"401 Unauthorized"}} for
    // session.turn_auto. classify_sidecar_error maps -32010 to
    // TaskErrorKind::Auth, so execute() must call
    // pool.remove_worker_and_kill(provider_id) before propagating the error.
    let mut env = HashMap::new();
    env.insert("BUSYTOK_MOCK_AUTH_FAIL".to_string(), "1".to_string());
    let mut cfg = mock_sidecar_config_with_env(env);
    cfg.health_interval = Duration::from_secs(3600);
    let (pool, executor, _supervisor, _holder) = make_pool_with_config(cfg, None);

    // Sanity: ensure_worker populated the pool with one worker for
    // TEST_PROVIDER_ID before execute() ran.
    let before = pool.worker_snapshots().await;
    assert_eq!(
        before.len(),
        1,
        "pool must hold the test-prov worker before execute()"
    );

    // execute() hits the auth-fail branch and must propagate an error.
    let err = match executor.execute(&executor_input()).await {
        Ok(_) => panic!("expected auth-fail error, got success"),
        Err(e) => e,
    };
    let subagent_err = err
        .downcast_ref::<busytok_subagent::SubagentError>()
        .expect("error should downcast to SubagentError");
    assert!(
        matches!(
            subagent_err,
            busytok_subagent::SubagentError::SidecarRpc { .. }
        ),
        "expected SidecarRpc (AUTH_FAILURE -32010), got {subagent_err:?}"
    );
    assert!(
        format!("{subagent_err}").contains("-32010"),
        "error should carry the AUTH_FAILURE code, got: {subagent_err}"
    );

    // The kill: remove_worker_and_kill must have dropped the worker from the
    // pool map. worker_snapshots() reflects the live map, so it must now be
    // empty — the bad-credential worker is gone and the next execute() would
    // re-spawn (re-reading credentials from the provider catalog).
    let after = pool.worker_snapshots().await;
    assert!(
        after.is_empty(),
        "pool must be empty after auth-fail kill, got {after:?}"
    );
}

// --- eviction driver test (Plan 3 Task 5) ---
//
// `SidecarTaskExecutor::execute` must catch `HOT_SESSION_LIMIT_REACHED` from
// `turn_auto`, extract the LRU `candidate` from the RPC error's `data.candidate`
// field, drive the eviction flow (prepare_hibernate → persist memory + flip
// binding atomically → close), and retry `turn_auto` once. The retry must
// succeed because the sidecar released the slot via `session.close`.
//
// NOTE: The executor does NOT create subagent rows or persist the initial hot
// binding — that's `SubagentManager::delegate()`'s job. This test manually
// persists the binding after the first `execute()` to simulate the
// post-delegate DB state the eviction flow depends on.

#[tokio::test]
async fn executor_evicts_lru_session_on_hot_limit_and_retries() {
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, Some(db.clone()));

    // First delegate — fills the pool (max_hot=1).
    let input1 = ExecutorInput {
        task_id: "task-a".into(),
        subagent_id: "sub-a".into(),
        subagent_name: "a".into(),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        model: "test-model".into(),
        prompt: "do 1".into(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        tools: vec![],
        memory: empty_memory_snapshot(),
        context: empty_compact_context(),
        write_access: false,
        provider_id: TEST_PROVIDER_ID.to_string(),
        provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
        provider_base_url: "https://test".into(),
        provider_api_key: "sk-test".into(),
        model_reasoning: false,
        model_context_window: 8000,
        model_max_tokens: 1000,
        model_display_name: None,
    };
    let out1 = executor
        .execute(&input1)
        .await
        .expect("first delegate must succeed");
    assert_eq!(out1.status, TaskStatus::Completed);
    let sess_a = out1.adapter_session_id.expect("must have session id");

    // Manually persist the hot binding (simulating what
    // `SubagentManager::delegate()` does after a successful `execute()`). The
    // eviction flow's `find_hot_binding_by_session` query depends on this
    // binding existing.
    {
        let db_guard = db.lock().unwrap();
        db_guard
            .subagent_upsert_logical(&SubagentLogicalSubagentRow {
                id: "sub-a".into(),
                name: "a".into(),
                project_id: "p".into(),
                repo_path: "/r".into(),
                repo_hash: "h".into(),
                branch: None,
                intent: None,
                default_profile: "pi/search-cheap".into(),
                bound_provider_id: "test-provider".into(),
                bound_model_id: "test-model".into(),
                status: "hot".into(),
                created_at_ms: 0,
                updated_at_ms: 0,
                last_active_at_ms: Some(0),
            })
            .unwrap();
        let now = busytok_domain::now_ms();
        db_guard
            .subagent_commit_hot_binding_and_status(
                &SubagentHarnessBindingRow {
                    id: "bind_a".into(),
                    subagent_id: "sub-a".into(),
                    harness: "pi".into(),
                    adapter_session_id: Some(sess_a.clone()),
                    adapter_process_id: None,
                    is_hot: 1,
                    status: "hot".into(),
                    created_at_ms: now,
                    last_used_at_ms: Some(now),
                    closed_at_ms: None,
                    detail_json: None,
                },
                "sub-a",
            )
            .unwrap();
    }

    // Two-phase lifecycle: activate the session (simulates what the manager
    // does after committing the DB binding — moves the session from
    // SESS_PENDING to SESS_ORDER so it becomes an evictable LRU candidate).
    executor
        .activate_session(&sess_a, TEST_PROVIDER_ID)
        .await
        .unwrap();

    // Second delegate — different subagent, triggers eviction.
    let input2 = ExecutorInput {
        task_id: "task-b".into(),
        subagent_id: "sub-b".into(),
        subagent_name: "b".into(),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        model: "test-model".into(),
        prompt: "do 2".into(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        tools: vec![],
        memory: empty_memory_snapshot(),
        context: empty_compact_context(),
        write_access: false,
        provider_id: TEST_PROVIDER_ID.to_string(),
        provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
        provider_base_url: "https://test".into(),
        provider_api_key: "sk-test".into(),
        model_reasoning: false,
        model_context_window: 8000,
        model_max_tokens: 1000,
        model_display_name: None,
    };
    let out2 = executor
        .execute(&input2)
        .await
        .expect("eviction + retry must succeed");
    assert_eq!(out2.status, TaskStatus::Completed);
    assert!(out2.adapter_session_id.is_some());

    // Verify: sub-a is now warm (evicted), with memory written by the
    // eviction flow's prepare_hibernate → write_hot_summary path.
    {
        let db_guard = db.lock().unwrap();
        let sub_a = db_guard.subagent_get_logical("sub-a").unwrap().unwrap();
        assert_eq!(sub_a.status, "warm", "evicted subagent must be warm");
        let mem = db_guard.subagent_get_memory("sub-a").unwrap();
        assert!(mem.is_some(), "evicted subagent must have memory row");
        assert!(
            mem.unwrap().hot_summary.is_some(),
            "hot_summary must be written"
        );
    }

    supervisor.shutdown().await.unwrap();
}

// --- eviction failure paths (Plan 3 Task 7) ---
//
// Two regression tests for the eviction driver's failure modes:
//   1. Ghost session (state divergence): the sidecar returns
//      HOT_SESSION_LIMIT_REACHED with data.candidate=X, but the DB has no
//      hot binding for X. With the two-phase lifecycle (pending → active),
//      this can ONLY happen if the session was activated (moved to LRU)
//      but the binding was deleted/never committed — a permanent state
//      divergence, NOT a transient timing window. The executor must surface
//      `HotSessionStateDivergence` (fatal) so the operator is alerted.
//      Returning `HotSessionLimit` (transient) would cause an infinite
//      retry loop because the binding will never appear.
//   2. session.close fails during eviction (BUSYTOK_MOCK_CLOSE_FAILS=1).
//      The executor must abort (return Err) — the DB has been flipped to
//      closed/warm but the sidecar still holds the session hot, so
//      retrying turn_auto would hit HOT_SESSION_LIMIT_REACHED again and
//      diverge. State divergence risk → fatal.

#[tokio::test]
async fn executor_eviction_ghost_session_surfaces_state_divergence() {
    // With the two-phase session lifecycle (pending → active), the "ghost
    // session" timing window is closed: newly-created sessions start as
    // `pending` (NOT in LRU, NOT evictable) and only become LRU candidates
    // after Rust commits the DB binding and calls `session.activate`.
    //
    // Therefore, a sidecar that names an LRU candidate whose DB binding
    // doesn't exist is NOT a transient timing window — it's a permanent
    // sidecar/DB state divergence. The only way an activated session
    // (in SESS_ORDER) can lack a DB binding is a bug: either the binding
    // was deleted, the activate RPC was called without a prior commit, or
    // the sidecar and DB diverged permanently.
    //
    // The executor must surface `HotSessionStateDivergence` (fatal) — NOT
    // `HotSessionLimit` (transient). A transient error would cause an
    // infinite retry loop because the binding will never appear (it's not
    // a pending commit — it's permanently missing).
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, Some(db.clone()));

    // First delegate — fills the sidecar's pool (max_hot=1). The session
    // starts in SESS_PENDING (not yet an LRU candidate).
    let out1 = executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");
    let sess_a = out1.adapter_session_id.expect("must have session id");

    // Seed the DB binding, then activate the session — this mirrors what
    // the manager does after a successful commit. After activation, the
    // session is in SESS_ORDER (LRU-eligible / evictable).
    seed_hot_binding(&db, "sub-a", "a", &sess_a);
    executor
        .activate_session(&sess_a, TEST_PROVIDER_ID)
        .await
        .unwrap();

    // Simulate a permanent state divergence: delete the DB binding that
    // was just committed. The sidecar still has sess_a in SESS_ORDER, so
    // it will return sess_a as an LRU candidate — but the DB has no
    // binding for it. With the two-phase lifecycle, this can only happen
    // due to a bug (not a transient timing window).
    {
        let db_guard = db.lock().unwrap();
        db_guard
            .conn()
            .execute(
                "DELETE FROM subagent_harness_bindings WHERE adapter_session_id = ?1",
                rusqlite::params![&sess_a],
            )
            .unwrap();
    }

    // Second delegate — triggers HOT_SESSION_LIMIT_REACHED with
    // data.candidate=sess_a. evict_session calls find_hot_binding_by_session
    // → returns None (binding was deleted). The executor must surface
    // HotSessionStateDivergence (fatal), NOT HotSessionLimit (transient).
    let result = executor.execute(&evict_input("sub-b")).await;
    assert!(
        result.is_err(),
        "eviction must surface a fatal error when the DB has no binding for an activated candidate"
    );
    let err = match result {
        Ok(_) => panic!("expected error, got success"),
        Err(e) => e,
    };
    let downcasted = err.downcast_ref::<SubagentError>();
    assert!(
        downcasted.is_some(),
        "error must downcast to SubagentError, got bare anyhow: {err}"
    );
    match downcasted.unwrap() {
        SubagentError::HotSessionStateDivergence(msg) => {
            assert!(
                msg.contains(&sess_a),
                "divergence error should name the ghost session {sess_a}, got: {msg}"
            );
        }
        other => panic!(
            "expected SubagentError::HotSessionStateDivergence (fatal), \
             got {other:?} — with the two-phase lifecycle, a missing binding \
             for an activated session is a permanent divergence, not a \
             transient timing window"
        ),
    }

    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn executor_eviction_aborts_when_session_close_fails() {
    // The sidecar returns HOT_SESSION_LIMIT_REACHED with data.candidate=X.
    // The DB has a hot binding for X (so find_hot_binding_by_session succeeds),
    // prepare_hibernate succeeds, the binding is flipped to closed in the DB,
    // BUT session.close(X) fails (BUSYTOK_MOCK_CLOSE_FAILS=1).
    // The executor must return an error — the DB has been flipped to closed
    // but the sidecar still holds the session. The caller must not retry.
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let now = busytok_domain::now_ms();
    // Pre-seed sub-a with a hot binding. The mock sidecar generates session
    // IDs as "pi_sess_mock_${counter}" (see mock-sidecar.sh), so the first
    // session it creates will be "pi_sess_mock_1". We pre-seed this binding
    // because the executor does NOT persist hot bindings itself (that is the
    // SubagentManager's job) — without this seed, find_hot_binding_by_session
    // would return None and eviction would fail before reaching close.
    {
        let db_guard = db.lock().unwrap();
        db_guard
            .subagent_upsert_logical(&SubagentLogicalSubagentRow {
                id: "sub-a".into(),
                name: "a".into(),
                project_id: "p".into(),
                repo_path: "/r".into(),
                repo_hash: "h".into(),
                branch: None,
                intent: None,
                default_profile: "pi/search-cheap".into(),
                bound_provider_id: "test-provider".into(),
                bound_model_id: "test-model".into(),
                status: "hot".into(),
                created_at_ms: now,
                updated_at_ms: now,
                last_active_at_ms: Some(now),
            })
            .unwrap();
        db_guard
            .subagent_commit_hot_binding_and_status(
                &SubagentHarnessBindingRow {
                    id: "bind-a".into(),
                    subagent_id: "sub-a".into(),
                    harness: "pi".into(),
                    adapter_session_id: Some("pi_sess_mock_1".into()),
                    adapter_process_id: None,
                    is_hot: 1,
                    status: "hot".into(),
                    created_at_ms: now,
                    last_used_at_ms: Some(now),
                    closed_at_ms: None,
                    detail_json: None,
                },
                "sub-a",
            )
            .unwrap();
    }

    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.env
        .insert("BUSYTOK_MOCK_CLOSE_FAILS".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, Some(db.clone()));

    // First delegate — fills the sidecar's pool (max_hot=1), session pi_sess_mock_1.
    let input1 = ExecutorInput {
        task_id: "task-a".into(),
        subagent_id: "sub-a".into(),
        subagent_name: "a".into(),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        model: "test-model".into(),
        prompt: "do 1".into(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        tools: vec![],
        memory: empty_memory_snapshot(),
        context: empty_compact_context(),
        write_access: false,
        provider_id: TEST_PROVIDER_ID.to_string(),
        provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
        provider_base_url: "https://test".into(),
        provider_api_key: "sk-test".into(),
        model_reasoning: false,
        model_context_window: 8000,
        model_max_tokens: 1000,
        model_display_name: None,
    };
    let out1 = executor
        .execute(&input1)
        .await
        .expect("first delegate must succeed");
    let sess_a = out1.adapter_session_id.expect("must have session id");
    // Two-phase lifecycle: activate the session (simulates what the manager
    // does after committing the DB binding). The pre-seeded binding matches
    // sess_a, so activation makes it an evictable LRU candidate.
    executor
        .activate_session(&sess_a, TEST_PROVIDER_ID)
        .await
        .unwrap();

    // Second delegate — triggers HOT_SESSION_LIMIT_REACHED with
    // data.candidate=pi_sess_mock_1 (the LRU session).
    // Eviction: find_hot_binding_by_session(pi_sess_mock_1) → OK (pre-seeded),
    // prepare_hibernate → OK, commit_hibernate_binding_and_status → flips DB,
    // session.close(pi_sess_mock_1) → FAILS (BUSYTOK_MOCK_CLOSE_FAILS=1).
    // Executor must return an error, NOT retry.
    let input2 = ExecutorInput {
        task_id: "task-b".into(),
        subagent_id: "sub-b".into(),
        subagent_name: "b".into(),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        model: "test-model".into(),
        prompt: "do 2".into(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        tools: vec![],
        memory: empty_memory_snapshot(),
        context: empty_compact_context(),
        write_access: false,
        provider_id: TEST_PROVIDER_ID.to_string(),
        provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
        provider_base_url: "https://test".into(),
        provider_api_key: "sk-test".into(),
        model_reasoning: false,
        model_context_window: 8000,
        model_max_tokens: 1000,
        model_display_name: None,
    };
    let result = executor.execute(&input2).await;
    assert!(
        result.is_err(),
        "eviction must abort when session.close fails — state divergence risk"
    );
    let err = match result {
        Ok(_) => panic!("expected error, got success"),
        Err(e) => e,
    };
    // Bug 2 fix: the error must be a structured SubagentError::SidecarRpc,
    // NOT a bare anyhow wrapped as SubagentError::Store ("database error").
    let downcasted = err.downcast_ref::<SubagentError>();
    assert!(
        downcasted.is_some(),
        "error must downcast to SubagentError, got bare anyhow: {err}"
    );
    match downcasted.unwrap() {
        SubagentError::SidecarRpc { message, .. } => {
            assert!(
                message.contains("session.close failed"),
                "error message should mention the close failure, got: {message}"
            );
        }
        other => panic!(
            "expected SubagentError::SidecarRpc, got {other:?} — \
             Bug 2 regression: close failure misclassified"
        ),
    }

    // Verify the fatal-close invariant: the DB HAS been flipped (binding
    // is_hot=0, status='closed'; logical status='warm') BEFORE close was
    // attempted. This is the state divergence the fatal-close rule exists
    // to surface — DB says closed/warm, sidecar still holds the session hot.
    // Without these assertions a future refactor that accidentally moves
    // the flip after `close` would still pass the test above.
    {
        let db_guard = db.lock().unwrap();
        let sub_a = db_guard
            .subagent_get_logical("sub-a")
            .unwrap()
            .expect("sub-a logical row must exist");
        assert_eq!(
            sub_a.status, "warm",
            "logical status must be flipped to 'warm' before close (fatal-close invariant)"
        );
        // `subagent_hot_binding` filters on is_hot=1, so it returns None after
        // the flip — query the raw row to verify is_hot=0 and status='closed'.
        let binding_state: (i32, String) = db_guard
            .conn()
            .query_row(
                "SELECT is_hot, status FROM subagent_harness_bindings \
                 WHERE subagent_id = ?1 AND harness = 'pi'",
                rusqlite::params!["sub-a"],
                |row| Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("sub-a binding row must exist after eviction flip");
        assert_eq!(
            binding_state.0, 0,
            "binding is_hot must be 0 (flipped before close)"
        );
        assert_eq!(
            binding_state.1, "closed",
            "binding status must be 'closed' (flipped before close)"
        );
    }

    supervisor.shutdown().await.unwrap();
}

// --- parse_turn_auto_result status arms (Plan 3 Task 8 coverage backfill) ---
//
// `parse_turn_auto_result` maps the sidecar's `status` field to a
// `TaskStatus`. The happy-path mock always returns "completed", so the
// "failed", "timeout", and unknown-status (`_`) arms are uncovered. Each test
// drives one arm via the `BUSYTOK_MOCK_TURN_STATUS` mock env var so the
// executor parses the crafted status through the real code path.

#[tokio::test]
async fn executor_turn_auto_failed_status_maps_to_failed() {
    let mut env = HashMap::new();
    env.insert("BUSYTOK_MOCK_TURN_STATUS".to_string(), "failed".to_string());
    let mut cfg = mock_sidecar_config_with_env(env);
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, None);

    let out = executor
        .execute(&executor_input())
        .await
        .expect("turn_auto succeeds; status=failed is a result, not an error");
    assert_eq!(out.status, TaskStatus::Failed);

    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn executor_turn_auto_timeout_status_maps_to_failed() {
    let mut env = HashMap::new();
    env.insert(
        "BUSYTOK_MOCK_TURN_STATUS".to_string(),
        "timeout".to_string(),
    );
    let mut cfg = mock_sidecar_config_with_env(env);
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, None);

    let out = executor
        .execute(&executor_input())
        .await
        .expect("turn_auto succeeds; status=timeout is a result, not an error");
    assert_eq!(out.status, TaskStatus::Failed);

    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn executor_turn_auto_unknown_status_falls_back_to_completed() {
    // An unrecognized status string hits the `_` arm of the match and falls
    // back to `TaskStatus::Completed` (spec: unknown statuses are tolerated,
    // not fatal).
    let mut env = HashMap::new();
    env.insert(
        "BUSYTOK_MOCK_TURN_STATUS".to_string(),
        "bogus-xyz".to_string(),
    );
    let mut cfg = mock_sidecar_config_with_env(env);
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, None);

    let out = executor
        .execute(&executor_input())
        .await
        .expect("turn_auto succeeds; unknown status falls back to Completed");
    assert_eq!(out.status, TaskStatus::Completed);

    supervisor.shutdown().await.unwrap();
}

// --- eviction driver error-path coverage (Plan 3 Task 8 backfill) ---
//
// Four more eviction-flow branches are uncovered because the happy-path mock
// always succeeds: (1) HOT_SESSION_LIMIT_REACHED with no data.candidate
// (sidecar protocol violation), (2) prepare_hibernate failure, (3) null
// memory_delta (skip write_hot_summary), (4) retry turn_auto failure after a
// successful eviction close. Each is driven by a dedicated mock env var so
// the executor exercises the real error/skip/propagation path.

fn evict_input(id: &str) -> ExecutorInput {
    ExecutorInput {
        task_id: format!("task-{id}"),
        subagent_id: id.into(),
        subagent_name: id.into(),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        model: "test-model".into(),
        prompt: format!("do {id}"),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        tools: vec![],
        memory: empty_memory_snapshot(),
        context: empty_compact_context(),
        write_access: false,
        provider_id: TEST_PROVIDER_ID.to_string(),
        provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
        provider_base_url: "https://test".into(),
        provider_api_key: "sk-test".into(),
        model_reasoning: false,
        model_context_window: 8000,
        model_max_tokens: 1000,
        model_display_name: None,
    }
}

// Pre-seed a hot binding (and an empty memory row) for a subagent. The
// executor does NOT persist hot bindings or subagent rows itself — that is
// `SubagentManager::delegate()`'s job. Eviction-flow tests that need
// `find_hot_binding_by_session` to resolve must pre-seed the binding, mirroring
// the post-delegate DB state the eviction flow depends on.
fn seed_hot_binding(db: &SharedDb, subagent_id: &str, name: &str, session_id: &str) {
    let now = busytok_domain::now_ms();
    let db_guard = db.lock().unwrap();
    db_guard
        .subagent_upsert_logical(&SubagentLogicalSubagentRow {
            id: subagent_id.into(),
            name: name.into(),
            project_id: "p".into(),
            repo_path: "/r".into(),
            repo_hash: "h".into(),
            branch: None,
            intent: None,
            default_profile: "pi/search-cheap".into(),
            bound_provider_id: "test-provider".into(),
            bound_model_id: "test-model".into(),
            status: "hot".into(),
            created_at_ms: now,
            updated_at_ms: now,
            last_active_at_ms: Some(now),
        })
        .unwrap();
    db_guard
        .subagent_upsert_memory(&SubagentMemoryRow::new_empty(subagent_id))
        .unwrap();
    db_guard
        .subagent_commit_hot_binding_and_status(
            &SubagentHarnessBindingRow {
                id: format!("bind-{subagent_id}"),
                subagent_id: subagent_id.into(),
                harness: "pi".into(),
                adapter_session_id: Some(session_id.into()),
                adapter_process_id: None,
                is_hot: 1,
                status: "hot".into(),
                created_at_ms: now,
                last_used_at_ms: Some(now),
                closed_at_ms: None,
                detail_json: None,
            },
            subagent_id,
        )
        .unwrap();
}

#[tokio::test]
async fn executor_eviction_fails_when_candidate_missing_from_error() {
    // The sidecar returns HOT_SESSION_LIMIT_REACHED but omits the `data`
    // field entirely (sidecar protocol violation). `classify_hot_limit_error`
    // must return `ProtocolViolation` and the executor must surface a
    // structured `SubagentError::Validation` — never silently retry or evict
    // a phantom session (Bug 1 + Bug 2 fix).
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_LIMIT_NO_CANDIDATE".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, None);

    // Fill the pool (max_hot=1).
    let out1 = executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");
    let sess_a = out1.adapter_session_id.expect("must have session id");
    // Two-phase lifecycle: activate the session so it enters SESS_ORDER
    // (LRU-eligible). Without this, the mock sidecar would return
    // all_busy=true (no evictable candidate) instead of the protocol
    // violation error this test exercises.
    executor
        .activate_session(&sess_a, TEST_PROVIDER_ID)
        .await
        .unwrap();

    // Second delegate triggers HOT_SESSION_LIMIT_REACHED with no data field.
    let err = match executor.execute(&evict_input("sub-b")).await {
        Ok(_) => panic!("expected error, got success"),
        Err(e) => e,
    };
    // Bug 2 fix: the error must downcast to a structured SubagentError (not a
    // bare anyhow that would be mislabeled as "database error").
    let downcasted = err.downcast_ref::<SubagentError>();
    assert!(
        downcasted.is_some(),
        "error must downcast to SubagentError, got bare anyhow: {err}"
    );
    match downcasted.unwrap() {
        SubagentError::Validation(msg) => {
            assert!(
                msg.contains("missing data"),
                "error should explain the protocol violation, got: {msg}"
            );
        }
        other => panic!("expected SubagentError::Validation, got {other:?} — Bug 2 regression"),
    }

    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn executor_eviction_skips_when_all_sessions_busy() {
    // Bug 1 fix: when the sidecar returns HOT_SESSION_LIMIT_REACHED with
    // `data.candidate = null` + `data.all_busy = true` (all sessions are
    // in-use), the executor must NOT attempt eviction — there is no safe
    // candidate. It surfaces `SubagentError::HotSessionLimit` with an empty
    // candidate so the task fails with a clear, structured error instead of
    // a doomed eviction + "database error" mislabel (Bug 2).
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_LIMIT_ALL_BUSY".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, None);

    // Fill the pool (max_hot=1).
    executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");

    // Second delegate triggers HOT_SESSION_LIMIT_REACHED with all_busy=true.
    let err = match executor.execute(&evict_input("sub-b")).await {
        Ok(_) => panic!("expected error, got success"),
        Err(e) => e,
    };
    // Bug 2 fix: error must downcast to structured SubagentError.
    let downcasted = err.downcast_ref::<SubagentError>();
    assert!(
        downcasted.is_some(),
        "error must downcast to SubagentError, got bare anyhow: {err}"
    );
    match downcasted.unwrap() {
        SubagentError::HotSessionLimit { candidate } => {
            assert!(
                candidate.is_empty(),
                "candidate must be empty (all-busy path), got: {candidate}"
            );
        }
        other => {
            panic!("expected SubagentError::HotSessionLimit, got {other:?} — Bug 1 regression")
        }
    }

    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn executor_eviction_fails_without_db() {
    // The no-DB executor cannot evict safely (no atomic persistence). When
    // HOT_SESSION_LIMIT_REACHED is hit and `self.db.is_none()`, `evict_session`
    // must return an error rather than proceeding — calling `close` without
    // first flipping the DB binding would leave the sidecar and DB diverged
    // (sidecar releases its slot, DB still believes the session is hot).
    // This covers the `db.is_none()` short-circuit at the top of `evict_session`.
    //
    // Unlike `executor_eviction_fails_when_candidate_missing_from_error` (which
    // also uses a no-DB executor but errors in `classify_hot_limit_error`
    // before reaching `evict_session`), this test lets candidate extraction
    // succeed so `evict_session` itself is entered and the no-DB guard fires.
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, None); // No DB

    // First delegate — fills the pool (max_hot=1).
    let out1 = executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");
    let sess_a = out1.adapter_session_id.expect("must have session id");
    // Two-phase lifecycle: activate the session so it enters SESS_ORDER
    // (LRU-eligible). Without this, the mock sidecar would return
    // all_busy=true instead of a named candidate, and the no-DB guard
    // in evict_session would never be reached.
    executor
        .activate_session(&sess_a, TEST_PROVIDER_ID)
        .await
        .unwrap();

    // Second delegate — triggers HOT_SESSION_LIMIT_REACHED with a valid
    // data.candidate (classify_hot_limit_error returns Evict), then
    // evict_session hits the no-DB guard and returns an error before any RPC.
    let err = match executor.execute(&evict_input("sub-b")).await {
        Ok(_) => panic!("expected error, got success"),
        Err(e) => e,
    };
    assert!(
        format!("{err}").contains("eviction requires a DB"),
        "error should explain the no-DB guard, got: {err}"
    );

    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn executor_eviction_fails_when_prepare_hibernate_fails() {
    // Eviction reaches prepare_hibernate, but the sidecar returns a JSON-RPC
    // error. The executor must propagate the error (the `warn!` +
    // sidecar_to_anyhow map_err path) — it must NOT proceed to flip the DB
    // binding or call close.
    //
    // Uses `with_db` (the production path) because the no-DB path now
    // short-circuits eviction up front (see `evict_session`'s db.is_none()
    // guard). Seeding the hot binding lets `find_hot_binding_by_session`
    // succeed so the executor actually reaches `prepare_hibernate`.
    let db: SharedDb = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.env
        .insert("BUSYTOK_MOCK_PREPARE_HIBERNATE_FAILS".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, Some(db.clone()));

    let out1 = executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");
    let sess_a = out1.adapter_session_id.expect("must have session id");
    seed_hot_binding(&db, "sub-a", "a", &sess_a);
    // Two-phase lifecycle: activate the session so it becomes an LRU
    // candidate for the eviction flow.
    executor
        .activate_session(&sess_a, TEST_PROVIDER_ID)
        .await
        .unwrap();

    let err = match executor.execute(&evict_input("sub-b")).await {
        Ok(_) => panic!("expected error, got success"),
        Err(e) => e,
    };
    // Bug 2 fix: error must be a structured SubagentError (not bare anyhow).
    let downcasted = err.downcast_ref::<SubagentError>();
    assert!(
        downcasted.is_some(),
        "error must downcast to SubagentError, got bare anyhow: {err}"
    );
    match downcasted.unwrap() {
        SubagentError::SidecarRpc { message, .. } => {
            assert!(
                message.contains("prepare_hibernate"),
                "error should mention prepare_hibernate, got: {message}"
            );
        }
        other => panic!(
            "expected SubagentError::SidecarRpc for prepare_hibernate failure, got {other:?}"
        ),
    }

    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn executor_eviction_skips_memory_write_when_memory_delta_null() {
    // prepare_hibernate returns {"memory_delta":null,...}. The executor must
    // skip write_hot_summary (wrote_summary=false) but still flip the binding
    // to closed, close the session, and retry turn_auto successfully. With no
    // prior memory, the logical status MUST be 'cold' (spec §3.3: 'warm' iff
    // hot_summary IS NOT NULL).
    let db: SharedDb = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.env
        .insert("BUSYTOK_MOCK_NULL_MEMORY_DELTA".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, Some(db.clone()));

    let out1 = executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");
    let sess_a = out1.adapter_session_id.expect("must have session id");

    // Pre-seed the hot binding + an empty memory row (mirrors what
    // SubagentManager::delegate persists after a successful execute).
    seed_hot_binding(&db, "sub-a", "a", &sess_a);
    // Two-phase lifecycle: activate the session so it becomes an LRU
    // candidate for the eviction flow.
    executor
        .activate_session(&sess_a, TEST_PROVIDER_ID)
        .await
        .unwrap();

    let out2 = executor
        .execute(&evict_input("sub-b"))
        .await
        .expect("eviction + retry must succeed");
    assert_eq!(out2.status, TaskStatus::Completed);

    // sub-a is evicted with no memory (null delta, empty seeded row) → 'cold'.
    {
        let db_guard = db.lock().unwrap();
        let sub_a = db_guard.subagent_get_logical("sub-a").unwrap().unwrap();
        assert_eq!(
            sub_a.status, "cold",
            "evicted subagent with no memory must be cold (§3.3)"
        );
        let mem = db_guard
            .subagent_get_memory("sub-a")
            .unwrap()
            .expect("memory row should exist (seeded empty)");
        assert!(
            mem.hot_summary.is_none(),
            "hot_summary must NOT be written when memory_delta is null, got: {:?}",
            mem.hot_summary
        );
    }

    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn executor_eviction_keeps_warm_when_prior_memory_exists_and_delta_null() {
    // Regression for §3.3: prepare_hibernate returns a null memory_delta, but
    // the subagent already has a hot_summary from a prior session. The
    // executor must skip write_hot_summary (null delta) yet keep the logical
    // status 'warm' because recoverable memory still exists
    // (hot_summary IS NOT NULL). Falling to 'cold' here would discard the
    // prior memory's recoverability signal.
    let db: SharedDb = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.env
        .insert("BUSYTOK_MOCK_NULL_MEMORY_DELTA".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, Some(db.clone()));

    let out1 = executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");
    let sess_a = out1.adapter_session_id.expect("must have session id");

    // Pre-seed the hot binding + a memory row that already has a hot_summary
    // (simulates a prior session that wrote memory).
    seed_hot_binding(&db, "sub-a", "a", &sess_a);
    // Two-phase lifecycle: activate the session so it becomes an LRU
    // candidate for the eviction flow.
    executor
        .activate_session(&sess_a, TEST_PROVIDER_ID)
        .await
        .unwrap();
    {
        let db_guard = db.lock().unwrap();
        db_guard
            .subagent_write_hot_summary("sub-a", "prior session memory")
            .unwrap();
    }

    let out2 = executor
        .execute(&evict_input("sub-b"))
        .await
        .expect("eviction + retry must succeed");
    assert_eq!(out2.status, TaskStatus::Completed);

    // sub-a keeps 'warm' because the prior hot_summary is still present.
    {
        let db_guard = db.lock().unwrap();
        let sub_a = db_guard.subagent_get_logical("sub-a").unwrap().unwrap();
        assert_eq!(
            sub_a.status, "warm",
            "evicted subagent with prior memory must stay warm (§3.3)"
        );
        let mem = db_guard
            .subagent_get_memory("sub-a")
            .unwrap()
            .expect("memory row should exist");
        assert_eq!(
            mem.hot_summary.as_deref(),
            Some("prior session memory"),
            "prior hot_summary must be preserved when delta is null"
        );
    }

    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn executor_eviction_propagates_error_when_retry_turn_auto_fails() {
    // Full eviction succeeds (prepare_hibernate OK, binding flipped, close OK),
    // but the retry turn_auto fails. The executor must propagate the error
    // (the `warn!` + sidecar_to_anyhow map_err path after eviction) — it must
    // NOT swallow the retry failure and return a phantom success.
    let db: SharedDb = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.env.insert(
        "BUSYTOK_MOCK_TURN_AUTO_FAILS_AFTER_CLOSE".into(),
        "1".into(),
    );
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, Some(db.clone()));

    let out1 = executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");
    let sess_a = out1.adapter_session_id.expect("must have session id");
    seed_hot_binding(&db, "sub-a", "a", &sess_a);
    // Two-phase lifecycle: activate the session so it becomes an LRU
    // candidate for the eviction flow.
    executor
        .activate_session(&sess_a, TEST_PROVIDER_ID)
        .await
        .unwrap();

    let err = match executor.execute(&evict_input("sub-b")).await {
        Ok(_) => panic!("expected error, got success"),
        Err(e) => e,
    };
    // -32603 is an unknown application code → SubagentError::SidecarRpc
    // ("[-32603] turn_auto failed: internal error").
    assert!(
        format!("{err}").contains("turn_auto failed"),
        "error should mention the turn_auto failure, got: {err}"
    );

    supervisor.shutdown().await.unwrap();
}

// --- AlreadyEvicted bounded retry (concurrent eviction race) ---
//
// When two concurrent delegates both get the same LRU candidate, the first
// evictor flips the DB binding to is_hot=0 and calls close; the second
// evictor's `evict_session` detects is_hot=0 → returns `AlreadyEvicted`.
// The caller then retries `turn_auto` with bounded backoff, waiting for the
// first evictor's close to free the sidecar slot.
//
// Test 1 (retry succeeds): the concurrent close has already freed the slot
//   (mock removes the session from its pool during prepare_hibernate), so
//   the retry `turn_auto` succeeds.
// Test 2 (exhaustion): the concurrent close never happens (mock keeps the
//   session in its pool), so all retries hit HOT_SESSION_LIMIT_REACHED and
//   the executor surfaces `HotSessionLimit`.

/// Pre-seed a binding already flipped to `is_hot=0` + `status=closed`,
/// simulating a concurrent evictor that already committed the eviction.
/// The session is still in the sidecar's hot pool (close may or may not
/// have completed yet — that's what the retry loop handles).
fn seed_cold_binding(db: &SharedDb, subagent_id: &str, name: &str, session_id: &str) {
    let now = busytok_domain::now_ms();
    let db_guard = db.lock().unwrap();
    db_guard
        .subagent_upsert_logical(&SubagentLogicalSubagentRow {
            id: subagent_id.into(),
            name: name.into(),
            project_id: "p".into(),
            repo_path: "/r".into(),
            repo_hash: "h".into(),
            branch: None,
            intent: None,
            default_profile: "pi/search-cheap".into(),
            bound_provider_id: "test-provider".into(),
            bound_model_id: "test-model".into(),
            status: "warm".into(),
            created_at_ms: now,
            updated_at_ms: now,
            last_active_at_ms: Some(now),
        })
        .unwrap();
    db_guard
        .subagent_upsert_memory(&SubagentMemoryRow::new_empty(subagent_id))
        .unwrap();
    db_guard
        .subagent_commit_hot_binding_and_status(
            &SubagentHarnessBindingRow {
                id: format!("bind-{subagent_id}"),
                subagent_id: subagent_id.into(),
                harness: "pi".into(),
                adapter_session_id: Some(session_id.into()),
                adapter_process_id: None,
                is_hot: 0,
                status: "closed".into(),
                created_at_ms: now,
                last_used_at_ms: Some(now),
                closed_at_ms: Some(now),
                detail_json: None,
            },
            subagent_id,
        )
        .unwrap();
}

#[tokio::test]
async fn executor_eviction_already_evicted_retry_succeeds() {
    // Concurrent eviction race: a concurrent evictor already flipped the DB
    // binding to is_hot=0. The mock sidecar's prepare_hibernate (with
    // BUSYTOK_MOCK_PREPARE_HIBERNATE_EVICTS=1) removes the session from its
    // pool before returning — simulating the concurrent evictor's close
    // having already freed the slot. The executor returns AlreadyEvicted,
    // retries turn_auto, and the retry succeeds because the slot is now free.
    let db: SharedDb = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.env
        .insert("BUSYTOK_MOCK_PREPARE_HIBERNATE_EVICTS".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, Some(db.clone()));

    // First delegate succeeds — sub-a gets a hot session.
    let out1 = executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");
    let sess_a = out1.adapter_session_id.expect("must have session id");

    // Pre-flip the DB binding to is_hot=0 (concurrent evictor already
    // committed the eviction). The sidecar still holds the session in its
    // pool — until prepare_hibernate runs with EVICTS=1.
    seed_cold_binding(&db, "sub-a", "a", &sess_a);
    // Two-phase lifecycle: activate the session so it becomes an LRU
    // candidate. Even though the DB binding is pre-flipped to is_hot=0,
    // the sidecar still holds the session and needs it in SESS_ORDER
    // to return it as a candidate.
    executor
        .activate_session(&sess_a, TEST_PROVIDER_ID)
        .await
        .unwrap();

    // Second delegate — sub-b. The mock pool is full (sess-a still in it),
    // so turn_auto returns HOT_SESSION_LIMIT_REACHED with candidate=sess-a.
    // evict_session: prepare_hibernate removes sess-a from the mock pool
    // (EVICTS=1), DB check finds is_hot=0 → AlreadyEvicted. Retry turn_auto
    // succeeds because the mock pool now has room.
    let out2 = executor
        .execute(&evict_input("sub-b"))
        .await
        .expect("AlreadyEvicted retry must succeed");
    assert_eq!(out2.status, TaskStatus::Completed);
    assert!(
        out2.adapter_session_id.is_some(),
        "retry turn_auto must return a session id"
    );

    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn executor_eviction_already_evicted_exhaustion_surfaces_hot_limit() {
    // Concurrent eviction race: the DB binding is already is_hot=0
    // (concurrent evictor flipped it), but the concurrent evictor's close
    // NEVER completes — the sidecar pool stays full. Without
    // BUSYTOK_MOCK_PREPARE_HIBERNATE_EVICTS, prepare_hibernate returns the
    // memory delta but does NOT remove the session from the mock pool.
    // evict_session returns AlreadyEvicted, all 5 retries hit
    // HOT_SESSION_LIMIT_REACHED, and the executor surfaces HotSessionLimit.
    let db: SharedDb = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    // NOTE: BUSYTOK_MOCK_PREPARE_HIBERNATE_EVICTS is NOT set — the mock pool
    // keeps the session, simulating a concurrent close that never completes.
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, Some(db.clone()));

    // First delegate succeeds — sub-a gets a hot session.
    let out1 = executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");
    let sess_a = out1.adapter_session_id.expect("must have session id");

    // Pre-flip the DB binding to is_hot=0 (concurrent evictor already
    // committed the eviction). The sidecar still holds the session — and
    // will keep holding it (no EVICTS flag).
    seed_cold_binding(&db, "sub-a", "a", &sess_a);
    // Two-phase lifecycle: activate the session so it becomes an LRU
    // candidate. Without activation, the mock would return all_busy=true
    // immediately (never exercising the eviction + retry-exhaustion path).
    executor
        .activate_session(&sess_a, TEST_PROVIDER_ID)
        .await
        .unwrap();

    // Second delegate — sub-b. evict_session returns AlreadyEvicted (is_hot=0).
    // Retry turn_auto keeps hitting HOT_SESSION_LIMIT_REACHED because the mock
    // pool never frees the slot. After EVICT_WAIT_MAX_RETRIES, the executor
    // surfaces SubagentError::HotSessionLimit.
    let err = match executor.execute(&evict_input("sub-b")).await {
        Ok(_) => panic!("expected HotSessionLimit after AlreadyEvicted exhaustion, got success"),
        Err(e) => e,
    };
    let downcasted = err.downcast_ref::<SubagentError>();
    assert!(
        downcasted.is_some(),
        "error must downcast to SubagentError, got bare anyhow: {err}"
    );
    match downcasted.unwrap() {
        SubagentError::HotSessionLimit { candidate } => {
            // candidate is empty — the executor surfaces it without a
            // specific candidate because the retry exhausted waiting for
            // a concurrent close (not because all sessions were busy).
            assert!(
                candidate.is_empty(),
                "exhausted-retry HotSessionLimit should have empty candidate, got: {candidate}"
            );
        }
        other => panic!(
            "expected SubagentError::HotSessionLimit after AlreadyEvicted exhaustion, got {other:?}"
        ),
    }

    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn executor_eviction_already_evicted_via_session_not_found() {
    // The second AlreadyEvicted path: prepare_hibernate returns
    // SESSION_NOT_FOUND (-32001) because a concurrent evictor already
    // closed the session. The mock (with PREPARE_HIBERNATE_NOT_FOUND=1)
    // removes the session from its pool and returns -32001. The executor
    // intercepts -32001 → AlreadyEvicted → retry turn_auto succeeds (slot
    // is now free). Unlike the is_hot=0 path, no DB pre-flip is needed —
    // the -32001 short-circuits before the DB check.
    let db: SharedDb = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.env.insert(
        "BUSYTOK_MOCK_PREPARE_HIBERNATE_NOT_FOUND".into(),
        "1".into(),
    );
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, Some(db.clone()));

    // First delegate succeeds — sub-a gets a hot session.
    let out1 = executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");
    let sess_a = out1.adapter_session_id.expect("must have session id");
    // Seed a hot binding (normal state — no pre-flip; the -32001 from
    // prepare_hibernate is what triggers AlreadyEvicted, not the DB state).
    seed_hot_binding(&db, "sub-a", "a", &sess_a);
    // Two-phase lifecycle: activate the session so it becomes an LRU
    // candidate for the eviction flow.
    executor
        .activate_session(&sess_a, TEST_PROVIDER_ID)
        .await
        .unwrap();

    // Second delegate — sub-b. turn_auto returns HOT_SESSION_LIMIT_REACHED
    // with candidate=sess-a. evict_session calls prepare_hibernate(sess-a):
    // mock removes sess-a from its pool + returns -32001. Executor
    // intercepts → AlreadyEvicted. Retry turn_auto succeeds (pool now empty).
    let out2 = executor
        .execute(&evict_input("sub-b"))
        .await
        .expect("SESSION_NOT_FOUND AlreadyEvicted retry must succeed");
    assert_eq!(out2.status, TaskStatus::Completed);
    assert!(
        out2.adapter_session_id.is_some(),
        "retry turn_auto must return a session id"
    );

    supervisor.shutdown().await.unwrap();
}

#[tokio::test]
async fn executor_evicted_retry_hot_limit_surfaces_transient() {
    // Concurrent slot takeover after eviction (P0 regression):
    //
    // sub-a holds the only hot session. sub-b triggers eviction (Evicted
    // outcome — close succeeds). But BUSYTOK_MOCK_REFILL_AFTER_CLOSE=1
    // simulates a concurrent task grabbing the freed slot before sub-b's
    // retry. The retry turn_auto returns HOT_SESSION_LIMIT_REACHED again.
    //
    // Before the fix, the Evicted path only allowed 1 retry, and the retry
    // guard only matched AlreadyEvicted — so HOT_SESSION_LIMIT_REACHED on
    // the Evicted retry fell through to the generic failed_after_eviction
    // path, returning a raw error (logged as error_kind=Unknown).
    //
    // After the fix, both Evicted and AlreadyEvicted retry
    // HOT_SESSION_LIMIT_REACHED with bounded backoff (EVICT_WAIT_MAX_RETRIES=5).
    // After exhaustion, the executor surfaces SubagentError::HotSessionLimit
    // (NOT a generic error) so execute_and_persist can re-queue the task.
    let db: SharedDb = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.env
        .insert("BUSYTOK_MOCK_REFILL_AFTER_CLOSE".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, Some(db.clone()));

    // First delegate succeeds — sub-a gets a hot session.
    let out1 = executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");
    let sess_a = out1.adapter_session_id.expect("must have session id");
    // Seed a hot binding (normal state — the Evicted path will flip it).
    seed_hot_binding(&db, "sub-a", "a", &sess_a);
    // Two-phase lifecycle: activate the session so it becomes an LRU
    // candidate. Without activation, the mock would return all_busy=true
    // immediately (never exercising the eviction + refill + retry path).
    executor
        .activate_session(&sess_a, TEST_PROVIDER_ID)
        .await
        .unwrap();

    // Second delegate — sub-b. turn_auto returns HOT_SESSION_LIMIT_REACHED
    // with candidate=sess-a. evict_session succeeds (Evicted): prepare_hibernate
    // returns memory delta, close removes sess-a from pool. But
    // REFILL_AFTER_CLOSE adds a fake session to the pool, so the retry
    // turn_auto for sub-b sees the pool as full and returns
    // HOT_SESSION_LIMIT_REACHED again. After EVICT_WAIT_MAX_RETRIES, the
    // executor surfaces SubagentError::HotSessionLimit (NOT a generic error).
    let err = match executor.execute(&evict_input("sub-b")).await {
        Ok(_) => panic!("expected HotSessionLimit after Evicted retry exhaustion, got success"),
        Err(e) => e,
    };
    let downcasted = err.downcast_ref::<SubagentError>();
    assert!(
        downcasted.is_some(),
        "error must downcast to SubagentError (not bare anyhow) so execute_and_persist can re-queue, got: {err}"
    );
    match downcasted.unwrap() {
        SubagentError::HotSessionLimit { candidate } => {
            // candidate is empty — the executor surfaces it without a
            // specific candidate because the retry exhausted waiting for
            // the slot to free (not because all sessions were busy).
            assert!(
                candidate.is_empty(),
                "exhausted-retry HotSessionLimit should have empty candidate, got: {candidate}"
            );
        }
        other => panic!(
            "expected SubagentError::HotSessionLimit after Evicted retry exhaustion, got {other:?}"
        ),
    }

    supervisor.shutdown().await.unwrap();
}

// --- parse_turn_auto_result memory_update extraction (Plan 4 Task 4) ---
//
// `parse_turn_auto_result` must extract `result.memory_update` from the
// sidecar's turn_auto response into `ExecutorOutput.memory_update` (Plan 4
// replaces the Task 3 `MemoryUpdate::default()` placeholder). Two cases:
//   1. memory_update present → parse current_state_summary, key_files,
//      decisions, open_questions verbatim into MemoryUpdate.
//   2. memory_update absent → MemoryUpdate::default() (preserves existing
//      memory per §6.2: current_state_summary None → no overwrite).

#[test]
fn parse_turn_auto_result_extracts_memory_update() {
    let resp = serde_json::json!({
        "adapter_session_id": "sess-1",
        "session_reused": false,
        "status": "completed",
        "result": {
            "task_summary": "did thing",
            "memory_update": {
                "current_state_summary": "new state",
                "key_files": [{"path": "src/a.ts", "reason": "r", "last_seen_at_ms": 1, "score": 2}],
                "decisions": ["decide"],
                "open_questions": [{"question": "q?", "status": "open", "created_at_ms": 1, "last_seen_at_ms": 1}],
            },
        },
        "usage": {"model": "m", "provider": "p", "input_tokens": 1, "output_tokens": 1, "cache_read_tokens": 0, "cache_write_tokens": 0, "cost_usd": 0.0},
    });
    let out = busytok_subagent::sidecar::executor::parse_turn_auto_result_for_test(&resp);
    assert_eq!(
        out.memory_update.current_state_summary.as_deref(),
        Some("new state")
    );
    assert_eq!(out.memory_update.key_files.len(), 1);
    assert_eq!(out.memory_update.key_files[0].path, "src/a.ts");
    assert_eq!(out.memory_update.decisions, vec!["decide".to_string()]);
    assert_eq!(out.memory_update.open_questions.len(), 1);
}

#[test]
fn parse_turn_auto_result_omits_memory_update_when_absent() {
    let resp = serde_json::json!({
        "adapter_session_id": "sess-1",
        "session_reused": false,
        "status": "completed",
        "result": {"task_summary": "did thing"},
        "usage": {"model": "m", "provider": "p", "input_tokens": 1, "output_tokens": 1, "cache_read_tokens": 0, "cache_write_tokens": 0, "cost_usd": 0.0},
    });
    let out = busytok_subagent::sidecar::executor::parse_turn_auto_result_for_test(&resp);
    assert!(out.memory_update.current_state_summary.is_none());
    assert!(out.memory_update.key_files.is_empty());
}

/// `SidecarTaskExecutor::pool()` returns the underlying `WorkerPool` handle
/// so wiring code (and tests) can reach the per-provider supervisors. The
/// getter must return the same `Arc` passed to `with_pool`.
#[test]
fn executor_pool_getter_returns_underlying_pool() {
    let harness = make_harness();
    let pool_from_executor = harness._executor.pool();
    // The pool returned by the getter must be the same Arc the harness holds.
    assert!(
        Arc::ptr_eq(pool_from_executor, &harness.pool),
        "pool() must return the same Arc<WorkerPool> passed to with_pool"
    );
}

#[tokio::test]
async fn cancel_for_task_is_noop_without_worker() {
    let gate = Arc::new(PressureGate::new());
    let pool = Arc::new(WorkerPool::new(
        mock_config(),
        None,
        HashMap::new(),
        Some(gate),
        SubagentResourcePolicyConfig::default(),
    ));
    let executor = SidecarTaskExecutor::with_pool(pool, None);

    executor
        .cancel_for_task("sub-no-worker", TEST_PROVIDER_ID, "task-1")
        .await
        .expect("cancel without a worker is best-effort and should succeed");
}

#[tokio::test]
async fn cancel_for_task_is_noop_when_worker_is_not_running() {
    let (_pool, executor, _supervisor, _holder) = make_pool_with_config(mock_config(), None);

    // The worker is registered but its child process has not started.
    // Cancellation must not spawn a sidecar solely to send cancel.
    executor
        .cancel_for_task("sub-stopped-worker", TEST_PROVIDER_ID, "task-2")
        .await
        .expect("cancel for a stopped worker should be a no-op");
}

#[tokio::test]
async fn close_session_is_noop_without_worker_or_when_stopped() {
    let gate = Arc::new(PressureGate::new());
    let pool = Arc::new(WorkerPool::new(
        mock_config(),
        None,
        HashMap::new(),
        Some(gate),
        SubagentResourcePolicyConfig::default(),
    ));
    let executor = SidecarTaskExecutor::with_pool(pool, None);
    executor
        .close_session("session-no-worker", TEST_PROVIDER_ID)
        .await
        .expect("close without a worker should be idempotent");

    let (_pool, executor, _supervisor, _holder) = make_pool_with_config(mock_config(), None);
    executor
        .close_session("session-stopped-worker", TEST_PROVIDER_ID)
        .await
        .expect("close for a stopped worker should be idempotent");
}

#[tokio::test]
async fn cancel_for_task_reaches_running_sidecar() {
    let h = make_harness();
    let r = h
        .manager
        .delegate(req("cancel-rpc", "complete before cancel"))
        .await
        .unwrap();
    assert_eq!(await_task_done(&h.manager, &r.task_id).await, "completed");

    // The completed session is still owned by the running worker. This call
    // exercises the task-scoped RPC path without needing an in-flight model
    // request; the fixture intentionally returns method-not-found so the
    // caller's diagnostics path is covered.
    let err = h
        ._executor
        .cancel_for_task(&r.subagent_id, TEST_PROVIDER_ID, &r.task_id)
        .await
        .expect_err("the mock sidecar intentionally lacks session.cancel");
    assert!(
        err.to_string().contains("session.cancel") || err.to_string().contains("method not found"),
        "cancel RPC errors must preserve diagnostics: {err}"
    );
    h.supervisor.shutdown().await.unwrap();
}

// --- Two-phase session lifecycle regression tests ---
//
// The two-phase lifecycle (pending → active) closes the P0 timing window
// where `purge_session` could close a session waiting for DB binding commit.
// New sessions start as `pending` (NOT in LRU, NOT evictable). Rust calls
// `session.activate` after committing the DB binding. On commit failure,
// Rust calls `session.close` to clean up the orphaned pending session.
//
// These tests verify:
//   1. A pending session is NOT evicted when the pool is full.
//   2. The manager activates the session after a successful delegate.
//   3. The manager closes the session on commit failure (no is_hot=1 leak).

/// A pending session (not yet activated) must NOT be evicted when the pool
/// is full. The sidecar returns `all_busy=true` (no LRU candidates), and the
/// executor surfaces `HotSessionLimit` (transient). After activating the
/// pending session, a subsequent delegate triggers normal eviction — proving
/// the pending session survived the transient error and was not closed.
#[tokio::test]
async fn pending_session_not_evicted_when_pool_full() {
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, Some(db.clone()));

    // First delegate — fills the pool (max_hot=1). The session starts in
    // SESS_PENDING (NOT activated, NOT in LRU).
    let out1 = executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");
    let sess_a = out1.adapter_session_id.expect("must have session id");

    // NOTE: We deliberately do NOT call activate_session — simulating the
    // timing window where turn_auto succeeded but the DB binding hasn't
    // been committed yet.

    // Second delegate — triggers HOT_SESSION_LIMIT_REACHED. Because
    // SESS_ORDER is empty (all sessions pending), the mock returns
    // all_busy=true. The executor must surface HotSessionLimit (transient)
    // and must NOT close the pending session.
    let result = executor.execute(&evict_input("sub-b")).await;
    assert!(
        result.is_err(),
        "eviction must surface an error when pool is full with only pending sessions"
    );
    let err = match result {
        Ok(_) => panic!("expected error, got success"),
        Err(e) => e,
    };
    let downcasted = err.downcast_ref::<SubagentError>();
    assert!(
        downcasted.is_some(),
        "error must downcast to SubagentError, got bare anyhow: {err}"
    );
    match downcasted.unwrap() {
        SubagentError::HotSessionLimit { candidate } => {
            assert!(
                candidate.is_empty(),
                "candidate must be empty (all_busy=true), got: {candidate}"
            );
        }
        other => panic!(
            "expected SubagentError::HotSessionLimit (transient), got {other:?} — \
             pending sessions must cause a transient retry, not a fatal error"
        ),
    }

    // The pending session survived — it was NOT closed by the eviction
    // attempt. Activate it now (simulates the manager's post-commit
    // activate), then run a third delegate that should trigger normal
    // eviction (session is now in SESS_ORDER / LRU).
    seed_hot_binding(&db, "sub-a", "a", &sess_a);
    executor
        .activate_session(&sess_a, TEST_PROVIDER_ID)
        .await
        .unwrap();

    let out3 = executor
        .execute(&evict_input("sub-c"))
        .await
        .expect("eviction must succeed after activation — pending session survived");
    assert_eq!(out3.status, TaskStatus::Completed);

    supervisor.shutdown().await.unwrap();
}

/// After a successful `delegate()`, the manager must call `activate_session`
/// to move the session from pending to LRU. Verify with `maxHot=1`: a second
/// delegate for a DIFFERENT subagent must trigger normal eviction (not
/// all_busy), proving the first session was activated after its commit.
///
/// If activate was NOT called, sub-a's session stays in SESS_PENDING (not in
/// SESS_ORDER/LRU) → the sidecar returns `all_busy=true` → `HotSessionLimit`
/// → the task is re-queued indefinitely → `await_task_done` panics.
#[tokio::test]
async fn delegate_activates_session_after_successful_commit() {
    let mut env = HashMap::new();
    env.insert(
        "BUSYTOK_MOCK_HOT_SESSION_LIMIT".to_string(),
        "1".to_string(),
    );
    let h = make_harness_with_env(env);

    // First delegate — sub-a. Manager commits the binding and calls
    // activate_session, moving the session from SESS_PENDING to SESS_ORDER.
    let r1 = h
        .manager
        .delegate(req("reviewer-a", "first turn"))
        .await
        .unwrap();
    assert_eq!(r1.status, TaskStatus::Running);
    let status1 = await_task_done(&h.manager, &r1.task_id).await;
    assert_eq!(status1, "completed");

    // Second delegate — DIFFERENT subagent. With maxHot=1, the pool is full.
    // If activate was called on sub-a's session, it's in SESS_ORDER (LRU) →
    // the sidecar returns it as an evictable candidate → eviction succeeds →
    // delegate completes.
    //
    // If activate was NOT called, sub-a's session is still in SESS_PENDING
    // (not in LRU) → the sidecar returns all_busy=true → HotSessionLimit →
    // task is re-queued → await_task_done panics (never reaches terminal
    // status).
    let r2 = h
        .manager
        .delegate(req("reviewer-b", "second turn"))
        .await
        .unwrap();
    assert_eq!(r2.status, TaskStatus::Running);
    let status2 = await_task_done(&h.manager, &r2.task_id).await;
    assert_eq!(
        status2, "completed",
        "second delegate must complete — eviction succeeded, proving activate was called"
    );

    // Verify: two different subagents (different names → different IDs).
    assert_ne!(
        r1.subagent_id, r2.subagent_id,
        "different subagent names must resolve to different subagents"
    );

    // Verify: sub-b has a hot binding (it's the active hot session now).
    // sub-a's binding was flipped to warm by the eviction flow.
    {
        let db_guard = h.db.lock().unwrap();
        let binding_b = db_guard
            .subagent_hot_binding(&r2.subagent_id, "pi")
            .unwrap();
        assert!(binding_b.is_some(), "sub-b must have a hot binding");
        assert_eq!(binding_b.unwrap().is_hot, 1);
    }

    h.supervisor.shutdown().await.unwrap();
}

/// When the DB binding commit fails, the manager must call `close_session`
/// to clean up the orphaned pending session. Verify: the task fails, and
/// no `is_hot=1` binding leaks into the DB.
#[tokio::test]
async fn delegate_closes_session_on_commit_failure() {
    let h = make_harness();

    // Drop the bindings table to force `subagent_commit_hot_binding_and_status`
    // to fail. This simulates a DB-level commit failure.
    {
        let db_guard = h.db.lock().unwrap();
        db_guard
            .conn()
            .execute("DROP TABLE subagent_harness_bindings", [])
            .unwrap();
    }

    // Delegate — execute succeeds (session created in SESS_PENDING), but
    // the binding commit fails. The manager must call close_session to
    // clean up the orphaned pending session.
    let r = h
        .manager
        .delegate(req("commit-fail-sub", "do"))
        .await
        .unwrap();
    assert_eq!(r.status.as_str(), "running");
    let final_status = await_task_done(&h.manager, &r.task_id).await;
    assert_eq!(
        final_status, "failed",
        "task must fail when binding commit fails"
    );

    // Verify: the task error mentions the database failure.
    let task_row = {
        let db_guard = h.db.lock().unwrap();
        db_guard.subagent_get_task(&r.task_id).unwrap().unwrap()
    };
    let err_str = task_row.error.as_deref().unwrap_or("");
    assert!(
        err_str.contains("database error") || err_str.contains("Store"),
        "commit failure should surface as store error, got: {err_str}"
    );

    // Verify: no is_hot=1 binding leaked into the DB. The commit failed,
    // so no binding row should exist with is_hot=1 for this subagent.
    // (The table was dropped, so no rows exist at all — this is the
    // strongest possible guarantee that no hot binding leaked.)
    //
    // We can't query the dropped table, but we CAN verify that the
    // subagent's logical status is NOT 'hot' (it should be 'cold' or
    // 'failed' because the binding was never committed).
    {
        let db_guard = h.db.lock().unwrap();
        let sub = db_guard.subagent_get_logical(&r.subagent_id).unwrap();
        if let Some(sub) = sub {
            assert_ne!(
                sub.status, "hot",
                "subagent must NOT be 'hot' when binding commit failed — \
                 no is_hot=1 binding should exist"
            );
        }
    }

    // Verify: the manager called `session.close` on the sidecar to clean up
    // the orphaned pending session. The mock sidecar tracks total closes in
    // its `adapter.health` response (`total_closes` field).
    let handle = h.supervisor.ensure_started().await.unwrap();
    let health = handle.health().await.unwrap();
    let total_closes = health["total_closes"].as_u64().unwrap_or(0);
    assert!(
        total_closes >= 1,
        "manager must call session.close on binding commit failure — \
         expected total_closes >= 1, got {total_closes} \
         (orphaned pending session was not cleaned up)"
    );

    h.supervisor.shutdown().await.unwrap();
}

/// A failure in the first persistence phase (after the task row is written)
/// must still close the pending sidecar session. Early `?` returns used to
/// bypass the lifecycle cleanup block and leak a non-evictable session.
#[tokio::test]
async fn delegate_closes_session_on_usage_persist_failure() {
    let h = make_harness();
    {
        let db_guard = h.db.lock().unwrap();
        db_guard
            .conn()
            .execute("DROP TABLE subagent_usage_records", [])
            .unwrap();
    }

    let r = h
        .manager
        .delegate(req("usage-persist-fail", "do"))
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if h.manager.task_status(&r.task_id).unwrap().as_deref() == Some("failed") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "phase-one persistence failure must eventually mark the task failed"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let handle = h.supervisor.ensure_started().await.unwrap();
    let health = handle.health().await.unwrap();
    assert!(
        health["total_closes"].as_u64().unwrap_or(0) >= 1,
        "phase-one persistence failure must close the pending session"
    );
    h.supervisor.shutdown().await.unwrap();
}

/// When `session.activate` RPC fails, the manager must:
/// 1. Roll back the DB binding (flip `is_hot=0`, status → warm/cold).
/// 2. Close the orphaned pending session in the sidecar.
/// 3. The task itself still completes (the turn already succeeded — the
///    user got their result; only the hot binding is lost).
///
/// After rollback, a DIFFERENT subagent's delegate must succeed — proving
/// the capacity slot was freed (no stuck pending session).
#[tokio::test]
async fn delegate_rolls_back_binding_when_activate_fails() {
    let mut env = HashMap::new();
    env.insert("BUSYTOK_MOCK_ACTIVATE_FAILS".to_string(), "1".to_string());
    let h = make_harness_with_env(env);

    // Delegate — turn_auto succeeds, DB binding is committed, but
    // session.activate fails (simulated by BUSYTOK_MOCK_ACTIVATE_FAILS=1).
    // The manager must roll back the binding and close the pending session.
    let r = h
        .manager
        .delegate(req("sub-activate-fail", "first turn"))
        .await
        .unwrap();
    assert_eq!(r.status, TaskStatus::Running);
    let status = await_task_done(&h.manager, &r.task_id).await;
    // The task still completes — the turn succeeded; only the hot binding
    // was rolled back.
    assert_eq!(
        status, "completed",
        "task must complete even when activate fails — the turn already succeeded"
    );

    // The task status is set to "completed" BEFORE the activate call (the
    // turn already succeeded — the user got their result). The activate/
    // rollback runs in the same background task but AFTER the status write.
    // Poll the sidecar's total_closes to wait for the activate/rollback to
    // complete: when total_closes >= 1, the pending session was closed after
    // the activate failure.
    let handle = h.supervisor.ensure_started().await.unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let health = handle.health().await.unwrap();
        let total_closes = health["total_closes"].as_u64().unwrap_or(0);
        if total_closes >= 1 {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "activate/rollback did not complete within 5s — \
                 total_closes still 0"
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // Verify: the binding was rolled back — is_hot=0, status='closed'
    // (NOT is_hot=1/status='hot' which would indicate a stuck binding).
    {
        let db_guard = h.db.lock().unwrap();
        // `subagent_hot_binding` filters on is_hot=1, so it returns None after
        // rollback — assert None to verify no hot binding lingers.
        let hot_binding = db_guard.subagent_hot_binding(&r.subagent_id, "pi").unwrap();
        assert!(
            hot_binding.is_none(),
            "no hot binding should exist after activate failure rollback"
        );
        // Query the raw row (without the is_hot=1 filter) to verify the
        // binding was flipped to is_hot=0, status='closed'.
        let binding_state: (i32, String) = db_guard
            .conn()
            .query_row(
                "SELECT is_hot, status FROM subagent_harness_bindings \
                 WHERE subagent_id = ?1 AND harness = 'pi'",
                rusqlite::params![&r.subagent_id],
                |row| Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("binding row must exist after activate failure rollback");
        assert_eq!(
            binding_state.0, 0,
            "binding is_hot must be 0 (rolled back after activate failure)"
        );
        assert_eq!(
            binding_state.1, "closed",
            "binding status must be 'closed' (rolled back after activate failure)"
        );
        // The logical subagent status must NOT be 'hot'.
        let sub = db_guard.subagent_get_logical(&r.subagent_id).unwrap();
        if let Some(sub) = sub {
            assert_ne!(
                sub.status, "hot",
                "subagent must NOT be 'hot' after activate failure rollback"
            );
        }
    }

    // Critical: a different subagent's delegate must succeed. With
    // max_hot=3 (default), the pool has capacity — but this proves no
    // capacity is permanently occupied by the activate failure.
    let r2 = h
        .manager
        .delegate(req("sub-after-rollback", "second turn"))
        .await
        .unwrap();
    assert_eq!(r2.status, TaskStatus::Running);
    let status2 = await_task_done(&h.manager, &r2.task_id).await;
    assert_eq!(
        status2, "completed",
        "second delegate must complete — activate failure did not permanently block capacity"
    );

    // Wait for the second delegate's activate/rollback to complete before
    // shutting down (same timing issue as the first delegate).
    let deadline2 = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let health = handle.health().await.unwrap();
        let total_closes = health["total_closes"].as_u64().unwrap_or(0);
        if total_closes >= 2 {
            break;
        }
        if std::time::Instant::now() > deadline2 {
            panic!(
                "second activate/rollback did not complete within 5s — \
                 total_closes = {total_closes}, expected >= 2"
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    h.supervisor.shutdown().await.unwrap();
}

/// P3 test: per-session activate failure + sequential recovery.
///
/// Scenario:
/// 1. Delegate A for sub-X: turn_auto creates session S1, binding committed,
///    but `session.activate(S1)` fails (per-session mock: S1 in fail list).
///    The mock removes S1 from the sub→sess mapping on activate failure,
///    so the next delegate for sub-X creates a NEW session S2.
/// 2. A's rollback completes: binding flipped to is_hot=0, S1 closed.
/// 3. Delegate B for sub-X (same subagent): turn_auto creates session S2
///    (different sid), binding committed, activate succeeds.
/// 4. Verify: B's binding is hot (is_hot=1, sid=S2), B's task completed.
///
/// NOTE: This test verifies SEQUENTIAL recovery — B starts after A's rollback
/// completes. The P1-1 concurrent guard (skip rollback when binding has a
/// different sid) is a defense-in-depth measure that requires a multi-
/// threaded mock sidecar to test reliably. With the current single-threaded
/// mock, B's turn_auto can only be processed after A's activate returns,
/// and A's rollback runs immediately after — so B's commit always lands
/// after A's rollback. The guard is exercised in production when real
/// concurrency allows B's commit to win the race.
#[tokio::test]
async fn delegate_recovers_after_per_session_activate_failure() {
    // Fail activate ONLY for the first session (pi_sess_mock_1).
    // The mock removes the failed session from the sub→sess mapping so
    // the next delegate creates a new session (pi_sess_mock_2).
    let mut env = HashMap::new();
    env.insert(
        "BUSYTOK_MOCK_ACTIVATE_FAILS_FOR_SESSION".to_string(),
        "pi_sess_mock_1".to_string(),
    );
    let h = make_harness_with_env(env);

    // Delegate A — turn_auto succeeds, binding committed with sid=S1,
    // activate fails (per-session). Task A completes (turn succeeded).
    let r1 = h
        .manager
        .delegate(req("sub-per-session-fail", "first turn"))
        .await
        .unwrap();
    assert_eq!(r1.status, TaskStatus::Running);
    let status1 = await_task_done(&h.manager, &r1.task_id).await;
    assert_eq!(
        status1, "completed",
        "task A must complete — turn succeeded; only activate failed"
    );

    // Wait for A's activate-failure rollback to complete (total_closes >= 1:
    // A's orphaned pending session S1 was closed).
    let handle = h.supervisor.ensure_started().await.unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let health = handle.health().await.unwrap();
        let total_closes = health["total_closes"].as_u64().unwrap_or(0);
        if total_closes >= 1 {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "A's activate/rollback did not complete within 5s — \
                 total_closes still 0"
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // Verify: A's binding was rolled back (no hot binding for sub-X).
    {
        let db_guard = h.db.lock().unwrap();
        let binding = db_guard
            .subagent_hot_binding(&r1.subagent_id, "pi")
            .unwrap();
        assert!(
            binding.is_none(),
            "A's hot binding must be rolled back after activate failure"
        );
    }

    // Delegate B for the SAME subagent — the mock's sub→sess mapping was
    // cleared for S1 on activate failure, so B creates a NEW session S2.
    // B's binding commits with sid=S2, activate succeeds.
    let r2 = h
        .manager
        .delegate(req("sub-per-session-fail", "second turn"))
        .await
        .unwrap();
    assert_eq!(r2.status, TaskStatus::Running);
    let status2 = await_task_done(&h.manager, &r2.task_id).await;
    assert_eq!(
        status2, "completed",
        "task B must complete — recovery after per-session activate failure"
    );

    // Poll until the binding is hot (B's activate completed).
    let deadline2 = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let active_session_id = {
            let db_guard = h.db.lock().unwrap();
            db_guard
                .subagent_hot_binding(&r1.subagent_id, "pi")
                .unwrap()
                .filter(|binding| binding.is_hot == 1)
                .and_then(|binding| binding.adapter_session_id)
        };
        if let Some(adapter_session_id) = active_session_id {
            assert_ne!(
                Some(adapter_session_id.as_str()),
                Some("pi_sess_mock_1"),
                "B's binding must NOT point to A's failed session S1"
            );
            break;
        }
        if std::time::Instant::now() > deadline2 {
            panic!("B's binding did not become hot within 5s");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    h.supervisor.shutdown().await.unwrap();
}

/// When the sidecar restarts (losing all sessions), a subsequent delegate
/// for the same subagent must recover: the stale hot binding is replaced
/// with a fresh session, and activate succeeds on the new session.
#[tokio::test]
async fn delegate_recovers_after_sidecar_restart() {
    let h = make_harness();

    // First delegate — succeeds normally.
    let r1 = h
        .manager
        .delegate(req("sub-restart", "first turn"))
        .await
        .unwrap();
    assert_eq!(r1.status, TaskStatus::Running);
    let status1 = await_task_done(&h.manager, &r1.task_id).await;
    assert_eq!(status1, "completed");

    // Restart the sidecar — all sessions are lost.
    h.supervisor.shutdown().await.unwrap();

    // Second delegate for the SAME subagent — the manager finds the
    // existing hot binding, tries to reuse the session. The sidecar
    // has no record of the old session, so it creates a new one. The
    // new session is activated successfully.
    let r2 = h
        .manager
        .delegate(req("sub-restart", "second turn"))
        .await
        .unwrap();
    assert_eq!(r2.status, TaskStatus::Running);
    let status2 = await_task_done(&h.manager, &r2.task_id).await;
    assert_eq!(
        status2, "completed",
        "delegate after sidecar restart must complete"
    );

    // Verify: the binding is hot (the new session was activated).
    {
        let db_guard = h.db.lock().unwrap();
        let binding = db_guard
            .subagent_hot_binding(&r2.subagent_id, "pi")
            .unwrap();
        assert!(binding.is_some(), "binding must exist");
        let b = binding.unwrap();
        assert_eq!(
            b.is_hot, 1,
            "binding must be hot after successful re-delegate"
        );
    }

    h.supervisor.shutdown().await.unwrap();
}

/// Activation is part of the successful hot-binding commit protocol. If the
/// worker disappeared between `turn_auto` and the post-commit lifecycle step,
/// silently returning `Ok(())` would leave a durable `is_hot=1` binding with
/// no sidecar session. The manager must receive an error so its existing
/// rollback path can restore the warm/cold invariant.
#[tokio::test]
async fn activate_session_errors_when_worker_is_missing() {
    let pool = Arc::new(WorkerPool::new(
        mock_config(),
        None,
        make_providers(),
        None,
        SubagentResourcePolicyConfig::default(),
    ));
    let executor = SidecarTaskExecutor::with_pool(Arc::clone(&pool), None);

    let err = executor
        .activate_session("session-without-worker", TEST_PROVIDER_ID)
        .await
        .expect_err("activation must fail when no worker owns the session");

    assert!(
        err.to_string().contains("worker") && err.to_string().contains(TEST_PROVIDER_ID),
        "error should identify the missing provider worker, got: {err}"
    );
}

/// A worker can still exist in the pool after its child sidecar exited. The
/// activation path must restart the worker and surface the session-not-found
/// response, rather than treating `try_is_running() == false` as success.
#[tokio::test]
async fn activate_session_surfaces_missing_session_after_worker_restart() {
    let (pool, executor, supervisor, _holder) = make_pool_with_config(mock_config(), None);
    supervisor.shutdown().await.unwrap();

    let err = executor
        .activate_session("session-lost-with-sidecar", TEST_PROVIDER_ID)
        .await
        .expect_err("activation must fail when the restarted sidecar lost the session");

    assert!(
        err.to_string().contains("activate")
            && (err.to_string().contains("not found")
                || err.to_string().contains("SESSION_NOT_FOUND")),
        "error should preserve the activation/session-loss diagnosis, got: {err}"
    );

    // Keep the pool alive until the RPC has completed; this also makes the
    // ownership of the restarted supervisor explicit in the test.
    drop(pool);
}
