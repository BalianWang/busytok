#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use busytok_config::{SubagentResourcePolicyConfig, SubagentSettings};
use busytok_store::Database;
use busytok_subagent::mock_executor::TaskExecutor;
use busytok_subagent::models::DelegateRequest;
use busytok_subagent::pressure::{PressureGate, PressureResponder};
use busytok_subagent::sidecar::config::SidecarConfig;
use busytok_subagent::sidecar::executor::SidecarTaskExecutor;
use busytok_subagent::sidecar::{
    PiSidecarSupervisor, ProviderRuntimeEntry, ResponderFactory, WorkerPool,
};
use busytok_subagent::SubagentManager;

/// The test provider ID used by the pool-based test.
const TEST_PROVIDER_ID: &str = "test-prov";

fn mock_sidecar_script() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../busytok-subagent/tests/fixtures/mock-sidecar.sh");
    p
}

/// Resolve the shell binary used to launch the mock sidecar script.
/// On Windows, bash is not on PATH by default; use Git Bash.
fn sidecar_shell_path() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            return PathBuf::from(program_files)
                .join("Git")
                .join("bin")
                .join("bash.exe");
        }
        PathBuf::from(r"C:\Program Files\Git\bin\bash.exe")
    }

    #[cfg(not(windows))]
    {
        PathBuf::from("/bin/bash")
    }
}

/// Convert the mock sidecar script path to a form bash can execute.
/// On Windows, Git Bash expects MSYS-style paths (/c/users/...).
fn mock_sidecar_bundle_path() -> PathBuf {
    let path = mock_sidecar_script();
    #[cfg(windows)]
    {
        let raw = path.to_string_lossy().replace('\\', "/");
        if let Some((drive, rest)) = raw.split_once(":/") {
            let drive = drive.to_ascii_lowercase();
            return PathBuf::from(format!("/{drive}/{rest}"));
        }
        PathBuf::from(raw)
    }

    #[cfg(not(windows))]
    {
        path
    }
}

fn mock_sidecar_config() -> SidecarConfig {
    SidecarConfig {
        node_binary: sidecar_shell_path(),
        bundle_path: mock_sidecar_bundle_path(),
        env: HashMap::new(),
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

/// Build the provider runtime entries map (Task 7: replaces the old
/// `ProviderLookup` + `CredentialReader` closures). The pool injects
/// `OPENAI_API_KEY` / `OPENAI_BASE_URL` from this entry at spawn time.
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

/// Build a responder factory that constructs a real `PressureResponder` and
/// keeps a strong ref alive in a shared holder (mirrors production wiring
/// where `BusytokSupervisor` holds the strong ref).
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

/// Build `SubagentSettings` with the `pi/search-cheap` profile bound to
/// `TEST_PROVIDER_ID` so `SubagentManager::delegate` threads a non-`None`
/// `provider_id` into `ExecutorInput` (required by the pool-based executor).
fn settings_with_test_provider() -> SubagentSettings {
    let mut settings = SubagentSettings::default();
    settings
        .profiles
        .get_mut("pi/search-cheap")
        .expect("default profiles must include pi/search-cheap")
        .provider_id = Some(TEST_PROVIDER_ID.to_string());
    settings
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
        bound_provider_id: None,
        bound_model_id: None,
    }
}

#[tokio::test]
async fn sidecar_shutdown_kills_subprocess_then_restart_works() {
    let db = Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));

    // Build a pool + executor + supervisor via the two-phase bootstrap
    // (mirrors production wiring in `construct_sidecar`).
    let gate = Arc::new(PressureGate::new());
    let pool = Arc::new(WorkerPool::new(
        mock_sidecar_config(),
        Some(Arc::clone(&db)),
        make_providers(),
        Some(Arc::clone(&gate)),
        SubagentResourcePolicyConfig::default(),
    ));
    let executor = Arc::new(SidecarTaskExecutor::with_pool(
        Arc::clone(&pool),
        Some(Arc::clone(&db)),
    ));
    let (factory, _holder) = make_responder_factory(Arc::clone(&gate), Arc::downgrade(&executor));
    pool.set_responder_factory(factory);
    let supervisor = pool
        .ensure_worker(TEST_PROVIDER_ID)
        .expect("ensure_worker test-prov");

    let exec_dyn: Arc<dyn TaskExecutor> = executor.clone();
    let manager = SubagentManager::new(
        Arc::clone(&db),
        settings_with_test_provider(),
        "pi",
        exec_dyn,
    );

    // First delegate spawns the sidecar.
    let r1 = manager.delegate(req("reviewer", "first")).await.unwrap();
    assert!(r1.adapter_session_id.is_some());

    // Graceful shutdown — sidecar process exits.
    supervisor.shutdown().await.unwrap();

    // Second delegate restarts the sidecar (lazy spawn on ensure_started).
    let r2 = manager.delegate(req("reviewer", "second")).await.unwrap();
    assert!(r2.adapter_session_id.is_some());

    supervisor.shutdown().await.unwrap();
}
