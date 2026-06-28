#![allow(clippy::unwrap_used)]

use std::sync::Mutex;

use busytok_config::SubagentSettings;
use busytok_store::{Database, SubagentHarnessBindingRow};
use busytok_subagent::manager::SubagentManager;
use busytok_subagent::memory::{KeyFile, MemoryUpdate, OpenQuestion};
use busytok_subagent::mock_executor::{
    ExecutorInput, ExecutorOutput, MockTaskExecutor, TaskExecutor,
};
use busytok_subagent::models::{
    DelegateRequest, ResolveParams, SubagentStatus, TaskStatus, TaskUsage,
};
use busytok_subagent::SubagentError;

async fn manager() -> SubagentManager {
    // std::sync::Mutex — matches the supervisor's db field type.
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    SubagentManager::new(
        db,
        SubagentSettings::default(),
        "pi",
        std::sync::Arc::new(MockTaskExecutor),
    )
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
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
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

#[tokio::test]
async fn hibernate_closes_existing_hot_binding() {
    // delegate creates no hot binding (Plan 1), so seed one manually to cover
    // the `if let Some(mut b) = binding` branch in manager::hibernate.
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
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
        })
    }
}

#[tokio::test]
async fn delegate_builds_context_and_merges_memory_update() {
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
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
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
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
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
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
