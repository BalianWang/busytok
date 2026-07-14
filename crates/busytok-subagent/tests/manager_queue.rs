#![allow(clippy::unwrap_used)]
//! Task 7 §8.3 step 2 "queue only": background dispatcher end-to-end tests.
//!
//! These tests exercise the race-free insert-as-queued path in `delegate()`
//! (Plan 6 Task 3) AND the background `spawn_task_dispatcher` worker (Task 7)
//! that polls for queued tasks and executes them when the pressure gate
//! clears. The dispatcher uses `tokio::sync::watch` for deterministic
//! shutdown (Finding 3 fix): tests send `true` on the channel + `await` the
//! `JoinHandle` so no background task leaks between tests.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use busytok_config::SubagentSettings;
use busytok_subagent::mock_executor::{ExecutorInput, ExecutorOutput, TaskExecutor};
use busytok_subagent::models::{DelegateRequest, QueueReason, TaskStatus};
use busytok_subagent::pressure::{PressureAction, PressureGate};
use busytok_subagent::SubagentManager;

const TEST_PROVIDER_ID: &str = "test-prov";
const TEST_MODEL_NAME: &str = "test-model";

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

/// Minimal mock executor that returns a fixed `Completed` result. Used by
/// both the queued-when-paused test (verifies delegate returns early without
/// invoking the executor) and the dispatcher test (verifies the dispatcher
/// actually invokes the executor after the gate clears).
struct RecordingExecutor;

#[async_trait]
impl TaskExecutor for RecordingExecutor {
    async fn execute(&self, _input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        Ok(ExecutorOutput {
            adapter_session_id: None,
            session_reused: false,
            status: TaskStatus::Completed,
            summary: "done".into(),
            usage: Default::default(),
            memory_update: Default::default(),
            error_kind: None,
        })
    }
}

/// Create an in-memory database seeded with a test provider + model so
/// `delegate()` can create subagents with valid bound fields.
fn test_db() -> Arc<Mutex<busytok_store::Database>> {
    let db = Arc::new(Mutex::new(
        busytok_store::Database::open_in_memory().unwrap(),
    ));
    seed_test_provider_model(&db.lock().unwrap());
    db
}

fn seed_test_provider_model(db: &busytok_store::Database) {
    let now = busytok_domain::now_ms();
    db.conn().execute(
        "INSERT INTO providers (id, name, provider_kind, base_url, enabled, api_key, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        rusqlite::params![
            TEST_PROVIDER_ID,
            "Test Provider",
            serde_json::to_string(&busytok_domain::ProviderKind::OpenAiCompatible).unwrap(),
            "https://api.test.com",
            1i64,
            "sk-test",
            now,
        ],
    ).unwrap();
    db.conn().execute(
        "INSERT INTO models (id, provider_id, model_id, enabled, created_at_ms, updated_at_ms, display_name, reasoning, context_window, max_tokens)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, NULL, 0, 128000, 16384)",
        rusqlite::params![
            "test-model-row",
            TEST_PROVIDER_ID,
            TEST_MODEL_NAME,
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
        timeout_seconds: Some(5),
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: Some(TEST_PROVIDER_ID.to_string()),
        bound_model_id: Some(TEST_MODEL_NAME.to_string()),
        reuse_policy: None,
    }
}

