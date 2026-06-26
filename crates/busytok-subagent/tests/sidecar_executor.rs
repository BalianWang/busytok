#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use busytok_config::SubagentSettings;
use busytok_store::repository::{SubagentHarnessBindingRow, SubagentLogicalSubagentRow};
use busytok_store::Database;
use busytok_subagent::mock_executor::{ExecutorInput, TaskExecutor};
use busytok_subagent::models::{DelegateRequest, TaskStatus};
use busytok_subagent::sidecar::config::SidecarConfig;
use busytok_subagent::sidecar::executor::SidecarTaskExecutor;
use busytok_subagent::sidecar::PiSidecarSupervisor;
use busytok_subagent::SubagentManager;

type SharedDb = Arc<std::sync::Mutex<Database>>;

fn mock_sidecar_script() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/mock-sidecar.sh");
    p
}

fn mock_sidecar_config_with_env(env: HashMap<String, String>) -> SidecarConfig {
    SidecarConfig {
        node_binary: PathBuf::from("bash"),
        bundle_path: mock_sidecar_script(),
        env,
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(3600),
        task_timeout: Duration::from_secs(10),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_millis(10),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
    }
}

fn mock_config() -> SidecarConfig {
    mock_sidecar_config_with_env(HashMap::new())
}

struct TestHarness {
    manager: SubagentManager,
    db: SharedDb,
    supervisor: Arc<PiSidecarSupervisor>,
}

fn make_harness() -> TestHarness {
    make_harness_with_env(HashMap::new())
}

fn make_harness_with_env(env: HashMap<String, String>) -> TestHarness {
    let db: SharedDb = Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    let supervisor =
        PiSidecarSupervisor::new(mock_sidecar_config_with_env(env), Some(Arc::clone(&db)));
    let executor: Arc<dyn TaskExecutor> =
        Arc::new(SidecarTaskExecutor::new(Arc::clone(&supervisor)));
    let manager =
        SubagentManager::new(Arc::clone(&db), SubagentSettings::default(), "pi", executor);
    TestHarness {
        manager,
        db,
        supervisor,
    }
}

fn req(name: &str, prompt: &str) -> DelegateRequest {
    DelegateRequest {
        subagent_name: name.to_string(),
        subagent_id: None,
        cwd: "/tmp/repo".to_string(),
        profile: "pi/search-cheap".to_string(),
        intent: None,
        prompt: prompt.to_string(),
        timeout_seconds: Some(10),
        model_override: None,
        source_harness: None,
        source_session_id: None,
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

    // Sidecar returned a real session.
    assert!(
        r.adapter_session_id.is_some(),
        "expected adapter_session_id"
    );
    assert_eq!(r.adapter, "pi");
    assert_eq!(r.status, TaskStatus::Completed);
    assert!(r.usage.input_tokens.is_some());

    // Hot binding was upserted.
    {
        let db = h.db.lock().unwrap();
        let binding = db.subagent_hot_binding(&r.subagent_id, "pi").unwrap();
        assert!(binding.is_some(), "hot binding not found");
        let binding = binding.unwrap();
        assert_eq!(binding.is_hot, 1);
        assert_eq!(binding.status, "hot");
        assert_eq!(binding.adapter_session_id, r.adapter_session_id);

        // Subagent status is Hot (not Warm).
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
    let r2 = h
        .manager
        .delegate(req("reviewer", "second turn"))
        .await
        .unwrap();

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
async fn delegate_via_sidecar_empty_session_id_falls_to_warm() {
    // Regression: an empty `adapter_session_id` from the sidecar must NOT
    // trigger the hot path. Spec §3.3 requires a real backing session for a
    // hot binding; an empty id has no real session, so the delegate falls
    // back to warm (no hot binding row, status stays Warm).
    let mut env = HashMap::new();
    env.insert("BUSYTOK_MOCK_EMPTY_SESSION".to_string(), "1".to_string());
    let h = make_harness_with_env(env);
    let r = h
        .manager
        .delegate(req("reviewer", "empty session turn"))
        .await
        .unwrap();

    // The executor extracts Some("") verbatim — the delegate is the authority
    // that decides hot vs warm, and an empty id must be treated as warm.
    assert_eq!(
        r.adapter_session_id.as_deref(),
        Some(""),
        "executor should pass through the empty id"
    );
    assert_eq!(r.status, TaskStatus::Completed);

    {
        let db = h.db.lock().unwrap();

        // No hot binding row — spec §3.3 (no hot binding without a real session).
        let binding = db.subagent_hot_binding(&r.subagent_id, "pi").unwrap();
        assert!(
            binding.is_none(),
            "empty adapter_session_id must not create a hot binding"
        );

        // Logical subagent status is Warm (not Hot).
        let sub = db.subagent_get_logical(&r.subagent_id).unwrap();
        assert!(sub.is_some());
        assert_eq!(sub.unwrap().status, "warm");
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
        model: None,
        prompt: "do something".to_string(),
        timeout_seconds: Some(5),
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
    let supervisor = PiSidecarSupervisor::new(cfg, None);
    let executor = SidecarTaskExecutor::new(supervisor.clone());

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
    let supervisor = PiSidecarSupervisor::new(cfg, None);
    let executor = SidecarTaskExecutor::new(supervisor.clone());

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
    let supervisor = PiSidecarSupervisor::new(cfg, Some(db.clone()));
    let executor = SidecarTaskExecutor::with_db(supervisor.clone(), db.clone());

    // First delegate — fills the pool (max_hot=1).
    let input1 = ExecutorInput {
        subagent_id: "sub-a".into(),
        subagent_name: "a".into(),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        model: None,
        prompt: "do 1".into(),
        timeout_seconds: None,
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
                default_model: None,
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
        model: None,
        prompt: "do 2".into(),
        timeout_seconds: None,
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
    let supervisor = PiSidecarSupervisor::new(cfg, Some(db.clone()));
    let executor = SidecarTaskExecutor::with_db(supervisor.clone(), db.clone());

    // First delegate — fills the sidecar's pool (max_hot=1)
    let input1 = ExecutorInput {
        subagent_id: "sub-a".into(),
        subagent_name: "a".into(),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        model: None,
        prompt: "do 1".into(),
        timeout_seconds: None,
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
        model: None,
        prompt: "do 2".into(),
        timeout_seconds: None,
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
    assert!(
        format!("{err}").contains("no hot binding found"),
        "error should explain the sync failure, got: {err}"
    );

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
                default_model: None,
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
    let supervisor = PiSidecarSupervisor::new(cfg, Some(db.clone()));
    let executor = SidecarTaskExecutor::with_db(supervisor.clone(), db.clone());

    // First delegate — fills the sidecar's pool (max_hot=1), session pi_sess_mock_1.
    let input1 = ExecutorInput {
        subagent_id: "sub-a".into(),
        subagent_name: "a".into(),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        model: None,
        prompt: "do 1".into(),
        timeout_seconds: None,
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
        model: None,
        prompt: "do 2".into(),
        timeout_seconds: None,
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
    let err_msg = format!("{err}");
    assert!(
        err_msg.contains("session.close failed"),
        "error should mention the close failure, got: {err_msg}"
    );

    supervisor.shutdown().await.unwrap();
}
