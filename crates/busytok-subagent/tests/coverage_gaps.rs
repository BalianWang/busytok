//! Coverage gap tests for `busytok-subagent` (pressure.rs + sidecar/client.rs).
//!
//! Targets uncovered source lines reported by `cargo llvm-cov`:
//! - `pressure.rs`: multi-line tracing macro format strings (warn!/info!/error!)
//!   that are NOT evaluated when no subscriber is set; the in-flight dedup
//!   skip branch; the `evict_lru` failure path; the `prepare_hibernate_all`
//!   failure path.
//! - `sidecar/client.rs`: multi-line `debug!` format strings in the
//!   notification-skip / parse-skip / id-mismatch branches.
//!
//! Lines 223-246 in `pressure.rs` are DEAD CODE — `shutdown_internal()` always
//! returns `Ok(())` (all internal errors are swallowed with `let _ =`), so the
//! `if let Err(e) = ...` branch that escalates to ForceKill is unreachable
//! without modifying production code. These 10 lines are excluded from the
//! coverage target.

#![allow(
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    dead_code,
    unused_imports,
    unused_variables
)]

use std::collections::HashMap;
use std::sync::{Arc, Once, Weak};
use std::time::Duration;

use busytok_config::{ProviderConfig, ProviderKind, SubagentResourcePolicyConfig};
use busytok_store::Database;
use busytok_subagent::pressure::{PressureAction, PressureGate, PressureResponder};
use busytok_subagent::sidecar::{
    CredentialReader, PiSidecarSupervisor, ProviderLookup, ResponderFactory, SidecarConfig,
    SidecarError, SidecarRpcClient, SidecarTaskExecutor, WorkerPool,
};
use tokio::process::Command;

#[path = "support/mod.rs"]
mod support;

// ── Tracing subscriber init ────────────────────────────────────────────
//
// Multi-line tracing macros (warn!, info!, error!, debug!) expand their
// format string arguments lazily — only when a subscriber is present and the
// level is enabled. Without a subscriber, the format string lines are NOT
// counted as "executed" by the coverage instrumentation, even though the
// macro call site is reached. We initialize a subscriber once per test binary
// so the format string lines are covered.

static SUBSCRIBER_INIT: Once = Once::new();

fn init_subscriber() {
    SUBSCRIBER_INIT.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("trace")
            .with_test_writer()
            .try_init();
    });
}

// ── Shared helpers (mirror pressure.rs test helpers) ─────────────────────

const TEST_PROVIDER_ID: &str = "test-prov";