#[tokio::test]
async fn delegate_returns_queued_when_gate_paused() {
    let db = test_db();
    let settings = SubagentSettings::default();
    let executor = Arc::new(RecordingExecutor) as Arc<dyn TaskExecutor>;
    let gate = Arc::new(PressureGate::new());
    gate.set_action(PressureAction::PauseNewTasks);
    let manager = std::sync::Arc::new(SubagentManager::with_pressure_gate(
        Arc::clone(&db),
        settings,
        "mock",
        executor,
        Some(Arc::clone(&gate)),
    ));
    let result = manager.delegate(req("test", "hello")).await.unwrap();
    assert_eq!(
        result.status,
        TaskStatus::Queued,
        "delegate must return Queued when gate is paused"
    );
    assert_eq!(
        result.queue_reason,
        Some(QueueReason::PressureGatePaused),
        "queued-via-pressure-gate task must carry queue_reason = PressureGatePaused, got: {:?}",
        result.queue_reason
    );
    assert!(result.summary.is_none(), "queued task has no summary yet");

    // Verify task row is in "queued" status in DB.
    let db = db.lock().unwrap();
    let tasks = db.subagent_list_tasks(&result.subagent_id, 10).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, "queued");
    assert!(
        tasks[0].started_at_ms.is_none(),
        "queued task must not have started_at_ms set"
    );
    // Task 7 Round 3 Finding 1 fix: persisted execution params are on the row.
    assert_eq!(tasks[0].timeout_seconds, Some(5));
    assert!(tasks[0].model_override.is_none());
}

