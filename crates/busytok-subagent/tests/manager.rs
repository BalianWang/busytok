#![allow(clippy::unwrap_used)]

use std::sync::Mutex;

use async_trait::async_trait;
use busytok_config::{SubagentProfileConfig, SubagentSettings};
use busytok_store::provider_catalog::{
    create_model, create_provider, CreateModelReq, CreateProviderReq,
};
use busytok_store::{Database, SubagentHarnessBindingRow};
use busytok_subagent::manager::SubagentManager;
use busytok_subagent::memory::{KeyFile, MemoryUpdate, OpenQuestion};
use busytok_subagent::mock_executor::{
    ExecutorInput, ExecutorOutput, FailingTaskExecutor, MockTaskExecutor, TaskExecutor,
};
use busytok_subagent::models::{
    DelegateRequest, QueueReason, ResolveParams, SubagentStatus, TaskStatus, TaskUsage,
};
use busytok_subagent::pressure::{PressureAction, PressureGate};
use busytok_subagent::SubagentError;

/// A mock executor that blocks forever, simulating a long-running sidecar
/// call. Used to test cooperative cancel of in-flight executor calls — the
/// only exit is the cancel signal dropping the executor future via
/// `tokio::select!` in `execute_task`.
struct BlockingExecutor {
    started: std::sync::Arc<tokio::sync::Notify>,
}

impl BlockingExecutor {
    fn new() -> Self {
        Self {
            started: std::sync::Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn started(&self) -> std::sync::Arc<tokio::sync::Notify> {
        std::sync::Arc::clone(&self.started)
    }
}

#[async_trait::async_trait]
impl TaskExecutor for BlockingExecutor {
    async fn execute(&self, _input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        // Block forever until the cancel signal drops the future (via
        // `tokio::select!` in `execute_task`). This simulates a long-running
        // sidecar turn_auto call that never completes on its own.
        self.started.notify_one();
        std::future::pending::<()>().await;
        unreachable!("BlockingExecutor should never complete — cancel drops the future");
    }
}

/// Executor that counts how many times `execute()` is called. Used to verify
/// that cancelled tasks are never executed — whether filtered by
/// `pick_oldest_queued_task` (status='cancelled' not picked) or skipped by
/// the dispatcher's pre-execute re-check (status flipped between pick and
/// execute).
struct CountingExecutor {
    calls: std::sync::atomic::AtomicU32,
}

/// Records lifecycle cleanup calls so the manager tests can assert that an
/// explicit hibernate releases the matching sidecar session after the DB
/// binding is closed.
struct HibernateCloseTrackingExecutor {
    closes: Mutex<Vec<(String, String)>>,
}

impl HibernateCloseTrackingExecutor {
    fn new() -> Self {
        Self {
            closes: Mutex::new(Vec::new()),
        }
    }
}

/// Executor used to exercise hibernate's compensating rollback when the
/// sidecar refuses to close a session after the DB transition. The manager
/// must restore the hot binding instead of leaving DB and sidecar state split.
struct HibernateCloseFailingExecutor;

#[async_trait]
impl TaskExecutor for HibernateCloseFailingExecutor {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        MockTaskExecutor.execute(input).await
    }

    async fn close_session(
        &self,
        _adapter_session_id: &str,
        _provider_id: &str,
    ) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("simulated close failure"))
    }
}

/// Deliberately blocks the sidecar close RPC so a regression test can open the
/// exact window between hibernate's DB commit and session cleanup. A correct
/// manager must keep a same-subagent delegate behind that lifecycle boundary;
/// otherwise the second task reuses `session-race` and hibernate closes the
/// session after the new hot binding has already been committed.
struct HibernateDelegateRaceExecutor {
    execute_calls: std::sync::atomic::AtomicUsize,
    close_started: tokio::sync::Notify,
    release_close: tokio::sync::Notify,
    closed: std::sync::atomic::AtomicBool,
}