fn test_provider() -> ProviderConfig {
    ProviderConfig {
        id: TEST_PROVIDER_ID.to_string(),
        name: "Test".to_string(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://test.example.com/v1".to_string(),
        api_key_env_name: "TEST_API_KEY".to_string(),
        base_url_env_name: None,
        models: vec!["test-model".to_string()],
        enabled: true,
    }
}

fn make_providers() -> ProviderLookup {
    Arc::new(|pid: &str| -> Option<ProviderConfig> {
        match pid {
            TEST_PROVIDER_ID => Some(test_provider()),
            _ => None,
        }
    })
}

fn canned_credential_reader() -> CredentialReader {
    Arc::new(|_pid: &str| Ok(Some("test-key".to_string())))
}

fn mock_config_and_db() -> (SidecarConfig, Arc<std::sync::Mutex<Database>>) {
    let db = Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    let config = SidecarConfig {
        node_binary: std::path::PathBuf::from("/usr/bin/true"),
        bundle_path: std::path::PathBuf::from("/dev/null"),
        env: HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(30),
        task_timeout: Duration::from_secs(300),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
        provider_id: String::new(),
        api_key_env_name: String::new(),
        base_url_env_name: String::new(),
    };
    (config, db)
}

fn live_sidecar_config_and_db() -> (SidecarConfig, Arc<std::sync::Mutex<Database>>) {
    let db = Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    let config = SidecarConfig {
        node_binary: support::sidecar_shell_path(),
        bundle_path: support::mock_sidecar_bundle_path(),
        env: HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(3600),
        task_timeout: Duration::from_secs(5),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_millis(10),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
        provider_id: String::new(),
        api_key_env_name: String::new(),
        base_url_env_name: String::new(),
    };
    (config, db)
}

/// Build a live sidecar config with extra env vars (e.g.
/// BUSYTOK_MOCK_PREPARE_HIBERNATE_FAILS=1) and a per-response delay.
fn live_sidecar_config_with_env(
    extra_env: HashMap<String, String>,
    delay_ms: u64,
) -> (SidecarConfig, Arc<std::sync::Mutex<Database>>) {
    let db = Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    let mut env = HashMap::new();
    if delay_ms > 0 {
        env.insert("BUSYTOK_MOCK_DELAY_MS".to_string(), delay_ms.to_string());
    }
    for (k, v) in extra_env {
        env.insert(k, v);
    }
    let config = SidecarConfig {
        node_binary: support::sidecar_shell_path(),
        bundle_path: support::mock_sidecar_bundle_path(),
        env,
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
    };
    (config, db)
}

/// Build a dummy executor (pool with `/usr/bin/true` config) whose only purpose
/// is to keep the `Weak<SidecarTaskExecutor>` in the responder upgradeable.
fn make_dummy_executor(db: Arc<std::sync::Mutex<Database>>) -> Arc<SidecarTaskExecutor> {
    let (config, _) = mock_config_and_db();
    let pool = Arc::new(WorkerPool::new(
        config,
        Some(Arc::clone(&db)),
        make_providers(),
        canned_credential_reader(),
        None,
        SubagentResourcePolicyConfig::default(),
    ));
    Arc::new(SidecarTaskExecutor::with_pool(
        Arc::clone(&pool),
        Some(Arc::clone(&db)),
    ))
}

/// Build an executor whose `db` is `None` — `evict_lru()` returns `Err`,
/// exercising the `hibernate_lru_failed` warn! path in pressure.rs.
fn make_no_db_executor() -> (Arc<SidecarTaskExecutor>, Arc<WorkerPool>) {
    let (config, db) = mock_config_and_db();
    let pool = Arc::new(WorkerPool::new(
        config,
        Some(db),
        make_providers(),
        canned_credential_reader(),
        None,
        SubagentResourcePolicyConfig::default(),
    ));
    let executor = Arc::new(SidecarTaskExecutor::with_pool(Arc::clone(&pool), None));
    (executor, pool)
}

// ── pressure.rs: tracing format string coverage ──────────────────────────

#[tokio::test]
async fn respond_resume_logs_with_subscriber() {
    init_subscriber();
    let (config, db) = mock_config_and_db();
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = make_dummy_executor(Arc::clone(&db));
    let gate = Arc::new(PressureGate::new());
    let responder = PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&exec),
        Arc::clone(&gate),
    );

    gate.set_action(PressureAction::PauseNewTasks);
    assert!(gate.is_paused());
    responder.respond(PressureAction::Resume).await;
    assert!(!gate.is_paused());
    // Covers line 174: info!("pressure cleared")
}

#[tokio::test]
async fn respond_hibernate_lru_logs_with_subscriber() {
    init_subscriber();
    let (config, db) = mock_config_and_db();
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = make_dummy_executor(Arc::clone(&db));
    let gate = Arc::new(PressureGate::new());
    let responder = PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&exec),
        Arc::clone(&gate),
    );

    // evict_lru succeeds (empty DB, no hot bindings) — covers the info! on
    // line 179 but NOT the warn! on line 182.
    responder.respond(PressureAction::HibernateLru).await;
    assert!(!gate.is_paused());
    assert!(matches!(
        gate.last_action(),
        Some(PressureAction::HibernateLru)
    ));
}

#[tokio::test]
async fn respond_hibernate_lru_failure_logs_warn_with_subscriber() {
    init_subscriber();
    // Executor with db=None → evict_lru returns Err → warn! on line 182.
    let (config, db) = mock_config_and_db();
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let (exec, _pool) = make_no_db_executor();
    let gate = Arc::new(PressureGate::new());
    let responder = PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&exec),
        Arc::clone(&gate),
    );

    responder.respond(PressureAction::HibernateLru).await;
    assert!(!gate.is_paused());
    assert!(matches!(
        gate.last_action(),
        Some(PressureAction::HibernateLru)
    ));
    // Covers line 182: warn!("hibernate_lru_failed")
}