#[tokio::test]
async fn dispatcher_executes_queued_task_when_gate_clears() {
    let db = test_db();
    let settings = SubagentSettings::default();
    let executor = Arc::new(RecordingExecutor) as Arc<dyn TaskExecutor>;
    let gate = Arc::new(PressureGate::new());
    gate.set_action(PressureAction::PauseNewTasks);
    let manager = Arc::new(SubagentManager::with_pressure_gate(
        Arc::clone(&db),
        settings,
        "mock",
        executor,
        Some(Arc::clone(&gate)),
    ));

    // Round 3 Finding 4 fix: spawn_task_dispatcher takes a watch::Receiver<bool>
    // for shutdown signaling (JoinHandle drop = detach, NOT abort).
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = manager.spawn_task_dispatcher(shutdown_rx);

    // Queue a task while the gate is paused — dispatcher must NOT pick it up.
    let result = manager.delegate(req("test", "hello")).await.unwrap();
    assert_eq!(result.status, TaskStatus::Queued);

    // Confirm the task stays queued while the gate is paused (poll briefly).
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    {
        let db = db.lock().unwrap();
        let tasks = db.subagent_list_tasks(&result.subagent_id, 10).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].status, "queued",
            "task must remain queued while gate is paused"
        );
    }

    // Clear the gate — dispatcher should pick up + execute.
    gate.set_action(PressureAction::Resume);

    // Poll for up to 5s for the task to complete.
    let mut completed = false;
    for _ in 0..50 {
        let status_now = {
            let db = db.lock().unwrap();
            let tasks = db.subagent_list_tasks(&result.subagent_id, 10).unwrap();
            tasks.iter().any(|t| t.status == "completed")
        };
        if status_now {
            completed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(completed, "queued task must be executed after gate clears");

    // Deterministic shutdown: send true + await handle.
    let _ = shutdown_tx.send(true);
    let _ = handle.await;
}

/// Spec §6.4 line 737 (per-subagent FIFO, Finding 2 fix): the dispatcher
/// must NOT pick a queued task for a subagent that already has a running
/// task. This test queues two tasks for the SAME subagent while the gate is
/// paused, then clears the gate. The dispatcher should execute them
/// sequentially (one at a time), never concurrently.
#[tokio::test]
async fn dispatcher_serializes_tasks_per_subagent_fifo() {
    let db = test_db();
    let settings = SubagentSettings::default();
    let executor = Arc::new(RecordingExecutor) as Arc<dyn TaskExecutor>;
    let gate = Arc::new(PressureGate::new());
    gate.set_action(PressureAction::PauseNewTasks);
    let manager = Arc::new(SubagentManager::with_pressure_gate(
        Arc::clone(&db),
        settings,
        "mock",
        executor,
        Some(Arc::clone(&gate)),
    ));

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = manager.spawn_task_dispatcher(shutdown_rx);

    // Queue two tasks for the SAME subagent (by reusing the name).
    let r1 = manager.delegate(req("fifo-sub", "first")).await.unwrap();
    assert_eq!(r1.status, TaskStatus::Queued);
    let r2 = manager.delegate(req("fifo-sub", "second")).await.unwrap();
    assert_eq!(r2.status, TaskStatus::Queued);
    assert_eq!(
        r1.subagent_id, r2.subagent_id,
        "both tasks must target the same subagent"
    );

    // Clear the gate — dispatcher should pick the OLDEST queued task first
    // (FIFO by created_at_ms), execute it, then pick the second.
    gate.set_action(PressureAction::Resume);

    // Poll for up to 5s for BOTH tasks to complete.
    let mut both_completed = false;
    for _ in 0..50 {
        let completed_count = {
            let db = db.lock().unwrap();
            let tasks = db.subagent_list_tasks(&r1.subagent_id, 10).unwrap();
            tasks.iter().filter(|t| t.status == "completed").count()
        };
        if completed_count == 2 {
            both_completed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        both_completed,
        "both queued tasks must be executed after gate clears (per-subagent FIFO)"
    );

    // Deterministic shutdown.
    let _ = shutdown_tx.send(true);
    let _ = handle.await;
}

/// `pick_oldest_queued_task` returns `None` when there are no queued tasks.
#[test]
fn pick_oldest_queued_task_returns_none_when_empty() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    let picked = db.subagent_pick_oldest_queued_task().unwrap();
    assert!(picked.is_none(), "no queued tasks → None");
}

/// `pick_oldest_queued_task` returns `None` when the only queued task belongs
/// to a subagent that already has a running task (per-subagent FIFO guard,
/// Finding 2 fix).
#[test]
fn pick_oldest_queued_task_skips_subagent_with_running_task() {
    use busytok_store::repository::{SubagentLogicalSubagentRow, SubagentTaskRow};
    let db = busytok_store::Database::open_in_memory().unwrap();
    let sub_id = "sub-fifo-guard";
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test(sub_id, "fifo-guard"))
        .unwrap();
    // Insert one RUNNING task and one QUEUED task for the same subagent.
    let now = busytok_domain::now_ms();
    db.subagent_insert_task(&SubagentTaskRow {
        id: "running-task".into(),
        subagent_id: sub_id.into(),
        source_harness: None,
        source_session_id: None,
        intent: None,
        profile: "pi/search-cheap".into(),
        prompt: Some("running".into()),
        prompt_artifact_ref: None,
        output_schema_name: None,
        output_schema_version: 1,
        status: "running".into(),
        result_summary: None,
        result_json: None,
        error: None,
        created_at_ms: now,
        started_at_ms: Some(now),
        completed_at_ms: None,
        timeout_seconds: None,
        model_override: None,
        error_kind: None,
        queue_reason: None,
    })
    .unwrap();
    db.subagent_insert_task(&SubagentTaskRow {
        id: "queued-task".into(),
        subagent_id: sub_id.into(),
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
        created_at_ms: now + 1,
        started_at_ms: None,
        completed_at_ms: None,
        timeout_seconds: None,
        model_override: None,
        error_kind: None,
        queue_reason: None,
    })
    .unwrap();
    // Per-subagent FIFO guard: the queued task must NOT be picked because
    // the subagent already has a running task.
    let picked = db.subagent_pick_oldest_queued_task().unwrap();
    assert!(
        picked.is_none(),
        "queued task for subagent with running task must NOT be picked"
    );
}

/// `pick_oldest_queued_task` atomically picks + flips to "running" (Round 3
/// Finding 1 fix): after a successful pick, the row's status is "running"
/// and `started_at_ms` is set.
#[test]
fn pick_oldest_queued_task_flips_status_to_running() {
    use busytok_store::repository::{SubagentLogicalSubagentRow, SubagentTaskRow};
    let db = busytok_store::Database::open_in_memory().unwrap();
    let sub_id = "sub-pick-flip";
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test(sub_id, "pick-flip"))
        .unwrap();
    db.subagent_insert_task(&SubagentTaskRow::for_test(
        "task-pick-flip",
        sub_id,
        "pi/search-cheap",
        "go",
    ))
    .unwrap();

    let picked = db.subagent_pick_oldest_queued_task().unwrap();
    let picked = picked.expect("queued task must be picked");
    assert_eq!(picked.id, "task-pick-flip");
    assert_eq!(picked.status, "running", "pick must flip status to running");
    assert!(
        picked.started_at_ms.is_some(),
        "pick must set started_at_ms"
    );

    // Second pick must return None — the task is now "running", not "queued".
    let picked_again = db.subagent_pick_oldest_queued_task().unwrap();
    assert!(
        picked_again.is_none(),
        "task already flipped to running must NOT be picked again"
    );
}