impl HibernateDelegateRaceExecutor {
    fn new() -> Self {
        Self {
            execute_calls: std::sync::atomic::AtomicUsize::new(0),
            close_started: tokio::sync::Notify::new(),
            release_close: tokio::sync::Notify::new(),
            closed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn execute_call_count(&self) -> usize {
        self.execute_calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl TaskExecutor for HibernateDelegateRaceExecutor {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        let call = self
            .execute_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let adapter_session_id =
            if call == 0 || !self.closed.load(std::sync::atomic::Ordering::SeqCst) {
                "session-race"
            } else {
                // Once hibernate has actually closed the old session, a real
                // sidecar would cold-start a new one for the next delegate.
                "session-race-new"
            };
        Ok(ExecutorOutput {
            adapter_session_id: Some(adapter_session_id.to_string()),
            session_reused: call > 0 && adapter_session_id == "session-race",
            status: TaskStatus::Completed,
            summary: format!("race: {}", input.prompt),
            usage: TaskUsage {
                model: Some(input.model.clone()),
                provider: Some("race".to_string()),
                input_tokens: Some(input.prompt.len() as i64),
                output_tokens: Some(0),
                ..Default::default()
            },
            memory_update: MemoryUpdate::default(),
            error_kind: None,
        })
    }

    async fn close_session(
        &self,
        _adapter_session_id: &str,
        _provider_id: &str,
    ) -> anyhow::Result<()> {
        self.close_started.notify_one();
        self.release_close.notified().await;
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl TaskExecutor for HibernateCloseTrackingExecutor {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        MockTaskExecutor.execute(input).await
    }

    async fn close_session(
        &self,
        adapter_session_id: &str,
        provider_id: &str,
    ) -> anyhow::Result<()> {
        self.closes
            .lock()
            .unwrap()
            .push((adapter_session_id.to_string(), provider_id.to_string()));
        Ok(())
    }
}

impl CountingExecutor {
    fn new() -> Self {
        Self {
            calls: std::sync::atomic::AtomicU32::new(0),
        }
    }

    fn call_count(&self) -> u32 {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl TaskExecutor for CountingExecutor {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ExecutorOutput {
            adapter_session_id: None,
            session_reused: false,
            status: TaskStatus::Completed,
            summary: format!("[counted] prompt: {}", input.prompt),
            usage: TaskUsage {
                model: Some(input.model.clone()),
                provider: Some("counting".to_string()),
                input_tokens: Some(input.prompt.len() as i64),
                output_tokens: Some(0),
                ..Default::default()
            },
            memory_update: MemoryUpdate::default(),
            error_kind: None,
        })
    }
}

/// Install a thread-local tracing subscriber so `tracing!` macro arguments
/// (event_code, reason, etc.) are evaluated and counted by line coverage.
/// Returns a guard that restores the previous default on drop. Each
/// `#[tokio::test]` runs on its own thread, so parallel tests don't interfere.
fn install_tracing() -> tracing::subscriber::DefaultGuard {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_test_writer()
        .finish();
    tracing::subscriber::set_default(subscriber)
}

async fn manager() -> std::sync::Arc<SubagentManager> {
    // std::sync::Mutex — matches the supervisor's db field type.
    let db = test_db();
    std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    ))
}

#[tokio::test]
async fn lifecycle_registry_is_shared_by_successor_managers() {
    let first = manager().await;
    let shared = first.lifecycle_registry();
    let second = SubagentManager::with_pressure_gate_and_lifecycle_registry(
        test_db(),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
        None,
        std::sync::Arc::clone(&shared),
    );
    assert!(
        std::sync::Arc::ptr_eq(&shared, &second.lifecycle_registry()),
        "runtime successor must retain the previous lifecycle registry"
    );
}

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

// Thread-local storage for the seeded test provider/model IDs. Each
// `#[tokio::test]` runs on its own thread, and `test_db()` seeds fresh
// values before `req()` is called.
thread_local! {
    static TEST_PROVIDER_ID: std::cell::RefCell<String> =
        const { std::cell::RefCell::new(String::new()) };
    static TEST_MODEL_ID: std::cell::RefCell<String> =
        const { std::cell::RefCell::new(String::new()) };
}

const TEST_MODEL_NAME: &str = "test-model";

/// Create an in-memory database seeded with a test provider + model so
/// `delegate()` can create subagents with valid bound fields. The seeded
/// IDs are stored in thread-locals for `req()` / `req_with_cwd()` to read.
fn test_db() -> std::sync::Arc<std::sync::Mutex<Database>> {
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    seed_test_provider_model(&db.lock().unwrap());
    db
}

fn seed_test_provider_model(db: &Database) {
    let provider = create_provider(
        db.conn(),
        CreateProviderReq {
            name: "Test Provider".into(),
            provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: true,
            api_key: Some("sk-test".into()),
        },
    )
    .unwrap();
    create_model(
        db.conn(),
        CreateModelReq {
            provider_id: provider.id.clone(),
            model_id: TEST_MODEL_NAME.into(),
            enabled: true,
            tags: vec![],
            display_name: None,
            reasoning: None,
            context_window: Some(128000),
            max_tokens: Some(16384),
        },
    )
    .unwrap();
    // Task 5: seed additional models used by `model_override` tests. The
    // validation chain in `execute_task` now verifies the effective model
    // (override or bound) exists in the bound provider's model list, so any
    // model_override value referenced by a test must be seeded here.
    for mid in ["gpt-4o", "claude-fancy-override"] {
        create_model(
            db.conn(),
            CreateModelReq {
                provider_id: provider.id.clone(),
                model_id: mid.into(),
                enabled: true,
                tags: vec![],
                display_name: None,
                reasoning: None,
                context_window: Some(128000),
                max_tokens: Some(16384),
            },
        )
        .unwrap();
    }
    TEST_PROVIDER_ID.with(|c| *c.borrow_mut() = provider.id);
    TEST_MODEL_ID.with(|c| *c.borrow_mut() = TEST_MODEL_NAME.to_string());
}

fn bound_provider_id() -> String {
    TEST_PROVIDER_ID.with(|c| c.borrow().clone())
}

fn bound_model_id() -> String {
    TEST_MODEL_ID.with(|c| c.borrow().clone())
}

fn req_with_cwd(name: &str, prompt: &str, cwd: &str) -> DelegateRequest {
    DelegateRequest {
        subagent_name: name.to_string(),
        subagent_id: None,
        cwd: cwd.to_string(),
        profile: "pi/search-cheap".to_string(),
        intent: None,
        prompt: prompt.to_string(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: Some(bound_provider_id()),
        bound_model_id: Some(bound_model_id()),
        reuse_policy: None,
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
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: Some(bound_provider_id()),
        bound_model_id: Some(bound_model_id()),
        reuse_policy: None,
    }
}

#[tokio::test]
async fn delegate_creates_subagent_then_reuses_it() {
    let m = manager().await;
    let r1 = m.delegate(req("reviewer", "step one")).await.unwrap();
    assert_eq!(r1.subagent_name, "reviewer");
    assert_eq!(r1.status.as_str(), "running");
    let final_status = await_task_done(&m, &r1.task_id).await;
    assert_eq!(final_status, "completed");

    let r2 = m.delegate(req("reviewer", "step two")).await.unwrap();
    assert_eq!(r2.subagent_id, r1.subagent_id, "same subagent reused");
}

#[tokio::test]
async fn list_returns_active_subagents() {
    let m = manager().await;
    let r1 = m.delegate(req("a", "do")).await.unwrap();
    let _ = await_task_done(&m, &r1.task_id).await;
    let r2 = m.delegate(req("b", "do")).await.unwrap();
    let _ = await_task_done(&m, &r2.task_id).await;
    // no filters → all active subagents
    let list = m.list(None, None, false).await.unwrap();
    assert_eq!(list.len(), 2);
    // status filter narrows the set. MockTaskExecutor produces no memory_update,
    // so hot_summary stays None and §3.3 says cold (NOT warm) on a fresh subagent.
    let cold = m
        .list(Some(SubagentStatus::Cold), None, false)
        .await
        .unwrap();
    assert_eq!(
        cold.len(),
        2,
        "both go cold after a mock task with no memory_update"
    );
}

#[tokio::test]
async fn delete_then_lookup_fails() {
    let m = manager().await;
    let r = m.delegate(req("reviewer", "do")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;
    m.delete(
        ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        },
        false,
    )
    .await
    .unwrap();
    // soft-deleted rows are excluded from the active list
    let list = m.list(None, None, false).await.unwrap();
    assert!(list.iter().all(|s| s.id != r.subagent_id));
}

#[tokio::test]
async fn hibernate_clears_hot_binding_keeps_state() {
    let m = manager().await;
    let r = m.delegate(req("reviewer", "do")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;
    m.hibernate(ResolveParams {
        id: Some(r.subagent_id.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    let detail = m
        .show(ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    // after hibernate the subagent still exists (warm — memory was written), not deleted
    assert_ne!(detail.status.as_str(), "deleted");
}

#[tokio::test]
async fn reject_invalid_subagent_name() {
    let m = manager().await;
    let bad = req("bad name!", "do");
    assert!(m.delegate(bad).await.is_err());
}

#[tokio::test]
async fn tasks_returns_history_for_subagent() {
    let m = manager().await;
    let r = m.delegate(req("reviewer", "first")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;
    let r2 = m.delegate(req("reviewer", "second")).await.unwrap();
    let _ = await_task_done(&m, &r2.task_id).await;
    let tasks = m
        .tasks(
            ResolveParams {
                id: Some(r.subagent_id.clone()),
                ..Default::default()
            },
            20,
        )
        .await
        .unwrap();
    assert_eq!(tasks.len(), 2, "both delegated tasks should be listed");
    assert_eq!(tasks[0].subagent_id, r.subagent_id);
    assert_eq!(tasks[0].status.as_str(), "completed");
}

#[tokio::test]
async fn tasks_resolves_by_name_and_clamps_limit() {
    let m = manager().await;
    let r = m
        .delegate(req_with_cwd("worker", "do", "/tmp/worker-repo"))
        .await
        .unwrap();
    let _ = await_task_done(&m, &r.task_id).await;
    // resolve by name + cwd
    let tasks = m
        .tasks(
            ResolveParams {
                name: Some("worker".to_string()),
                cwd: Some("/tmp/worker-repo".to_string()),
                ..Default::default()
            },
            1,
        )
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1, "limit=1 should clamp the result");
    assert_eq!(tasks[0].subagent_id, r.subagent_id);
}

#[tokio::test]
async fn hard_delete_removes_subagent_and_excludes_from_list() {
    let m = manager().await;
    let r = m.delegate(req("reviewer", "do")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;
    m.delete(
        ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        },
        true, // hard delete
    )
    .await
    .unwrap();
    // hard-deleted rows are gone even from the include_deleted list
    let list = m.list(None, None, true).await.unwrap();
    assert!(list.iter().all(|s| s.id != r.subagent_id));
    // looking it up by id now fails
    assert!(m
        .show(ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .is_err());
}

#[tokio::test]
async fn delegate_with_subagent_id_shortcut_reuses_existing() {
    let m = manager().await;
    let r1 = m.delegate(req("reviewer", "first")).await.unwrap();
    let _ = await_task_done(&m, &r1.task_id).await;
    // second delegate resolves by UUID directly, bypassing name resolution
    let r2 = m
        .delegate(DelegateRequest {
            subagent_id: Some(r1.subagent_id.clone()),
            ..req("ignored-name", "second")
        })
        .await
        .unwrap();
    assert_eq!(
        r2.subagent_id, r1.subagent_id,
        "id shortcut reuses subagent"
    );
    assert_eq!(r2.status.as_str(), "running");
    let final_status = await_task_done(&m, &r2.task_id).await;
    assert_eq!(final_status, "completed");
}

#[tokio::test]
async fn delegate_with_model_override_wins_over_profile_model() {
    let m = manager().await;
    let r = m
        .delegate(DelegateRequest {
            model_override: Some("claude-fancy-override".to_string()),
            ..req("reviewer", "do")
        })
        .await
        .unwrap();
    assert_eq!(r.model.as_deref(), Some("claude-fancy-override"));
}

#[tokio::test]
async fn delegate_with_unknown_profile_returns_profile_not_found() {
    let m = manager().await;
    let err = m
        .delegate(DelegateRequest {
            profile: "custom/unknown".to_string(),
            ..req("reviewer", "do")
        })
        .await
        .err()
        .unwrap();
    assert_eq!(err.code(), "subagent.profile_not_found");
}

#[tokio::test]
async fn delegate_with_unknown_profile_and_model_override_succeeds() {
    let m = manager().await;
    let r = m
        .delegate(DelegateRequest {
            profile: "custom/unknown".to_string(),
            model_override: Some("gpt-4o".to_string()),
            ..req("reviewer", "do")
        })
        .await
        .unwrap();
    assert_eq!(r.model.as_deref(), Some("gpt-4o"));
    assert_eq!(r.profile, "custom/unknown");
}

#[tokio::test]
async fn delegate_review_and_plan_profiles_resolve_default_models() {
    let m = manager().await;
    let r_review = m
        .delegate(DelegateRequest {
            profile: "pi/review-cheap".to_string(),
            ..req("reviewer", "do")
        })
        .await
        .unwrap();
    // delegate() returns Running immediately with `model: req.model_override.clone()`
    // (None here). The resolved model (bound_model_id) is computed in execute_task
    // (background). Verify it after completion via the subagent's bound_model_id.
    assert!(
        r_review.model.is_none(),
        "Running result carries model_override only — None when no override"
    );
    let _ = await_task_done(&m, &r_review.task_id).await;
    let shown_review = m
        .show(ResolveParams {
            id: Some(r_review.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        !shown_review.bound_model_id.is_empty(),
        "review profile should map to a model (bound_model_id)"
    );
    let r_plan = m
        .delegate(DelegateRequest {
            profile: "pi/plan-cheap".to_string(),
            ..req("planner", "do")
        })
        .await
        .unwrap();
    assert!(
        r_plan.model.is_none(),
        "Running result carries model_override only — None when no override"
    );
    let _ = await_task_done(&m, &r_plan.task_id).await;
    let shown_plan = m
        .show(ResolveParams {
            id: Some(r_plan.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        !shown_plan.bound_model_id.is_empty(),
        "plan profile should map to a model (bound_model_id)"
    );
}

#[tokio::test]
async fn delegate_rejected_when_feature_disabled() {
    let db = test_db();
    let settings = SubagentSettings {
        enabled: false,
        ..Default::default()
    };
    let m = std::sync::Arc::new(SubagentManager::new(
        db,
        settings,
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    ));
    let err = m.delegate(req("reviewer", "do")).await.unwrap_err();
    assert!(matches!(err, SubagentError::Disabled));
}

#[tokio::test]
async fn show_by_name_resolves_within_repo_scope() {
    let m = manager().await;
    let r = m
        .delegate(req_with_cwd("reviewer", "do", "/tmp/scope-repo"))
        .await
        .unwrap();
    let _ = await_task_done(&m, &r.task_id).await;
    let shown = m
        .show(ResolveParams {
            name: Some("reviewer".to_string()),
            cwd: Some("/tmp/scope-repo".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(shown.id, r.subagent_id);
    assert_eq!(shown.name, "reviewer");
}

#[tokio::test]
async fn show_unknown_id_returns_not_found() {
    let m = manager().await;
    let err = m
        .show(ResolveParams {
            id: Some("nonexistent-uuid".to_string()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::NotFound(_)));
}

#[tokio::test]
async fn resolve_without_id_or_name_returns_invalid_argument() {
    let m = manager().await;
    let err = m.show(ResolveParams::default()).await.unwrap_err();
    assert!(matches!(err, SubagentError::InvalidArgument(_)));
    assert_eq!(err.code(), "subagent.invalid_argument");
}

#[tokio::test]
async fn resolve_with_both_id_and_name_returns_invalid_argument() {
    let m = manager().await;
    let err = m
        .show(ResolveParams {
            id: Some("some-uuid".to_string()),
            name: Some("reviewer".to_string()),
            cwd: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::InvalidArgument(_)));
    assert_eq!(err.code(), "subagent.invalid_argument");
}

#[tokio::test]
async fn name_only_resolve_without_cwd_returns_invalid_argument() {
    let m = manager().await;
    // delegate first so the subagent exists
    let r = m.delegate(req("reviewer", "do")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;
    // show by name without cwd → rejected by server-side contract
    let err = m
        .show(ResolveParams {
            name: Some("reviewer".to_string()),
            id: None,
            cwd: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::InvalidArgument(_)));
    assert_eq!(err.code(), "subagent.invalid_argument");
    // hibernate by name without cwd → same
    let err = m
        .hibernate(ResolveParams {
            name: Some("reviewer".to_string()),
            id: None,
            cwd: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::InvalidArgument(_)));
    // tasks by name without cwd → same
    let err = m
        .tasks(
            ResolveParams {
                name: Some("reviewer".to_string()),
                id: None,
                cwd: None,
            },
            10,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::InvalidArgument(_)));
}

#[tokio::test]
async fn soft_then_hard_delete_by_name_succeeds() {
    let m = manager().await;
    let r = m.delegate(req("reviewer", "do")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;
    // soft delete by name + cwd
    m.delete(
        ResolveParams {
            name: Some("reviewer".to_string()),
            id: None,
            cwd: Some("/tmp/repo".to_string()),
        },
        false,
    )
    .await
    .unwrap();
    // hard delete by same name + cwd — must reach the tombstone
    m.delete(
        ResolveParams {
            name: Some("reviewer".to_string()),
            id: None,
            cwd: Some("/tmp/repo".to_string()),
        },
        true,
    )
    .await
    .unwrap();
    // now truly gone — resolve by name returns NotFound (not tombstone)
    let err = m
        .show(ResolveParams {
            name: Some("reviewer".to_string()),
            id: None,
            cwd: Some("/tmp/repo".to_string()),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::NotFound(_)));
}

#[tokio::test]
async fn soft_deleted_subagent_cannot_be_resolved_by_id() {
    let m = manager().await;
    let r = m.delegate(req("reviewer", "do")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;
    let id = r.subagent_id.clone();
    // soft delete
    m.delete(
        ResolveParams {
            id: Some(id.clone()),
            ..Default::default()
        },
        false,
    )
    .await
    .unwrap();
    // resolve by id now fails (tombstone filtered)
    let err = m
        .show(ResolveParams {
            id: Some(id.clone()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::NotFound(_)));
    // hibernate on tombstone also fails
    let err = m
        .hibernate(ResolveParams {
            id: Some(id.clone()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::NotFound(_)));
    // delegate with subagent_id on tombstone fails
    let err = m
        .delegate(DelegateRequest {
            subagent_id: Some(id.clone()),
            ..req("reviewer", "do")
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::NotFound(_)));
}

#[tokio::test]
async fn hard_delete_can_operate_on_soft_deleted_subagent() {
    let m = manager().await;
    let r = m.delegate(req("reviewer", "do")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;
    let id = r.subagent_id.clone();
    // soft delete first
    m.delete(
        ResolveParams {
            id: Some(id.clone()),
            ..Default::default()
        },
        false,
    )
    .await
    .unwrap();
    // hard delete on tombstone succeeds (resolve_by_id_include_deleted)
    m.delete(
        ResolveParams {
            id: Some(id.clone()),
            ..Default::default()
        },
        true,
    )
    .await
    .unwrap();
    // now truly gone
    let err = m
        .show(ResolveParams {
            id: Some(id),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::NotFound(_)));
}

#[tokio::test]
async fn tasks_with_negative_limit_returns_invalid_argument() {
    let m = manager().await;
    let r = m.delegate(req("reviewer", "do")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;
    let err = m
        .tasks(
            ResolveParams {
                id: Some(r.subagent_id.clone()),
                ..Default::default()
            },
            -1,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::InvalidArgument(_)));
    assert_eq!(err.code(), "subagent.invalid_argument");
}

#[tokio::test]
async fn tasks_with_large_limit_is_clamped() {
    let m = manager().await;
    let r = m.delegate(req("reviewer", "do")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;
    // limit 10000 should be clamped to 500 and succeed (returns 1 task)
    let tasks = m
        .tasks(
            ResolveParams {
                id: Some(r.subagent_id.clone()),
                ..Default::default()
            },
            10000,
        )
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
}

#[tokio::test]
async fn hibernate_unknown_id_returns_not_found() {
    let m = manager().await;
    let err = m
        .hibernate(ResolveParams {
            id: Some("no-such-id".to_string()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::NotFound(_)));
}

#[tokio::test]
async fn delete_unknown_id_returns_not_found() {
    let m = manager().await;
    let err = m
        .delete(
            ResolveParams {
                id: Some("no-such-id".to_string()),
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::NotFound(_)));
}

#[tokio::test]
async fn list_with_include_deleted_returns_soft_deleted_rows() {
    let m = manager().await;
    let r = m.delegate(req("reviewer", "do")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;
    m.delete(
        ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        },
        false,
    )
    .await
    .unwrap();
    // active list excludes it
    let active = m.list(None, None, false).await.unwrap();
    assert!(active.iter().all(|s| s.id != r.subagent_id));
    // include_deleted surfaces it again
    let with_deleted = m.list(None, None, true).await.unwrap();
    assert!(with_deleted.iter().any(|s| s.id == r.subagent_id));
    // status filter for Deleted narrows to it
    let deleted_only = m
        .list(Some(SubagentStatus::Deleted), None, true)
        .await
        .unwrap();
    assert!(deleted_only.iter().any(|s| s.id == r.subagent_id));
}

#[tokio::test]
async fn hibernate_then_show_status_is_cold_when_no_memory() {
    let m = manager().await;
    let r = m.delegate(req("reviewer", "do")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;
    m.hibernate(ResolveParams {
        id: Some(r.subagent_id.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    let detail = m
        .show(ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    // MockTaskExecutor produces no memory_update, so hot_summary is None and
    // hibernate leaves the subagent cold (§3.3: warm iff hot_summary IS NOT NULL).
    assert_eq!(detail.status.as_str(), "cold");
}

struct WarmMemoryExecutor;

#[async_trait]
impl TaskExecutor for WarmMemoryExecutor {
    async fn execute(&self, _input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        Ok(ExecutorOutput {
            adapter_session_id: None,
            session_reused: false,
            status: TaskStatus::Completed,
            summary: "done".into(),
            usage: Default::default(),
            memory_update: MemoryUpdate {
                current_state_summary: Some("kept warm".into()),
                key_files: Vec::<KeyFile>::new(),
                decisions: Vec::<String>::new(),
                open_questions: Vec::<OpenQuestion>::new(),
            },
            error_kind: None,
        })
    }
}

#[tokio::test]
async fn hibernate_without_binding_keeps_warm_status_when_memory_exists() {
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(WarmMemoryExecutor),
    ));
    let r = m.delegate(req("warm-reviewer", "do")).await.unwrap();
    assert_eq!(r.status.as_str(), "running");
    let final_status = await_task_done(&m, &r.task_id).await;
    assert_eq!(final_status, "completed");

    m.hibernate(ResolveParams {
        id: Some(r.subagent_id.clone()),
        ..Default::default()
    })
    .await
    .unwrap();

    let detail = m
        .show(ResolveParams {
            id: Some(r.subagent_id),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        detail.status.as_str(),
        "warm",
        "hibernate without hot binding should keep warm when memory exists"
    );
}

#[tokio::test]
async fn task_counts_returns_zero_when_task_table_query_fails() {
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    ));
    {
        let db = db.lock().unwrap();
        db.conn().execute("DROP TABLE subagent_tasks", []).unwrap();
    }

    assert_eq!(
        m.task_counts(),
        (0, 0),
        "task_counts should degrade to zeroes on DB query failure"
    );
}

#[tokio::test]
async fn hibernate_closes_existing_hot_binding() {
    // delegate creates no hot binding (Plan 1), so seed one manually to cover
    // the `if let Some(mut b) = binding` branch in manager::hibernate.
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    ));
    let r = m.delegate(req("reviewer", "do")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;

    // insert a hot binding for the "pi" adapter
    {
        let g = db.lock().unwrap();
        g.subagent_upsert_binding(&SubagentHarnessBindingRow {
            id: format!("bind_{}", r.subagent_id),
            subagent_id: r.subagent_id.clone(),
            harness: "pi".to_string(),
            adapter_session_id: Some("sess-1".to_string()),
            adapter_process_id: Some("123".to_string()),
            is_hot: 1,
            status: "hot".to_string(),
            created_at_ms: busytok_domain::now_ms(),
            last_used_at_ms: None,
            closed_at_ms: None,
            detail_json: None,
        })
        .unwrap();
        // Keep the state transition successful while forcing the
        // observational resource-event write down its warning path.
        g.conn()
            .execute("DROP TABLE subagent_resource_events", [])
            .unwrap();
    }

    m.hibernate(ResolveParams {
        id: Some(r.subagent_id.clone()),
        ..Default::default()
    })
    .await
    .unwrap();

    // the binding should no longer be hot. `subagent_hot_binding` filters by
    // is_hot = 1, so None proves hibernate closed the hot session.
    let g = db.lock().unwrap();
    let binding = g.subagent_hot_binding(&r.subagent_id, "pi").unwrap();
    assert!(
        binding.is_none(),
        "hot binding should be cleared after hibernate"
    );
}

#[tokio::test]
async fn hibernate_closes_matching_sidecar_session_after_db_commit() {
    let db = test_db();
    let executor = std::sync::Arc::new(HibernateCloseTrackingExecutor::new());
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::clone(&executor) as std::sync::Arc<dyn TaskExecutor>,
    ));
    let r = m.delegate(req("reviewer-close", "do")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;

    let provider_id = bound_provider_id();
    {
        let g = db.lock().unwrap();
        g.subagent_upsert_binding(&SubagentHarnessBindingRow {
            id: format!("bind_{}", r.subagent_id),
            subagent_id: r.subagent_id.clone(),
            harness: "pi".to_string(),
            adapter_session_id: Some("sess-hibernate".to_string()),
            adapter_process_id: None,
            is_hot: 1,
            status: "hot".to_string(),
            created_at_ms: busytok_domain::now_ms(),
            last_used_at_ms: None,
            closed_at_ms: None,
            detail_json: None,
        })
        .unwrap();
    }

    m.hibernate(ResolveParams {
        id: Some(r.subagent_id.clone()),
        ..Default::default()
    })
    .await
    .unwrap();

    assert_eq!(
        executor.closes.lock().unwrap().as_slice(),
        &[("sess-hibernate".to_string(), provider_id)]
    );
}

#[tokio::test]
async fn hibernate_close_failure_restores_hot_binding() {
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(HibernateCloseFailingExecutor),
    ));
    let r = m.delegate(req("hibernate-close-fail", "do")).await.unwrap();
    let _ = await_task_done(&m, &r.task_id).await;

    {
        let g = db.lock().unwrap();
        g.subagent_upsert_binding(&SubagentHarnessBindingRow {
            id: format!("bind_{}", r.subagent_id),
            subagent_id: r.subagent_id.clone(),
            harness: "pi".to_string(),
            adapter_session_id: Some("sess-close-fail".to_string()),
            adapter_process_id: None,
            is_hot: 1,
            status: "hot".to_string(),
            created_at_ms: busytok_domain::now_ms(),
            last_used_at_ms: None,
            closed_at_ms: None,
            detail_json: None,
        })
        .unwrap();
    }

    let err = m
        .hibernate(ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::SidecarRpc { .. }));

    let g = db.lock().unwrap();
    let binding = g
        .subagent_hot_binding(&r.subagent_id, "pi")
        .unwrap()
        .expect("close failure must restore the hot binding");
    assert_eq!(
        binding.adapter_session_id.as_deref(),
        Some("sess-close-fail")
    );
    assert_eq!(binding.status, "hot");
    assert_eq!(
        g.subagent_get_logical(&r.subagent_id)
            .unwrap()
            .unwrap()
            .status,
        "hot"
    );
}

#[tokio::test]
async fn hibernate_serializes_same_subagent_delegate_until_session_close() {
    let db = test_db();
    let executor = std::sync::Arc::new(HibernateDelegateRaceExecutor::new());
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::clone(&executor) as std::sync::Arc<dyn TaskExecutor>,
    ));

    let first = m.delegate(req("hibernate-race", "first")).await.unwrap();
    assert_eq!(await_task_done(&m, &first.task_id).await, "completed");

    let hibernate_manager = std::sync::Arc::clone(&m);
    let hibernate_task = tokio::spawn(async move {
        hibernate_manager
            .hibernate(ResolveParams {
                name: Some("hibernate-race".to_string()),
                cwd: Some("/tmp/repo".to_string()),
                ..Default::default()
            })
            .await
    });
    // The notification is emitted only after the DB binding/status transition
    // has committed. The close RPC remains blocked, so the lifecycle boundary
    // is held open for the concurrent delegate below.
    executor.close_started.notified().await;

    let second = m.delegate(req("hibernate-race", "second")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        executor.execute_call_count(),
        1,
        "same-subagent delegate must wait while hibernate closes its session"
    );

    // Once the close completes, the delegate may cold-start a replacement
    // session and commit a binding for that new session.
    executor.release_close.notify_one();
    assert_eq!(hibernate_task.await.unwrap().unwrap(), first.subagent_id);
    assert_eq!(await_task_done(&m, &second.task_id).await, "completed");

    let db_guard = db.lock().unwrap();
    let binding = db_guard
        .subagent_hot_binding(&first.subagent_id, "pi")
        .unwrap()
        .expect("replacement delegate should leave a hot binding");
    assert_eq!(
        binding.adapter_session_id.as_deref(),
        Some("session-race-new"),
        "hibernate must not close the replacement delegate's session"
    );
}

#[tokio::test]
async fn hibernate_times_out_while_same_subagent_task_is_running() {
    let db = test_db();
    let executor = std::sync::Arc::new(BlockingExecutor::new());
    let started = executor.started();
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        executor,
    ));

    let r = m
        .delegate(req("hibernate-timeout", "long task"))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), started.notified())
        .await
        .expect("delegate should reach the blocking executor before hibernate");
    let err = m
        .hibernate(ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, SubagentError::Validation(message) if message.contains("lifecycle is busy"))
    );

    // Release the background task so this test does not leave a detached
    // executor behind for subsequent tests.
    m.cancel_task(&r.task_id, Some("cleanup after lifecycle timeout"))
        .await
        .unwrap();
    assert_eq!(await_task_done(&m, &r.task_id).await, "cancelled");
}

// --- Plan 4 Task 3: ContextBuilder + MemoryUpdater wiring -------------------

struct MemoryUpdateExecutor {
    captured_input: Mutex<Option<ExecutorInput>>,
}

#[async_trait::async_trait]
impl TaskExecutor for MemoryUpdateExecutor {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        // Capture a clone of the input's context string to verify it was built.
        let captured = ExecutorInput {
            task_id: input.task_id.clone(),
            subagent_id: input.subagent_id.clone(),
            subagent_name: input.subagent_name.clone(),
            cwd: input.cwd.clone(),
            profile: input.profile.clone(),
            model: input.model.clone(),
            prompt: input.prompt.clone(),
            prompt_artifact_ref: None,
            timeout_seconds: input.timeout_seconds,
            tools: input.tools.clone(),
            memory: busytok_subagent::context::MemorySnapshot {
                hot_summary: input.memory.hot_summary.clone(),
                long_summary: input.memory.long_summary.clone(),
                key_files: input.memory.key_files.clone(),
                decisions: input.memory.decisions.clone(),
                open_questions: input.memory.open_questions.clone(),
            },
            context: busytok_subagent::context::CompactContext {
                compact_context: input.context.compact_context.clone(),
                budget_tokens: input.context.budget_tokens,
                source: input.context.source.clone(),
            },
            write_access: input.write_access,
            provider_id: input.provider_id.clone(),
            provider_kind: input.provider_kind.clone(),
            provider_base_url: input.provider_base_url.clone(),
            provider_api_key: input.provider_api_key.clone(),
            model_reasoning: input.model_reasoning,
            model_context_window: input.model_context_window,
            model_max_tokens: input.model_max_tokens,
            model_display_name: input.model_display_name.clone(),
        };
        *self.captured_input.lock().unwrap() = Some(captured);
        Ok(ExecutorOutput {
            adapter_session_id: None,
            session_reused: false,
            status: TaskStatus::Completed,
            summary: "task done".into(),
            usage: TaskUsage::default(),
            memory_update: MemoryUpdate {
                current_state_summary: Some("Investigated auth; found refresh gap.".into()),
                key_files: vec![KeyFile {
                    path: "src/auth/token.ts".into(),
                    reason: "refresh logic".into(),
                    last_seen_at_ms: 5000,
                    score: 3,
                }],
                decisions: vec!["Focus on read-only analysis".into()],
                open_questions: vec![OpenQuestion {
                    question: "Concurrent refresh handled?".into(),
                    status: "open".into(),
                    created_at_ms: 5000,
                    last_seen_at_ms: 5000,
                }],
            },
            error_kind: None,
        })
    }
}

#[tokio::test]
async fn delegate_builds_context_and_merges_memory_update() {
    let db = test_db();
    let executor = std::sync::Arc::new(MemoryUpdateExecutor {
        captured_input: Mutex::new(None),
    });
    let manager = std::sync::Arc::new(SubagentManager::new(
        db.clone(),
        SubagentSettings::default(),
        "pi",
        executor.clone(),
    ));
    let req = DelegateRequest {
        subagent_name: "auth-investigator".into(),
        subagent_id: None,
        cwd: "/repo".into(),
        profile: "pi/review-cheap".into(),
        intent: Some("Study auth".into()),
        prompt: "Check refresh logic".into(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: Some("cli".into()),
        source_session_id: None,
        bound_provider_id: Some(bound_provider_id()),
        bound_model_id: Some(bound_model_id()),
        reuse_policy: None,
    };
    let result = manager.delegate(req).await.unwrap();
    assert_eq!(result.status, TaskStatus::Running);
    let final_status = await_task_done(&manager, &result.task_id).await;
    assert_eq!(final_status, "completed");

    // Verify context was built and sent to the executor.
    let captured = executor
        .captured_input
        .lock()
        .unwrap()
        .clone()
        .expect("input captured");
    assert!(
        captured
            .context
            .compact_context
            .contains("Check refresh logic"),
        "context contains the prompt"
    );
    assert!(
        captured
            .context
            .compact_context
            .contains("auth-investigator"),
        "context contains the subagent name"
    );
    assert_eq!(captured.context.source, "busytok-context-builder/v1");

    // Verify memory merged: hot_summary from current_state_summary (not task_summary).
    // Scoped: db_guard MUST be dropped before manager.show() below, since show()
    // re-locks the same std::sync::Mutex (non-reentrant → deadlock if held).
    {
        let db_guard = db.lock().unwrap();
        let mem = db_guard
            .subagent_get_memory(&result.subagent_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            mem.hot_summary.as_deref(),
            Some("Investigated auth; found refresh gap."),
            "hot_summary from current_state_summary, not task_summary"
        );
        let files: Vec<serde_json::Value> =
            serde_json::from_str(mem.key_files_json.as_deref().unwrap()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["path"], "src/auth/token.ts");
        let decisions: Vec<String> =
            serde_json::from_str(mem.decisions_json.as_deref().unwrap()).unwrap();
        assert_eq!(decisions, vec!["Focus on read-only analysis"]);
        let qs: Vec<serde_json::Value> =
            serde_json::from_str(mem.open_questions_json.as_deref().unwrap()).unwrap();
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0]["question"], "Concurrent refresh handled?");
    }

    // Memory was written → status is Warm (not Cold) per §3.3.
    let shown = manager
        .show(ResolveParams {
            id: Some(result.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        shown.status.as_str(),
        "warm",
        "hot_summary IS NOT NULL after memory_update → status='warm' (§3.3)"
    );
}

#[tokio::test]
async fn delegate_mock_executor_fresh_subagent_status_is_cold_not_warm() {
    // P1-1 regression: no adapter_session_id + no memory_update => hot_summary
    // is None => status must be Cold (NOT Warm). The old code unconditionally
    // set Warm, violating §3.3: "warm iff hot_summary IS NOT NULL".
    let db = test_db();
    let settings = SubagentSettings::default();
    let executor = std::sync::Arc::new(MockTaskExecutor);
    let manager = std::sync::Arc::new(SubagentManager::new(db.clone(), settings, "mock", executor));
    let req = DelegateRequest {
        subagent_name: "cold-test".to_string(),
        subagent_id: None,
        cwd: "/repo".to_string(),
        profile: "pi/review-cheap".to_string(),
        intent: None,
        prompt: "do something".to_string(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: Some(bound_provider_id()),
        bound_model_id: Some(bound_model_id()),
        reuse_policy: None,
    };
    let result = manager.delegate(req).await.unwrap();
    let _ = await_task_done(&manager, &result.task_id).await;
    // Verify status is Cold, not Warm.
    let shown = manager
        .show(ResolveParams {
            name: None,
            id: Some(result.subagent_id.clone()),
            cwd: None,
        })
        .await
        .unwrap();
    assert_eq!(
        shown.status.as_str(),
        "cold",
        "fresh subagent with no memory_update must be Cold (§3.3: warm iff hot_summary IS NOT NULL)"
    );
    // Verify hot_summary is None.
    let db_guard = db.lock().unwrap();
    let mem = db_guard
        .subagent_get_memory(&result.subagent_id)
        .unwrap()
        .unwrap();
    assert!(
        mem.hot_summary.is_none(),
        "no memory_update => hot_summary is None"
    );
}

#[tokio::test]
async fn delegate_populates_attempts_after_first_completed_task() {
    // C-1 regression: recent_tasks must be re-fetched AFTER the task result is
    // persisted so the attempts logic sees the just-completed task's
    // result_summary. Before the fix, the pre-execution snapshot (fetched
    // before set_task_status) had result_summary=None for the current task,
    // so memory_updater skipped the attempt entry — attempts_json was always
    // empty on the first completed task. MockTaskExecutor returns a non-empty
    // summary (which becomes result_summary via set_task_status), so after the
    // fix the fresh snapshot's most-recent task carries that summary and an
    // attempt entry is appended.
    let db = test_db();
    let manager = std::sync::Arc::new(SubagentManager::new(
        db.clone(),
        SubagentSettings::default(),
        "mock",
        std::sync::Arc::new(MockTaskExecutor),
    ));
    let req = DelegateRequest {
        subagent_name: "worker".to_string(),
        subagent_id: None,
        cwd: "/repo".to_string(),
        profile: "pi/review-cheap".to_string(),
        intent: None,
        prompt: "investigate the auth module".to_string(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: Some(bound_provider_id()),
        bound_model_id: Some(bound_model_id()),
        reuse_policy: None,
    };
    let result = manager.delegate(req).await.unwrap();
    assert_eq!(result.status.as_str(), "running");
    // delegate() now returns Running immediately; wait for completion,
    // then read the task row from the DB for the summary.
    let final_status = await_task_done(&manager, &result.task_id).await;
    assert_eq!(final_status, "completed");
    let task_row = {
        let db_guard = db.lock().unwrap();
        db_guard
            .subagent_get_task(&result.task_id)
            .unwrap()
            .unwrap()
    };
    let summary = task_row
        .result_summary
        .as_deref()
        .expect("delegate always sets result_summary on completion");
    assert!(
        !summary.is_empty(),
        "mock executor must return a non-empty summary so it becomes result_summary"
    );

    // Scoped: db_guard MUST be dropped before any later manager call that
    // re-locks the same std::sync::Mutex (non-reentrant → deadlock if held).
    let attempts: Vec<String> = {
        let db_guard = db.lock().unwrap();
        let mem = db_guard
            .subagent_get_memory(&result.subagent_id)
            .unwrap()
            .expect("memory row must exist after delegate");
        serde_json::from_str(mem.attempts_json.as_deref().unwrap_or("[]")).unwrap()
    };
    assert!(
        !attempts.is_empty(),
        "attempts must be non-empty after the first completed task (C-1 fix), \
         got: {attempts:?}"
    );
    assert!(
        attempts[0].contains(&result.task_id),
        "attempts entry must reference the just-completed task id, got: {:?}",
        attempts[0]
    );
    assert!(
        attempts[0].contains("investigate the auth module"),
        "attempts entry must include the task summary (from result_summary), \
         got: {:?}",
        attempts[0]
    );
}

// ---------------------------------------------------------------------------
// Coverage tests for manager.rs error/edge paths
// ---------------------------------------------------------------------------

/// Executor that returns a generic (non-SubagentError) anyhow error.
struct BoomExecutor;

#[async_trait]
impl TaskExecutor for BoomExecutor {
    async fn execute(&self, _input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        Err(anyhow::anyhow!("executor boom"))
    }
}

/// Executor that returns a non-empty adapter_session_id (triggers hot binding
/// commit path in execute_task).
struct HotSessionExecutor;

#[async_trait]
impl TaskExecutor for HotSessionExecutor {
    async fn execute(&self, _input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        Ok(ExecutorOutput {
            adapter_session_id: Some("sess-hot-1".to_string()),
            session_reused: false,
            status: TaskStatus::Completed,
            summary: "done".into(),
            usage: Default::default(),
            memory_update: Default::default(),
            error_kind: None,
        })
    }
}

/// Executor returns a `SubagentError` (via anyhow) → downcast Ok branch.
/// `FailingTaskExecutor` returns `SubagentError::SidecarSpawn` wrapped in anyhow.
///
/// After the Bug #1/#2 fix, `delegate()` returns `Running` immediately and the
/// executor error surfaces asynchronously via `execute_and_persist`, which
/// persists `status="failed"` + `error=e.to_string()`. The error string for
/// `SidecarSpawn` is `"sidecar spawn failed: ..."` (from `#[error("sidecar
/// spawn failed: {0}")]`), proving the downcast succeeded — a generic
/// `anyhow::Error` would have been wrapped as `Store` with Display
/// `"database error"`.
#[tokio::test]
async fn delegate_executor_subagent_error_downcasts_and_propagates() {
    let _guard = install_tracing();
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(FailingTaskExecutor {
            reason: "init failed".into(),
        }),
    ));
    let r = m.delegate(req("fail-sub", "do")).await.unwrap();
    assert_eq!(r.status.as_str(), "running");
    let final_status = await_task_done(&m, &r.task_id).await;
    assert_eq!(final_status, "failed");
    let task_row = {
        let g = db.lock().unwrap();
        g.subagent_get_task(&r.task_id).unwrap().unwrap()
    };
    let err_str = task_row.error.as_deref().unwrap_or("");
    assert!(
        err_str.contains("sidecar spawn failed"),
        "SubagentError should downcast — expected 'sidecar spawn failed' in error, got: {err_str}"
    );
    assert!(
        err_str.contains("init failed"),
        "expected 'init failed' in error, got: {err_str}"
    );
}

/// Executor returns a generic anyhow error → downcast Err branch.
///
/// After the Bug #1/#2 fix, the error is persisted to the task row. A
/// non-`SubagentError` anyhow is wrapped as `SubagentError::Store` (Display:
/// `"database error"`) — distinct from `SidecarSpawn`'s `"sidecar spawn
/// failed: ..."`, proving the wrapping path was taken.
#[tokio::test]
async fn delegate_executor_generic_error_wrapped_as_store() {
    let _guard = install_tracing();
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(BoomExecutor),
    ));
    let r = m.delegate(req("boom-sub", "do")).await.unwrap();
    assert_eq!(r.status.as_str(), "running");
    let final_status = await_task_done(&m, &r.task_id).await;
    assert_eq!(final_status, "failed");
    let task_row = {
        let g = db.lock().unwrap();
        g.subagent_get_task(&r.task_id).unwrap().unwrap()
    };
    let err_str = task_row.error.as_deref().unwrap_or("");
    assert!(
        err_str.contains("database error"),
        "non-SubagentError anyhow should be wrapped as Store — expected 'database error' in error, got: {err_str}"
    );
}

/// When no model_override + fresh subagent, execute_task resolves the model
/// from `subagent.bound_model_id` (spec §3.3: bound_model_id is the
/// authoritative source after model_override; profiles no longer carry a
/// model).
#[tokio::test]
async fn delegate_sets_model_when_execute_task_returns_none() {
    let _guard = install_tracing();
    let mut settings = SubagentSettings::default();
    settings.profiles.insert(
        "custom/empty-model".to_string(),
        SubagentProfileConfig {
            write_access: false,
            tools: vec![],
            context_budget_tokens: 3000,
            timeout_seconds: 120,
        },
    );
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        db,
        settings,
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    ));
    let expected_model = bound_model_id();
    let r = m
        .delegate(DelegateRequest {
            profile: "custom/empty-model".to_string(),
            ..req("empty-model-sub", "do")
        })
        .await
        .unwrap();
    assert_eq!(r.profile, "custom/empty-model");
    // delegate() returns Running immediately with `model: req.model_override.clone()`
    // (None here). The resolved model (bound_model_id) is computed in execute_task
    // (background). Verify it after completion via the subagent's bound_model_id.
    assert!(
        r.model.is_none(),
        "Running result carries model_override only — None when no override"
    );
    let _ = await_task_done(&m, &r.task_id).await;
    let shown = m
        .show(ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        shown.bound_model_id, expected_model,
        "model falls back to bound_model_id when profile model is empty and no override"
    );
}

/// Hot binding commit failure (table dropped) → Store error path.
/// The executor returns a non-empty adapter_session_id, triggering the hot
/// binding commit path in `execute_task`. Dropping the bindings table makes
/// the commit fail with `SubagentError::Store` (Display: `"database error"`).
///
/// After the Bug #1/#2 fix, the error surfaces asynchronously: `delegate()`
/// returns `Running`, `execute_and_persist` catches the error and persists
/// `status="failed"` + `error=e.to_string()`.
#[tokio::test]
async fn delegate_hot_binding_commit_failure_returns_store_error() {
    let _guard = install_tracing();
    let db = test_db();
    {
        let g = db.lock().unwrap();
        g.conn()
            .execute("DROP TABLE subagent_harness_bindings", [])
            .unwrap();
    }
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(HotSessionExecutor),
    ));
    let r = m.delegate(req("hot-bind-sub", "do")).await.unwrap();
    assert_eq!(r.status.as_str(), "running");
    let final_status = await_task_done(&m, &r.task_id).await;
    assert_eq!(final_status, "failed");
    let task_row = {
        let g = db.lock().unwrap();
        g.subagent_get_task(&r.task_id).unwrap().unwrap()
    };
    let err_str = task_row.error.as_deref().unwrap_or("");
    assert!(
        err_str.contains("database error"),
        "hot binding commit failure should surface as store_error — expected 'database error' in error, got: {err_str}"
    );
}

/// Dispatcher shuts down promptly when the watch channel sends true (lines 568-571).
#[tokio::test]
async fn dispatcher_shutdown_returns_promptly() {
    let _guard = install_tracing();
    let db = test_db();
    let manager = std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    ));
    let (tx, rx) = tokio::sync::watch::channel(false);
    let handle = manager.clone().spawn_task_dispatcher(rx);
    // Send shutdown immediately.
    tx.send(true).unwrap();
    // Must complete within 2s (dispatcher polls every 200ms).
    tokio::time::timeout(std::time::Duration::from_secs(2), handle)
        .await
        .expect("dispatcher shuts down promptly after shutdown signal")
        .expect("dispatcher task joined cleanly");
}

/// Dispatcher skips queued tasks while the gate is paused (line 580 `continue`).
/// A queued task is inserted manually; the dispatcher must NOT pick it up
/// while the gate remains paused across multiple ticks.
#[tokio::test]
async fn dispatcher_skips_queued_task_while_gate_paused() {
    let _guard = install_tracing();
    let db = test_db();
    let gate = std::sync::Arc::new(PressureGate::new());
    gate.set_action(PressureAction::PauseNewTasks);
    let manager = std::sync::Arc::new(SubagentManager::with_pressure_gate(
        db.clone(),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
        Some(gate.clone()),
    ));
    // Seed a subagent + queued task so the dispatcher has something to skip.
    {
        use busytok_store::repository::{SubagentLogicalSubagentRow, SubagentTaskRow};
        let g = db.lock().unwrap();
        g.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test(
            "sub-paused-skip",
            "paused-skip-sub",
        ))
        .unwrap();
        g.subagent_insert_task(&SubagentTaskRow {
            id: "task-paused-skip".into(),
            subagent_id: "sub-paused-skip".into(),
            source_harness: None,
            source_session_id: None,
            intent: None,
            profile: "pi/search-cheap".into(),
            prompt: Some("queued".into()),
            prompt_artifact_ref: None,
            output_schema_name: None,
            output_schema_version: 1,
            status: "queued".into(),
            result_summary: None,
            result_json: None,
            error: None,
            created_at_ms: busytok_domain::now_ms(),
            started_at_ms: None,
            completed_at_ms: None,
            timeout_seconds: None,
            model_override: None,
            error_kind: None,
            queue_reason: None,
        })
        .unwrap();
    }
    let (tx, rx) = tokio::sync::watch::channel(false);
    let handle = manager.clone().spawn_task_dispatcher(rx);
    // Wait for several dispatcher ticks (interval = 200ms).
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    // Task must still be queued — the dispatcher skipped it (gate paused).
    {
        let g = db.lock().unwrap();
        let tasks = g.subagent_list_tasks("sub-paused-skip", 10).unwrap();
        let task = tasks.iter().find(|t| t.id == "task-paused-skip").unwrap();
        assert_eq!(
            task.status, "queued",
            "task must remain queued while gate is paused (line 580 continue)"
        );
    }
    tx.send(true).unwrap();
    let _ = handle.await;
}

/// `queue_reason` field on `DelegateResult` — distinguishes "blocked by
/// pressure gate" from "subagent busy" for CLI/automation observability.
/// When the gate is paused, `delegate` must return `status: Queued` with
/// `queue_reason: PressureGatePaused`.
#[tokio::test]
async fn delegate_queue_reason_pressure_gate_paused() {
    let _guard = install_tracing();
    let db = test_db();
    let gate = std::sync::Arc::new(PressureGate::new());
    gate.set_action(PressureAction::PauseNewTasks);
    let manager = std::sync::Arc::new(SubagentManager::with_pressure_gate(
        db.clone(),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
        Some(gate.clone()),
    ));
    let r = manager.delegate(req("sub-qr-gate", "do")).await.unwrap();
    assert_eq!(r.status, TaskStatus::Queued);
    assert_eq!(
        r.queue_reason,
        Some(QueueReason::PressureGatePaused),
        "queued while gate paused must set queue_reason = PressureGatePaused"
    );
}

/// `queue_reason` = `SubagentBusy` when the subagent already has a running
/// task (no gate pause). The second delegate for the same subagent must
/// return `status: Queued` with `queue_reason: SubagentBusy`.
#[tokio::test]
async fn delegate_queue_reason_subagent_busy() {
    let _guard = install_tracing();
    let db = test_db();
    // Use a BlockingExecutor so the first task stays "running" — the
    // second delegate for the same subagent must queue.
    let manager = std::sync::Arc::new(SubagentManager::new(
        db.clone(),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(BlockingExecutor::new()) as std::sync::Arc<dyn TaskExecutor>,
    ));
    // First delegate starts executing (running).
    let r1 = manager.delegate(req("sub-qr-busy", "first")).await.unwrap();
    assert_eq!(r1.status, TaskStatus::Running);
    assert_eq!(
        r1.queue_reason, None,
        "running task must have no queue_reason"
    );
    // Second delegate for the same subagent → must queue (subagent busy).
    let r2 = manager
        .delegate(req("sub-qr-busy", "second"))
        .await
        .unwrap();
    assert_eq!(r2.status, TaskStatus::Queued);
    assert_eq!(
        r2.queue_reason,
        Some(QueueReason::SubagentBusy),
        "queued while subagent busy must set queue_reason = SubagentBusy"
    );
    // Clean up: cancel the blocking task so the runtime can shut down.
    let _ = manager.cancel_task(&r1.task_id, None).await;
}

/// Dispatcher marks a queued task as 'failed' when the subagent can't be
/// resolved (lines 602-615). The task is inserted with FK off so it references
/// a non-existent subagent_id.
#[tokio::test]
async fn dispatcher_marks_task_failed_when_subagent_missing() {
    let _guard = install_tracing();
    let db = test_db();
    let manager = std::sync::Arc::new(SubagentManager::new(
        db.clone(),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    ));
    // Insert a queued task with a non-existent subagent_id. FK is disabled
    // temporarily so the INSERT succeeds.
    {
        use busytok_store::repository::SubagentTaskRow;
        let g = db.lock().unwrap();
        g.conn().execute_batch("PRAGMA foreign_keys = OFF").unwrap();
        g.subagent_insert_task(&SubagentTaskRow::for_test(
            "task-orphan",
            "ghost-sub-id",
            "pi/search-cheap",
            "orphan task",
        ))
        .unwrap();
        g.conn().execute_batch("PRAGMA foreign_keys = ON").unwrap();
    }
    let (tx, rx) = tokio::sync::watch::channel(false);
    let handle = manager.clone().spawn_task_dispatcher(rx);
    // Poll for the dispatcher to pick + fail the orphan task.
    let mut failed = false;
    for _ in 0..30 {
        let status = {
            let g = db.lock().unwrap();
            g.subagent_list_tasks("ghost-sub-id", 10)
                .unwrap()
                .into_iter()
                .find(|t| t.id == "task-orphan")
                .map(|t| (t.status, t.error))
        };
        if let Some((status, error)) = status {
            if status == "failed" {
                assert_eq!(
                    error.as_deref(),
                    Some("subagent not found"),
                    "orphan task error message"
                );
                failed = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        failed,
        "orphan task must be marked failed when subagent can't be resolved"
    );
    tx.send(true).unwrap();
    let _ = handle.await;
}

/// Dispatcher's execute_task fails → marks task as 'failed' (lines 621-631).
/// Uses FailingTaskExecutor so execute_task returns Err. The task is queued
/// while the gate is paused, then the gate is cleared so the dispatcher picks
/// it up. Also covers the queued-path info! args (lines 218, 220) and the
/// executor downcast Ok branch (lines 389-390) via the dispatcher path.
#[tokio::test]
async fn dispatcher_marks_task_failed_when_execute_task_errors() {
    let _guard = install_tracing();
    let db = test_db();
    let gate = std::sync::Arc::new(PressureGate::new());
    gate.set_action(PressureAction::PauseNewTasks);
    let manager = std::sync::Arc::new(SubagentManager::with_pressure_gate(
        db.clone(),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(FailingTaskExecutor {
            reason: "dispatch boom".into(),
        }),
        Some(gate.clone()),
    ));
    // Queue a task while the gate is paused — delegate returns Queued and
    // the queued info! (lines 214-221) fires with the subscriber installed.
    let r = manager
        .delegate(req("fail-dispatch-sub", "do"))
        .await
        .unwrap();
    assert_eq!(r.status, TaskStatus::Queued);

    let (tx, rx) = tokio::sync::watch::channel(false);
    let handle = manager.clone().spawn_task_dispatcher(rx);

    // Clear the gate — dispatcher picks + execute_task fails.
    gate.set_action(PressureAction::Resume);

    let mut failed = false;
    for _ in 0..50 {
        let status = {
            let g = db.lock().unwrap();
            g.subagent_list_tasks(&r.subagent_id, 10)
                .unwrap()
                .into_iter()
                .find(|t| t.id == r.task_id)
                .map(|t| (t.status, t.error))
        };
        if let Some((status, error)) = status {
            if status == "failed" {
                assert!(
                    error.is_some(),
                    "failed task must have an error message: {error:?}"
                );
                failed = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        failed,
        "task must be marked failed after execute_task returns Err"
    );
    tx.send(true).unwrap();
    let _ = handle.await;
}

/// Invalid subagent name triggers resolve_by_name error → the warn! at
/// line 136-140 evaluates `e.code()` with the subscriber installed.
#[tokio::test]
async fn delegate_invalid_name_evaluates_reject_warn_args() {
    let _guard = install_tracing();
    let m = manager().await;
    let err = m.delegate(req("bad name!", "do")).await.unwrap_err();
    assert_eq!(err.code(), "subagent.invalid_name");
}

// ---------------------------------------------------------------------------
// Phase 2 Task 2: SubagentManager aggregate data methods
//   * recent_tasks_all(limit)        — recent tasks across ALL subagents
//   * task_counts_by_subagent()      — per-subagent task counts
//   * last_task_by_subagent()        — per-subagent (created_at_ms, status)
// These tests seed rows directly via the store helpers (bypassing delegate)
// so the assertions reflect purely the DB query + manager mapping layer.
// ---------------------------------------------------------------------------

/// Seed a logical subagent row with the given id (also used as the name).
fn seed_logical_subagent(db: &busytok_store::Database, id: &str) {
    use busytok_store::repository::SubagentLogicalSubagentRow;
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test(id, id))
        .unwrap();
}

/// Seed a task row with explicit status + created_at_ms (ms epoch).
fn seed_task(
    db: &busytok_store::Database,
    id: &str,
    subagent_id: &str,
    status: &str,
    created_at_ms: i64,
) {
    use busytok_store::repository::SubagentTaskRow;
    let mut row = SubagentTaskRow::for_test(id, subagent_id, "pi/search-cheap", "prompt");
    row.status = status.to_string();
    row.created_at_ms = created_at_ms;
    db.subagent_insert_task(&row).unwrap();
}

fn manager_with_db(
    db: std::sync::Arc<std::sync::Mutex<Database>>,
) -> std::sync::Arc<SubagentManager> {
    std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    ))
}

#[tokio::test]
async fn recent_tasks_all_returns_across_all_subagents_in_desc_order() {
    let db = test_db();
    {
        let g = db.lock().unwrap();
        seed_logical_subagent(&g, "sub-a");
        seed_logical_subagent(&g, "sub-b");
        seed_task(&g, "t1", "sub-a", "completed", 1000);
        seed_task(&g, "t2", "sub-b", "failed", 2000);
        seed_task(&g, "t3", "sub-a", "completed", 3000);
    }
    let manager = manager_with_db(db);
    let tasks = manager.recent_tasks_all(20).await.unwrap();
    assert_eq!(tasks.len(), 3, "all three tasks across both subagents");
    assert_eq!(tasks[0].id, "t3", "newest first (desc by created_at_ms)");
    assert_eq!(tasks[1].id, "t2");
    assert_eq!(tasks[2].id, "t1");
    // Mapping reuses task_row_to_summary → status parsed, profile preserved.
    assert_eq!(tasks[0].subagent_id, "sub-a");
    assert_eq!(tasks[1].subagent_id, "sub-b");
    assert_eq!(tasks[0].profile, "pi/search-cheap");
    assert_eq!(tasks[0].status.as_str(), "completed");
    assert_eq!(tasks[1].status.as_str(), "failed");
}

#[tokio::test]
async fn recent_tasks_all_respects_limit() {
    let db = test_db();
    {
        let g = db.lock().unwrap();
        seed_logical_subagent(&g, "sub-a");
        for i in 0..10 {
            seed_task(&g, &format!("t{i}"), "sub-a", "completed", 1000 + i);
        }
    }
    let manager = manager_with_db(db);
    let tasks = manager.recent_tasks_all(3).await.unwrap();
    assert_eq!(tasks.len(), 3);
    assert_eq!(tasks[0].id, "t9", "limit=3 returns the 3 newest");
}

#[tokio::test]
async fn recent_tasks_all_empty_when_no_tasks() {
    let db = test_db();
    let manager = manager_with_db(db);
    let tasks = manager.recent_tasks_all(20).await.unwrap();
    assert!(tasks.is_empty());
}

#[tokio::test]
async fn task_counts_by_subagent_groups_correctly_across_subagents() {
    let db = test_db();
    {
        let g = db.lock().unwrap();
        seed_logical_subagent(&g, "sub-a");
        seed_logical_subagent(&g, "sub-b");
        seed_task(&g, "t1", "sub-a", "completed", 1000);
        seed_task(&g, "t2", "sub-a", "failed", 2000);
        seed_task(&g, "t3", "sub-b", "completed", 3000);
    }
    let manager = manager_with_db(db);
    let counts = manager.task_counts_by_subagent().await.unwrap();
    assert_eq!(counts.get("sub-a"), Some(&2), "sub-a has 2 tasks");
    assert_eq!(counts.get("sub-b"), Some(&1), "sub-b has 1 task");
    assert_eq!(
        counts.len(),
        2,
        "only subagents with tasks appear (no zero-count rows)"
    );
}

#[tokio::test]
async fn task_counts_by_subagent_empty_when_no_tasks() {
    let db = test_db();
    let manager = manager_with_db(db);
    let counts = manager.task_counts_by_subagent().await.unwrap();
    assert!(counts.is_empty());
}

#[tokio::test]
async fn last_task_by_subagent_returns_latest_per_subagent() {
    let db = test_db();
    {
        let g = db.lock().unwrap();
        seed_logical_subagent(&g, "sub-a");
        seed_logical_subagent(&g, "sub-b");
        // sub-a: t1 completed @1000, t2 failed @2000 → last is t2
        seed_task(&g, "t1", "sub-a", "completed", 1000);
        seed_task(&g, "t2", "sub-a", "failed", 2000);
        // sub-b: single task completed @5000
        seed_task(&g, "t3", "sub-b", "completed", 5000);
    }
    let manager = manager_with_db(db);
    let lasts = manager.last_task_by_subagent().await.unwrap();
    assert_eq!(lasts.len(), 2, "one entry per subagent that has tasks");
    let (created_at, status) = lasts.get("sub-a").unwrap();
    assert_eq!(*created_at, 2000, "sub-a last task is t2 @2000ms");
    assert_eq!(status, "failed");
    let (created_at_b, status_b) = lasts.get("sub-b").unwrap();
    assert_eq!(*created_at_b, 5000);
    assert_eq!(status_b, "completed");
}

#[tokio::test]
async fn last_task_by_subagent_empty_when_no_tasks() {
    let db = test_db();
    let manager = manager_with_db(db);
    let lasts = manager.last_task_by_subagent().await.unwrap();
    assert!(lasts.is_empty());
}

// --- Task 5: execute_task validation chain (spec §4.3 fail-fast) ---------

/// `execute_task_fails_when_bound_provider_disabled` — Task 5 spec §4.3
/// fail-fast: if the subagent's bound provider is disabled, `delegate()`
/// must reject the request with `SubagentError::Validation` carrying
/// "bound provider disabled" (NOT pass the disabled provider downstream to
/// the executor/sidecar, which would surface as an opaque auth/network
/// error). The reuse path is exercised (`subagent_id` set, bound fields
/// ignored on the request) so the subagent's persisted `bound_provider_id`
/// is the source of truth.
#[tokio::test]
async fn execute_task_fails_when_bound_provider_disabled() {
    use busytok_store::repository::{SubagentLogicalSubagentRow, SubagentMemoryRow};

    let _guard = install_tracing();
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    // Insert a provider that is DISABLED (spec §4.3 fail-fast).
    let provider = create_provider(
        db.lock().unwrap().conn(),
        CreateProviderReq {
            name: "P1-disabled".into(),
            provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: false, // disabled — execute_task must reject
            api_key: Some("sk-test".into()),
        },
    )
    .unwrap();
    let model = create_model(
        db.lock().unwrap().conn(),
        CreateModelReq {
            provider_id: provider.id.clone(),
            model_id: "gpt-4o".into(),
            enabled: true,
            tags: vec![],
            display_name: None,
            reasoning: None,
            context_window: Some(128000),
            max_tokens: Some(16384),
        },
    )
    .unwrap();
    // Insert a subagent bound to the disabled provider. The reuse path
    // (delegate with subagent_id set) reads bound fields from this row,
    // ignoring the request's bound_provider_id / bound_model_id.
    {
        let g = db.lock().unwrap();
        g.subagent_upsert_logical(&SubagentLogicalSubagentRow {
            id: "sub-1".into(),
            name: "test-sub".into(),
            project_id: "h".into(),
            repo_path: "/tmp".into(),
            repo_hash: "h".into(),
            branch: None,
            intent: None,
            default_profile: "pi/search-cheap".into(),
            bound_provider_id: provider.id.clone(),
            bound_model_id: model.model_id.clone(),
            status: "cold".into(),
            created_at_ms: 1000,
            updated_at_ms: 1000,
            last_active_at_ms: None,
        })
        .unwrap();
        g.subagent_upsert_memory(&SubagentMemoryRow::new_empty("sub-1"))
            .unwrap();
    }

    let manager = std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "mock",
        std::sync::Arc::new(MockTaskExecutor),
    ));

    let req = DelegateRequest {
        subagent_name: "test-sub".into(),
        subagent_id: Some("sub-1".into()), // reuse path — bound fields ignored
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        intent: None,
        prompt: "hi".into(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: None,
        bound_model_id: None,
        reuse_policy: None,
    };
    let result = manager.delegate(req).await;
    assert!(
        result.is_err(),
        "delegate should fail when bound provider is disabled"
    );
    let err = result.unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("bound provider disabled"),
        "expected 'bound provider disabled' in error, got: {msg}"
    );
    // Verify the error is SubagentError::Validation (not a store error).
    assert!(
        matches!(err, SubagentError::Validation(_)),
        "expected SubagentError::Validation variant, got {err:?}"
    );
}

/// `execute_task_fails_when_bound_provider_missing_api_key` — Task 5 spec
/// §4.3 fail-fast: a provider with an empty api_key must be rejected at
/// validation time (not deferred to the sidecar, where it would surface as
/// an opaque -32010 AUTH_FAILURE on the first turn).
#[tokio::test]
async fn execute_task_fails_when_bound_provider_missing_api_key() {
    use busytok_store::repository::{SubagentLogicalSubagentRow, SubagentMemoryRow};

    let _guard = install_tracing();
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    // Enabled provider but with NO api_key — must be rejected.
    let provider = create_provider(
        db.lock().unwrap().conn(),
        CreateProviderReq {
            name: "P1-no-key".into(),
            provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: true,
            api_key: None, // missing — execute_task must reject
        },
    )
    .unwrap();
    let model = create_model(
        db.lock().unwrap().conn(),
        CreateModelReq {
            provider_id: provider.id.clone(),
            model_id: "gpt-4o".into(),
            enabled: true,
            tags: vec![],
            display_name: None,
            reasoning: None,
            context_window: Some(128000),
            max_tokens: Some(16384),
        },
    )
    .unwrap();
    {
        let g = db.lock().unwrap();
        g.subagent_upsert_logical(&SubagentLogicalSubagentRow {
            id: "sub-nokey".into(),
            name: "test-sub-nokey".into(),
            project_id: "h".into(),
            repo_path: "/tmp".into(),
            repo_hash: "h".into(),
            branch: None,
            intent: None,
            default_profile: "pi/search-cheap".into(),
            bound_provider_id: provider.id.clone(),
            bound_model_id: model.model_id.clone(),
            status: "cold".into(),
            created_at_ms: 1000,
            updated_at_ms: 1000,
            last_active_at_ms: None,
        })
        .unwrap();
        g.subagent_upsert_memory(&SubagentMemoryRow::new_empty("sub-nokey"))
            .unwrap();
    }

    let manager = std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "mock",
        std::sync::Arc::new(MockTaskExecutor),
    ));

    let req = DelegateRequest {
        subagent_name: "test-sub-nokey".into(),
        subagent_id: Some("sub-nokey".into()),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        intent: None,
        prompt: "hi".into(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: None,
        bound_model_id: None,
        reuse_policy: None,
    };
    let result = manager.delegate(req).await;
    assert!(
        result.is_err(),
        "delegate should fail when bound provider has no api key"
    );
    let err = result.unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("bound provider missing api key"),
        "expected 'bound provider missing api key' in error, got: {msg}"
    );
    assert!(
        matches!(err, SubagentError::Validation(_)),
        "expected SubagentError::Validation variant, got {err:?}"
    );
}

/// `execute_task_fails_when_bound_model_not_found` — Task 5 spec §4.3
/// fail-fast: if the effective model id (from `task.model_override` or
/// `subagent.bound_model_id`) doesn't exist in the bound provider's model
/// list, `delegate()` must reject with `SubagentError::Validation` carrying
/// "bound model not found".
#[tokio::test]
async fn execute_task_fails_when_bound_model_not_found() {
    use busytok_store::repository::{SubagentLogicalSubagentRow, SubagentMemoryRow};

    let _guard = install_tracing();
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    let provider = create_provider(
        db.lock().unwrap().conn(),
        CreateProviderReq {
            name: "P1-model-missing".into(),
            provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: true,
            api_key: Some("sk-test".into()),
        },
    )
    .unwrap();
    // Note: NO model is created for this provider — the bound_model_id
    // below refers to a non-existent model.
    {
        let g = db.lock().unwrap();
        g.subagent_upsert_logical(&SubagentLogicalSubagentRow {
            id: "sub-nomodel".into(),
            name: "test-sub-nomodel".into(),
            project_id: "h".into(),
            repo_path: "/tmp".into(),
            repo_hash: "h".into(),
            branch: None,
            intent: None,
            default_profile: "pi/search-cheap".into(),
            bound_provider_id: provider.id.clone(),
            bound_model_id: "ghost-model".into(), // doesn't exist
            status: "cold".into(),
            created_at_ms: 1000,
            updated_at_ms: 1000,
            last_active_at_ms: None,
        })
        .unwrap();
        g.subagent_upsert_memory(&SubagentMemoryRow::new_empty("sub-nomodel"))
            .unwrap();
    }

    let manager = std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "mock",
        std::sync::Arc::new(MockTaskExecutor),
    ));

