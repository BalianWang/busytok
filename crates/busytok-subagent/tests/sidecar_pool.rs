//! Integration tests for `WorkerPool` (Task 2 — Phase 3, updated Task 7).
//!
//! These tests verify the per-provider supervisor map, fixed env injection
//! (`OPENAI_API_KEY` / `OPENAI_BASE_URL`), two-phase bootstrap via
//! `set_responder_factory` (P1c fix), and async `remove_worker_and_kill` /
//! `update_provider_and_kill_old` (P1b fix). They do NOT spawn real sidecar
//! processes — they verify config/env construction and map lifecycle only.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use busytok_config::SubagentResourcePolicyConfig;
use busytok_subagent::pressure::{PressureGate, PressureResponder};
use busytok_subagent::sidecar::{
    PiSidecarSupervisor, ProviderRuntimeEntry, ResponderFactory, SidecarConfig, SidecarError,
    SidecarTaskExecutor, WorkerPool,
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
    }
}

fn deepseek_entry() -> ProviderRuntimeEntry {
    ProviderRuntimeEntry {
        provider_id: "deepseek".to_string(),
        api_key: "test-key".to_string(),
        base_url: "https://api.deepseek.com/v1".to_string(),
    }
}

fn openai_entry() -> ProviderRuntimeEntry {
    ProviderRuntimeEntry {
        provider_id: "openai".to_string(),
        api_key: "test-key".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
    }
}