// ---------------------------------------------------------------------------
// P1-1 fix (spec §6.4): per-subagent serialization in `delegate()`.
// ---------------------------------------------------------------------------

/// Executor that sleeps for a fixed duration to keep the task in "running"
/// state while a second `delegate()` is called concurrently.
struct DelayingExecutor(std::time::Duration, Arc<std::sync::atomic::AtomicUsize>);

#[async_trait]
impl TaskExecutor for DelayingExecutor {
    async fn execute(&self, _input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(self.0).await;
        Ok(ExecutorOutput {
            adapter_session_id: None,
            session_reused: false,
            status: TaskStatus::Completed,
            summary: "done".into(),
            usage: Default::default(),
            memory_update: Default::default(),
            error_kind: None,
        })
    }
}

/// Spec §6.4 line 737: "Same logical subagent: tasks are serialized (FIFO
/// queue per subagent)." When a subagent already has a running task, a
/// second `delegate()` must insert as `'queued'` (not run concurrently).
#[tokio::test]
async fn delegate_queues_when_subagent_already_running() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let db = test_db();
    let settings = SubagentSettings::default();
    let call_count = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(DelayingExecutor(
        std::time::Duration::from_millis(500),
        Arc::clone(&call_count),
    )) as Arc<dyn TaskExecutor>;
    let manager = std::sync::Arc::new(SubagentManager::with_pressure_gate(
        Arc::clone(&db),
        settings,
        "mock",
        executor,
        None,
    ));

    // Run two delegates concurrently for the SAME subagent.
    // r1 inserts as "running" + starts executing (sleeps 500ms).
    // r2 sees the running task → inserts as "queued" → returns Queued.
    let (r1, r2) = tokio::join!(
        manager.delegate(req("busy-sub", "first")),
        manager.delegate(req("busy-sub", "second")),
    );
    let r1 = r1.unwrap();
    let r2 = r2.unwrap();

    assert_eq!(r1.status, TaskStatus::Running, "first task starts running");
    assert_eq!(
        r2.status,
        TaskStatus::Queued,
        "second task must be queued — subagent was busy (spec §6.4)"
    );
    assert_eq!(
        r2.queue_reason,
        Some(QueueReason::SubagentBusy),
        "queued-via-subagent-busy task must carry queue_reason = SubagentBusy, got: {:?}",
        r2.queue_reason
    );
    assert_eq!(
        r1.subagent_id, r2.subagent_id,
        "both tasks target the same subagent"
    );

    // Wait for the first task to complete (now async via tokio::spawn).
    await_task_done(&manager, &r1.task_id).await;

    // Executor called exactly once — second task was queued, not executed.
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "executor called once — second task was queued"
    );

    // DB state: one completed, one queued.
    {
        let db = db.lock().unwrap();
        let tasks = db.subagent_list_tasks(&r1.subagent_id, 10).unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(
            tasks.iter().any(|t| t.status == "completed"),
            "first task is completed"
        );
        let queued = tasks
            .iter()
            .find(|t| t.status == "queued")
            .expect("second task is queued");
        assert!(
            queued.started_at_ms.is_none(),
            "queued task has no started_at_ms"
        );
    }
}

// ---------------------------------------------------------------------------
// P1-2 fix (spec §4.3): `prompt_artifact_ref` end-to-end + validation.
// ---------------------------------------------------------------------------

