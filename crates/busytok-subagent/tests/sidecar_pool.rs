//! Integration tests for `WorkerPool` (Task 2 — Phase 3).
//!
//! These tests verify the per-provider supervisor map, credential injection
//! via the injected `credential_reader` seam (P1 fix), two-phase bootstrap
//! via `set_responder_factory` (P1c fix), and async `remove_worker_and_kill`
//! (P1b fix). They do NOT spawn real sidecar processes — they verify
//! config/env construction and map lifecycle only.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use busytok_config::{ProviderConfig, ProviderKind, SubagentResourcePolicyConfig};
use busytok_subagent::pressure::{PressureGate, PressureResponder};
use busytok_subagent::sidecar::{
    CredentialReader, PiSidecarSupervisor, ProviderLookup, ResponderFactory, SidecarConfig,
    SidecarError, SidecarTaskExecutor, WorkerPool,
};

/// Build a base `SidecarConfig` for tests. Uses `/usr/bin/true` as the node
/// binary and `/dev/null` as the bundle — `ensure_worker` constructs the
/// supervisor but does NOT spawn it, so these never run.
fn base_config() -> SidecarConfig {
    SidecarConfig {
        node_binary: PathBuf::from("/usr/bin/true"),
        bundle_path: PathBuf::from("/dev/null"),
        env: {
            let mut m = HashMap::new();
            m.insert(
                "BUSYTOK_SIDECAR_MAX_HOT_SESSIONS".to_string(),
                "3".to_string(),
            );
            m
        },
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
    }
}

fn deepseek_provider() -> ProviderConfig {
    ProviderConfig {
        id: "deepseek".to_string(),
        name: "DeepSeek".to_string(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://api.deepseek.com/v1".to_string(),
        api_key_env_name: "DEEPSEEK_API_KEY".to_string(),
        base_url_env_name: Some("DEEPSEEK_BASE_URL".to_string()),
        models: vec!["deepseek-chat".to_string()],
        enabled: true,
    }
}

fn openai_provider() -> ProviderConfig {
    ProviderConfig {
        id: "openai".to_string(),
        name: "OpenAI".to_string(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://api.openai.com/v1".to_string(),
        api_key_env_name: "OPENAI_API_KEY".to_string(),
        base_url_env_name: None,
        models: vec!["gpt-4o".to_string()],
        enabled: true,
    }
}

/// Build a providers closure that knows about `deepseek` and `openai`.
fn make_providers() -> ProviderLookup {
    Arc::new(|pid: &str| -> Option<ProviderConfig> {
        match pid {
            "deepseek" => Some(deepseek_provider()),
            "openai" => Some(openai_provider()),
            "disabled-prov" => Some(ProviderConfig {
                enabled: false,
                ..openai_provider()
            }),
            _ => None,
        }
    })
}

/// Build a credential_reader closure that always returns `Ok(Some("test-key"))`.
fn canned_credential_reader() -> CredentialReader {
    Arc::new(|_pid: &str| Ok(Some("test-key".to_string())))
}

/// Build a responder_factory that constructs a real `PressureResponder` and
/// keeps a strong ref alive in a shared holder (so the Weak stored on the
/// supervisor stays upgradeable). The factory captures the shared
/// `Weak<SidecarTaskExecutor>` (Phase 3 Task 3: one pool-wide executor, not
/// a per-supervisor executor) and the pressure_gate so the responder shares
/// the pool's gate.
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
            // Keep a strong ref so the Weak stored on the supervisor stays
            // upgradeable. In production, Task 4's wiring holds the strong
            // ref in BusytokSupervisor; here we use a shared holder.
            holder_for_closure
                .lock()
                .unwrap()
                .push(Arc::clone(&responder));
            responder
        },
    );
    (factory, holder)
}

/// Build a fully-wired test pool with a shared PressureGate, the
/// responder_factory set, and a pool-wide `SidecarTaskExecutor`.
/// Returns `(pool, gate, executor)` — the executor must be kept alive by
/// the caller so the `Weak<SidecarTaskExecutor>` in each responder stays
/// upgradeable (two-phase bootstrap: pool → executor → factory → pool).
fn make_test_pool_with_creds(
    credential_reader: CredentialReader,
) -> (Arc<WorkerPool>, Arc<PressureGate>, Arc<SidecarTaskExecutor>) {
    let gate = Arc::new(PressureGate::new());
    let pool = Arc::new(WorkerPool::new(
        base_config(),
        None,
        make_providers(),
        credential_reader,
        Some(Arc::clone(&gate)),
        SubagentResourcePolicyConfig::default(),
    ));
    // Phase 2: construct executor (captures pool), then factory (captures
    // executor weak), then set factory on pool. After this, ensure_worker
    // can construct supervisors with responders wired.
    let executor = Arc::new(SidecarTaskExecutor::with_pool(Arc::clone(&pool), None));
    let (factory, _holder) = make_responder_factory(Arc::clone(&gate), Arc::downgrade(&executor));
    pool.set_responder_factory(factory);
    (pool, gate, executor)
}