#[tokio::test]
async fn respond_pause_new_tasks_logs_with_subscriber() {
    init_subscriber();
    let (config, db) = mock_config_and_db();
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = make_dummy_executor(Arc::clone(&db));
    let gate = Arc::new(PressureGate::new());
    let responder = PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&exec),
        Arc::clone(&gate),
    );

    responder.respond(PressureAction::PauseNewTasks).await;
    assert!(gate.is_paused());
    // Covers line 192: info!("pause new tasks")
}

#[tokio::test]
async fn respond_graceful_restart_with_prepare_hibernate_failure_logs_warn() {
    init_subscriber();
    // Live sidecar with BUSYTOK_MOCK_PREPARE_HIBERNATE_FAILS=1 →
    // prepare_hibernate_all returns Err → warn! on line 207.
    let mut env = HashMap::new();
    env.insert(
        "BUSYTOK_MOCK_PREPARE_HIBERNATE_FAILS".to_string(),
        "1".to_string(),
    );
    let (config, db) = live_sidecar_config_with_env(env, 0);
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = make_dummy_executor(Arc::clone(&db));
    let gate = Arc::new(PressureGate::new());
    let responder = PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&exec),
        Arc::clone(&gate),
    );

    // Start the sidecar so ensure_started() succeeds.
    let _ = sup.ensure_started().await.unwrap();
    assert!(sup.try_is_running());

    responder.respond(PressureAction::GracefulRestart).await;

    // Gate is cleared to Resume after graceful restart.
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
    // Covers line 200 (info! "graceful restart") + line 207 (warn! "prepare_hibernate_failed")
}

#[tokio::test]
async fn respond_graceful_restart_spawn_fails_logs_ensure_started_failed() {
    init_subscriber();
    // Missing binary → ensure_started fails → warn! on line 217.
    let (mut config, db) = mock_config_and_db();
    config.node_binary = std::path::PathBuf::from("/definitely/missing/binary");
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = make_dummy_executor(Arc::clone(&db));
    let gate = Arc::new(PressureGate::new());
    let responder = PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&exec),
        Arc::clone(&gate),
    );

    responder.respond(PressureAction::GracefulRestart).await;
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
    // Covers line 200 (info!) + line 217 (warn! "ensure_started_failed")
}

#[tokio::test]
async fn respond_force_kill_logs_with_subscriber() {
    init_subscriber();
    let (config, db) = mock_config_and_db();
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = make_dummy_executor(Arc::clone(&db));
    let gate = Arc::new(PressureGate::new());
    let responder = PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&exec),
        Arc::clone(&gate),
    );

    // force_kill is a no-op when no child is running. Gate is cleared to Resume.
    responder.respond(PressureAction::ForceKill).await;
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
    // Covers line 254 (error! "force kill") + line 265 (info! "force kill done")
}

#[tokio::test]
async fn respond_supervisor_dropped_logs_warn_with_subscriber() {
    init_subscriber();
    let (config, db) = mock_config_and_db();
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = make_dummy_executor(Arc::clone(&db));
    let gate = Arc::new(PressureGate::new());
    let sup_weak = Arc::downgrade(&sup);
    let exec_weak = Arc::downgrade(&exec);
    let responder = PressureResponder::new(sup_weak, exec_weak, Arc::clone(&gate));

    // Drop the supervisor — the weak ref fails to upgrade.
    drop(sup);
    responder.respond(PressureAction::ForceKill).await;
    // Gate unchanged (Responder couldn't act).
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
    // Covers line 160: warn!("supervisor dropped")
}

#[tokio::test]
async fn respond_executor_dropped_logs_warn_with_subscriber() {
    init_subscriber();
    let (config, db) = mock_config_and_db();
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = make_dummy_executor(Arc::clone(&db));
    let gate = Arc::new(PressureGate::new());
    let sup_weak = Arc::downgrade(&sup);
    let exec_weak = Arc::downgrade(&exec);
    // Drop the executor — the weak ref fails to upgrade.
    drop(exec);
    let responder = PressureResponder::new(sup_weak, exec_weak, Arc::clone(&gate));
    responder.respond(PressureAction::HibernateLru).await;
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
    // Covers line 167: warn!("executor dropped")
}