/// Build a providers map that knows about `deepseek` and `openai`.
fn make_providers() -> HashMap<String, ProviderRuntimeEntry> {
    let mut m = HashMap::new();
    m.insert("deepseek".to_string(), deepseek_entry());
    m.insert("openai".to_string(), openai_entry());
    m
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
fn make_test_pool() -> (Arc<WorkerPool>, Arc<PressureGate>, Arc<SidecarTaskExecutor>) {
    let gate = Arc::new(PressureGate::new());
    let pool = Arc::new(WorkerPool::new(
        base_config(),
        None,
        make_providers(),
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
/// `OPENAI_API_KEY` + `OPENAI_BASE_URL` + the base config's
/// `BUSYTOK_SIDECAR_MAX_HOT_SESSIONS`. Sidecar only recognizes these fixed
/// env names (Task 7).
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
        env.get("OPENAI_BASE_URL"),
        Some(&"https://api.deepseek.com/v1".to_string()),
        "OPENAI_BASE_URL must be set to the provider base_url"
    );
    assert_eq!(
        env.get("BUSYTOK_SIDECAR_MAX_HOT_SESSIONS"),
        Some(&"3".to_string()),
        "base config env vars must be preserved"
    );
}

/// `ensure_worker_injects_credentials` for the openai provider — same
/// fixed env names (`OPENAI_API_KEY` + `OPENAI_BASE_URL`).
#[test]
fn ensure_worker_injects_credentials_openai() {
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
/// new ensure_worker creates a NEW supervisor. The provider entry is LEFT
/// IN PLACE by `remove_worker_and_kill` (Task 7 fix: only the worker is
/// removed; the provider entry stays so ensure_worker can re-spawn — this
/// matches the auth-fail kill recovery path in the executor, which does
/// NOT call provider_changed). To drop the provider entry as well (for
/// disabled / deleted providers), call `remove_provider_entry` separately.
#[tokio::test]
async fn remove_worker_then_ensure_creates_new_supervisor() {
    let (pool, _gate, _exec) = make_test_pool();

    let sup1 = pool.ensure_worker("deepseek").expect("first ensure");

    pool.remove_worker_and_kill("deepseek")
        .await
        .expect("remove_worker_and_kill");

    // After remove_worker_and_kill, ONLY the worker is gone — the provider
    // entry stays so ensure_worker can re-spawn (auth-fail recovery path).
    let sup2 = pool
        .ensure_worker("deepseek")
        .expect("ensure_worker must re-spawn after remove_worker_and_kill");
    assert!(
        !Arc::ptr_eq(&sup1, &sup2),
        "re-spawned supervisor must be a NEW Arc (not the killed one)"
    );
    assert_eq!(
        pool.worker_snapshots().await.len(),
        1,
        "pool must have one worker after re-spawn"
    );

    // After remove_provider_entry, ensure_worker fails with "unknown
    // provider" (the disabled/deleted-provider path).
    pool.remove_provider_entry("deepseek");
    match pool.ensure_worker("deepseek") {
        Err(SidecarError::Spawn(msg)) => {
            assert!(
                msg.contains("unknown provider"),
                "error should mention 'unknown provider', got: {msg}"
            );
        }
        Err(other) => panic!("expected SidecarError::Spawn, got {other:?}"),
        Ok(_) => panic!("unknown provider should error after remove_provider_entry"),
    }
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
/// providers map → error.
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

/// `shutdown_all` (graceful) clears the map and drains all supervisors.
/// Supervisors in this test are not started (no real child), so
/// `shutdown().await` is a no-op on `child=None` — but the map drain happens
/// regardless.
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

/// `set_responder_factory` called twice logs a warning and ignores the second
/// call (OnceLock rejects the overwrite).
#[test]
fn set_responder_factory_called_twice_is_ignored() {
    let (_pool, gate, _exec) = make_test_pool();
    // First call succeeds (set in make_test_pool, but that's internal).
    // We need to call it again on the same pool — but make_test_pool already
    // called set_responder_factory. So we build a fresh pool and call twice.
    let pool2 = Arc::new(WorkerPool::new(
        base_config(),
        None,
        make_providers(),
        Some(Arc::clone(&gate)),
        SubagentResourcePolicyConfig::default(),
    ));
    let executor2 = Arc::new(SidecarTaskExecutor::with_pool(Arc::clone(&pool2), None));
    let (factory1, _h1) = make_responder_factory(Arc::clone(&gate), Arc::downgrade(&executor2));
    let (factory2, _h2) = make_responder_factory(Arc::clone(&gate), Arc::downgrade(&executor2));
    pool2.set_responder_factory(factory1);
    // Second call — OnceLock::set returns Err, warning is logged, call is ignored.
    pool2.set_responder_factory(factory2);
    // Verify the first factory is still active by ensuring ensure_worker works.
    let sup = pool2
        .ensure_worker("deepseek")
        .expect("ensure_worker after double set");
    let env = &sup.config().env;
    assert_eq!(env.get("OPENAI_API_KEY"), Some(&"test-key".to_string()));
}

/// `remove_worker_and_kill` when no worker exists for the given provider_id
/// is a no-op (returns Ok, no panic).
#[tokio::test]
async fn remove_worker_and_kill_with_no_worker_is_noop() {
    let (pool, _gate, _exec) = make_test_pool();
    // Don't create any worker — remove should be a no-op.
    let result = pool.remove_worker_and_kill("nonexistent").await;
    assert!(
        result.is_ok(),
        "remove_worker_and_kill on nonexistent must be Ok"
    );
}

/// `supervisor_for_session` with no DB returns the first supervisor
/// (single-provider fallback for no-DB test paths).
#[tokio::test]
async fn supervisor_for_session_with_no_db_returns_first() {
    let (pool, _gate, _exec) = make_test_pool();
    pool.ensure_worker("deepseek").expect("ensure deepseek");
    pool.ensure_worker("openai").expect("ensure openai");

    // No DB was passed to the pool, so supervisor_for_session should
    // fall back to returning the first candidate.
    let result = pool.supervisor_for_session("any-session-id");
    assert!(result.is_some(), "must return Some fallback when no DB");
    let (pid, _sup) = result.unwrap();
    // The first inserted provider — HashMap iteration order is not
    // guaranteed, but one of the two must be returned.
    assert!(
        pid == "deepseek" || pid == "openai",
        "fallback must return a known provider, got {pid}"
    );
}

/// `update_provider_and_kill_old` (Task 7): updating a provider's entry
/// kills the old worker and lets `ensure_worker` re-spawn with the new
/// credentials/base_url.
#[tokio::test]
async fn update_provider_and_kill_old_respawns_with_new_credentials() {
    let (pool, _gate, _exec) = make_test_pool();
    let sup1 = pool.ensure_worker("deepseek").expect("first ensure");
    assert_eq!(
        sup1.config().env.get("OPENAI_BASE_URL"),
        Some(&"https://api.deepseek.com/v1".to_string())
    );

    // Update the provider entry with a new base_url + api_key.
    pool.update_provider_and_kill_old(ProviderRuntimeEntry {
        provider_id: "deepseek".to_string(),
        api_key: "new-key".to_string(),
        base_url: "https://api.updated.example.com/v1".to_string(),
    })
    .await
    .expect("update_provider_and_kill_old");

    // Re-spawn — must reflect the new credentials.
    let sup2 = pool.ensure_worker("deepseek").expect("re-spawn after update");
    assert!(
        !Arc::ptr_eq(&sup1, &sup2),
        "re-spawned supervisor must be a NEW Arc"
    );
    assert_eq!(
        sup2.config().env.get("OPENAI_API_KEY"),
        Some(&"new-key".to_string()),
        "re-spawned worker must use the updated api_key"
    );
    assert_eq!(
        sup2.config().env.get("OPENAI_BASE_URL"),
        Some(&"https://api.updated.example.com/v1".to_string()),
        "re-spawned worker must use the updated base_url"
    );
}