/// `prompt` and `prompt_artifact_ref` are mutually exclusive (spec §4.3).
/// Setting both is rejected with `InvalidArgument`.
#[tokio::test]
async fn delegate_rejects_both_prompt_and_artifact_ref() {
    let db = test_db();
    let settings = SubagentSettings::default();
    let executor = Arc::new(RecordingExecutor) as Arc<dyn TaskExecutor>;
    let manager = std::sync::Arc::new(SubagentManager::with_pressure_gate(
        Arc::clone(&db),
        settings,
        "mock",
        executor,
        None,
    ));
    let mut r = req("test", "inline prompt");
    r.prompt_artifact_ref = Some("sub/task/prompt.txt".to_string());
    let err = manager.delegate(r).await.unwrap_err();
    assert_eq!(
        err.code(),
        "subagent.invalid_argument",
        "both prompt and prompt_artifact_ref set → InvalidArgument"
    );
}

/// Setting neither `prompt` nor `prompt_artifact_ref` is rejected.
#[tokio::test]
async fn delegate_rejects_neither_prompt_nor_artifact_ref() {
    let db = test_db();
    let settings = SubagentSettings::default();
    let executor = Arc::new(RecordingExecutor) as Arc<dyn TaskExecutor>;
    let manager = std::sync::Arc::new(SubagentManager::with_pressure_gate(
        Arc::clone(&db),
        settings,
        "mock",
        executor,
        None,
    ));
    let r = req("test", "");
    let err = manager.delegate(r).await.unwrap_err();
    assert_eq!(
        err.code(),
        "subagent.invalid_argument",
        "neither prompt nor prompt_artifact_ref set → InvalidArgument"
    );
}

/// `prompt_artifact_ref` is preserved end-to-end: DelegateRequest → task row
/// → ExecutorInput → (sidecar RPC). Verifies spec §4.3 contract.
#[tokio::test]
async fn delegate_preserves_prompt_artifact_ref_end_to_end() {
    use std::sync::Mutex as StdMutex;

    struct CapturingExecutor(Arc<StdMutex<Option<ExecutorInput>>>);
    #[async_trait]
    impl TaskExecutor for CapturingExecutor {
        async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
            *self.0.lock().unwrap() = Some(input.clone());
            Ok(ExecutorOutput {
                adapter_session_id: None,
                session_reused: false,
                status: TaskStatus::Completed,
                summary: "done".into(),
                usage: Default::default(),
                memory_update: Default::default(),
                error_kind: None,
            })
        }
    }

    let db = test_db();
    let settings = SubagentSettings::default();
    let captured = Arc::new(StdMutex::new(None));
    let executor = Arc::new(CapturingExecutor(Arc::clone(&captured))) as Arc<dyn TaskExecutor>;
    let manager = std::sync::Arc::new(SubagentManager::with_pressure_gate(
        Arc::clone(&db),
        settings,
        "mock",
        executor,
        None,
    ));

    let mut r = req("artifact-sub", "");
    r.prompt_artifact_ref = Some("sub123/task456/prompt.txt".to_string());
    let result = manager.delegate(r).await.unwrap();
    assert_eq!(result.status, TaskStatus::Running);
    await_task_done(&manager, &result.task_id).await;

    // Verify task row in DB has the artifact ref.
    {
        let db = db.lock().unwrap();
        let tasks = db.subagent_list_tasks(&result.subagent_id, 10).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].prompt_artifact_ref.as_deref(),
            Some("sub123/task456/prompt.txt"),
            "task row must preserve prompt_artifact_ref"
        );
    }

    // Verify ExecutorInput received the artifact ref.
    let captured_input = captured
        .lock()
        .unwrap()
        .take()
        .expect("executor was called");
    assert_eq!(
        captured_input.prompt_artifact_ref.as_deref(),
        Some("sub123/task456/prompt.txt"),
        "ExecutorInput must preserve prompt_artifact_ref"
    );
}