    let req = DelegateRequest {
        subagent_name: "test-sub-nomodel".into(),
        subagent_id: Some("sub-nomodel".into()),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        intent: None,
        prompt: "hi".into(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: None,
        bound_model_id: None,
        reuse_policy: None,
    };
    let result = manager.delegate(req).await;
    assert!(
        result.is_err(),
        "delegate should fail when bound model is not found"
    );
    let err = result.unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("bound model not found"),
        "expected 'bound model not found' in error, got: {msg}"
    );
    assert!(
        matches!(err, SubagentError::Validation(_)),
        "expected SubagentError::Validation variant, got {err:?}"
    );
}

/// `execute_task_fails_when_bound_model_disabled` — Task 5 spec §4.3
/// fail-fast: a disabled model in the bound provider must be rejected at
/// validation time.
#[tokio::test]
async fn execute_task_fails_when_bound_model_disabled() {
    use busytok_store::repository::{SubagentLogicalSubagentRow, SubagentMemoryRow};

    let _guard = install_tracing();
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    let provider = create_provider(
        db.lock().unwrap().conn(),
        CreateProviderReq {
            name: "P1-model-disabled".into(),
            provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: true,
            api_key: Some("sk-test".into()),
        },
    )
    .unwrap();
    let model = create_model(
        db.lock().unwrap().conn(),
        CreateModelReq {
            provider_id: provider.id.clone(),
            model_id: "gpt-4o".into(),
            enabled: false, // disabled — execute_task must reject
            tags: vec![],
            display_name: None,
            reasoning: None,
            context_window: Some(128000),
            max_tokens: Some(16384),
        },
    )
    .unwrap();
    {
        let g = db.lock().unwrap();
        g.subagent_upsert_logical(&SubagentLogicalSubagentRow {
            id: "sub-model-disabled".into(),
            name: "test-sub-model-disabled".into(),
            project_id: "h".into(),
            repo_path: "/tmp".into(),
            repo_hash: "h".into(),
            branch: None,
            intent: None,
            default_profile: "pi/search-cheap".into(),
            bound_provider_id: provider.id.clone(),
            bound_model_id: model.model_id.clone(),
            status: "cold".into(),
            created_at_ms: 1000,
            updated_at_ms: 1000,
            last_active_at_ms: None,
        })
        .unwrap();
        g.subagent_upsert_memory(&SubagentMemoryRow::new_empty("sub-model-disabled"))
            .unwrap();
    }

    let manager = std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "mock",
        std::sync::Arc::new(MockTaskExecutor),
    ));

    let req = DelegateRequest {
        subagent_name: "test-sub-model-disabled".into(),
        subagent_id: Some("sub-model-disabled".into()),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        intent: None,
        prompt: "hi".into(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: None,
        bound_model_id: None,
        reuse_policy: None,
    };
    let result = manager.delegate(req).await;
    assert!(
        result.is_err(),
        "delegate should fail when bound model is disabled"
    );
    let err = result.unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("bound model disabled"),
        "expected 'bound model disabled' in error, got: {msg}"
    );
    assert!(
        matches!(err, SubagentError::Validation(_)),
        "expected SubagentError::Validation variant, got {err:?}"
    );
}

