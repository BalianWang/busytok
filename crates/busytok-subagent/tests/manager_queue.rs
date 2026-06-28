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
use busytok_subagent::models::{DelegateRequest, TaskStatus};
use busytok_subagent::pressure::{PressureAction, PressureGate};
use busytok_subagent::SubagentManager;

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
        })
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
        timeout_seconds: Some(5),
        model_override: None,
        source_harness: None,
        source_session_id: None,
    }
}

#[tokio::test]
async fn delegate_returns_queued_when_gate_paused() {
    let db = Arc::new(Mutex::new(
        busytok_store::Database::open_in_memory().unwrap(),
    ));
    let settings = SubagentSettings::default();
    let executor = Arc::new(RecordingExecutor) as Arc<dyn TaskExecutor>;
    let gate = Arc::new(PressureGate::new());
    gate.set_action(PressureAction::PauseNewTasks);
    let manager = SubagentManager::with_pressure_gate(
        Arc::clone(&db),
        settings,
        "mock",
        executor,
        Some(Arc::clone(&gate)),
    );
    let result = manager.delegate(req("test", "hello")).await.unwrap();
    assert_eq!(
        result.status,
        TaskStatus::Queued,
        "delegate must return Queued when gate is paused"
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
    let db = Arc::new(Mutex::new(
        busytok_store::Database::open_in_memory().unwrap(),
    ));
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
    let db = Arc::new(Mutex::new(
        busytok_store::Database::open_in_memory().unwrap(),
    ));
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