// ---------------------------------------------------------------------------
// HotSessionLimit re-queue regression (P0 fix):
// When two different subagents compete for the global hot session limit,
// the task that hits HotSessionLimit must be RE-QUEUED (not failed).
// Root cause: `execute_and_persist` treated ALL errors as terminal `failed`,
// including transient HotSessionLimit capacity contention. Fix: re-queue
// the task (flip status back to `queued`) when within deadline; mark `failed`
// with `error_kind = hot_session_limit` only after deadline is exceeded.
// ---------------------------------------------------------------------------

/// Mock executor that always returns `SubagentError::HotSessionLimit`.
/// Simulates the capacity contention that occurs when a different subagent
/// is holding the only hot session slot and retries are exhausted.
struct HotSessionLimitExecutor;

#[async_trait]
impl TaskExecutor for HotSessionLimitExecutor {
    async fn execute(&self, _input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        Err(anyhow::Error::from(
            busytok_subagent::SubagentError::HotSessionLimit {
                candidate: String::new(),
            },
        ))
    }
}

/// Mock executor that returns `HotSessionLimit` for the first `fail_count`
/// invocations, then succeeds. Simulates a hot session slot freeing up after
/// a concurrent task completes.
struct FlakeyHotSessionExecutor {
    call_count: Arc<std::sync::atomic::AtomicU32>,
    fail_count: u32,
}

#[async_trait]
impl TaskExecutor for FlakeyHotSessionExecutor {
    async fn execute(&self, _input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        let n = self
            .call_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n < self.fail_count {
            return Err(anyhow::Error::from(
                busytok_subagent::SubagentError::HotSessionLimit {
                    candidate: String::new(),
                },
            ));
        }
        Ok(ExecutorOutput {
            adapter_session_id: None,
            session_reused: false,
            status: TaskStatus::Completed,
            summary: "done after retry".into(),
            usage: Default::default(),
            memory_update: Default::default(),
            error_kind: None,
        })
    }
}