// --- I-2: fail-fast on missing context_window / max_tokens metadata ---------

/// `execute_task_fails_when_model_missing_context_window` — I-2 spec §4.3
/// fail-fast: a model with NULL `context_window` must be rejected at
/// validation time (not deferred to the Pi SDK, which would interpret 0 as
/// "no context" — silent breakage). The reuse path is exercised
/// (`subagent_id` set, bound fields from row) so the subagent's persisted
/// binding is the source of truth.
#[tokio::test]
async fn execute_task_fails_when_model_missing_context_window() {
    use busytok_store::repository::{SubagentLogicalSubagentRow, SubagentMemoryRow};

    let _guard = install_tracing();
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    let provider = create_provider(
        db.lock().unwrap().conn(),
        CreateProviderReq {
            name: "P1-no-ctx".into(),
            provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: true,
            api_key: Some("sk-test".into()),
        },
    )
    .unwrap();
    // Create a model with NULL context_window (simulates a pre-existing/seed
    // model before the metadata backfill, or a direct SQL insert).
    let model = create_model(
        db.lock().unwrap().conn(),
        CreateModelReq {
            provider_id: provider.id.clone(),
            model_id: "gpt-4o".into(),
            enabled: true,
            tags: vec![],
            display_name: None,
            reasoning: None,
            context_window: None, // missing — execute_task must reject
            max_tokens: Some(16384),
        },
    )
    .unwrap();
    {
        let g = db.lock().unwrap();
        g.subagent_upsert_logical(&SubagentLogicalSubagentRow {
            id: "sub-no-ctx".into(),
            name: "test-sub-no-ctx".into(),
            project_id: "h".into(),
            repo_path: "/tmp".into(),
            repo_hash: "h".into(),
            branch: None,
            intent: None,
            default_profile: "pi/search-cheap".into(),
            bound_provider_id: provider.id.clone(),
            bound_model_id: model.model_id.clone(),
            status: "cold".into(),
            created_at_ms: 1000,
            updated_at_ms: 1000,
            last_active_at_ms: None,
        })
        .unwrap();
        g.subagent_upsert_memory(&SubagentMemoryRow::new_empty("sub-no-ctx"))
            .unwrap();
    }

    let manager = std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "mock",
        std::sync::Arc::new(MockTaskExecutor),
    ));

    let req = DelegateRequest {
        subagent_name: "test-sub-no-ctx".into(),
        subagent_id: Some("sub-no-ctx".into()), // reuse path — bound fields from row
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        intent: None,
        prompt: "hi".into(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: None,
        bound_model_id: None,
        reuse_policy: None,
    };
    let err = manager.delegate(req).await.unwrap_err();
    assert!(
        matches!(err, SubagentError::Validation(_)),
        "expected SubagentError::Validation variant, got {err:?}"
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("missing context_window metadata"),
        "expected 'missing context_window metadata' in error, got: {msg}"
    );
}

