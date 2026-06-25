#![allow(clippy::unwrap_used)]

use busytok_config::SubagentSettings;
use busytok_store::Database;
use busytok_subagent::manager::SubagentManager;
use busytok_subagent::models::{DelegateRequest, ResolveParams, SubagentStatus};

async fn manager() -> SubagentManager {
    // std::sync::Mutex — matches the supervisor's db field type.
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    SubagentManager::new(db, SubagentSettings::default(), "pi")
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