/// Helper: poll a task's (status, error_kind, error) until it reaches a
/// terminal state or timeout. Returns the final tuple.
async fn await_task_terminal(
    db: &Arc<Mutex<busytok_store::Database>>,
    subagent_id: &str,
    task_id: &str,
) -> (String, Option<String>, Option<String>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let snap = {
            let db = db.lock().unwrap();
            db.subagent_list_tasks(subagent_id, 10)
                .unwrap()
                .into_iter()
                .find(|t| t.id == task_id)
                .map(|t| (t.status, t.error_kind, t.error))
        };
        if let Some((ref status, _, _)) = snap {
            if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
                return snap.unwrap();
            }
        }
        if std::time::Instant::now() > deadline {
            panic!("task {task_id} did not reach terminal status within 10s (last: {snap:?})");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Regression (P0): when `execute_task` returns `HotSessionLimit`, the task
/// must be re-queued (status flips back to "queued") — NOT marked as "failed".
/// This is the core fix for the bug where concurrent different-subagent tasks
/// cause one to fail with "hot session limit reached" instead of
/// waiting/retrying/re-queuing.
#[tokio::test]
async fn hot_session_limit_requeues_task_within_deadline() {
    let db = test_db();
    let settings = SubagentSettings::default();
    let executor = Arc::new(HotSessionLimitExecutor) as Arc<dyn TaskExecutor>;
    let manager = std::sync::Arc::new(SubagentManager::with_pressure_gate(
        Arc::clone(&db),
        settings,
        "mock",
        executor,
        None,
    ));

    // delegate() inserts as "running" + spawns background execute_and_persist.
    let result = manager
        .delegate(req("hot-limit-sub", "do work"))
        .await
        .unwrap();
    assert_eq!(result.status, TaskStatus::Running);

    // Poll for up to 5s for the task to be re-queued (status = "queued").
    // The background executor returns HotSessionLimit immediately, so
    // execute_and_persist should flip it to "queued" within milliseconds.
    let mut requeued = false;
    for _ in 0..100 {
        let snap = {
            let db = db.lock().unwrap();
            db.subagent_list_tasks(&result.subagent_id, 10)
                .unwrap()
                .into_iter()
                .find(|t| t.id == result.task_id)
                .map(|t| (t.status, t.error_kind))
        };
        if let Some((status, error_kind)) = snap {
            if status == "queued" {
                assert!(
                    error_kind.is_none(),
                    "re-queued task must NOT have error_kind set, got: {error_kind:?}"
                );
                requeued = true;
                break;
            }
            if status == "failed" {
                panic!(
                    "task was marked failed instead of re-queued — HotSessionLimit regression. \
                     error_kind: {error_kind:?}"
                );
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        requeued,
        "task must be re-queued after HotSessionLimit (not failed)"
    );
}

/// Regression: when a task is re-queued via the HotSessionLimit path
/// (running → queued), `started_at_ms` MUST be cleared to NULL. A queued
/// task with a non-null `started_at_ms` is an invalid state — it implies
/// the task has started execution when it hasn't (or has been re-queued
/// for retry). The invariant is: `queued ⟹ started_at_ms IS NULL`.
///
/// This test catches the bug where `set_task_status_if_not_cancelled`
/// only updated `status` but left `started_at_ms` populated, creating
/// the "queued but already started" contradiction observed in production.
#[tokio::test]
async fn hot_session_limit_requeue_clears_started_at_ms() {
    let db = test_db();
    let settings = SubagentSettings::default();
    let executor = Arc::new(HotSessionLimitExecutor) as Arc<dyn TaskExecutor>;
    let manager = Arc::new(SubagentManager::with_pressure_gate(
        Arc::clone(&db),
        settings,
        "mock",
        executor,
        None,
    ));

    // delegate() inserts as "running" with started_at_ms = Some(now).
    let result = manager
        .delegate(req("hot-limit-started-ms-sub", "do work"))
        .await
        .unwrap();
    assert_eq!(result.status, TaskStatus::Running);

    // Poll for the re-queue to complete (status → "queued").
    let mut requeued = false;
    for _ in 0..100 {
        let snap = {
            let db = db.lock().unwrap();
            db.subagent_list_tasks(&result.subagent_id, 10)
                .unwrap()
                .into_iter()
                .find(|t| t.id == result.task_id)
                .map(|t| (t.status, t.started_at_ms, t.queue_reason))
        };
        if let Some((status, started_at_ms, queue_reason)) = snap {
            if status == "queued" {
                // PRIMARY ASSERTION: started_at_ms must be None.
                // A queued task must NOT retain started_at_ms from its
                // previous running state — that would create a
                // "queued but already started" contradiction.
                assert!(
                    started_at_ms.is_none(),
                    "re-queued task must have started_at_ms = NULL, \
                     got: {started_at_ms:?} — running→queued transition \
                     did not clear started_at_ms"
                );
                // SECONDARY ASSERTION: queue_reason must be set so external
                // orchestrators can distinguish hot_session_limit re-queue
                // from same-subagent serialization.
                assert_eq!(
                    queue_reason.as_deref(),
                    Some("hot_session_limit"),
                    "re-queued task must carry queue_reason = 'hot_session_limit', \
                     got: {queue_reason:?}"
                );
                requeued = true;
                break;
            }
            if status == "failed" {
                panic!(
                    "task was marked failed instead of re-queued — \
                     HotSessionLimit regression"
                );
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        requeued,
        "task must be re-queued after HotSessionLimit (not failed)"
    );
}

/// Regression (P0): when the task exceeds the re-queue deadline
/// (`HOT_SESSION_RETRY_DEADLINE_MS`), `execute_and_persist` must mark it as
/// "failed" with `error_kind = "hot_session_limit"` — NOT `"unknown"` and NOT
/// infinite re-queuing. This addresses the `error_kind = "unknown"` issue
/// in the bug report.
#[tokio::test]
async fn hot_session_limit_marks_failed_after_deadline() {
    let db = test_db();
    let settings = SubagentSettings::default();
    let executor = Arc::new(HotSessionLimitExecutor) as Arc<dyn TaskExecutor>;
    let gate = Arc::new(PressureGate::new());
    gate.set_action(PressureAction::PauseNewTasks);
    let manager = Arc::new(SubagentManager::with_pressure_gate(
        Arc::clone(&db),
        settings,
        "mock",
        executor,
        Some(Arc::clone(&gate)),
    ));

    // Queue a task while the gate is paused.
    let result = manager
        .delegate(req("hot-limit-deadline-sub", "do work"))
        .await
        .unwrap();
    assert_eq!(result.status, TaskStatus::Queued);

    // Age the task beyond the re-queue deadline so the next
    // execute_and_persist cycle will hit the deadline-exceeded path.
    // HOT_SESSION_RETRY_DEADLINE_MS = 300_000 (5 min); set created_at_ms to
    // 400_000ms in the past so age_ms > deadline.
    {
        let db = db.lock().unwrap();
        db.conn()
            .execute(
                "UPDATE subagent_tasks SET created_at_ms = ?1 WHERE id = ?2",
                rusqlite::params![busytok_domain::now_ms() - 400_000, result.task_id,],
            )
            .unwrap();
    }

    // Start the dispatcher + clear the gate.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = manager.clone().spawn_task_dispatcher(shutdown_rx);
    gate.set_action(PressureAction::Resume);

    // Poll for up to 10s for the task to be marked failed.
    let (status, error_kind, error) =
        await_task_terminal(&db, &result.subagent_id, &result.task_id).await;

    assert_eq!(
        status, "failed",
        "deadline-exceeded task must be marked failed"
    );
    assert_eq!(
        error_kind.as_deref(),
        Some("hot_session_limit"),
        "error_kind must be 'hot_session_limit' (not 'unknown'), got: {error_kind:?}"
    );
    assert!(
        error.as_deref().unwrap_or("").contains("hot session limit"),
        "error message must mention hot session limit, got: {error:?}"
    );

    // Deterministic shutdown.
    let _ = shutdown_tx.send(true);
    let _ = handle.await;
}

/// Regression (P0): full re-queue → retry → success cycle. When the hot
/// session slot frees up (simulated by a flaky executor that fails N times
/// then succeeds), the re-queued task must eventually complete — NOT remain
/// stuck in re-queue loops or fail.
///
/// This mirrors the user's expected behavior: "一边运行、另一边进入可观测的
/// queued / 等待态" then "两边都完成" when the slot frees up.
#[tokio::test]
async fn hot_session_limit_requeue_then_succeeds_when_slot_frees() {
    let db = test_db();
    let settings = SubagentSettings::default();
    let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let executor = Arc::new(FlakeyHotSessionExecutor {
        call_count: Arc::clone(&call_count),
        fail_count: 2,
    }) as Arc<dyn TaskExecutor>;
    let manager = std::sync::Arc::new(SubagentManager::with_pressure_gate(
        Arc::clone(&db),
        settings,
        "mock",
        executor,
        None,
    ));

    // Start the dispatcher so re-queued tasks get picked up automatically.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = manager.clone().spawn_task_dispatcher(shutdown_rx);

    // delegate() → running → HotSessionLimit → re-queue → running →
    // HotSessionLimit → re-queue → running → success → completed.
    let result = manager
        .delegate(req("hot-limit-flakey-sub", "do work"))
        .await
        .unwrap();

    let (status, error_kind, _) =
        await_task_terminal(&db, &result.subagent_id, &result.task_id).await;

    assert_eq!(
        status, "completed",
        "task must eventually complete after re-queue retries — got status: {status}"
    );
    assert!(
        error_kind.is_none(),
        "completed task must have no error_kind, got: {error_kind:?}"
    );
    // Executor was called 3 times: 2 failures + 1 success.
    assert_eq!(
        call_count.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "executor called 3 times (2 HotSessionLimit + 1 success)"
    );

    // Deterministic shutdown.
    let _ = shutdown_tx.send(true);
    let _ = handle.await;
}