/// `execute_task_fails_when_model_missing_max_tokens` — I-2 companion: a
/// model with NULL `max_tokens` must also be rejected at validation time.
#[tokio::test]
async fn execute_task_fails_when_model_missing_max_tokens() {
    use busytok_store::repository::{SubagentLogicalSubagentRow, SubagentMemoryRow};

    let _guard = install_tracing();
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    let provider = create_provider(
        db.lock().unwrap().conn(),
        CreateProviderReq {
            name: "P1-no-max".into(),
            provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: true,
            api_key: Some("sk-test".into()),
        },
    )
    .unwrap();
    let model = create_model(
        db.lock().unwrap().conn(),
        CreateModelReq {
            provider_id: provider.id.clone(),
            model_id: "gpt-4o".into(),
            enabled: true,
            tags: vec![],
            display_name: None,
            reasoning: None,
            context_window: Some(128000),
            max_tokens: None, // missing — execute_task must reject
        },
    )
    .unwrap();
    {
        let g = db.lock().unwrap();
        g.subagent_upsert_logical(&SubagentLogicalSubagentRow {
            id: "sub-no-max".into(),
            name: "test-sub-no-max".into(),
            project_id: "h".into(),
            repo_path: "/tmp".into(),
            repo_hash: "h".into(),
            branch: None,
            intent: None,
            default_profile: "pi/search-cheap".into(),
            bound_provider_id: provider.id.clone(),
            bound_model_id: model.model_id.clone(),
            status: "cold".into(),
            created_at_ms: 1000,
            updated_at_ms: 1000,
            last_active_at_ms: None,
        })
        .unwrap();
        g.subagent_upsert_memory(&SubagentMemoryRow::new_empty("sub-no-max"))
            .unwrap();
    }

    let manager = std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "mock",
        std::sync::Arc::new(MockTaskExecutor),
    ));

    let req = DelegateRequest {
        subagent_name: "test-sub-no-max".into(),
        subagent_id: Some("sub-no-max".into()),
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        intent: None,
        prompt: "hi".into(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: None,
        bound_model_id: None,
        reuse_policy: None,
    };
    let err = manager.delegate(req).await.unwrap_err();
    assert!(
        matches!(err, SubagentError::Validation(_)),
        "expected SubagentError::Validation variant, got {err:?}"
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("missing max_tokens metadata"),
        "expected 'missing max_tokens metadata' in error, got: {msg}"
    );
}

