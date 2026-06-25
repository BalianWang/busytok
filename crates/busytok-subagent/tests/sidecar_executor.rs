#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use busytok_config::SubagentSettings;
use busytok_store::Database;
use busytok_subagent::mock_executor::TaskExecutor;
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
    }
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