// ── pressure.rs: in-flight deduplication skip branch ──────────────────────

#[tokio::test]
async fn respond_skips_when_another_respond_is_in_flight() {
    init_subscriber();

    // Live sidecar with 1s delay per response. The first respond() will hold
    // the in_flight Mutex for ~1s while waiting for prepare_hibernate_all.
    let (config, db) = live_sidecar_config_with_env(HashMap::new(), 1000);
    let sup = PiSidecarSupervisor::new(config, Some(Arc::clone(&db)));
    let exec = make_dummy_executor(Arc::clone(&db));
    let gate = Arc::new(PressureGate::new());
    let responder = Arc::new(PressureResponder::new(
        Arc::downgrade(&sup),
        Arc::downgrade(&exec),
        Arc::clone(&gate),
    ));

    // Start the sidecar so the first respond's ensure_started returns quickly.
    let _ = sup.ensure_started().await.unwrap();

    // Spawn the first respond(GracefulRestart) — holds in_flight lock during
    // the delayed prepare_hibernate_all RPC.
    let responder_clone = Arc::clone(&responder);
    let first = tokio::spawn(async move {
        responder_clone
            .respond(PressureAction::GracefulRestart)
            .await;
    });

    // Give the first respond time to acquire the in_flight lock and enter the
    // prepare_hibernate_all RPC (which is delayed by 1s).
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // The second respond(Resume) should skip — in_flight is held.
    // This covers lines 148, 151, 153 (the skip branch).
    responder.respond(PressureAction::Resume).await;

    // The gate should be GracefulRestart (set by the first respond before
    // the delayed RPC) — NOT Resume, because the second respond skipped.
    assert!(
        gate.is_paused(),
        "gate should be paused (GracefulRestart) because second respond skipped"
    );
    assert!(matches!(
        gate.last_action(),
        Some(PressureAction::GracefulRestart)
    ));

    // Wait for the first respond to complete.
    let _ = first.await;

    // After the first respond finishes, the gate is cleared to Resume.
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
}

// ── sidecar/client.rs: debug! format string coverage ────────────────────
//
// The debug! macros in client.rs are multi-line, so their format string lines
// (88, 97, 98, 109) are NOT covered without a subscriber. We re-exercise the
// same paths as sidecar_client.rs but with a subscriber initialized.

/// Bash snippet to extract the numeric `id` from a single-line JSON-RPC
/// request read from stdin. Mirrors the pattern used by sidecar_client.rs.
const EXTRACT_ID: &str =
    r#"ID=$(printf '%s' "$LINE" | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p')"#;

/// Spawn `script` under bash with piped stdio and return `(child, client)`.
async fn spawn_client(script: &str) -> (tokio::process::Child, SidecarRpcClient) {
    let mut cmd = Command::new(support::sidecar_shell_path());
    cmd.arg("-c").arg(script);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let client = SidecarRpcClient::new(stdin, stdout);
    (child, client)
}

#[tokio::test]
async fn client_debug_paths_covered_with_subscriber() {
    init_subscriber();

    // Single test that exercises all three debug! paths:
    // 1. Unparseable line → debug!("parse_skipped") on line 88
    // 2. Notification (method, no id) → debug!("notification_skipped") on lines 97-98
    // 3. Mismatched id → debug!("id_mismatch") on line 109
    let script = format!(
        r#"IFS= read -r LINE
{EXTRACT_ID}
printf 'this is not json\n'
printf '{{"jsonrpc":"2.0","method":"task.event","params":{{"foo":1}}}}\n'
printf '{{"jsonrpc":"2.0","result":{{"wrong":true}},"id":99999}}\n'
printf '{{"jsonrpc":"2.0","result":{{"ok":true}},"id":%s}}\n' "$ID"
sleep 1"#
    );
    let (_child, mut client) = spawn_client(&script).await;
    let result = client
        .call_with_timeout(
            "adapter.health",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .expect("client should skip non-matching lines and return the matching response");
    assert_eq!(result["ok"], serde_json::json!(true));
}