// --- Task 9: coverage-gap closures for execute_task validation paths --------

/// Spec §3.3 "both or neither": delegating with only one of
/// `bound_provider_id` / `bound_model_id` set must fail with a Validation
/// error BEFORE name resolution or DB writes. Covers the `_ =>` match arm in
/// `delegate()` (lines 175-179).
#[tokio::test]
async fn delegate_rejects_mismatched_bound_fields_provider_only() {
    let _guard = install_tracing();
    let m = manager().await;
    let mut r = req("mismatch-p", "do");
    // provider set, model cleared → mismatch
    r.bound_model_id = None;
    let err = m.delegate(r).await.unwrap_err();
    assert!(matches!(err, SubagentError::Validation(_)));
    let msg = format!("{err}");
    assert!(
        msg.contains("must be provided together"),
        "expected 'must be provided together' in error, got: {msg}"
    );
}

/// Same invariant, opposite direction: model set, provider cleared.
#[tokio::test]
async fn delegate_rejects_mismatched_bound_fields_model_only() {
    let _guard = install_tracing();
    let m = manager().await;
    let mut r = req("mismatch-m", "do");
    // model set, provider cleared → mismatch
    r.bound_provider_id = None;
    let err = m.delegate(r).await.unwrap_err();
    assert!(matches!(err, SubagentError::Validation(_)));
    let msg = format!("{err}");
    assert!(
        msg.contains("must be provided together"),
        "expected 'must be provided together' in error, got: {msg}"
    );
}

/// `execute_task` must fail fast with "bound provider not found" when the
/// provider referenced by an existing subagent's `bound_provider_id` has been
/// deleted from the catalog. Exercises the reuse path (`subagent_id` set) so
/// the subagent's stored bound fields are the source of truth, and deletes the
/// provider between the first and second delegate call. Covers the
/// `ok_or_else(|| Validation("bound provider not found: ..."))` arm.
#[tokio::test]
async fn execute_task_fails_when_bound_provider_deleted() {
    use busytok_store::repository::{SubagentLogicalSubagentRow, SubagentMemoryRow};

    let _guard = install_tracing();
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    let provider = create_provider(
        db.lock().unwrap().conn(),
        CreateProviderReq {
            name: "P1-deletable".into(),
            provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: true,
            api_key: Some("sk-test".into()),
        },
    )
    .unwrap();
    let model = create_model(
        db.lock().unwrap().conn(),
        CreateModelReq {
            provider_id: provider.id.clone(),
            model_id: "gpt-4o".into(),
            enabled: true,
            tags: vec![],
            display_name: None,
            reasoning: None,
            context_window: Some(128000),
            max_tokens: Some(16384),
        },
    )
    .unwrap();
    // Insert a subagent bound to the (soon-to-be-deleted) provider.
    {
        let g = db.lock().unwrap();
        g.subagent_upsert_logical(&SubagentLogicalSubagentRow {
            id: "sub-del".into(),
            name: "test-sub-del".into(),
            project_id: "h".into(),
            repo_path: "/tmp".into(),
            repo_hash: "h".into(),
            branch: None,
            intent: None,
            default_profile: "pi/search-cheap".into(),
            bound_provider_id: provider.id.clone(),
            bound_model_id: model.model_id.clone(),
            status: "cold".into(),
            created_at_ms: 1000,
            updated_at_ms: 1000,
            last_active_at_ms: None,
        })
        .unwrap();
        g.subagent_upsert_memory(&SubagentMemoryRow::new_empty("sub-del"))
            .unwrap();
    }
    // Delete the provider from the catalog AFTER the subagent is bound.
    db.lock()
        .unwrap()
        .delete_provider(&provider.id)
        .expect("delete provider");

    let manager = std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "mock",
        std::sync::Arc::new(MockTaskExecutor),
    ));

    let req = DelegateRequest {
        subagent_name: "test-sub-del".into(),
        subagent_id: Some("sub-del".into()), // reuse path — bound fields from row
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        intent: None,
        prompt: "hi".into(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: None,
        bound_model_id: None,
        reuse_policy: None,
    };
    let err = manager.delegate(req).await.unwrap_err();
    assert!(
        matches!(err, SubagentError::Validation(_)),
        "expected SubagentError::Validation variant, got {err:?}"
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("bound provider not found"),
        "expected 'bound provider not found' in error, got: {msg}"
    );
}

/// `delete(hard=true)` by name WITHOUT `cwd` must return `InvalidArgument`
/// ("cwd is required when name is provided"). The hard-delete path resolves
/// the subagent directly (not via `self.resolve()`), so its cwd check is a
/// separate code path from the soft-delete / show / hibernate paths. Covers
/// the `ok_or_else(|| InvalidArgument(...))` arm in `delete()`.
#[tokio::test]
async fn hard_delete_by_name_without_cwd_returns_invalid_argument() {
    let _guard = install_tracing();
    let m = manager().await;
    // delegate first so the subagent exists (not strictly required — the cwd
    // check happens before the lookup — but keeps the test realistic).
    let _ = m.delegate(req("harddel", "do")).await.unwrap();
    let err = m
        .delete(
            ResolveParams {
                id: None,
                name: Some("harddel".to_string()),
                cwd: None,
            },
            true, // hard=true → uses the dedicated hard-delete resolve path
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::InvalidArgument(_)));
    let msg = format!("{err}");
    assert!(
        msg.contains("cwd is required"),
        "expected 'cwd is required' in error, got: {msg}"
    );
}

// --- P1 fix: u64→i64 timeout truncation rejection --------------------------

/// `delegate()` must reject `timeout_seconds` values that exceed `i64::MAX`.
/// Without this check, `as i64` truncation wraps the value to negative,
/// corrupting the reaper SQL cutoff and causing healthy tasks to be reaped.
/// Verifies the exact value from the review finding: `9223372036854775808`
/// (= 2^63 = i64::MAX + 1) wraps to `i64::MIN` under `as i64`.
#[tokio::test]
async fn delegate_rejects_oversized_timeout_exceeding_i64_max() {
    let _guard = install_tracing();
    let m = manager().await;
    let err = m
        .delegate(DelegateRequest {
            timeout_seconds: Some(9_223_372_036_854_775_808u64), // 2^63
            ..req("oversized", "do")
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, SubagentError::InvalidArgument(_)),
        "expected InvalidArgument, got {err:?}"
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("exceeds i64::MAX"),
        "expected 'exceeds i64::MAX' in error, got: {msg}"
    );
}

/// `delegate()` must reject `timeout_seconds` values that fit in `i64` but
/// would overflow the reaper's `timeout_seconds * 1000 + REAPER_BUFFER_MS`
/// SQL arithmetic. `i64::MAX` itself is the worst case: `i64::MAX * 1000`
/// overflows `i64`.
#[tokio::test]
async fn delegate_rejects_timeout_that_overflows_reaper_arithmetic() {
    let _guard = install_tracing();
    let m = manager().await;
    let err = m
        .delegate(DelegateRequest {
            timeout_seconds: Some(i64::MAX as u64), // fits i64 but * 1000 overflows
            ..req("overflow", "do")
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, SubagentError::InvalidArgument(_)),
        "expected InvalidArgument, got {err:?}"
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("overflow"),
        "expected 'overflow' in error, got: {msg}"
    );
}

/// `delegate()` must accept the boundary value: the largest `timeout_seconds`
/// where `timeout * 1000 + REAPER_BUFFER_MS` does NOT overflow `i64`. This is
/// `(i64::MAX - 60_000) / 1000` seconds. The task should be created and
/// complete successfully.
#[tokio::test]
async fn delegate_accepts_boundary_timeout_at_max_safe_value() {
    let _guard = install_tracing();
    let m = manager().await;
    // (i64::MAX - 60_000) / 1000 — the exact boundary.
    let boundary = ((i64::MAX - 60_000i64) / 1000i64) as u64;
    let result = m
        .delegate(DelegateRequest {
            timeout_seconds: Some(boundary),
            ..req("boundary", "do")
        })
        .await
        .unwrap();
    assert_eq!(result.status, TaskStatus::Running);
    let final_status = await_task_done(&m, &result.task_id).await;
    assert_eq!(final_status, "completed");
}

/// `delegate()` must accept a normal, realistic timeout (e.g. 600s). This is
/// a sanity check that the validation doesn't over-reject legitimate values.
#[tokio::test]
async fn delegate_accepts_normal_timeout() {
    let _guard = install_tracing();
    let m = manager().await;
    let result = m
        .delegate(DelegateRequest {
            timeout_seconds: Some(600),
            ..req("normal-timeout", "do")
        })
        .await
        .unwrap();
    assert_eq!(result.status, TaskStatus::Running);
    let final_status = await_task_done(&m, &result.task_id).await;
    assert_eq!(final_status, "completed");
}

// ── reuse_policy conflict detection (P1-4) ─────────────────────────────────

