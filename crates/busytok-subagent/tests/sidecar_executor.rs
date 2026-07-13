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
                    adapter_session_id: Some(sess_a),
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

    // Second delegate — different subagent, triggers eviction.
    let input2 = ExecutorInput {
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
//   1. The sidecar returns HOT_SESSION_LIMIT_REACHED with data.candidate=X,
//      but the DB has no hot binding for X (sidecar/service out of sync).
//      The executor must error out rather than silently succeeding or
//      retrying indefinitely.
//   2. session.close fails during eviction (BUSYTOK_MOCK_CLOSE_FAILS=1).
//      The executor must abort (return Err) — the DB has been flipped to
//      closed/warm but the sidecar still holds the session hot, so
//      retrying turn_auto would hit HOT_SESSION_LIMIT_REACHED again and
//      diverge. State divergence risk → fatal.

#[tokio::test]
async fn executor_eviction_fails_when_db_has_no_binding_for_candidate() {
    // The sidecar returns HOT_SESSION_LIMIT_REACHED with data.candidate=X,
    // but the DB has no hot binding for X (sidecar and busytok-service are
    // out of sync). The executor must error out rather than silently
    // succeeding or retrying indefinitely.
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let mut cfg = mock_config();
    cfg.max_hot_sessions = 1;
    // HOT_LIMIT=1 means the sidecar's pool is full after 1 session.
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.health_interval = Duration::from_secs(3600);
    let (_pool, executor, supervisor, _holder) = make_pool_with_config(cfg, Some(db.clone()));

    // First delegate — fills the sidecar's pool (max_hot=1)
    let input1 = ExecutorInput {
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
    let _out1 = executor
        .execute(&input1)
        .await
        .expect("first delegate must succeed");

    // NOTE: We deliberately do NOT persist the hot binding to the DB.
    // This simulates the out-of-sync state: the sidecar has session X in
    // its pool, but the DB has no hot binding for X. When the sidecar
    // returns data.candidate=X, evict_session's find_hot_binding_by_session
    // will return None.

    // Second delegate — triggers HOT_SESSION_LIMIT_REACHED with data.candidate,
    // but eviction can't find the binding in the DB → must error out.
    let input2 = ExecutorInput {
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
        "eviction must fail when the DB has no hot binding for the candidate"
    );
    let err = match result {
        Ok(_) => panic!("expected error, got success"),
        Err(e) => e,
    };
    // Bug 2 fix: the error must be a structured SubagentError::HotSessionStateDivergence,
    // NOT a bare anyhow wrapped as SubagentError::Store ("database error") and
    // NOT HotSessionLimit (which would mislead callers into retrying for capacity).
    // This is a state sync issue — the sidecar holds a session the DB has no
    // binding for. Downcast to verify the structured variant survives the
    // anyhow round-trip.
    let downcasted = err.downcast_ref::<SubagentError>();
    assert!(
        downcasted.is_some(),
        "error must downcast to SubagentError, got bare anyhow: {err}"
    );
    match downcasted.unwrap() {
        SubagentError::HotSessionStateDivergence(msg) => {
            assert!(
                msg.contains("pi_sess_mock_1"),
                "error message should name the divergent session, got: {msg}"
            );
        }
        other => panic!(
            "expected SubagentError::HotSessionStateDivergence, got {other:?} — \
             Bug 2 regression: state-divergence misclassified"
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
    let _out1 = executor
        .execute(&input1)
        .await
        .expect("first delegate must succeed");

    // Second delegate — triggers HOT_SESSION_LIMIT_REACHED with
    // data.candidate=pi_sess_mock_1 (the LRU session).
    // Eviction: find_hot_binding_by_session(pi_sess_mock_1) → OK (pre-seeded),
    // prepare_hibernate → OK, commit_hibernate_binding_and_status → flips DB,
    // session.close(pi_sess_mock_1) → FAILS (BUSYTOK_MOCK_CLOSE_FAILS=1).
    // Executor must return an error, NOT retry.
    let input2 = ExecutorInput {
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
    executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");

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
    executor
        .execute(&evict_input("sub-a"))
        .await
        .expect("first delegate must succeed");

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