/// Convenience wrapper: `make_test_pool_with_creds` with the canned
/// credential reader.
fn make_test_pool() -> (Arc<WorkerPool>, Arc<PressureGate>, Arc<SidecarTaskExecutor>) {
    make_test_pool_with_creds(canned_credential_reader())
}

// --- Step 1 test cases (per task-2-brief.md) ---

/// `ensure_worker_creates_supervisor_lazily` — first call creates; second
/// call returns the same Arc (lazy singleton per provider).
#[test]
fn ensure_worker_creates_supervisor_lazily() {
    let (pool, _gate, _exec) = make_test_pool();
    let sup1 = pool.ensure_worker("deepseek").expect("first ensure_worker");
    let sup2 = pool
        .ensure_worker("deepseek")
        .expect("second ensure_worker");
    assert!(
        Arc::ptr_eq(&sup1, &sup2),
        "second ensure_worker must return the same Arc"
    );
}

/// `ensure_worker_injects_credentials` — verify env map contains
/// `OPENAI_API_KEY` + the provider's `api_key_env_name` + `OPENAI_BASE_URL`
/// + the provider's `base_url_env_name` (when set) + the base config's
///   `BUSYTOK_SIDECAR_MAX_HOT_SESSIONS`.
#[test]
fn ensure_worker_injects_credentials() {
    let (pool, _gate, _exec) = make_test_pool();
    let sup = pool.ensure_worker("deepseek").expect("ensure_worker");
    let env = &sup.config().env;
    assert_eq!(
        env.get("OPENAI_API_KEY"),
        Some(&"test-key".to_string()),
        "OPENAI_API_KEY must be set to the credential value"
    );
    assert_eq!(
        env.get("DEEPSEEK_API_KEY"),
        Some(&"test-key".to_string()),
        "provider's api_key_env_name must be set to the credential value"
    );
    assert_eq!(
        env.get("OPENAI_BASE_URL"),
        Some(&"https://api.deepseek.com/v1".to_string()),
        "OPENAI_BASE_URL must be set to the provider base_url"
    );
    assert_eq!(
        env.get("DEEPSEEK_BASE_URL"),
        Some(&"https://api.deepseek.com/v1".to_string()),
        "provider's base_url_env_name must be set to the provider base_url"
    );
    assert_eq!(
        env.get("BUSYTOK_SIDECAR_MAX_HOT_SESSIONS"),
        Some(&"3".to_string()),
        "base config env vars must be preserved"
    );
    // Config metadata fields (observability — Phase 3 Task 1).
    assert_eq!(sup.config().provider_id, "deepseek");
    assert_eq!(sup.config().api_key_env_name, "DEEPSEEK_API_KEY");
    assert_eq!(sup.config().base_url_env_name, "DEEPSEEK_BASE_URL");
}

/// `ensure_worker_injects_credentials` for a provider with
/// `base_url_env_name = None` — should default to `OPENAI_BASE_URL`.
#[test]
fn ensure_worker_injects_credentials_default_base_url_env_name() {
    let (pool, _gate, _exec) = make_test_pool();
    let sup = pool.ensure_worker("openai").expect("ensure_worker");
    let env = &sup.config().env;
    assert_eq!(
        env.get("OPENAI_API_KEY"),
        Some(&"test-key".to_string()),
        "OPENAI_API_KEY must be set"
    );
    assert_eq!(
        env.get("OPENAI_BASE_URL"),
        Some(&"https://api.openai.com/v1".to_string()),
        "OPENAI_BASE_URL must be set to provider base_url"
    );
    // Config metadata: base_url_env_name defaults to OPENAI_BASE_URL when None.
    assert_eq!(sup.config().base_url_env_name, "OPENAI_BASE_URL");
}