#[tokio::test]
async fn reuse_policy_create_fails_when_subagent_exists() {
    let _guard = install_tracing();
    let m = manager().await;
    // First delegate creates the subagent.
    let r1 = m.delegate(req("reviewer", "first")).await.unwrap();
    let _ = await_task_done(&m, &r1.task_id).await;
    // Second delegate with reuse_policy="create" should fail.
    let err = m
        .delegate(DelegateRequest {
            reuse_policy: Some("create".into()),
            ..req("reviewer", "second")
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::InvalidArgument(_)));
    assert!(format!("{err}").contains("already exists"));
    assert!(format!("{err}").contains("--reuse-policy=reuse"));
}

#[tokio::test]
async fn reuse_policy_create_succeeds_when_subagent_does_not_exist() {
    let _guard = install_tracing();
    let m = manager().await;
    let r = m
        .delegate(DelegateRequest {
            reuse_policy: Some("create".into()),
            ..req("new-sub", "do")
        })
        .await
        .unwrap();
    assert_eq!(r.status.as_str(), "running");
    let final_status = await_task_done(&m, &r.task_id).await;
    assert_eq!(final_status, "completed");
    assert!(r.created, "created should be true for a new subagent");
}

#[tokio::test]
async fn reuse_policy_reuse_fails_when_subagent_not_found() {
    let _guard = install_tracing();
    let m = manager().await;
    let err = m
        .delegate(DelegateRequest {
            reuse_policy: Some("reuse".into()),
            ..req("nonexistent", "do")
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::InvalidArgument(_)));
    assert!(format!("{err}").contains("not found"));
    assert!(format!("{err}").contains("--reuse-policy=create"));
}

#[tokio::test]
async fn reuse_policy_reuse_succeeds_when_subagent_exists() {
    let _guard = install_tracing();
    let m = manager().await;
    // Create the subagent first.
    let r1 = m.delegate(req("worker", "first")).await.unwrap();
    let _ = await_task_done(&m, &r1.task_id).await;
    // Reuse it with reuse_policy="reuse".
    let r2 = m
        .delegate(DelegateRequest {
            reuse_policy: Some("reuse".into()),
            ..req("worker", "second")
        })
        .await
        .unwrap();
    assert_eq!(r2.subagent_id, r1.subagent_id);
    assert!(!r2.created, "created should be false when reusing");
}

#[tokio::test]
async fn reuse_policy_default_fails_on_binding_mismatch() {
    let _guard = install_tracing();
    let m = manager().await;
    // Create a subagent with the default bound provider/model.
    let r1 = m.delegate(req("mismatch-test", "first")).await.unwrap();
    let _ = await_task_done(&m, &r1.task_id).await;
    // Try to reuse with DIFFERENT bound fields (non-existent IDs).
    let err = m
        .delegate(DelegateRequest {
            bound_provider_id: Some("wrong-provider".into()),
            bound_model_id: Some("wrong-model".into()),
            ..req("mismatch-test", "second")
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::InvalidArgument(_)));
    assert!(format!("{err}").contains("differs"));
}

#[tokio::test]
async fn reuse_policy_default_succeeds_when_binding_matches() {
    let _guard = install_tracing();
    let m = manager().await;
    let r1 = m.delegate(req("match-test", "first")).await.unwrap();
    let _ = await_task_done(&m, &r1.task_id).await;
    // Reuse with the SAME bound fields → should succeed.
    let r2 = m
        .delegate(DelegateRequest {
            bound_provider_id: Some(bound_provider_id()),
            bound_model_id: Some(bound_model_id()),
            ..req("match-test", "second")
        })
        .await
        .unwrap();
    assert_eq!(r2.subagent_id, r1.subagent_id);
    assert!(!r2.created);
}

// I1 fix: --reuse-policy=reuse explicitly bypasses the binding conflict
// check, so a delegate with mismatched bound fields must SUCCEED (reusing the
// existing subagent). execute_task reads the subagent's STORED binding, so the
// mismatched request bound fields are never used for execution.
#[tokio::test]
async fn reuse_policy_reuse_succeeds_with_mismatched_bindings() {
    let _guard = install_tracing();
    let m = manager().await;
    // Create a subagent bound to the seeded test provider/model (A/X).
    let r1 = m.delegate(req("reuse-mismatch", "first")).await.unwrap();
    let _ = await_task_done(&m, &r1.task_id).await;
    // Delegate with reuse_policy="reuse" and DIFFERENT bound fields (B/Y).
    // The conflict check is bypassed; execute_task uses the stored A/X binding.
    let r2 = m
        .delegate(DelegateRequest {
            reuse_policy: Some("reuse".into()),
            bound_provider_id: Some("other-provider".into()),
            bound_model_id: Some("other-model".into()),
            ..req("reuse-mismatch", "second")
        })
        .await
        .expect("reuse policy must bypass the binding conflict check");
    assert_eq!(r2.status.as_str(), "running");
    let final_status = await_task_done(&m, &r2.task_id).await;
    assert_eq!(final_status, "completed");
    assert!(
        !r2.created,
        "created should be false when reusing an existing subagent"
    );
}

// Bug #3 fix: `--reuse-policy fail` must error when the subagent already
// exists — even if the bound fields also differ. The "already exists" check
// runs FIRST (subagent exists → fail), before the binding conflict check.
// The hint should point users to `--reuse-policy=reuse` to override.
#[tokio::test]
async fn reuse_policy_fail_errors_with_mismatched_bindings() {
    let _guard = install_tracing();
    let m = manager().await;
    // Create a subagent bound to the seeded test provider/model (A/X).
    let r1 = m.delegate(req("fail-mismatch", "first")).await.unwrap();
    let _ = await_task_done(&m, &r1.task_id).await;
    // Delegate with reuse_policy="fail" and DIFFERENT bound fields (B/Y).
    // Bug #3 fix: "fail" when subagent exists → error "already exists" (the
    // binding mismatch is secondary — the name conflict is the primary error).
    let err = m
        .delegate(DelegateRequest {
            reuse_policy: Some("fail".into()),
            bound_provider_id: Some("other-provider".into()),
            bound_model_id: Some("other-model".into()),
            ..req("fail-mismatch", "second")
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::InvalidArgument(_)));
    let msg = format!("{err}");
    assert!(
        msg.contains("already exists"),
        "expected 'already exists' in conflict error, got: {msg}"
    );
    assert!(
        msg.contains("--reuse-policy=reuse"),
        "expected hint to use --reuse-policy=reuse, got: {msg}"
    );
}

// Bug #3 fix: `--reuse-policy fail` must error even when the bound fields
// MATCH — the name conflict alone is sufficient to trigger the failure.
// This is the core regression: previously, "fail" fell into the default
// catch-all and was silently treated as create-or-reuse.
#[tokio::test]
async fn reuse_policy_fail_errors_when_subagent_exists_with_matching_bindings() {
    let _guard = install_tracing();
    let m = manager().await;
    // Create a subagent.
    let r1 = m.delegate(req("fail-match", "first")).await.unwrap();
    let _ = await_task_done(&m, &r1.task_id).await;
    // Delegate with reuse_policy="fail" and the SAME bound fields.
    // Must error "already exists" — not silently reuse the subagent.
    let err = m
        .delegate(DelegateRequest {
            reuse_policy: Some("fail".into()),
            ..req("fail-match", "second")
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::InvalidArgument(_)));
    let msg = format!("{err}");
    assert!(
        msg.contains("already exists"),
        "expected 'already exists' in error, got: {msg}"
    );
    assert!(
        msg.contains("--reuse-policy=reuse"),
        "expected hint to use --reuse-policy=reuse, got: {msg}"
    );
}

#[tokio::test]
async fn delegate_result_created_is_true_for_new_subagent() {
    let _guard = install_tracing();
    let m = manager().await;
    let r = m.delegate(req("fresh-sub", "do")).await.unwrap();
    assert!(r.created, "created should be true for a new subagent");
}

#[tokio::test]
async fn delegate_result_created_is_false_for_reused_subagent() {
    let _guard = install_tracing();
    let m = manager().await;
    let r1 = m.delegate(req("reused-sub", "first")).await.unwrap();
    let _ = await_task_done(&m, &r1.task_id).await;
    let r2 = m.delegate(req("reused-sub", "second")).await.unwrap();
    assert!(!r2.created, "created should be false when reusing");
}

// ── cancel_task (P1-5) ─────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_task_cancels_queued_task() {
    let _guard = install_tracing();
    let m = manager().await;
    // Delegate a task to create a task row.
    let r = m.delegate(req("cancel-queued", "do")).await.unwrap();
    // Wait for the background task to complete — cancel_task reads the
    // task's status from the DB and must see "completed" (not "running").
    let _ = await_task_done(&m, &r.task_id).await;
    // The task should already be completed (mock executor is synchronous).
    // Cancel should return cancelled=false since it's terminal.
    let outcome = m.cancel_task(&r.task_id, None).await.unwrap();
    assert!(
        !outcome.cancelled,
        "completed task should not be cancellable"
    );
    assert_eq!(outcome.previous_status, "completed");
}

#[tokio::test]
async fn cancel_task_returns_not_found_for_missing_task() {
    let _guard = install_tracing();
    let m = manager().await;
    let err = m
        .cancel_task("nonexistent-task-id", None)
        .await
        .unwrap_err();
    assert!(matches!(err, SubagentError::NotFound(_)));
    assert!(format!("{err}").contains("task"));
}

#[tokio::test]
async fn cancel_task_cancels_running_task() {
    let _guard = install_tracing();
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    ));
    let r = m.delegate(req("cancel-running", "do")).await.unwrap();
    // Wait for the background task to complete before manually flipping
    // the status to "running" — otherwise the background write could
    // overwrite our manual status change.
    let _ = await_task_done(&m, &r.task_id).await;
    // Manually set the task to running via DB.
    {
        let g = db.lock().unwrap();
        g.subagent_set_task_status(&r.task_id, "running", None, None)
            .unwrap();
    }
    let outcome = m
        .cancel_task(&r.task_id, Some("user-requested"))
        .await
        .unwrap();
    assert!(outcome.cancelled);
    assert_eq!(outcome.previous_status, "running");
    assert_eq!(outcome.new_status, "cancelled");
}

#[tokio::test]
async fn cancel_task_skips_already_terminal_task() {
    let _guard = install_tracing();
    let m = manager().await;
    let r = m.delegate(req("cancel-terminal", "do")).await.unwrap();
    // Wait for the background task to reach terminal status before
    // asserting cancel is a no-op.
    let _ = await_task_done(&m, &r.task_id).await;
    // Task is already completed. Cancel should be a no-op.
    let outcome = m.cancel_task(&r.task_id, None).await.unwrap();
    assert!(!outcome.cancelled);
    assert_eq!(outcome.previous_status, "completed");
    assert_eq!(outcome.new_status, "completed");
}

// ── cooperative cancel of in-flight executor (P1-5) ────────────────────────

#[tokio::test]
async fn cancel_task_aborts_in_flight_executor() {
    let _guard = install_tracing();
    let db = test_db();
    let executor = BlockingExecutor::new();
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(executor) as std::sync::Arc<dyn TaskExecutor>,
    ));

    // Bug #1/#2 fix: delegate() returns Running immediately and spawns
    // execute_and_persist in the background. The BlockingExecutor hangs on
    // a pending future inside execute_task's `tokio::select!` — the
    // cooperative cancel signal is the only way out.
    let r = m
        .delegate(req("cancel-in-flight", "do slow thing"))
        .await
        .unwrap();
    assert_eq!(r.status.as_str(), "running");
    let task_id = r.task_id.clone();

    // Give the background execute_task time to reach the BlockingExecutor
    // (which hangs on pending future). The cancel signal is already
    // registered at the start of execute_task, so 200ms is ample.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Verify the task is running (background executor is blocked).
    {
        let g = db.lock().unwrap();
        let task = g.subagent_get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, "running", "task should be running");
    }

    // Cancel the in-flight task — this sends the cooperative cancel signal.
    let outcome = m.cancel_task(&task_id, Some("test abort")).await.unwrap();
    assert!(outcome.cancelled);
    assert_eq!(outcome.previous_status, "running");
    assert_eq!(outcome.new_status, "cancelled");

    // The background execute_task's tokio::select! drops the BlockingExecutor
    // future, returns Ok(Cancelled). execute_and_persist sees Ok → calls hook
    // (None in test) → done. Status was already flipped to "cancelled" by
    // cancel_task. await_task_done confirms the terminal state.
    let final_status = await_task_done(&m, &task_id).await;
    assert_eq!(final_status, "cancelled");
}

/// Executor that records `cancel()` calls. Used to verify that
/// `cancel_task` invokes the execution-protocol cancel (sends
/// `session.cancel` RPC to the sidecar in production).
struct CancelTrackingExecutor {
    cancel_calls: CancelCallLog,
}

type CancelCallLog = std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>;

impl CancelTrackingExecutor {
    fn new() -> (Self, CancelCallLog) {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Self {
                cancel_calls: std::sync::Arc::clone(&calls),
            },
            calls,
        )
    }
}

#[async_trait::async_trait]
impl TaskExecutor for CancelTrackingExecutor {
    async fn execute(&self, _input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        // Block forever — the only exit is the cancel signal dropping the future.
        std::future::pending::<()>().await;
        unreachable!();
    }

    async fn cancel(&self, subagent_id: &str, provider_id: &str) -> anyhow::Result<()> {
        self.cancel_calls
            .lock()
            .unwrap()
            .push((subagent_id.to_string(), provider_id.to_string()));
        Ok(())
    }
}

/// Verify that `cancel_task` calls `executor.cancel()` with the correct
/// `subagent_id` and `provider_id` — the execution-protocol cancel that
/// sends `session.cancel` RPC to the sidecar (stopping token generation
/// at the LLM provider).
#[tokio::test]
async fn cancel_task_invokes_executor_cancel() {
    let _guard = install_tracing();
    let db = test_db();
    let (executor, cancel_calls) = CancelTrackingExecutor::new();
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(executor) as std::sync::Arc<dyn TaskExecutor>,
    ));
    // Spawn delegate — it will hang on the BlockingExecutor (pending forever).
    let m2 = std::sync::Arc::clone(&m);
    let delegate_task =
        tokio::spawn(async move { m2.delegate(req("cancel-tracking", "do slow thing")).await });
    // Give delegate time to insert the task and reach the executor.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    // Find the task id and the subagent's UUID (the value executor.cancel()
    // should receive as `subagent_id`).
    let (task_id, expected_subagent_id) = {
        let g = db.lock().unwrap();
        let subagents = g.subagent_list_filtered(None, None, false).unwrap();
        let sa = subagents
            .iter()
            .find(|s| s.name == "cancel-tracking")
            .expect("subagent should exist");
        let tasks = g.subagent_list_tasks(&sa.id, 10).unwrap();
        assert_eq!(tasks[0].status, "running");
        (tasks[0].id.clone(), sa.id.clone())
    };
    // Cancel — this should fire executor.cancel() in a spawned task.
    let outcome = m
        .cancel_task(&task_id, Some("test executor cancel"))
        .await
        .unwrap();
    assert!(outcome.cancelled);
    // Wait for the spawned cancel task to complete (fire-and-forget with
    // 5s timeout — should finish instantly since CancelTrackingExecutor
    // is in-process).
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    // Verify executor.cancel() was called with the correct args.
    let calls = cancel_calls.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        1,
        "executor.cancel() should be called exactly once"
    );
    assert_eq!(
        calls[0].0, expected_subagent_id,
        "cancel subagent_id should match the task's subagent UUID"
    );
    assert_eq!(
        calls[0].1,
        bound_provider_id(),
        "cancel provider_id should match the bound provider"
    );
    // Clean up: drop the delegate task (it will be cancelled by the signal).
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), delegate_task).await;
}

