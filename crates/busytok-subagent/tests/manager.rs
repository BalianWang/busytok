#![allow(clippy::unwrap_used)]

use busytok_config::SubagentSettings;
use busytok_store::{Database, SubagentHarnessBindingRow};
use busytok_subagent::manager::SubagentManager;
use busytok_subagent::models::{DelegateRequest, ResolveParams, SubagentStatus};
use busytok_subagent::SubagentError;

async fn manager() -> SubagentManager {
    // std::sync::Mutex — matches the supervisor's db field type.
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    SubagentManager::new(db, SubagentSettings::default(), "pi")
}

fn req_with_cwd(name: &str, prompt: &str, cwd: &str) -> DelegateRequest {
    DelegateRequest {
        subagent_name: name.to_string(),
        subagent_id: None,
        cwd: cwd.to_string(),
        profile: "pi/search-cheap".to_string(),
        intent: None,
        prompt: prompt.to_string(),
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
    // status filter narrows the set
    let warm = m
        .list(Some(SubagentStatus::Warm), None, false)
        .await
        .unwrap();
    assert_eq!(warm.len(), 2, "both go warm after a mock task");
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
async fn delegate_with_unknown_profile_falls_back_to_no_model() {
    let m = manager().await;
    let r = m
        .delegate(DelegateRequest {
            profile: "custom/unknown".to_string(),
            ..req("reviewer", "do")
        })
        .await
        .unwrap();
    // unknown profile → profile_model returns None → model is None
    assert!(r.model.is_none());
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
    let m = SubagentManager::new(db, settings, "pi");
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
async fn resolve_without_id_or_name_returns_invalid_name() {
    let m = manager().await;
    let err = m.show(ResolveParams::default()).await.unwrap_err();
    assert!(matches!(err, SubagentError::InvalidName(_)));
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
async fn hibernate_then_show_status_is_warm() {
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
    // memory was written during delegate, so hibernate leaves it warm
    assert_eq!(detail.status.as_str(), "warm");
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