/// `ensure_worker_sets_pressure_responder` — after ensure_worker, the
/// supervisor has a responder set (C6 fix — verify
/// `sup.pressure_responder().is_some()`).
#[test]
fn ensure_worker_sets_pressure_responder() {
    let (pool, _gate, _exec) = make_test_pool();
    let sup = pool.ensure_worker("deepseek").expect("ensure_worker");
    assert!(
        sup.pressure_responder().is_some(),
        "ensure_worker must set the pressure responder (C6 fix)"
    );
}

/// `remove_worker_then_ensure_creates_new_supervisor` — after remove, a
/// new ensure_worker creates a NEW supervisor (with fresh keychain read).
#[tokio::test]
async fn remove_worker_then_ensure_creates_new_supervisor() {
    // Use a counting credential_reader to verify fresh reads.
    let call_count: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let count_for_closure = Arc::clone(&call_count);
    let credential_reader: CredentialReader = Arc::new(move |_pid: &str| {
        *count_for_closure.lock().unwrap() += 1;
        Ok(Some("test-key".to_string()))
    });

    let (pool, _gate, _exec) = make_test_pool_with_creds(credential_reader);

    let sup1 = pool.ensure_worker("deepseek").expect("first ensure");
    assert_eq!(
        *call_count.lock().unwrap(),
        1,
        "first ensure reads credential"
    );

    pool.remove_worker_and_kill("deepseek")
        .await
        .expect("remove_worker_and_kill");

    let sup2 = pool.ensure_worker("deepseek").expect("second ensure");
    assert_eq!(
        *call_count.lock().unwrap(),
        2,
        "second ensure reads credential again (fresh keychain read)"
    );
    assert!(
        !Arc::ptr_eq(&sup1, &sup2),
        "second ensure must create a NEW supervisor (different Arc)"
    );
}

/// `worker_snapshots_returns_all_workers` — multiple providers → multiple
/// snapshots.
#[tokio::test]
async fn worker_snapshots_returns_all_workers() {
    let (pool, _gate, _exec) = make_test_pool();
    pool.ensure_worker("deepseek").expect("ensure deepseek");
    pool.ensure_worker("openai").expect("ensure openai");

    let mut snaps = pool.worker_snapshots().await;
    snaps.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(snaps.len(), 2, "expected 2 snapshots (deepseek + openai)");
    let pids: Vec<&str> = snaps.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(pids, vec!["deepseek", "openai"]);
}

/// `ensure_worker_fails_for_unknown_provider` — provider_id not in
/// providers → error.
#[test]
fn ensure_worker_fails_for_unknown_provider() {
    let (pool, _gate, _exec) = make_test_pool();
    match pool.ensure_worker("nonexistent") {
        Err(SidecarError::Spawn(msg)) => {
            assert!(
                msg.contains("unknown provider"),
                "error should mention 'unknown provider', got: {msg}"
            );
        }
        Err(other) => panic!("expected SidecarError::Spawn, got {other:?}"),
        Ok(_) => panic!("unknown provider should error"),
    }
}

/// `ensure_worker_fails_for_disabled_provider` — provider.enabled = false
/// → error.
#[test]
fn ensure_worker_fails_for_disabled_provider() {
    let (pool, _gate, _exec) = make_test_pool();
    match pool.ensure_worker("disabled-prov") {
        Err(SidecarError::Spawn(msg)) => {
            assert!(
                msg.contains("disabled"),
                "error should mention 'disabled', got: {msg}"
            );
        }
        Err(other) => panic!("expected SidecarError::Spawn, got {other:?}"),
        Ok(_) => panic!("disabled provider should error"),
    }
}

/// `ensure_worker_fails_for_missing_api_key` — keyring has no key → error
/// (`Ok(None)` from credential_reader).
#[test]
fn ensure_worker_fails_for_missing_api_key() {
    let credential_reader: CredentialReader = Arc::new(|_| Ok(None));
    let (pool, _gate, _exec) = make_test_pool_with_creds(credential_reader);

    match pool.ensure_worker("deepseek") {
        Err(SidecarError::Spawn(msg)) => {
            assert!(
                msg.contains("no API key"),
                "error should mention 'no API key', got: {msg}"
            );
        }
        Err(other) => panic!("expected SidecarError::Spawn, got {other:?}"),
        Ok(_) => panic!("missing API key should error"),
    }
}

