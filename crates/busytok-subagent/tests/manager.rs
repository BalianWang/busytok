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
    DelegateRequest, ResolveParams, SubagentStatus, TaskStatus, TaskUsage,
};
use busytok_subagent::pressure::{PressureAction, PressureGate};
use busytok_subagent::SubagentError;

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

async fn manager() -> SubagentManager {
    // std::sync::Mutex — matches the supervisor's db field type.
    let db = test_db();
    SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    )
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
    }
}

#[tokio::test]
async fn delegate_creates_subagent_then_reuses_it() {
    let m = manager().await;
    let r1 = m.delegate(req("reviewer", "step one")).await.unwrap();
    assert_eq!(r1.subagent_name, "reviewer");
    assert_eq!(r1.status.as_str(), "completed");

    let r2 = m.delegate(req("reviewer", "step two")).await.unwrap();
    assert_eq!(r2.subagent_id, r1.subagent_id, "same subagent reused");
}

#[tokio::test]
async fn list_returns_active_subagents() {
    let m = manager().await;
    m.delegate(req("a", "do")).await.unwrap();
    m.delegate(req("b", "do")).await.unwrap();
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
    m.delegate(req("reviewer", "second")).await.unwrap();
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
    assert_eq!(r2.status.as_str(), "completed");
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
    assert!(
        r_review.model.is_some(),
        "review profile should map to a model"
    );
    let r_plan = m
        .delegate(DelegateRequest {
            profile: "pi/plan-cheap".to_string(),
            ..req("planner", "do")
        })
        .await
        .unwrap();
    assert!(r_plan.model.is_some(), "plan profile should map to a model");
}

#[tokio::test]
async fn delegate_rejected_when_feature_disabled() {
    let db = test_db();
    let settings = SubagentSettings {
        enabled: false,
        ..Default::default()
    };
    let m = SubagentManager::new(db, settings, "pi", std::sync::Arc::new(MockTaskExecutor));
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
    let _ = m.delegate(req("reviewer", "do")).await.unwrap();
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
    let _ = m.delegate(req("reviewer", "do")).await.unwrap();
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
    let m = SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(WarmMemoryExecutor),
    );
    let r = m.delegate(req("warm-reviewer", "do")).await.unwrap();
    assert_eq!(r.status.as_str(), "completed");

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
    let m = SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    );
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
    let m = SubagentManager::new(
        std::sync::Arc::clone(&db),
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    );
    let r = m.delegate(req("reviewer", "do")).await.unwrap();

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

// --- Plan 4 Task 3: ContextBuilder + MemoryUpdater wiring -------------------

struct MemoryUpdateExecutor {
    captured_input: Mutex<Option<ExecutorInput>>,
}

#[async_trait::async_trait]
impl TaskExecutor for MemoryUpdateExecutor {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        // Capture a clone of the input's context string to verify it was built.
        let captured = ExecutorInput {
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
    let manager = SubagentManager::new(
        db.clone(),
        SubagentSettings::default(),
        "pi",
        executor.clone(),
    );
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
    };
    let result = manager.delegate(req).await.unwrap();
    assert_eq!(result.status, TaskStatus::Completed);

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
    let manager = SubagentManager::new(db.clone(), settings, "mock", executor);
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
    };
    let result = manager.delegate(req).await.unwrap();
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
    let manager = SubagentManager::new(
        db.clone(),
        SubagentSettings::default(),
        "mock",
        std::sync::Arc::new(MockTaskExecutor),
    );
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
    };
    let result = manager.delegate(req).await.unwrap();
    assert_eq!(result.status.as_str(), "completed");
    let summary = result
        .summary
        .as_deref()
        .expect("delegate always wraps Some(out.summary)");
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

/// Executor returns a `SubagentError` (via anyhow) → downcast Ok branch (line 390).
/// `FailingTaskExecutor` returns `SubagentError::SidecarSpawn` wrapped in anyhow.
#[tokio::test]
async fn delegate_executor_subagent_error_downcasts_and_propagates() {
    let _guard = install_tracing();
    let db = test_db();
    let m = SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(FailingTaskExecutor {
            reason: "init failed".into(),
        }),
    );
    let err = m.delegate(req("fail-sub", "do")).await.unwrap_err();
    assert_eq!(
        err.code(),
        "subagent.sidecar_spawn_failed",
        "SubagentError should downcast and propagate its code"
    );
}

/// Executor returns a generic anyhow error → downcast Err branch (lines 391-393).
#[tokio::test]
async fn delegate_executor_generic_error_wrapped_as_store() {
    let _guard = install_tracing();
    let db = test_db();
    let m = SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(BoomExecutor),
    );
    let err = m.delegate(req("boom-sub", "do")).await.unwrap_err();
    assert_eq!(
        err.code(),
        "subagent.store_error",
        "non-SubagentError anyhow should be wrapped as Store"
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
    let m = SubagentManager::new(db, settings, "pi", std::sync::Arc::new(MockTaskExecutor));
    let expected_model = bound_model_id();
    let r = m
        .delegate(DelegateRequest {
            profile: "custom/empty-model".to_string(),
            ..req("empty-model-sub", "do")
        })
        .await
        .unwrap();
    assert_eq!(r.profile, "custom/empty-model");
    assert_eq!(
        r.model.as_deref(),
        Some(expected_model.as_str()),
        "model falls back to bound_model_id when profile model is empty and no override"
    );
}

/// Hot binding commit failure (table dropped) → lines 509-515.
/// The executor returns a non-empty adapter_session_id, triggering the hot
/// binding commit path. Dropping the bindings table makes the commit fail.
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
    let m = SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(HotSessionExecutor),
    );
    let err = m.delegate(req("hot-bind-sub", "do")).await.unwrap_err();
    assert_eq!(
        err.code(),
        "subagent.store_error",
        "hot binding commit failure should surface as store_error"
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

fn manager_with_db(db: std::sync::Arc<std::sync::Mutex<Database>>) -> SubagentManager {
    SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    )
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

    let manager = SubagentManager::new(
        db,
        SubagentSettings::default(),
        "mock",
        std::sync::Arc::new(MockTaskExecutor),
    );

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

    let manager = SubagentManager::new(
        db,
        SubagentSettings::default(),
        "mock",
        std::sync::Arc::new(MockTaskExecutor),
    );

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

    let manager = SubagentManager::new(
        db,
        SubagentSettings::default(),
        "mock",
        std::sync::Arc::new(MockTaskExecutor),
    );

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

    let manager = SubagentManager::new(
        db,
        SubagentSettings::default(),
        "mock",
        std::sync::Arc::new(MockTaskExecutor),
    );

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

    let manager = SubagentManager::new(
        db,
        SubagentSettings::default(),
        "mock",
        std::sync::Arc::new(MockTaskExecutor),
    );

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