/// `cancel_task` flips a queued task to "cancelled" in the DB. No cancel
/// signal is registered for queued tasks (the executor isn't running), so
/// `cancel_task` just flips the DB status — the `if let Some(tx)` signal-send
/// branch is a no-op. The dispatcher's `pick_oldest_queued_task` filters by
/// `status='queued'`, so a cancelled task is never picked.
#[tokio::test]
async fn cancel_task_flips_queued_task_to_cancelled() {
    let _guard = install_tracing();
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    ));
    let r = m.delegate(req("cancel-queued", "do")).await.unwrap();
    // Wait for the background task to complete before simulating a
    // queued task — the status flip below must not race with the
    // background executor's terminal write.
    let _ = await_task_done(&m, &r.task_id).await;
    // MockTaskExecutor completes synchronously, so the task is now
    // "completed". Simulate a queued task (as if the dispatcher hasn't
    // picked it up yet) by flipping the status back to "queued".
    {
        let g = db.lock().unwrap();
        g.subagent_set_task_status(&r.task_id, "queued", None, None)
            .unwrap();
    }
    let outcome = m.cancel_task(&r.task_id, None).await.unwrap();
    assert!(outcome.cancelled);
    assert_eq!(outcome.previous_status, "queued");
    assert_eq!(outcome.new_status, "cancelled");
    let task = {
        let g = db.lock().unwrap();
        g.subagent_get_task(&r.task_id).unwrap().unwrap()
    };
    assert_eq!(task.status, "cancelled");
}

/// Dispatcher-level test: a cancelled queued task must never reach the
/// executor. Spawns the real dispatcher with a `CountingExecutor` and verifies
/// `execute()` call count stays 0 after multiple poll cycles. This exercises
/// the `pick_oldest_queued_task` SQL filter (status='cancelled' is never
/// picked). The dispatcher's pre-execute re-check and `execute_task`'s
/// pre-execute cancel check are defense-in-depth for the race between pick
/// and execute — they are not directly exercised by this test because the
/// task is cancelled before the dispatcher starts.
#[tokio::test]
async fn dispatcher_skips_cancelled_queued_task() {
    let _guard = install_tracing();
    let db = test_db();
    let executor = std::sync::Arc::new(CountingExecutor::new());
    let manager = std::sync::Arc::new(SubagentManager::new(
        db.clone(),
        SubagentSettings::default(),
        "pi",
        executor.clone() as std::sync::Arc<dyn TaskExecutor>,
    ));
    // Seed a subagent + queued task directly (bypassing delegate() so the
    // task starts as "queued", not "completed").
    {
        use busytok_store::repository::{SubagentLogicalSubagentRow, SubagentTaskRow};
        let g = db.lock().unwrap();
        g.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test(
            "sub-cancel-skip",
            "cancel-skip-sub",
        ))
        .unwrap();
        g.subagent_insert_task(&SubagentTaskRow {
            id: "task-cancel-skip".into(),
            subagent_id: "sub-cancel-skip".into(),
            source_harness: None,
            source_session_id: None,
            intent: None,
            profile: "pi/search-cheap".into(),
            prompt: Some("queued then cancelled".into()),
            prompt_artifact_ref: None,
            output_schema_name: None,
            output_schema_version: 1,
            status: "queued".into(),
            result_summary: None,
            result_json: None,
            error: None,
            created_at_ms: busytok_domain::now_ms(),
            started_at_ms: None,
            completed_at_ms: None,
            timeout_seconds: None,
            model_override: None,
            error_kind: None,
            queue_reason: None,
        })
        .unwrap();
    }
    // Cancel the queued task BEFORE starting the dispatcher.
    let outcome = manager
        .cancel_task("task-cancel-skip", Some("test pre-execute skip"))
        .await
        .unwrap();
    assert!(outcome.cancelled);
    assert_eq!(outcome.previous_status, "queued");
    // Start the dispatcher. It polls every 200ms; over 600ms (3 ticks) the
    // cancelled task must never be picked or executed.
    let (tx, rx) = tokio::sync::watch::channel(false);
    let handle = manager.clone().spawn_task_dispatcher(rx);
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    tx.send(true).unwrap();
    let _ = handle.await;
    assert_eq!(
        executor.call_count(),
        0,
        "executor must NOT be called for a cancelled task"
    );
    let task = {
        let g = db.lock().unwrap();
        g.subagent_get_task("task-cancel-skip").unwrap().unwrap()
    };
    assert_eq!(task.status, "cancelled");
}

// ── Bug #1/#2: delegate() must return Running immediately ───────────────────

/// Bug #1/#2 fix: `delegate()` must return `Running` immediately without
/// blocking on the executor. Previously, `delegate()` awaited `execute_task()`
/// synchronously — a long-running sidecar call would block the CLI until
/// the task reached a terminal state.
///
/// This test uses `BlockingExecutor` (which never completes on its own)
/// and verifies that `delegate()` returns within 2 seconds. If the bug
/// regressed, this test would hang indefinitely.
#[tokio::test]
async fn delegate_returns_running_without_blocking_on_executor() {
    let _guard = install_tracing();
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(BlockingExecutor::new()) as std::sync::Arc<dyn TaskExecutor>,
    ));

    let start = std::time::Instant::now();
    let r = m
        .delegate(req("nonblock-check", "do slow thing"))
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(r.status.as_str(), "running");
    assert!(
        elapsed.as_secs() < 2,
        "delegate() must return immediately, took {elapsed:?}"
    );

    // Clean up: cancel the blocking task so the test can exit cleanly.
    let _ = m.cancel_task(&r.task_id, Some("test cleanup")).await;
    let _ = await_task_done(&m, &r.task_id).await;
}

// ── Bug #4: subagent show reflects hot status during running task ──────────

/// Bug #4 fix: `subagent show` must reflect `status: hot` and
/// `last_active_at_ms: <now>` while a task is running — not stay `cold`
/// until the task completes.
///
/// Previously, `status` and `last_active_at_ms` were only written on task
/// completion (via `commit_hot_binding_and_status` which uses
/// `COALESCE(last_active_at_ms, ?1)` — only fills NULL, never refreshes).
/// The fix overlays runtime status at read-time in `show()` / `list()` via
/// `overlay_runtime_status()` — a display-time computation that checks
/// `has_running_task()` and overrides the returned DTO's `status` to `Hot`.
/// The DB row is NOT modified, preserving the §3.3 invariant.
#[tokio::test]
async fn show_reflects_hot_status_while_task_running() {
    let _guard = install_tracing();
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(BlockingExecutor::new()) as std::sync::Arc<dyn TaskExecutor>,
    ));

    // Delegate a task — it will hang on BlockingExecutor.
    let r = m.delegate(req("hot-check", "do slow thing")).await.unwrap();
    assert_eq!(r.status.as_str(), "running");

    // Give the background execute_task time to register the cancel signal
    // and reach the BlockingExecutor.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // While the task is running, `show` must reflect hot status.
    let detail = m
        .show(ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(
        detail.status.as_str(),
        "hot",
        "subagent show should reflect 'hot' status while task is running"
    );
    assert!(
        detail.last_active_at_ms.is_some(),
        "last_active_at_ms should be set while task is running"
    );
    let now = busytok_domain::now_ms();
    let last_active = detail.last_active_at_ms.unwrap();
    // The timestamp should be recent (within the last 5 seconds).
    assert!(
        now - last_active < 5_000,
        "last_active_at_ms should be recent: now={now}, last_active={last_active}"
    );

    // Clean up.
    let _ = m.cancel_task(&r.task_id, Some("test cleanup")).await;
    let _ = await_task_done(&m, &r.task_id).await;
}

// ── Bug #4 invariant: failed task does not leave status='hot' in DB ────────

/// Bug #4 invariant test: after a task fails, the logical subagent's
/// persistent `status` must NOT be `hot` — the spec §3.3 invariant
/// ("status='hot' iff is_hot=1 binding exists") must hold. The read-time
/// overlay in `show()` only affects the returned DTO, not the DB row.
///
/// This test was prompted by code review feedback: the previous fix
/// (persisting `status='hot'` at task start via `touch_subagent_active`)
/// would leave `hot` in the DB after a failure, with no hot binding to
/// back it — breaking the invariant.
#[tokio::test]
async fn failed_task_does_not_leave_hot_status_in_db() {
    let _guard = install_tracing();
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(BoomExecutor) as std::sync::Arc<dyn TaskExecutor>,
    ));

    // Delegate — BoomExecutor will fail. `show()` returns `running` overlay,
    // but the DB row must not be persistently `hot`.
    let r = m.delegate(req("invariant-check", "do")).await.unwrap();
    assert_eq!(r.status.as_str(), "running");

    // While running, `show()` should report `hot` (read-time overlay).
    let detail = m
        .show(ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        detail.status.as_str(),
        "hot",
        "show() should overlay 'hot' while task is running"
    );

    // Wait for the task to fail.
    let final_status = await_task_done(&m, &r.task_id).await;
    assert_eq!(final_status, "failed");

    // After failure, `show()` must NOT report `hot` — no running task, no
    // hot binding. The persistent invariant holds.
    let detail = m
        .show(ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_ne!(
        detail.status.as_str(),
        "hot",
        "failed task must not leave 'hot' status — invariant: status='hot' iff is_hot=1 binding exists"
    );
}

// ── Bug #4: list() overlay consistency with show() ────────────────────────

/// Bug #4 fix: `list()` must also overlay runtime status — a subagent with
/// a running task should appear as `hot` in the list, consistent with
/// `show()`. This closes a coverage gap identified in review: only `show()`
/// was tested, not `list()`.
#[tokio::test]
async fn list_overlays_hot_status_for_running_task() {
    let _guard = install_tracing();
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(BlockingExecutor::new()) as std::sync::Arc<dyn TaskExecutor>,
    ));

    // Delegate a task — it will hang on BlockingExecutor.
    let r = m
        .delegate(req("list-overlay-check", "do slow thing"))
        .await
        .unwrap();
    assert_eq!(r.status.as_str(), "running");

    // Give the background execute_task time to reach the BlockingExecutor.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // `list()` must show the subagent as `hot` (read-time overlay).
    let list = m.list(None, None, false).await.unwrap();
    let found = list.iter().find(|s| s.id == r.subagent_id);
    assert!(found.is_some(), "subagent should appear in list()");
    let found = found.unwrap();
    assert_eq!(
        found.status.as_str(),
        "hot",
        "list() should overlay 'hot' for subagent with running task"
    );
    assert!(
        found.last_active_at_ms.is_some(),
        "list() should set last_active_at_ms for running subagent"
    );

    // Clean up.
    let _ = m.cancel_task(&r.task_id, Some("test cleanup")).await;
    let _ = await_task_done(&m, &r.task_id).await;
}

// ── Bug #4: cancelled task does NOT trigger overlay ──────────────────────

/// Bug #4 fix: after a task is cancelled, the overlay must NOT fire —
/// `has_running_task` returns false for cancelled tasks. The subagent's
/// status should reflect the DB state (warm/cold), not `hot`.
#[tokio::test]
async fn cancelled_task_does_not_trigger_hot_overlay() {
    let _guard = install_tracing();
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(BlockingExecutor::new()) as std::sync::Arc<dyn TaskExecutor>,
    ));

    // Delegate a task — it will hang on BlockingExecutor.
    let r = m
        .delegate(req("cancel-overlay-check", "do slow thing"))
        .await
        .unwrap();
    assert_eq!(r.status.as_str(), "running");

    // Give the background execute_task time to reach the BlockingExecutor.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // While running, overlay should show `hot`.
    let detail = m
        .show(ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(detail.status.as_str(), "hot");

    // Cancel the task.
    let _ = m.cancel_task(&r.task_id, Some("test cancel")).await;
    let _ = await_task_done(&m, &r.task_id).await;

    // After cancellation, overlay should NOT fire — task is `cancelled`,
    // not `running`. Status should be `cold` (no memory written).
    let detail = m
        .show(ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_ne!(
        detail.status.as_str(),
        "hot",
        "cancelled task must not trigger hot overlay"
    );
}

// ── Bug: list --status hot misses running subagents ────────────────────────

/// Bug: `list(status=Some(Hot))` filtered at SQL level BEFORE the runtime
/// overlay, so subagents with running tasks (but DB status cold/warm) were
/// excluded. The fix queries without the status filter, applies the overlay,
/// then filters in Rust.
#[tokio::test]
async fn list_status_hot_includes_running_subagents() {
    let _guard = install_tracing();
    let db = test_db();
    let m = std::sync::Arc::new(SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(BlockingExecutor::new()) as std::sync::Arc<dyn TaskExecutor>,
    ));

    // Delegate a long-running task — it will hang on BlockingExecutor.
    let r = m
        .delegate(req("hot-filter-check", "do slow thing"))
        .await
        .unwrap();
    assert_eq!(r.status.as_str(), "running");

    // Give the background execute_task time to reach the BlockingExecutor.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // `show()` correctly returns `hot` (overlay applied).
    let detail = m
        .show(ResolveParams {
            id: Some(r.subagent_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(detail.status.as_str(), "hot", "show() should overlay hot");

    // Root-cause lock: verify the PERSISTENT DB row is NOT 'hot'. This
    // proves `show()` returned 'hot' purely from the read-time overlay,
    // not from a stale persistent write. If someone reintroduces the
    // `touch_subagent_active()` pattern (writing status='hot' at task
    // start), this assertion fails — protecting the §3.3 invariant.
    let persisted_status = {
        let g = db.lock().unwrap();
        g.subagent_list_filtered(None, None, false)
            .unwrap()
            .iter()
            .find(|s| s.id == r.subagent_id)
            .map(|s| s.status.clone())
            .expect("subagent row must exist in DB")
    };
    assert_ne!(
        persisted_status, "hot",
        "persisted DB status must NOT be 'hot' — runtime overlay must be \
         display-time only. Got: {persisted_status}. If this fails, someone \
         reintroduced persistent hot-status writes at task start."
    );

    // `list(status=Some(Hot))` must include this subagent — the SQL filter
    // must NOT exclude it before the overlay runs.
    let hot_list = m
        .list(Some(SubagentStatus::Hot), None, false)
        .await
        .unwrap();
    let found = hot_list.iter().find(|s| s.id == r.subagent_id);
    assert!(
        found.is_some(),
        "list --status hot must include subagent with running task, got {} subagents",
        hot_list.len()
    );

    // `list(status=Some(Cold))` must NOT include this subagent — it's hot
    // (has a running task), so it should be excluded from the cold filter.
    let cold_list = m
        .list(Some(SubagentStatus::Cold), None, false)
        .await
        .unwrap();
    let found_cold = cold_list.iter().find(|s| s.id == r.subagent_id);
    assert!(
        found_cold.is_none(),
        "list --status cold must NOT include subagent with running task"
    );

    // Clean up.
    let _ = m.cancel_task(&r.task_id, Some("test cleanup")).await;
    let _ = await_task_done(&m, &r.task_id).await;
}