/// `ensure_worker_fails_for_keychain_error` — `get_key` returns `Err(...)`
/// → `SidecarError::Spawn("keychain read failed: ...")`.
#[test]
fn ensure_worker_fails_for_keychain_error() {
    let credential_reader: CredentialReader =
        Arc::new(|_| Err(anyhow::anyhow!("simulated keychain failure")));
    let (pool, _gate, _exec) = make_test_pool_with_creds(credential_reader);

    match pool.ensure_worker("deepseek") {
        Err(SidecarError::Spawn(msg)) => {
            assert!(
                msg.contains("keychain read failed"),
                "error should mention 'keychain read failed', got: {msg}"
            );
            assert!(
                msg.contains("simulated keychain failure"),
                "error should include the underlying cause, got: {msg}"
            );
        }
        Err(other) => panic!("expected SidecarError::Spawn, got {other:?}"),
        Ok(_) => panic!("keychain error should propagate"),
    }
}

/// `ensure_worker_concurrent_same_provider_no_duplicate` — two concurrent
/// calls for the same provider → same Arc (no leak / no duplicate
/// supervisor). Uses `tokio::task::spawn_blocking` + `join!` to exercise
/// actual concurrency (ensure_worker is synchronous).
#[tokio::test]
async fn ensure_worker_concurrent_same_provider_no_duplicate() {
    let (pool, _gate, _exec) = make_test_pool();
    let p1 = Arc::clone(&pool);
    let p2 = Arc::clone(&pool);

    let h1 = tokio::task::spawn_blocking(move || p1.ensure_worker("deepseek"));
    let h2 = tokio::task::spawn_blocking(move || p2.ensure_worker("deepseek"));
    let (r1, r2) = tokio::join!(h1, h2);

    let sup1 = r1.expect("task 1 panicked").expect("ensure_worker 1");
    let sup2 = r2.expect("task 2 panicked").expect("ensure_worker 2");
    assert!(
        Arc::ptr_eq(&sup1, &sup2),
        "concurrent ensure_worker calls must return the same Arc (no duplicate supervisor)"
    );
}

/// `ensure_worker_panics_if_responder_factory_unset` — P1c fail-fast: if
/// `set_responder_factory` was never called, `ensure_worker` panics with
/// a clear bootstrap-incomplete message.
#[test]
#[should_panic(expected = "responder_factory not set")]
fn ensure_worker_panics_if_responder_factory_unset() {
    let gate = Arc::new(PressureGate::new());
    // Note: set_responder_factory NOT called — bootstrap incomplete.
    let pool = WorkerPool::new(
        base_config(),
        None,
        make_providers(),
        canned_credential_reader(),
        Some(gate),
        SubagentResourcePolicyConfig::default(),
    );
    // This should panic (P1c fail-fast).
    let _ = pool.ensure_worker("deepseek");
}

/// `for_each_supervisor` iterates over all workers (I5 fix — for
/// `evict_lru` iteration across all providers in Task 3).
#[test]
fn for_each_supervisor_iterates_all_workers() {
    let (pool, _gate, _exec) = make_test_pool();
    pool.ensure_worker("deepseek").expect("ensure deepseek");
    pool.ensure_worker("openai").expect("ensure openai");

    // `for_each_supervisor` takes `impl Fn` (no mutable captures), so use
    // a Mutex to collect pids. The production use case (Task 3's
    // `evict_lru`) calls async methods on each supervisor and doesn't need
    // mutable capture.
    let pids: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let pids_capture = Arc::clone(&pids);
    pool.for_each_supervisor(|pid, _sup| {
        pids_capture.lock().unwrap().push(pid.to_string());
    });
    let mut pids = pids.lock().unwrap().clone();
    pids.sort();
    assert_eq!(pids, vec!["deepseek".to_string(), "openai".to_string()]);
}

/// `shutdown_all` clears the map and force-kills all supervisors (best
/// effort — supervisors in this test are not started, so force_kill is a
/// no-op on a None child).
#[tokio::test]
async fn shutdown_all_clears_map() {
    let (pool, _gate, _exec) = make_test_pool();
    pool.ensure_worker("deepseek").expect("ensure deepseek");
    pool.ensure_worker("openai").expect("ensure openai");
    assert_eq!(
        pool.worker_snapshots().await.len(),
        2,
        "map has 2 workers before shutdown"
    );

    pool.shutdown_all().await;

    assert_eq!(
        pool.worker_snapshots().await.len(),
        0,
        "map must be empty after shutdown_all"
    );
}
