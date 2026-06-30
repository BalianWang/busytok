#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use busytok_config::SubagentSettings;
use busytok_store::Database;
use busytok_subagent::mock_executor::TaskExecutor;
use busytok_subagent::models::DelegateRequest;
use busytok_subagent::sidecar::config::SidecarConfig;
use busytok_subagent::sidecar::executor::SidecarTaskExecutor;
use busytok_subagent::sidecar::PiSidecarSupervisor;
use busytok_subagent::SubagentManager;

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
        provider_id: String::new(),
        api_key_env_name: String::new(),
        base_url_env_name: String::new(),
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
        timeout_seconds: Some(10),
        model_override: None,
        source_harness: None,
        source_session_id: None,
    }
}

#[tokio::test]
async fn sidecar_shutdown_kills_subprocess_then_restart_works() {
    let db = Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    let supervisor = PiSidecarSupervisor::new(mock_sidecar_config(), Some(Arc::clone(&db)));
    let executor: Arc<dyn TaskExecutor> =
        Arc::new(SidecarTaskExecutor::new(Arc::clone(&supervisor)));
    let manager =
        SubagentManager::new(Arc::clone(&db), SubagentSettings::default(), "pi", executor);

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
