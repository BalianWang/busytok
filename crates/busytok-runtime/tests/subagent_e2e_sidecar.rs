#![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
//! End-to-end subagent lifecycle through the real Pi sidecar subprocess.
//!
//! Constructs a `BusytokSupervisor` with `pi_sidecar.enabled = true` and
//! injects mock-sidecar.sh as the sidecar bundle via a pre-resolved
//! `SidecarConfig` passed to `BusytokSupervisor::new_with_sidecar_config`
//! (no env-var escape hatch in production code). Exercises the full
//! delegate → list → show → hibernate → delete lifecycle through the
//! `RuntimeControl` dispatch path — the same path the control server uses.
//!
//! Regression value: catches integration bugs that unit tests miss —
//! supervisor constructs the sidecar incorrectly, settings don't propagate,
//! the shutdown sequence doesn't cleanly stop the sidecar, etc.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use busytok_config::{BusytokPaths, BusytokSettings, ProviderKind};
use busytok_control::dispatch::RuntimeControl;
use busytok_protocol::dto::*;
use busytok_runtime::BusytokSupervisor;
use busytok_subagent::sidecar::SidecarConfig;
use serial_test::serial;

/// Wait for a task to reach a terminal status (completed/failed/cancelled).
/// Polls the DB every 5ms with a 5-second timeout. Bridges the async
/// execution gap: `delegate()` returns `Running` immediately and spawns
/// execution in the background (Bug #1/#2 fix).
async fn await_task_done(
    m: &std::sync::Arc<busytok_subagent::SubagentManager>,
    task_id: &str,
) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(status) = m.task_status(task_id).unwrap() {
            if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
                return status;
            }
        }
        if std::time::Instant::now() > deadline {
            panic!("task {task_id} did not reach terminal status within 5s");
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

/// Path to the mock-sidecar.sh fixture, resolved relative to
/// CARGO_MANIFEST_DIR (crates/busytok-runtime). The fixture lives in
/// busytok-subagent/tests/fixtures/.
fn mock_sidecar_path() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(format!(
        "{manifest}/../busytok-subagent/tests/fixtures/mock-sidecar.sh"
    ))
}

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

fn mock_sidecar_bundle_path() -> PathBuf {
    let path = mock_sidecar_path();
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

/// In-memory description of a provider to seed into SQL for sidecar-wired
/// tests. Replaces the old `settings.providers.push(ProviderConfig{...})`
/// pattern after Task 10 removed `BusytokSettings.providers`.
#[derive(Clone)]
struct TestProviderSeed {
    id: &'static str,
    name: &'static str,
    base_url: &'static str,
    api_key_env_name: &'static str,
    models: &'static [&'static str],
    enabled: bool,
}

const TEST_PROVIDER_SEED: TestProviderSeed = TestProviderSeed {
    id: "test-provider",
    name: "Test Provider",
    base_url: "https://api.test-provider.example.com/v1",
    api_key_env_name: "TEST_API_KEY",
    models: &["test-model"],
    enabled: true,
};

/// Seed a list of `TestProviderSeed` into SQL. Mirrors the old
/// `seed_providers_from_settings` flow but reads from a `[TestProviderSeed]`
/// instead of `settings.providers` (which no longer exists). Each provider's
/// models are also seeded so the delegate's whitelist validation passes.
/// Uses `INSERT OR REPLACE` for providers so re-seeding overwrites cleanly.
/// Models use `INSERT OR REPLACE` on the (provider_id, model_id) unique
/// constraint.
fn seed_test_providers(db: &busytok_store::Database, seeds: &[TestProviderSeed]) {
    use rusqlite::params;
    let now = busytok_domain::now_ms();
    for p in seeds {
        let api_key = std::env::var(format!("BUSYTOK_{}", p.api_key_env_name))
            .or_else(|_| std::env::var(p.api_key_env_name))
            .ok();
        db.conn()
            .execute(
                "INSERT OR REPLACE INTO providers \
                 (id, name, provider_kind, base_url, enabled, api_key, created_at_ms, updated_at_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![
                    p.id,
                    p.name,
                    serde_json::to_string(&ProviderKind::OpenAiCompatible).unwrap(),
                    p.base_url,
                    p.enabled as i64,
                    api_key,
                    now,
                ],
            )
            .expect("seed provider to SQL");
        for model_id in p.models {
            let model_pk = format!("seed-{}-{}", p.id, model_id);
            // I-2: `execute_task` fails fast on NULL context_window / max_tokens.
            // The migration 0007 backfill only runs at migration time, so test
            // seeds must supply both values explicitly. Defaults match the
            // manager.rs test seeds (128000 context, 16384 max_tokens).
            db.conn()
                .execute(
                    "INSERT OR REPLACE INTO models \
                     (id, provider_id, model_id, enabled, created_at_ms, updated_at_ms, \
                      display_name, reasoning, context_window, max_tokens) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        model_pk,
                        p.id,
                        model_id,
                        1_i64,
                        now,
                        model_id,    // display_name falls back to model_id
                        0_i64,       // reasoning = false
                        128_000_i64, // context_window
                        16_384_i64,  // max_tokens
                    ],
                )
                .expect("seed model to SQL");
        }
    }
}

/// Settings with pi_sidecar enabled, using system bash as the "node"
/// binary (mock-sidecar.sh is a bash script, not a Node bundle).
/// Configures a test provider so the WorkerPool can route delegate calls.
/// The API key is injected via `BUSYTOK_TEST_API_KEY` env var (checked by
/// `construct_sidecar`'s credential reader).
///
/// Returns the settings plus the list of `TestProviderSeed`s that must be
/// seeded into SQL before constructing the supervisor.
///
/// NOTE (Task 4): profiles no longer carry `provider_id` / `model` fields.
/// Provider/model binding is now per-subagent via SQL catalog
/// (`subagent.bound_provider_id` / `bound_model_id`). The DTO
/// `SubagentDelegateRequestDto` now carries `bound_provider_id` /
/// `bound_model_id`, so e2e tests that create new subagents pass both
/// fields (sourced from `TEST_PROVIDER_SEED`) and exercise the real
/// sidecar spawn path.
fn make_sidecar_settings() -> (BusytokSettings, Vec<TestProviderSeed>) {
    let mut settings = BusytokSettings::default();
    settings.subagent.pi_sidecar.enabled = true;
    settings.subagent.pi_sidecar.node_runtime = "system".to_string();
    settings.subagent.pi_sidecar.system_node_path =
        sidecar_shell_path().to_string_lossy().to_string();
    settings.subagent.pi_sidecar.idle_exit_seconds = 300;
    settings.subagent.pi_sidecar.task_timeout_seconds = 30;

    // Inject a fake API key via env var so `construct_sidecar`'s credential
    // reader returns it.
    std::env::set_var("BUSYTOK_TEST_API_KEY", "test-key-for-e2e");

    (settings, vec![TEST_PROVIDER_SEED.clone()])
}

/// Build a `SidecarConfig` that points at mock-sidecar.sh. Mirrors the
/// fields `resolve_sidecar_config` would produce for the test settings,
/// but with `bundle_path` set to the mock fixture (no env var needed).
fn make_sidecar_config() -> SidecarConfig {
    SidecarConfig {
        node_binary: sidecar_shell_path(),
        bundle_path: mock_sidecar_bundle_path(),
        env: HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(30),
        task_timeout: Duration::from_secs(30),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
    }
}

/// Construct a supervisor that loads sidecar-enabled settings from the
/// config file in `tmp` and injects the mock sidecar bundle via
/// `new_with_sidecar_config`. The settings file must still have
/// `pi_sidecar.enabled = true` so the sidecar wiring path is taken.
/// Providers from `seeds` are seeded into SQL before construction so
/// `construct_sidecar` can build the `WorkerPool`'s provider map
/// (Task 7: pool reads from SQL, not TOML).
fn make_sidecar_supervisor(
    db: busytok_store::Database,
    tmp: &tempfile::TempDir,
    settings: BusytokSettings,
    seeds: Vec<TestProviderSeed>,
) -> BusytokSupervisor {
    seed_test_providers(&db, &seeds);
    let paths = BusytokPaths::for_test(tmp.path());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");
    BusytokSupervisor::new_with_sidecar_config(db, paths, make_sidecar_config())
}

#[tokio::test]
#[serial]
async fn sidecar_e2e_delegate_list_show_hibernate_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let (settings, seeds) = make_sidecar_settings();
    let supervisor = make_sidecar_supervisor(db, &tmp, settings, seeds);

    // 1. delegate — must go through the sidecar subprocess.
    //    adapter_session_id being set proves the sidecar was used
    //    (the mock executor returns None for this field).
    let delegate_resp = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "e2e-reviewer".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "find the bug".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: Some("test-provider".to_string()),
            bound_model_id: Some("test-model".to_string()),
            reuse_policy: None,
        })
        .await
        .unwrap();

    let sub_id = delegate_resp.subagent_id.clone();
    assert_eq!(delegate_resp.status, "running");
    // Wait for async completion (Bug #1/#2 fix: delegate returns Running
    // immediately and execute_and_persist runs in the background).
    let final_status =
        await_task_done(&supervisor.subagent_manager(), &delegate_resp.task_id).await;
    assert_eq!(final_status, "completed");
    // adapter_session_id is now in the DB hot binding, not the delegate
    // response — query the hot binding to prove the sidecar was used
    // (the mock executor returns None for this field on the response).
    let binding = {
        let db_guard = supervisor.db_handle().lock().unwrap();
        db_guard
            .subagent_hot_binding(&delegate_resp.subagent_id, "pi")
            .unwrap()
    };
    assert!(
        binding.is_some(),
        "hot binding should exist — proves the sidecar was used"
    );
    let binding = binding.unwrap();
    assert!(
        binding
            .adapter_session_id
            .as_deref()
            .unwrap_or("")
            .starts_with("pi_sess_mock_"),
        "adapter_session_id should come from mock-sidecar.sh, got: {:?}",
        binding.adapter_session_id
    );

    // 2. list — the just-created subagent must appear.
    let list = supervisor
        .subagent_list(SubagentListRequestDto {
            status: None,
            project: None,
            include_deleted: Some(false),
        })
        .await
        .unwrap();
    assert!(
        list.subagents.iter().any(|s| s.id == sub_id),
        "delegated subagent must appear in list"
    );

    // 3. show by UUID — verify detail.
    let shown = supervisor
        .subagent_show(SubagentResolveRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
        })
        .await
        .unwrap();
    assert_eq!(shown.name, "e2e-reviewer");
    assert_eq!(shown.status, "hot", "subagent should be hot after delegate");

    // 4. hibernate — releases the hot session.
    supervisor
        .subagent_hibernate(SubagentResolveRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
        })
        .await
        .unwrap();

    // After hibernate, status should transition away from hot.
    let after_hibernate = supervisor
        .subagent_show(SubagentResolveRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
        })
        .await
        .unwrap();
    assert_ne!(
        after_hibernate.status, "hot",
        "subagent should not be hot after hibernate"
    );

    // 5. soft delete — removes from active list.
    supervisor
        .subagent_delete(SubagentDeleteRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
            hard: Some(false),
        })
        .await
        .unwrap();
    let after_list = supervisor
        .subagent_list(SubagentListRequestDto {
            status: None,
            project: None,
            include_deleted: Some(false),
        })
        .await
        .unwrap();
    assert!(
        after_list.subagents.iter().all(|s| s.id != sub_id),
        "soft-deleted subagent must not appear in active list"
    );

    // 6. verify resource events were written (sidecar_start at minimum).
    //    Scoped block ensures the MutexGuard is dropped before the await
    //    points in step 7 (clippy::await_holding_lock).
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        let events = db_guard.subagent_list_resource_events(None, 100).unwrap();
        assert!(
            events.iter().any(|e| e.event_type == "sidecar_start"),
            "sidecar_start resource event must be written"
        );
    }

    // 7. graceful shutdown — kills the sidecar subprocess and reconciles
    //    DB state (releases hot bindings, rolls back logical status).
    supervisor.shutdown_sidecar().await;

    // Post-shutdown DB assertion (spec §3.3 end-to-end): after graceful
    // shutdown, no hot bindings may remain for the harness, and the
    // previously-hot subagent's status must NOT be 'hot'. This guards the
    // shutdown reconciliation added to `shutdown_internal` — a dead sidecar
    // process must never leave a `status='hot'` row or an `is_hot=1`
    // binding in the store.
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        let hot_count: i64 = db_guard
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM subagent_harness_bindings \
                 WHERE is_hot = 1 AND harness = 'pi'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            hot_count, 0,
            "no hot bindings should exist for the pi harness after shutdown"
        );
        let sub = db_guard.subagent_get_logical(&sub_id).unwrap();
        let sub = sub.expect("logical subagent row must still exist after shutdown");
        assert_ne!(
            sub.status, "hot",
            "previously-hot subagent must not be 'hot' after shutdown reconciliation"
        );
    }

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn sidecar_e2e_delegate_then_shutdown_releases_hot_binding() {
    // Delegate creates a hot binding (is_hot=1, status='hot').
    // Graceful shutdown must release it (is_hot=0, status='closed')
    // and roll back logical status to warm/cold — WITHOUT hibernate
    // or delete first. This is the only test that genuinely exercises
    // the shutdown reconciliation path: the sibling lifecycle test
    // hibernates + deletes before shutdown, which drains hot bindings
    // first and makes `release_hot_bindings_for_shutdown` hit its
    // early-return (vacuous assertion).
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let (settings, seeds) = make_sidecar_settings();
    let supervisor = make_sidecar_supervisor(db, &tmp, settings, seeds);

    // 1. delegate — must go through the sidecar subprocess.
    let delegate_resp = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "shutdown-test".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "find the bug".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: Some("test-provider".to_string()),
            bound_model_id: Some("test-model".to_string()),
            reuse_policy: None,
        })
        .await
        .unwrap();

    let sub_id = delegate_resp.subagent_id.clone();
    assert_eq!(delegate_resp.status, "running");
    // Wait for async completion (Bug #1/#2 fix: delegate returns Running
    // immediately and execute_and_persist runs in the background).
    let final_status =
        await_task_done(&supervisor.subagent_manager(), &delegate_resp.task_id).await;
    assert_eq!(final_status, "completed");
    // adapter_session_id is now in the DB hot binding, not the delegate
    // response — query the hot binding to prove the sidecar was used
    // (the mock executor returns None for this field on the response).
    let binding = {
        let db_guard = supervisor.db_handle().lock().unwrap();
        db_guard
            .subagent_hot_binding(&delegate_resp.subagent_id, "pi")
            .unwrap()
    };
    assert!(
        binding.is_some(),
        "hot binding should exist — proves the sidecar was used"
    );
    let binding = binding.unwrap();
    assert!(
        binding
            .adapter_session_id
            .as_deref()
            .unwrap_or("")
            .starts_with("pi_sess_mock_"),
        "adapter_session_id should come from mock-sidecar.sh, got: {:?}",
        binding.adapter_session_id
    );

    // 2. verify the subagent is hot (proves a hot binding exists pre-shutdown).
    let shown = supervisor
        .subagent_show(SubagentResolveRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
        })
        .await
        .unwrap();
    assert_eq!(
        shown.status, "hot",
        "subagent must be hot after delegate (precondition for the shutdown assertion)"
    );

    // 3. graceful shutdown — WITHOUT hibernate or delete first. This is the
    //    only path that leaves a hot binding in place for
    //    `release_hot_bindings_for_shutdown` to reconcile.
    supervisor.shutdown_sidecar().await;

    // 4. post-shutdown DB assertions (spec §3.3 end-to-end). Scoped block
    //    ensures the MutexGuard is dropped before the cleanup `.await`
    //    (clippy::await_holding_lock).
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        // (a) No hot bindings may remain for the pi harness.
        let hot_count: i64 = db_guard
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM subagent_harness_bindings \
                 WHERE is_hot = 1 AND harness = ?1",
                rusqlite::params!["pi"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            hot_count, 0,
            "no hot bindings should exist for the pi harness after shutdown"
        );
        // (b) The previously-hot subagent's status must roll back to warm/cold.
        //     STRICT assertion (not `!= "hot"`) — a dead sidecar process must
        //     never leave a `status='hot'` row, and the rollback target is
        //     specifically warm (memory exists) or cold (no memory).
        let sub = db_guard
            .subagent_get_logical(&sub_id)
            .unwrap()
            .expect("logical subagent row must still exist after shutdown");
        assert!(
            sub.status == "warm" || sub.status == "cold",
            "previously-hot subagent must roll back to 'warm' or 'cold' after shutdown, got: {:?}",
            sub.status
        );
    }

    supervisor.shutdown_writer().await.unwrap();
}

// --- context built from memory: e2e (Plan 4 Task 6) ---
//
// Verifies the full memory ↔ context loop end-to-end through the real
// supervisor + mock sidecar subprocess:
//   1. First delegate — the mock returns `result.memory_update` (because
//      `BUSYTOK_MOCK_MEMORY_UPDATE=1` is injected into SidecarConfig.env).
//      The manager merges it: `hot_summary` becomes the mock's
//      `current_state_summary`; `key_files`/`decisions`/`open_questions`
//      are merged into the memory row.
//   2. Second delegate — the ContextBuilder reads the merged memory and
//      assembles a `compact_context` that includes the `hot_summary` text.
//      The mock sidecar echoes `context.compact_context` back in
//      `task_summary`, so the response summary MUST contain the first
//      delegate's `hot_summary` text. This proves the ContextBuilder read
//      the merged memory and the manager sent it to the sidecar.

/// Build a SidecarConfig that injects BUSYTOK_MOCK_MEMORY_UPDATE=1 so the
/// mock sidecar emits result.memory_update and echoes compact_context.
fn make_sidecar_config_with_memory_update() -> SidecarConfig {
    let mut cfg = make_sidecar_config();
    cfg.env
        .insert("BUSYTOK_MOCK_MEMORY_UPDATE".into(), "1".into());
    cfg
}

#[tokio::test]
#[serial]
async fn sidecar_e2e_delegate_merges_memory_and_builds_context_from_memory() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let (settings, seeds) = make_sidecar_settings();
    let sidecar_cfg = make_sidecar_config_with_memory_update();
    let paths = BusytokPaths::for_test(tmp.path());
    seed_test_providers(&db, &seeds);
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");
    let supervisor = BusytokSupervisor::new_with_sidecar_config(db, paths, sidecar_cfg);

    // First delegate — mock returns memory_update with current_state_summary.
    let resp1 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "auth-investigator".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/review-cheap".to_string(),
            intent: Some("Study auth".to_string()),
            prompt: "Check refresh logic".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: Some("cli".to_string()),
            source_session_id: None,
            bound_provider_id: Some("test-provider".to_string()),
            bound_model_id: Some("test-model".to_string()),
            reuse_policy: None,
        })
        .await
        .unwrap();
    let sub_id = resp1.subagent_id.clone();
    assert_eq!(resp1.status, "running");
    // Wait for async completion (Bug #1/#2 fix: delegate returns Running
    // immediately and execute_and_persist runs in the background).
    let final_status1 = await_task_done(&supervisor.subagent_manager(), &resp1.task_id).await;
    assert_eq!(final_status1, "completed");

    // Assert memory merged after first delegate.
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        let mem = db_guard
            .subagent_get_memory(&sub_id)
            .unwrap()
            .expect("memory row must exist after first delegate");
        assert_eq!(
            mem.hot_summary.as_deref(),
            Some("Investigated context; produced memory update."),
            "hot_summary from memory_update.current_state_summary"
        );
        let files: Vec<serde_json::Value> =
            serde_json::from_str(mem.key_files_json.as_deref().unwrap()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["path"], "src/auth/token.ts");
        let decisions: Vec<String> =
            serde_json::from_str(mem.decisions_json.as_deref().unwrap()).unwrap();
        assert_eq!(decisions, vec!["Focus on read-only analysis".to_string()]);
    }

    // Second delegate — the mock sidecar echoes context.compact_context back
    // in task_summary. If the ContextBuilder read the merged memory, the
    // echoed summary MUST contain the first delegate's hot_summary text.
    let resp2 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "auth-investigator".to_string(),
            subagent_id: Some(sub_id.clone()),
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/review-cheap".to_string(),
            intent: Some("Study auth".to_string()),
            prompt: "Continue investigation".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: Some("cli".to_string()),
            source_session_id: None,
            bound_provider_id: None,
            bound_model_id: None,
            reuse_policy: None,
        })
        .await
        .unwrap();
    assert_eq!(resp2.status, "running");
    // Wait for async completion (Bug #1/#2 fix: delegate returns Running
    // immediately and execute_and_persist runs in the background).
    let final_status2 = await_task_done(&supervisor.subagent_manager(), &resp2.task_id).await;
    assert_eq!(final_status2, "completed");
    // The summary now lives on the task row (the delegate response is
    // immediate and does not carry the executor's output). Query the
    // task row to assert the echoed compact_context.
    let task_row2 = {
        let db_guard = supervisor.db_handle().lock().unwrap();
        db_guard
            .subagent_get_task(&resp2.task_id)
            .unwrap()
            .expect("task row must exist after second delegate completes")
    };
    assert!(
        task_row2
            .result_summary
            .as_deref()
            .unwrap_or("")
            .contains("Investigated context; produced memory update."),
        "second delegate's result_summary echoes compact_context which must contain the first delegate's hot_summary; got: {:?}",
        task_row2.result_summary
    );

    // After the second delegate, key_files should still have 1 entry (deduped).
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        let mem = db_guard
            .subagent_get_memory(&sub_id)
            .unwrap()
            .expect("memory row must still exist after second delegate");
        let files: Vec<serde_json::Value> =
            serde_json::from_str(mem.key_files_json.as_deref().unwrap()).unwrap();
        assert_eq!(files.len(), 1, "key_files deduped across delegates");
    }

    supervisor.shutdown_sidecar().await;
    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn sidecar_e2e_misconfigured_sidecar_fails_delegate_not_silently_mock() {
    // P1-2 regression: when pi_sidecar.enabled=true but the sidecar config
    // cannot be resolved (e.g. runtime_dir points at a nonexistent bundle),
    // the supervisor must NOT silently fall back to MockTaskExecutor —
    // that would mask a deployment misconfiguration as "functional".
    // Instead, delegate must fail with a clear error, and
    // sidecar_init_error() must return Some(reason).
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());

    // Task 5: seed a valid provider+model into SQL so the `execute_task`
    // validation chain (bound provider exists + enabled + has api_key +
    // bound model exists + enabled) passes. Without this, delegate would
    // fail earlier with `subagent.validation_error: bound provider not
    // found`, masking the sidecar_spawn_failed error this test asserts.
    // The supervisor constructor uses `FailingTaskExecutor` (not the real
    // sidecar) because the sidecar config resolve failed, so the seeded
    // provider is never actually contacted — it just satisfies validation.
    std::env::set_var("BUSYTOK_TEST_API_KEY", "test-key-for-e2e");
    seed_test_providers(&db, &[TEST_PROVIDER_SEED.clone()]);

    // Settings with enabled=true but a runtime_dir that has no bundle —
    // resolve_sidecar_config will fail with "sidecar bundle not found".
    let mut settings = BusytokSettings::default();
    settings.subagent.pi_sidecar.enabled = true;
    settings.subagent.pi_sidecar.node_runtime = "system".to_string();
    settings.subagent.pi_sidecar.system_node_path = "bash".to_string();
    settings.subagent.pi_sidecar.runtime_dir = Some(tmp.path().to_string_lossy().to_string());

    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    let supervisor = BusytokSupervisor::with_adapters_and_settings(
        db,
        paths,
        vec![], // no adapters needed for this test
        settings,
    );

    // sidecar_init_error must be populated — the config resolve failure
    // must be surfaced, not silently swallowed.
    assert!(
        supervisor.sidecar_init_error().is_some(),
        "sidecar_init_error must be set when config resolve fails"
    );
    let init_err = supervisor.sidecar_init_error().unwrap();
    assert!(
        init_err.contains("bundle") || init_err.contains("not found"),
        "error should mention the bundle issue, got: {init_err}"
    );

    // delegate MUST fail — never return mock output. Bug #1/#2 fix:
    // `delegate()` returns `Running` immediately and `execute_and_persist`
    // runs in the background. The FailingTaskExecutor (injected because
    // sidecar config resolve failed) returns `SubagentError::SidecarSpawn`,
    // which `execute_and_persist` catches and persists as `status="failed"`
    // with the error string in the task row. The test asserts the final
    // status is "failed" (NOT "completed" — which would indicate a silent
    // Mock fallback) and that the error string carries the Display of
    // `SubagentError::SidecarSpawn` (proving the FailingTaskExecutor's
    // error downcasts through the manager — not a generic store_error).
    // Task 5: bound_provider_id / bound_model_id are required on the create
    // path (sourced from TEST_PROVIDER_SEED) so the validation chain in
    // `execute_task` passes and the FailingTaskExecutor's SidecarSpawn
    // error surfaces.
    let resp = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "misconfigured-test".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "find the bug".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: Some(TEST_PROVIDER_SEED.id.to_string()),
            bound_model_id: Some(TEST_PROVIDER_SEED.models[0].to_string()),
            reuse_policy: None,
        })
        .await
        .expect("delegate must succeed at the RPC layer (returns Running immediately)");

    // delegate() returns Running immediately — the failure happens in the
    // background. Wait for the terminal status and assert it is "failed"
    // (NOT "completed" — a "completed" status would mean a silent Mock
    // fallback masked the misconfiguration).
    assert_eq!(resp.status, "running");
    let final_status =
        await_task_done(&supervisor.subagent_manager(), &resp.task_id).await;
    assert_eq!(
        final_status, "failed",
        "task must reach 'failed' status — got success (silent mock fallback would have completed)"
    );

    // The task row's `error` field carries the Display of the
    // `SubagentError::SidecarSpawn` returned by FailingTaskExecutor. The
    // Display prefix is "sidecar spawn failed" — NOT a generic store
    // error — proving the FailingTaskExecutor's error downcasts to
    // SubagentError::SidecarSpawn through the manager.
    let task_row = {
        let db_guard = supervisor.db_handle().lock().unwrap();
        db_guard
            .subagent_get_task(&resp.task_id)
            .unwrap()
            .expect("task row must exist after delegate")
    };
    let err_str = task_row.error.as_deref().unwrap_or("");
    assert!(
        err_str.contains("sidecar spawn failed"),
        "task error must carry the Display of SubagentError::SidecarSpawn \
         (not a generic store_error), got: {err_str:?}"
    );

    supervisor.shutdown_writer().await.unwrap();
}

// --- hot session pool: e2e eviction (Plan 3 Task 7) ---
//
// Exercises the full eviction path end-to-end through the runtime supervisor:
// delegate fills the hot pool → second delegate (different subagent) triggers
// HOT_SESSION_LIMIT_REACHED → executor drives prepare_hibernate → persist →
// close → retries turn_auto. Verifies the evicted subagent lands at 'warm'
// (memory written) and a `session_hibernate` resource event is recorded.

#[tokio::test]
#[serial]
async fn sidecar_e2e_eviction_releases_lru_and_retries() {
    // max_hot_sessions=1: first delegate fills the pool. Second delegate
    // (different subagent) triggers eviction: executor catches
    // HOT_SESSION_LIMIT_REACHED, drives prepare_hibernate → persist → close,
    // then retries turn_auto. The evicted subagent must end up 'warm'
    // (memory written), the new subagent 'hot'.
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.max_hot_sessions = 1;
    seed_test_providers(&db, &seeds);
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    let mut cfg = make_sidecar_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    let supervisor = BusytokSupervisor::new_with_sidecar_config(db, paths, cfg);

    // 1. First delegate — fills the pool
    let resp1 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "evicted".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "do 1".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: Some("test-provider".to_string()),
            bound_model_id: Some("test-model".to_string()),
            reuse_policy: None,
        })
        .await
        .unwrap();
    assert_eq!(resp1.status, "running");
    // Wait for async completion (Bug #1/#2 fix: delegate returns Running
    // immediately and execute_and_persist runs in the background).
    let final_status1 =
        await_task_done(&supervisor.subagent_manager(), &resp1.task_id).await;
    assert_eq!(final_status1, "completed");
    let sub1 = resp1.subagent_id;

    // 2. Second delegate — triggers eviction
    let resp2 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "winner".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "do 2".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: Some("test-provider".to_string()),
            bound_model_id: Some("test-model".to_string()),
            reuse_policy: None,
        })
        .await
        .unwrap();
    assert_eq!(resp2.status, "running");
    // Wait for async completion (Bug #1/#2 fix). The second delegate
    // triggers eviction of sub1 (prepare_hibernate → persist → close),
    // then retries turn_auto for itself.
    let final_status2 =
        await_task_done(&supervisor.subagent_manager(), &resp2.task_id).await;
    assert_eq!(final_status2, "completed");
    let sub2 = resp2.subagent_id;

    // 3. Verify: sub1 is warm (evicted with memory), sub2 is hot
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        let s1 = db_guard.subagent_get_logical(&sub1).unwrap().unwrap();
        assert_eq!(
            s1.status, "warm",
            "evicted subagent must be warm (memory written during eviction)"
        );
        let s2 = db_guard.subagent_get_logical(&sub2).unwrap().unwrap();
        assert_eq!(s2.status, "hot", "new subagent must be hot");
    }

    // 4. Verify session_hibernate resource event was written for the eviction
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        let events = db_guard.subagent_list_resource_events(None, 100).unwrap();
        assert!(
            events.iter().any(|e| e.event_type == "session_hibernate"),
            "session_hibernate event must be written during eviction"
        );
    }

    supervisor.shutdown_sidecar().await;
    supervisor.shutdown_writer().await.unwrap();
}

/// Regression (P0, manager/runtime level): when the hot session pool is
/// full (simulated by `BUSYTOK_MOCK_HOT_LIMIT_ALL_BUSY=1`), the task must be
/// re-queued (status flips `running → queued`) with `error_kind` staying
/// `None` — NOT marked as `failed` with `error_kind = "unknown"`.
///
/// This test exercises the real sidecar subprocess path (not a mock
/// executor) and verifies the full `running → queued → running → completed`
/// cycle under transient capacity contention. It complements the
/// executor-level test `executor_evicted_retry_hot_limit_surfaces_transient`
/// which only verifies the executor returns `SubagentError::HotSessionLimit`.
///
/// Setup:
/// - `max_hot_sessions = 1`: only one hot session at a time.
/// - `BUSYTOK_MOCK_HOT_LIMIT_ALL_BUSY=1`: when the pool is full, return
///   HOT_SESSION_LIMIT_REACHED with `all_busy=true` (no candidate). The
///   executor skips eviction and retries with backoff
///   (ALL_BUSY_MAX_RETRIES × ALL_BUSY_BACKOFF ≈ 5s), then surfaces
///   `SubagentError::HotSessionLimit` → `execute_and_persist` re-queues.
/// - `BUSYTOK_MOCK_HOT_LIMIT_RESPONSES=N`: after N hot-limit responses,
///   the mock stops returning hot-limit and lets the session be created.
///   This simulates the concurrent task finishing and freeing the slot,
///   bounding the test (no infinite re-queue loop).
///
/// Expected flow:
/// 1. First delegate (sub-a) succeeds — fills the hot session slot.
/// 2. Second delegate (sub-b) hits AllBusy → executor retries 10x (5s) →
///    HotSessionLimit → `execute_and_persist` re-queues the task.
/// 3. Dispatcher picks up the queued task → retries → after the hot-limit
///    response budget is exhausted, the mock lets the session be created →
///    task completes.
///
/// Assertions:
/// - Task `sub-b` must pass through `queued` state at least once.
/// - While `queued`, `error_kind` must be `None` (not "unknown", not
///   "hot_session_limit" — those are terminal failure states).
/// - Final status must be `completed` (not `failed`).
/// - Final `error_kind` must be `None`.
#[tokio::test]
#[serial]
async fn sidecar_e2e_evicted_retry_hot_limit_requeues_then_completes() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.max_hot_sessions = 1;
    seed_test_providers(&db, &seeds);
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    let mut cfg = make_sidecar_config();
    cfg.max_hot_sessions = 1;
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_SESSION_LIMIT".into(), "1".into());
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_LIMIT_ALL_BUSY".into(), "1".into());
    // After 25 hot-limit responses, the mock stops returning hot-limit and
    // lets the session be created. This simulates the concurrent task
    // finishing and freeing the slot, bounding the test. Each executor
    // attempt uses 10 hot-limit responses (ALL_BUSY_MAX_RETRIES=10), so 25
    // responses covers 2 full attempts + a partial 3rd that succeeds.
    cfg.env
        .insert("BUSYTOK_MOCK_HOT_LIMIT_RESPONSES".into(), "25".into());
    let supervisor = BusytokSupervisor::new_with_sidecar_config(db, paths, cfg);

    // 1. First delegate — fills the pool.
    let resp1 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "matrix-a".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "do 1".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: Some("test-provider".to_string()),
            bound_model_id: Some("test-model".to_string()),
            reuse_policy: None,
        })
        .await
        .unwrap();
    assert_eq!(resp1.status, "running");
    let final_status1 =
        await_task_done(&supervisor.subagent_manager(), &resp1.task_id).await;
    assert_eq!(final_status1, "completed", "first task must complete");

    // 2. Second delegate — triggers eviction → refill → HotSessionLimit →
    //    re-queue → retry → eviction succeeds → completed.
    let resp2 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "matrix-b".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "do 2".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: Some("test-provider".to_string()),
            bound_model_id: Some("test-model".to_string()),
            reuse_policy: None,
        })
        .await
        .unwrap();
    assert_eq!(resp2.status, "running");

    // 3. Poll the task row, recording every status transition. The task
    //    MUST pass through `queued` at least once (proving the re-queue
    //    path was taken, not the terminal `failed` path). While `queued`,
    //    `error_kind` MUST be `None`.
    let mut saw_queued = false;
    let mut saw_queued_with_error_kind = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let final_status: String;
    let final_error_kind: Option<String>;
    let mut prev_status = String::new();
    loop {
        let snap = {
            let db_guard = supervisor.db_handle().lock().unwrap();
            db_guard
                .subagent_get_task(&resp2.task_id)
                .ok()
                .flatten()
                .map(|t| (t.status, t.error_kind))
        };
        if let Some((status, error_kind)) = snap {
            if status != prev_status {
                // Status transition detected — log it for debugging.
                eprintln!("task {} transition: {} → {}", resp2.task_id, prev_status, status);
                prev_status = status.clone();
            }
            if status == "queued" {
                saw_queued = true;
                if error_kind.is_some() {
                    saw_queued_with_error_kind = true;
                    eprintln!(
                        "ERROR: task was queued with error_kind={:?} (must be None)",
                        error_kind
                    );
                }
            }
            if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
                final_status = status;
                final_error_kind = error_kind;
                break;
            }
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "task {} did not reach terminal status within 30s (last status: {})",
                resp2.task_id, prev_status
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // 4. Assertions.
    assert!(
        saw_queued,
        "task must pass through 'queued' state (re-queue path), \
         final status: {final_status}"
    );
    assert!(
        !saw_queued_with_error_kind,
        "task must NOT have error_kind set while queued (must stay None)"
    );
    assert_eq!(
        final_status, "completed",
        "task must eventually complete after re-queue + retry"
    );
    assert!(
        final_error_kind.is_none(),
        "completed task must have error_kind = None, got: {final_error_kind:?}"
    );

    supervisor.shutdown_sidecar().await;
    supervisor.shutdown_writer().await.unwrap();
}

// --- doctor via settings.diagnostics (Plan 5 Task 3, spec §7.1 + §7.3) ---
//
// Verifies that the EXISTING settings.diagnostics RPC path now includes
// an optional `subagent` section with 10 §7.1 checks. No new RPC method —
// the doctor reuses the existing diagnostics infrastructure.

#[tokio::test]
async fn settings_diagnostics_includes_subagent_doctor_with_10_checks() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, _seeds) = make_sidecar_settings();
    // Disable sidecar so doctor's `sidecar_launchable` check is "ok"
    // (no bundle to launch in unit tests).
    settings.subagent.pi_sidecar.enabled = false;
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    let supervisor = BusytokSupervisor::with_adapters_and_settings(
        db,
        paths,
        vec![], // no adapters needed for this test
        settings,
    );

    // Call the EXISTING settings_diagnostics handler — no new RPC.
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let dto = envelope.data;

    // Subagent section is present.
    let sub = dto
        .subagent
        .as_ref()
        .expect("settings.diagnostics must include subagent section");

    // 10 checks total per spec §7.1 (Task 3 removed `default_model_config`).
    assert_eq!(sub.checks.len(), 10, "must have all 10 §7.1 checks");

    // 3 bundle-inspection checks return "error" in the default test setup
    // (pi_sidecar.enabled=false, no runtime_dir → dev fallback path
    // apps/pi-sidecar/dist has no node/manifest/bundle files).
    for name in [
        "bundled_node_arch",
        "bundle_manifest_readable",
        "pi_runtime_installed",
    ] {
        let check = sub
            .checks
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("missing check: {name}"));
        assert_eq!(
            check.status, "error",
            "check {name} should be 'error' (bundle missing in default test setup)"
        );
    }

    // artifact_store_writable is "ok" — doctor check self-heals the dir.
    let artifact_check = sub
        .checks
        .iter()
        .find(|c| c.name == "artifact_store_writable")
        .expect("missing artifact_store_writable check");
    assert_eq!(artifact_check.status, "ok", "artifacts dir writable => ok");

    // protocol_version is "warning" — pi_sidecar disabled, no init_error.
    let proto_check = sub
        .checks
        .iter()
        .find(|c| c.name == "protocol_version")
        .expect("missing protocol_version check");
    assert_eq!(
        proto_check.status, "warning",
        "pi_sidecar disabled => warning (no supervisor to probe)"
    );
    assert!(
        proto_check
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("disabled"),
        "protocol_version detail should mention disabled: {:?}",
        proto_check.detail
    );

    // Verify the real checks return "ok" (sidecar disabled => launchable ok).
    let launchable = sub
        .checks
        .iter()
        .find(|c| c.name == "sidecar_launchable")
        .expect("missing sidecar_launchable check");
    assert_eq!(launchable.status, "ok", "sidecar disabled => launchable ok");

    // overall_ok is false — 3 "error" checks (bundle missing) make it false.
    assert!(!sub.overall_ok, "error checks make overall_ok false");

    supervisor.shutdown_writer().await.unwrap();
}

// --- 6 real doctor checks (Task 5, spec §7.1 lines 865-870) ---
//
// Each test exercises one of the 6 previously-stubbed checks against real
// fixture dirs. Together with the updated 11-check test above, these verify
// the doctor returns concrete statuses (ok/error/warning) based on the
// filesystem + sidecar state, not the "not yet implemented" warning stub.

#[tokio::test]
#[serial]
async fn doctor_bundled_node_arch_check_validates_arch_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = tmp.path().join("rt");
    let arch_dir = runtime_dir.join("node").join(std::env::consts::ARCH);
    std::fs::create_dir_all(&arch_dir).unwrap();
    std::fs::write(arch_dir.join("node"), b"#!/bin/sh\n").unwrap();

    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, _seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings.subagent.pi_sidecar.runtime_dir = Some(runtime_dir.to_string_lossy().to_string());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "bundled_node_arch")
        .unwrap();
    assert_eq!(
        check.status, "ok",
        "arch matches + node exists → ok: {:?}",
        check.detail
    );

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_bundled_node_arch_check_errors_on_missing_node() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = tmp.path().join("rt");
    // Don't create the node binary — should error.
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, _seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings.subagent.pi_sidecar.runtime_dir = Some(runtime_dir.to_string_lossy().to_string());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "bundled_node_arch")
        .unwrap();
    assert_eq!(check.status, "error", "missing node → error");
    assert!(
        check.detail.as_deref().unwrap_or("").contains("not found"),
        "detail should say not found: {:?}",
        check.detail
    );

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_bundle_manifest_readable_check_validates_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = tmp.path().join("rt");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    // Write a valid manifest.json conforming to SidecarManifest schema
    // (Task 1: version/protocol_version/bundle/node_runtime_version).
    std::fs::write(
        runtime_dir.join("manifest.json"),
        br#"{"version":"1","protocol_version":1,"bundle":"pi-sidecar.bundle.js","node_runtime_version":"22.6.0"}"#,
    )
    .unwrap();

    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, _seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings.subagent.pi_sidecar.runtime_dir = Some(runtime_dir.to_string_lossy().to_string());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "bundle_manifest_readable")
        .unwrap();
    assert_eq!(check.status, "ok", "valid manifest.json → ok");

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_bundle_manifest_readable_check_fails_on_malformed_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = tmp.path().join("rt");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    // Write a malformed manifest.json (not valid JSON).
    std::fs::write(runtime_dir.join("manifest.json"), b"not json {{{").unwrap();

    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, _seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings.subagent.pi_sidecar.runtime_dir = Some(runtime_dir.to_string_lossy().to_string());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "bundle_manifest_readable")
        .unwrap();
    assert_eq!(check.status, "error", "malformed manifest.json → error");

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_artifact_store_writable_check_writes_probe_file() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, _seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "artifact_store_writable")
        .unwrap();
    // artifacts dir is created (self-healed) by the doctor check → writable.
    assert_eq!(
        check.status, "ok",
        "artifacts dir writable → ok: {:?}",
        check.detail
    );

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_protocol_version_check_is_warning_when_pi_sidecar_disabled() {
    // When pi_sidecar.enabled = false, there is no sidecar supervisor at
    // all (sidecar_supervisor = None) and no sidecar_init_error. The check
    // returns "warning" because there is nothing to probe — this is NOT
    // the "sidecar not running" case.
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, _seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "protocol_version")
        .unwrap();
    assert_eq!(
        check.status, "warning",
        "pi_sidecar disabled → warning (no supervisor to probe)"
    );
    assert!(
        check.detail.as_deref().unwrap_or("").contains("disabled"),
        "detail should mention disabled: {:?}",
        check.detail
    );

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_protocol_version_check_errors_when_enabled_but_bundle_missing() {
    // When pi_sidecar.enabled = true but the bundle is missing,
    // resolve_sidecar_config fails at construction → sidecar_supervisor is
    // None AND sidecar_init_error is Some. The check returns "error" (NOT
    // "warning") because the user enabled the sidecar but it's broken.
    // This exercises the `None` arm with `sidecar_init_error = Some`, NOT
    // the real probe arm (see
    // `doctor_protocol_version_check_probes_sidecar_when_not_running` for
    // the `Some(sup) => ensure_started()` probe path).
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, _seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = true;
    // No bundle installed → resolve_sidecar_config fails → init_error set.
    settings.subagent.pi_sidecar.runtime_dir =
        Some(tmp.path().join("rt").to_string_lossy().to_string());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);
    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "protocol_version")
        .unwrap();
    assert_eq!(
        check.status, "error",
        "enabled but probe fails → error (not warning)"
    );
    assert!(
        check.detail.as_deref().unwrap_or("").contains("probe"),
        "detail should mention probe failure: {:?}",
        check.detail
    );

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_protocol_version_check_probes_sidecar_when_not_running() {
    // Exercises the real `Some(sup) => match sup.ensure_started().await` arm
    // of the protocol_version doctor check — the path where the sidecar is
    // enabled, constructed successfully (via `new_with_sidecar_config`), but
    // NOT already running. The check spawns the mock sidecar via
    // `ensure_started` (verifies protocol via `adapter.initialize`), then
    // shuts it down via `shutdown_internal`. This is the arm that
    // `doctor_protocol_version_check_errors_when_enabled_but_bundle_missing`
    // does NOT cover — that test hits the `None` arm because
    // `resolve_sidecar_config` fails before the supervisor is constructed.
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let (mut settings, seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = true;
    let paths = BusytokPaths::for_test(tmp.path());
    seed_test_providers(&db, &seeds);
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    // Bypass resolve_sidecar_config → constructs a real PiSidecarSupervisor
    // (self.sidecar_supervisor is Some). Do NOT delegate first — the sidecar
    // must not be running when settings_diagnostics() is called.
    let supervisor = BusytokSupervisor::new_with_sidecar_config(db, paths, make_sidecar_config());

    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "protocol_version")
        .unwrap();
    assert_eq!(
        check.status, "ok",
        "mock sidecar starts + verifies protocol → ok"
    );
    assert!(
        check.detail.as_deref().unwrap_or("").contains("probe"),
        "detail should mention short-lived probe: {:?}",
        check.detail
    );

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn settings_diagnostics_subagent_flags_stale_subagents_over_30_days() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, _seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    let supervisor = BusytokSupervisor::with_adapters_and_settings(
        db,
        paths,
        vec![], // no adapters needed for this test
        settings,
    );

    // Insert a stale subagent (last_active 31 days ago).
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        let stale_ms = busytok_domain::now_ms() - (31 * 24 * 60 * 60 * 1000);
        db_guard
            .conn()
            .execute(
                "INSERT INTO subagent_logical_subagents \
                 (id, name, project_id, repo_path, repo_hash, intent, default_profile, \
                  bound_provider_id, bound_model_id, \
                  status, created_at_ms, updated_at_ms, last_active_at_ms) \
                 VALUES ('stale_sub', 'stale-test', 'proj', '/repo', 'hash', 'test', \
                         'pi/search-cheap', 'legacy', 'legacy-model', \
                         'warm', ?1, ?1, ?1)",
                rusqlite::params![stale_ms],
            )
            .unwrap();
    }

    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope.data.subagent.unwrap();
    let stale_check = sub
        .checks
        .iter()
        .find(|c| c.name == "subagents_unused_30d")
        .expect("must have subagents_unused_30d check");
    assert_eq!(stale_check.status, "warning", "stale subagent => warning");
    assert!(
        stale_check
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("stale_sub"),
        "detail should mention the stale subagent"
    );
    // overall_ok is false because the 3 bundle-inspection checks error in
    // this default test setup (no runtime_dir → dev fallback path has no
    // bundle files). The stale-subagent check is a WARNING (verified above),
    // not an error — warnings alone would not break overall_ok. This
    // assertion confirms the bundle errors (not the stale warning) are what
    // flips overall_ok.
    assert!(
        !sub.overall_ok,
        "bundle-missing errors make overall_ok false"
    );

    supervisor.shutdown_writer().await.unwrap();
}

// --- doctor SQLite failure paths (spec §7.1: readable/writable/schema) ---
//
// Verifies the 3-probe SQLite check reports "error" when (a) the schema
// version doesn't match SCHEMA_VERSION, and (b) the DB is read-only (write
// probe fails). The all-good case is covered by
// settings_diagnostics_includes_subagent_doctor_with_11_checks above.

#[tokio::test]
#[serial]
async fn doctor_sqlite_check_errors_on_schema_version_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    // Corrupt the schema version to trigger the mismatch branch.
    // DELETE + INSERT (not UPDATE) because _schema_version.version has a
    // UNIQUE constraint — UPDATE would fail if the target value exists.
    db.conn()
        .execute_batch("DELETE FROM _schema_version; INSERT INTO _schema_version (version, applied_at_ms) VALUES (999, 0);")
        .unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, _seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope
        .data
        .subagent
        .expect("subagent section must be present");
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "sqlite_readable")
        .expect("must have sqlite_readable check");
    assert_eq!(check.status, "error", "schema mismatch must report error");
    assert!(
        check
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("schema version mismatch"),
        "detail should explain the mismatch: {:?}",
        check.detail
    );

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_sqlite_check_errors_on_readonly_database() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("readonly_test.db");
    // Open normally to run migrations, then drop to close the connection.
    {
        let _db = busytok_store::Database::open(&db_path).unwrap();
    }
    // Reopen as read-only — BEGIN IMMEDIATE will fail (SQLITE_READONLY).
    let db = busytok_store::Database::open_readonly(&db_path).unwrap();

    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, _seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    let envelope = supervisor.settings_diagnostics().await.unwrap();
    let sub = envelope
        .data
        .subagent
        .expect("subagent section must be present");
    let check = sub
        .checks
        .iter()
        .find(|c| c.name == "sqlite_readable")
        .expect("must have sqlite_readable check");
    assert_eq!(check.status, "error", "read-only DB must report error");
    assert!(
        check
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("write probe failed"),
        "detail should explain the write probe failure: {:?}",
        check.detail
    );

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn doctor_sqlite_check_failure_leaves_connection_usable() {
    // Regression: the write probe must not leave an open transaction on the
    // connection after a failure. Previously `execute_batch("BEGIN IMMEDIATE;
    // DELETE ...; ROLLBACK;")` would abort at the failing DELETE and skip
    // ROLLBACK, polluting subsequent operations on the same connection.
    // Verify by triggering the read-only failure, then calling
    // settings_diagnostics AGAIN on the same supervisor — the second call's
    // SQLite check (readable probe: SELECT 1) must still work, proving the
    // connection isn't stuck in a dirty transaction.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("readonly_conn_test.db");
    {
        let _db = busytok_store::Database::open(&db_path).unwrap();
    }
    let db = busytok_store::Database::open_readonly(&db_path).unwrap();

    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, _seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false;
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    // First call — triggers the read-only failure in the write probe.
    let envelope1 = supervisor.settings_diagnostics().await.unwrap();
    let sub1 = envelope1
        .data
        .subagent
        .expect("subagent section must be present");
    let check1 = sub1
        .checks
        .iter()
        .find(|c| c.name == "sqlite_readable")
        .expect("must have sqlite_readable check");
    assert_eq!(
        check1.status, "error",
        "first call: read-only DB must error"
    );

    // Second call — the readable probe (SELECT 1) must still succeed and
    // not be blocked by a leftover transaction. If the connection were
    // stuck in a transaction, the SELECT 1 would either error or return
    // stale results. We assert the check still runs and reports the same
    // status (error, because the DB is still read-only), confirming the
    // connection is usable.
    let envelope2 = supervisor.settings_diagnostics().await.unwrap();
    let sub2 = envelope2
        .data
        .subagent
        .expect("subagent section must be present on second call");
    let check2 = sub2
        .checks
        .iter()
        .find(|c| c.name == "sqlite_readable")
        .expect("must have sqlite_readable check on second call");
    // The check must still execute — not panic, not hang, not return a
    // different error type. Same status as the first call confirms the
    // connection is in a clean state.
    assert_eq!(
        check2.status, "error",
        "second call must still report error (read-only), proving connection is usable"
    );
    // The detail must still report the write probe failure (not a different
    // error like "cannot start a transaction within a transaction").
    assert!(
        check2
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("write probe failed"),
        "second call detail should still be write probe failure, got: {:?}",
        check2.detail
    );

    supervisor.shutdown_writer().await.unwrap();
}

// --- crash recovery e2e (Plan 5 Task 5, spec §12.1 Case 4) ---
//
// Verifies the EXISTING crash recovery logic in PiSidecarSupervisor
// (supervisor.rs:106-322): when the sidecar process is killed mid-task,
// the supervisor does NOT crash busytok-service, the next delegate
// auto-restarts the sidecar, and the in-flight task is reconciled to
// `failed` with SIDECAR_CRASHED. Memory + task history survive in SQLite.
//
// Mock sidecar fixture: BUSYTOK_MOCK_CRASH_AFTER=2 causes the mock to exit 1
// after sending its second response (adapter.initialize + session.turn_auto).
// The first delegate's turn_auto response IS sent (so the delegate completes),
// then the mock exits 1. The supervisor's `try_wait` detects the exit, runs
// `reconcile_crash`, writes a `sidecar_crash` resource event, and exits the
// supervision loop. The next `ensure_started` respawns.
//
// NOTE: the brief specified BUSYTOK_MOCK_CRASH_AFTER=1, but the mock counts
// ALL messages (adapter.initialize counts as 1). With =1 the mock exits
// after the initialize response, before turn_auto — the first delegate would
// fail with SidecarError::Crashed, contradicting the brief's assertion that
// resp1.status == "completed". Using =2 makes the mock exit after the first
// turn_auto response, matching the brief's behavioral expectations.

#[tokio::test]
#[serial]
async fn sidecar_e2e_crash_recovery_next_delegate_restarts_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, seeds) = make_sidecar_settings();
    // Short idle so the sidecar doesn't linger between test phases.
    settings.subagent.pi_sidecar.idle_exit_seconds = 300;
    seed_test_providers(&db, &seeds);
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    // Config with BUSYTOK_MOCK_CRASH_AFTER=2: the mock exits after sending
    // its second response (adapter.initialize + session.turn_auto). The first
    // delegate's turn_auto response IS sent (so the delegate completes), then
    // the mock exits 1.
    let mut cfg = make_sidecar_config();
    cfg.env
        .insert("BUSYTOK_MOCK_CRASH_AFTER".into(), "2".into());
    let supervisor = BusytokSupervisor::new_with_sidecar_config(db, paths, cfg);

    // 1. First delegate — completes (mock sends response THEN exits).
    let resp1 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "crash-test".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "do 1".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: Some("test-provider".to_string()),
            bound_model_id: Some("test-model".to_string()),
            reuse_policy: None,
        })
        .await
        .expect("first delegate must return a response (response sent before crash)");
    let sub_id = resp1.subagent_id.clone();
    assert_eq!(resp1.status, "running");
    // Wait for async completion (Bug #1/#2 fix: delegate returns Running
    // immediately and execute_and_persist runs in the background). The mock
    // sends the turn_auto response THEN exits — so the task completes
    // before the crash is observed by the supervision loop.
    let final_status1 =
        await_task_done(&supervisor.subagent_manager(), &resp1.task_id).await;
    assert_eq!(final_status1, "completed");

    // 2. Wait for the supervision loop to observe the crash + write the
    //    sidecar_crash event. The loop polls every 100ms; give it up to 4s
    //    (80 iterations × 50ms) to avoid flaking on slow CI runners.
    let mut saw_crash = false;
    for _ in 0..80 {
        let crashed = {
            let db_guard = supervisor.db_handle().lock().unwrap();
            let events = db_guard.subagent_list_resource_events(None, 100).unwrap();
            events.iter().any(|e| e.event_type == "sidecar_crash")
        };
        if crashed {
            saw_crash = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        saw_crash,
        "sidecar_crash resource event must be written after the mock exits"
    );

    // 3. Second delegate — must auto-restart the sidecar (exponential
    //    backoff 1s on first restart). The supervisor's `ensure_started`
    //    path detects the dead child, calls `spawn_internal`, which sleeps
    //    1s (restart_backoff_base) then respawns. The test must tolerate
    //    this delay.
    let resp2 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "crash-test".to_string(),
            subagent_id: Some(sub_id.clone()),
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "do 2 after crash".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: None,
            bound_model_id: None,
            reuse_policy: None,
        })
        .await
        .expect("second delegate must return a response after auto-restart");
    assert_eq!(
        resp2.status, "running",
        "second delegate returns Running immediately after restart"
    );
    // Wait for async completion (Bug #1/#2 fix).
    let final_status2 =
        await_task_done(&supervisor.subagent_manager(), &resp2.task_id).await;
    assert_eq!(
        final_status2, "completed",
        "second delegate completes after restart"
    );
    assert_eq!(
        resp2.subagent_id, sub_id,
        "same logical subagent (memory preserved)"
    );

    // 4. Verify a sidecar_restart resource event was written.
    let saw_restart = {
        let db_guard = supervisor.db_handle().lock().unwrap();
        let events = db_guard.subagent_list_resource_events(None, 200).unwrap();
        events.iter().any(|e| e.event_type == "sidecar_restart")
    };
    assert!(
        saw_restart,
        "sidecar_restart event must be written on auto-restart"
    );

    // 5. Verify task history is preserved (both tasks visible).
    let tasks = supervisor
        .subagent_tasks(SubagentTasksRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
            limit: Some(50),
        })
        .await
        .unwrap();
    assert!(
        tasks.tasks.len() >= 2,
        "task history must be preserved across crash/restart; got {} tasks",
        tasks.tasks.len()
    );

    // 6. Verify the logical subagent still exists (memory preserved).
    let shown = supervisor
        .subagent_show(SubagentResolveRequestDto {
            name: None,
            id: Some(sub_id),
            cwd: None,
        })
        .await
        .unwrap();
    assert_eq!(shown.name, "crash-test");

    supervisor.shutdown_sidecar().await;
    supervisor.shutdown_writer().await.unwrap();
}

// --- double-crash regression test (P1 fix: supervision_started must reset) ---
//
// Verifies that the supervision loop is revived after a crash, so a SECOND
// crash is still detected. Without the fix (supervision_started never reset),
// the loop exits after the first crash and is never re-spawned — the second
// crash would go undetected (no sidecar_crash event, no DB reconciliation).
//
// Flow: crash #1 → restart → crash #2 → restart → success. Asserts 2 crash
// events + 2 restart events, proving the loop ran for both lifecycles.

#[tokio::test]
#[serial]
async fn sidecar_e2e_double_crash_second_crash_still_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.idle_exit_seconds = 300;
    seed_test_providers(&db, &seeds);
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");

    let mut cfg = make_sidecar_config();
    cfg.env
        .insert("BUSYTOK_MOCK_CRASH_AFTER".into(), "2".into());
    let supervisor = BusytokSupervisor::new_with_sidecar_config(db, paths, cfg);

    // 1. First delegate — completes, then mock crashes.
    let resp1 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "double-crash".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "crash 1".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: Some("test-provider".to_string()),
            bound_model_id: Some("test-model".to_string()),
            reuse_policy: None,
        })
        .await
        .expect("first delegate must complete");
    assert_eq!(resp1.status, "running");
    // Wait for async completion (Bug #1/#2 fix: delegate returns Running
    // immediately and execute_and_persist runs in the background).
    let final_status1 =
        await_task_done(&supervisor.subagent_manager(), &resp1.task_id).await;
    assert_eq!(final_status1, "completed");
    let sub_id = resp1.subagent_id.clone();

    // Wait for crash #1 to be detected by the supervision loop.
    let mut crash1_detected = false;
    for _ in 0..160 {
        let crashes = {
            let db_guard = supervisor.db_handle().lock().unwrap();
            let events = db_guard.subagent_list_resource_events(None, 200).unwrap();
            events
                .iter()
                .filter(|e| e.event_type == "sidecar_crash")
                .count()
        };
        if crashes >= 1 {
            crash1_detected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        crash1_detected,
        "first crash must be detected by the supervision loop"
    );

    // 2. Second delegate — triggers auto-restart, completes, then mock
    //    crashes AGAIN. This is the key: if supervision_started was not
    //    reset, no loop would be running to detect this second crash.
    let resp2 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "double-crash".to_string(),
            subagent_id: Some(sub_id.clone()),
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "crash 2".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: None,
            bound_model_id: None,
            reuse_policy: None,
        })
        .await
        .expect("second delegate must complete after restart");
    assert_eq!(resp2.status, "running");
    // Wait for async completion (Bug #1/#2 fix).
    let final_status2 =
        await_task_done(&supervisor.subagent_manager(), &resp2.task_id).await;
    assert_eq!(final_status2, "completed");

    // Wait for crash #2 to be detected. WITHOUT the fix, this would time
    // out because the supervision loop was never revived.
    let mut crash2_detected = false;
    for _ in 0..160 {
        let crashes = {
            let db_guard = supervisor.db_handle().lock().unwrap();
            let events = db_guard.subagent_list_resource_events(None, 200).unwrap();
            events
                .iter()
                .filter(|e| e.event_type == "sidecar_crash")
                .count()
        };
        if crashes >= 2 {
            crash2_detected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        crash2_detected,
        "second crash must be detected — if this times out, the supervision \
         loop was not revived after the first crash (supervision_started \
         was not reset)"
    );

    // 3. Third delegate — triggers second restart, completes.
    let resp3 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "double-crash".to_string(),
            subagent_id: Some(sub_id.clone()),
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "final".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: None,
            bound_model_id: None,
            reuse_policy: None,
        })
        .await
        .expect("third delegate must complete after second restart");
    assert_eq!(resp3.status, "running");
    // Wait for async completion (Bug #1/#2 fix).
    let final_status3 =
        await_task_done(&supervisor.subagent_manager(), &resp3.task_id).await;
    assert_eq!(final_status3, "completed");

    // 4. Verify event counts: at least 2 crashes + exactly 2 restarts.
    //    crash_count uses `>= 2` (not `== 2`) because the mock also crashes
    //    after delegate #3's 2nd message, and the supervision loop may
    //    detect that 3rd crash before this assertion runs — a legitimate
    //    race. restart_count is exactly 2 because restarts only happen on
    //    ensure_started (delegate), and there is no delegate #4.
    let (crash_count, restart_count) = {
        let db_guard = supervisor.db_handle().lock().unwrap();
        let events = db_guard.subagent_list_resource_events(None, 200).unwrap();
        let c = events
            .iter()
            .filter(|e| e.event_type == "sidecar_crash")
            .count();
        let r = events
            .iter()
            .filter(|e| e.event_type == "sidecar_restart")
            .count();
        (c, r)
    };
    assert!(
        crash_count >= 2,
        "expected at least 2 crash events, got {crash_count} (3rd crash may have been detected before assertion)"
    );
    assert_eq!(restart_count, 2, "exactly 2 restart events expected");

    supervisor.shutdown_sidecar().await;
    supervisor.shutdown_writer().await.unwrap();
}
//
// Spec §12.2 acceptance: "100 idle logical subagents: RSS does not grow
// linearly." Logical subagents are SQLite rows, not processes — RSS should
// grow by < 10MB for 100 rows. This test creates 100 rows via the store,
// measures busytok-service RSS before and after, and asserts sub-linear
// growth. It also verifies only 1 sidecar process exists when active
// (delegate to one subagent, count node/bash children).

#[tokio::test]
#[serial]
async fn sidecar_e2e_stress_100_subagents_rss_does_not_grow_linearly() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (settings, seeds) = make_sidecar_settings();
    seed_test_providers(&db, &seeds);
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");
    let supervisor = BusytokSupervisor::new_with_sidecar_config(db, paths, make_sidecar_config());

    // Measure RSS before creating any subagents. Use sysinfo directly —
    // constructing a ResourceMonitor here would duplicate the supervisor's
    // own monitor; the spec allows direct sysinfo for the measurement.
    let service_pid = std::process::id();
    let mut sys = sysinfo::System::new_all();
    // sysinfo 0.32 API: refresh_processes_specifics takes ProcessesToUpdate.
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(service_pid)]),
        false,
        ProcessRefreshKind::everything(),
    );
    let rss_before_mb = sys
        .process(sysinfo::Pid::from_u32(service_pid))
        .map(|p| (p.memory() as f64) / (1024.0 * 1024.0))
        .unwrap_or(0.0);

    // Create 100 idle logical subagents via direct DB insertion.
    let now_ms = busytok_domain::now_ms();
    {
        let db_guard = supervisor.db_handle().lock().unwrap();
        use busytok_store::SubagentLogicalSubagentRow;
        for i in 0..100 {
            db_guard
                .subagent_upsert_logical(&SubagentLogicalSubagentRow {
                    id: format!("stress_sub_{i}"),
                    name: format!("stress-{i}"),
                    project_id: "stress_proj".into(),
                    repo_path: "/r".into(),
                    repo_hash: format!("h{i}"),
                    branch: None,
                    intent: None,
                    default_profile: "pi/search-cheap".into(),
                    bound_provider_id: "test-provider".into(),
                    bound_model_id: "test-model".into(),
                    status: "cold".into(),
                    created_at_ms: now_ms,
                    updated_at_ms: now_ms,
                    last_active_at_ms: Some(now_ms),
                })
                .unwrap();
        }
    }

    // Measure RSS after creating 100 subagents.
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(service_pid)]),
        false,
        ProcessRefreshKind::everything(),
    );
    let rss_after_mb = sys
        .process(sysinfo::Pid::from_u32(service_pid))
        .map(|p| (p.memory() as f64) / (1024.0 * 1024.0))
        .unwrap_or(0.0);

    let growth_mb = rss_after_mb - rss_before_mb;
    // Spec §12.2: RSS does not grow linearly. 100 rows of ~200 bytes each
    // is ~20KB of actual data; SQLite page cache may grow by a few MB.
    // 10MB is a generous upper bound that still catches a regression where
    // subagents accidentally spawn processes or hold large in-memory state.
    assert!(
        growth_mb < 10.0,
        "RSS growth for 100 idle subagents must be < 10MB (got {growth_mb:.2}MB); \
         before={rss_before_mb:.2}MB after={rss_after_mb:.2}MB"
    );

    // Verify all 100 appear in list.
    let list = supervisor
        .subagent_list(SubagentListRequestDto {
            status: None,
            project: None,
            include_deleted: Some(false),
        })
        .await
        .unwrap();
    assert!(
        list.subagents.len() >= 100,
        "all 100 stress subagents must be listed; got {}",
        list.subagents.len()
    );

    // Spec §12.2: "Pi sidecar: exactly 1 process when active." Delegate to
    // one subagent, then count the mock-sidecar bash child processes.
    let _resp = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "stress-0".to_string(),
            subagent_id: Some("stress_sub_0".to_string()),
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "noop".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: None,
            bound_model_id: None,
            reuse_policy: None,
        })
        .await
        .unwrap();

    // Spec §12.2: "exactly 1 process when active." Count mock-sidecar
    // processes that are children of THIS test process.
    //
    // Filter by parent PID because `#[serial]` does NOT isolate across
    // test binaries — `cargo test` compiles each `tests/*.rs` as a separate
    // binary and runs binaries in parallel by default. Other binaries
    // (supervisor_shutdown, sidecar_supervisor, sidecar_executor) spawn
    // their own mock-sidecar.sh processes that a global scan would
    // incorrectly count as this test's sidecar. `#[serial]` only
    // serializes tests within the same binary, so the parent-PID filter
    // is the reliable isolation boundary.
    //
    // A short retry loop tolerates sysinfo's eventual-consistency process
    // table refresh on macOS — a freshly spawned child may not appear in
    // System::new_all() for a few hundred ms.
    let test_pid = sysinfo::Pid::from_u32(std::process::id());
    let mut sidecar_count = 0;
    for attempt in 0..5 {
        let mut sys_all = sysinfo::System::new_all();
        sys_all.refresh_processes(ProcessesToUpdate::All, true);
        sidecar_count = sys_all
            .processes()
            .values()
            .filter(|p| {
                p.parent() == Some(test_pid)
                    && p.cmd()
                        .iter()
                        .any(|arg| arg.to_string_lossy().contains("mock-sidecar.sh"))
            })
            .count();
        if sidecar_count == 1 {
            break;
        }
        if attempt < 4 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    assert_eq!(
        sidecar_count, 1,
        "exactly 1 sidecar process parented to this test must exist when \
         active (got {sidecar_count}); this test's pid={test_pid}"
    );

    supervisor.shutdown_sidecar().await;
    supervisor.shutdown_writer().await.unwrap();
}

// --- idle RSS regression test (spec §12.2: "Idle busytok-service RSS < 50MB") ---
//
// Spec §12.2 requires idle busytok-service RSS < 50MB when the Pi sidecar is
// not running. The 50MB budget targets the production service process; a test
// process carries extra overhead (test harness, tokio runtime, sysinfo, all
// crate deps). This test provides regression protection by:
// 1. Measuring RSS with no sidecar started (idle baseline).
// 2. Delegating + shutting down (exercising spawn → shutdown → resource
//    cleanup).
// 3. Measuring RSS after shutdown.
// 4. Asserting the delta is small (< 15MB) — catches resource monitor leaks,
//    un-dropped state, or retained caches that would grow RSS over time.
// 5. Asserting the absolute RSS stays below a test-process upper bound.
//
// The delta assertion is the primary regression signal; the absolute bound
// is a generous ceiling that catches catastrophic leaks without being
// environment-sensitive.

#[tokio::test]
#[serial]
async fn sidecar_e2e_idle_rss_does_not_leak_after_delegate_shutdown() {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};

    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (settings, seeds) = make_sidecar_settings();
    seed_test_providers(&db, &seeds);
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");
    let supervisor = BusytokSupervisor::new_with_sidecar_config(db, paths, make_sidecar_config());

    let pid = sysinfo::Pid::from_u32(std::process::id());

    // Baseline: measure RSS before any sidecar activity.
    let measure_rss = || {
        let mut sys = sysinfo::System::new_all();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            false,
            ProcessRefreshKind::everything(),
        );
        sys.process(pid)
            .map(|p| (p.memory() as f64) / (1024.0 * 1024.0))
            .unwrap_or(0.0)
    };

    let rss_baseline = measure_rss();
    assert!(
        rss_baseline > 0.0,
        "failed to measure baseline RSS for pid={pid}"
    );

    // Exercise the full lifecycle: delegate → sidecar spawns → shutdown.
    let _resp = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "idle-rss-test".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "noop".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: Some("test-provider".to_string()),
            bound_model_id: Some("test-model".to_string()),
            reuse_policy: None,
        })
        .await
        .unwrap();

    // Shutdown the sidecar and wait for cleanup.
    supervisor.shutdown_sidecar().await;

    // Give the OS a moment to reclaim the sidecar process's memory.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let rss_after = measure_rss();
    let delta = rss_after - rss_baseline;

    // Delta assertion (primary regression signal): RSS should not grow
    // significantly from a single delegate+shutdown cycle. A growth > 15MB
    // indicates a resource leak (monitor, cache, or un-dropped state).
    assert!(
        delta < 15.0,
        "RSS grew by {delta:.1}MB after one delegate+shutdown cycle \
         (baseline={rss_baseline:.1}MB, after={rss_after:.1}MB); \
         this indicates a resource leak — spec §12.2 requires idle service \
         RSS to stay bounded"
    );

    // Absolute assertion (spec §12.2 budget is 50MB for production; test
    // process overhead warrants a generous ceiling). Catches catastrophic
    // leaks. Adjust if test-process deps grow.
    assert!(
        rss_after < 200.0,
        "RSS after shutdown = {rss_after:.1}MB, exceeding 200MB test-process \
         ceiling (baseline={rss_baseline:.1}MB); spec §12.2 production budget \
         is 50MB — investigate the leak"
    );

    supervisor.shutdown_writer().await.unwrap();
}

// --- pressure gate wiring (Plan 6 Task 3, spec §8.3 step 2 "queue only") ---
//
// Verifies that `SubagentManager::delegate()` honors a paused `PressureGate`
// by inserting the task row as `'queued'` and returning
// `DelegateResult { status: Queued }` — NOT an error, NOT executing the task.
// The background `TaskDispatcher` (Task 7) picks it up when the gate clears.
// This test constructs a standalone `SubagentManager` with an explicit gate
// because the supervisor path skips gate construction when `pi_sidecar.enabled
// = false` (the unit-test default). The wiring (gate threaded into the
// manager via `with_pressure_gate`) is what's being verified.

#[tokio::test]
#[serial]
async fn delegate_returns_queued_when_pressure_gate_is_paused() {
    use std::sync::Arc;

    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let (mut settings, _seeds) = make_sidecar_settings();
    settings.subagent.pi_sidecar.enabled = false; // mock executor
    settings.subagent.enabled = true;
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    // Smoke-test intent: with `pi_sidecar.enabled = false` the supervisor
    // must NOT construct a pressure gate (gate construction is the sidecar
    // path's responsibility). The queue-only behavior below is exercised
    // through a standalone SubagentManager with an explicit gate.
    assert!(
        supervisor.pressure_gate().is_none(),
        "sidecar disabled → no pressure gate constructed"
    );

    // When sidecar is disabled, no pressure gate is constructed. Use a
    // direct SubagentManager test instead by constructing the manager
    // with an explicit gate. This test verifies the wiring path.
    let gate = Arc::new(busytok_subagent::PressureGate::new());
    gate.set_action(busytok_subagent::PressureAction::PauseNewTasks);

    let db2 = Arc::new(std::sync::Mutex::new(
        busytok_store::Database::open_in_memory().unwrap(),
    ));
    // I-1: the creation path now requires valid bound_provider_id +
    // bound_model_id (spec §3.3 strict). Seed the test provider + model
    // into db2 so the resolver's validate_bound_provider_model passes.
    // The model seed includes context_window + max_tokens (I-2 fail-fast).
    seed_test_providers(&db2.lock().unwrap(), &[TEST_PROVIDER_SEED.clone()]);
    let settings2 = busytok_config::SubagentSettings {
        enabled: true,
        ..Default::default()
    };
    let exec = Arc::new(busytok_subagent::mock_executor::MockTaskExecutor)
        as Arc<dyn busytok_subagent::mock_executor::TaskExecutor>;
    let manager = std::sync::Arc::new(
        busytok_subagent::SubagentManager::with_pressure_gate(
            db2.clone(),
            settings2,
            "pi",
            exec,
            Some(gate.clone()),
        ),
    );

    let req = busytok_subagent::DelegateRequest {
        subagent_name: "paused-test".to_string(),
        subagent_id: None,
        cwd: tmp.path().join("repo").to_string_lossy().to_string(),
        profile: "pi/search-cheap".to_string(),
        intent: None,
        prompt: "should be queued".to_string(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: Some(TEST_PROVIDER_SEED.id.to_string()),
        bound_model_id: Some(TEST_PROVIDER_SEED.models[0].to_string()),
        reuse_policy: None,
    };
    // §8.3 step 2 "queue only": delegate() accepts the task and returns
    // DelegateResult { status: Queued } — NOT an error. The background
    // TaskDispatcher (Task 7) picks it up when the gate clears.
    let result = manager
        .delegate(req)
        .await
        .expect("delegate must succeed (queue-only)");
    assert_eq!(
        result.status,
        busytok_subagent::TaskStatus::Queued,
        "delegate must return Queued status when gate is paused, not execute or error"
    );

    // Verify the race-free insert claim: the task row must exist in the DB
    // with `status = "queued"` and `started_at_ms = NULL` (not started).
    // Without this query, the Queued return value could in principle come
    // from a path that skipped DB insertion entirely.
    let row = db2
        .lock()
        .unwrap()
        .subagent_get_task(&result.task_id)
        .expect("DB get_task must not error")
        .expect("queued task row must exist in the DB after delegate returns Queued");
    assert_eq!(
        row.status, "queued",
        "DB row status must be 'queued' (got {:?})",
        row.status
    );
    assert!(
        row.started_at_ms.is_none(),
        "queued task must not have started_at_ms set (got {:?})",
        row.started_at_ms
    );

    // Sanity: the gate is still paused (delegate must not have cleared it).
    assert!(gate.is_paused(), "delegate must not clear the pause flag");
}

// --- pressure response e2e (Plan 6 Task 4, spec §8.3 escalation chain) ---
//
// Verifies the full §8.3 escalation chain end-to-end through the real
// supervisor + mock sidecar subprocess. The mock sidecar's RSS (a bash
// process, ~5-10MB) is compared against artificially low limits set in
// SidecarConfig to trigger each escalation tier:
//
// 1. `pressure_response_force_kills_on_rss_limit_exceeded`: hard/soft limit
//    = 1MB. The sidecar's RSS always exceeds this → `exceeds_hard` →
//    `ForceKill` action. The responder SIGKILLs the sidecar, writes a
//    `sidecar_crash` resource event, and clears the gate to `Resume`
//    (Finding 2 fix — prevents deadlock in paused state).
//
// 2. `pressure_response_pauses_on_memory_pressure`: `memory_pressure_free_mb
//    = 999999` (always pressured), soft limit = 800MB (not exceeded by the
//    ~10MB mock sidecar). `under_pressure` → `new_state = Pressure` →
//    `PauseNewTasks` action (§8.3 steps 1-2: pause queue + hibernate LRU).
//    The gate is set to paused; the responder also calls `evict_lru`.
//
// 3. `pressure_response_graceful_restarts_on_soft_limit_exceeded`: soft limit
//    = 1MB (mock sidecar ~5-10MB exceeds → `exceeds_soft`), hard limit =
//    1200MB (NOT exceeded → no ForceKill), `memory_pressure_free_mb = 0`
//    (system_available_mb is always >= 0, so `is_under_pressure` is false).
//    `exceeds_soft` → `GracefulRestart` action (§8.3 steps 3-4):
//    `prepare_hibernate_all` + `shutdown_internal` writes a `sidecar_stop`
//    resource event, then the responder clears the gate to `Resume` so the
//    next delegate() can lazy-restart the sidecar.

#[tokio::test]
#[serial]
async fn pressure_response_force_kills_on_rss_limit_exceeded() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let (mut settings, seeds) = make_sidecar_settings();
    settings.subagent.resource_policy.monitor_interval_seconds = 1;
    let paths = BusytokPaths::for_test(tmp.path());
    seed_test_providers(&db, &seeds);
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    // Use mock sidecar with hard/soft limit = 1MB (sidecar RSS always exceeds this).
    let mut config = make_sidecar_config();
    config.memory_hard_limit_mb = 1;
    config.memory_soft_limit_mb = 1;

    let supervisor = BusytokSupervisor::new_with_sidecar_config(db, paths, config);

    // Delegate once to start the sidecar + supervision loop.
    let _ = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "pressure-test".to_string(),
            subagent_id: None,
            cwd: tmp.path().to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "trigger sidecar".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: Some(5),
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: Some("test-provider".to_string()),
            bound_model_id: Some("test-model".to_string()),
            reuse_policy: None,
        })
        .await;

    // Wait for the supervision loop to sample + detect hard limit + force-kill.
    // Poll for up to 10s for a sidecar_crash event.
    let mut crashed = false;
    for _ in 0..100 {
        let crashed_now = {
            let db_guard = supervisor.db_handle().lock().unwrap();
            let events = db_guard.subagent_list_resource_events(None, 200).unwrap();
            events.iter().any(|e| e.event_type == "sidecar_crash")
        };
        if crashed_now {
            crashed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        crashed,
        "sidecar_crash event must be written after hard-limit force-kill"
    );

    // Finding 2 fix: after force-kill, the PressureResponder clears the
    // gate to Resume so the next delegate() can lazy-restart the sidecar.
    // If the gate stayed paused, the system would deadlock with no path
    // to restart.
    let gate = supervisor.pressure_gate().expect("gate must be present");
    // Give the responder a brief moment to clear the gate after the kill.
    // The `sidecar_crash` event is written by `force_kill` BEFORE the gate
    // is cleared, so we may observe the event before the gate clear.
    let mut gate_cleared = false;
    for _ in 0..50 {
        if !gate.is_paused() {
            gate_cleared = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        gate_cleared,
        "gate must be cleared (Resume) after force-kill to allow lazy restart"
    );

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn pressure_response_pauses_on_memory_pressure() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let (mut settings, seeds) = make_sidecar_settings();
    settings.subagent.resource_policy.monitor_interval_seconds = 1;
    // Always pressured: 999999MB free threshold exceeds any real machine's
    // available memory, so `is_under_pressure` returns true on every sample.
    settings.subagent.resource_policy.memory_pressure_free_mb = 999_999;
    let paths = BusytokPaths::for_test(tmp.path());
    seed_test_providers(&db, &seeds);
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    // soft/hard limits high enough that the mock sidecar (~10MB RSS) does
    // NOT exceed them — so only `PauseNewTasks` fires, not GracefulRestart
    // or ForceKill.
    let mut config = make_sidecar_config();
    config.memory_soft_limit_mb = 800;
    config.memory_hard_limit_mb = 1200;

    let supervisor = BusytokSupervisor::new_with_sidecar_config(db, paths, config);

    // Delegate once to start the sidecar + supervision loop.
    let _ = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "pressure-pause-test".to_string(),
            subagent_id: None,
            cwd: tmp.path().to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "trigger sidecar".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: Some(5),
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: Some("test-provider".to_string()),
            bound_model_id: Some("test-model".to_string()),
            reuse_policy: None,
        })
        .await;

    // Wait for the supervision loop to sample + detect memory pressure +
    // trigger PauseNewTasks. Poll for up to 10s for the gate to be paused.
    let gate = supervisor.pressure_gate().expect("gate must be present");
    let mut paused = false;
    for _ in 0..100 {
        if gate.is_paused() {
            paused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        paused,
        "gate must be paused after memory pressure triggers PauseNewTasks"
    );

    // Verify a `memory_pressure` resource event was written (the escalation
    // DB event fires before the responder action).
    let events = {
        let db_guard = supervisor.db_handle().lock().unwrap();
        db_guard.subagent_list_resource_events(None, 200).unwrap()
    };
    assert!(
        events.iter().any(|e| e.event_type == "memory_pressure"),
        "memory_pressure event must be written on Normal→Pressure transition"
    );

    supervisor.shutdown_writer().await.unwrap();
}

#[tokio::test]
#[serial]
async fn pressure_response_graceful_restarts_on_soft_limit_exceeded() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let (mut settings, seeds) = make_sidecar_settings();
    settings.subagent.resource_policy.monitor_interval_seconds = 1;
    // Ensure is_under_pressure is false: system_available_mb is always >= 0,
    // so a 0 threshold means pressure is never triggered by available memory.
    settings.subagent.resource_policy.memory_pressure_free_mb = 0;
    let paths = BusytokPaths::for_test(tmp.path());
    seed_test_providers(&db, &seeds);
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    // soft limit = 1MB (mock sidecar ~5-10MB exceeds → GracefulRestart),
    // hard limit = 1200MB (mock sidecar does NOT exceed → no ForceKill).
    let mut config = make_sidecar_config();
    config.memory_soft_limit_mb = 1;
    config.memory_hard_limit_mb = 1200;

    let supervisor = BusytokSupervisor::new_with_sidecar_config(db, paths, config);

    // Delegate once to start the sidecar + supervision loop.
    let _ = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "graceful-restart-test".to_string(),
            subagent_id: None,
            cwd: tmp.path().to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "trigger sidecar".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: Some(5),
            model_override: None,
            source_harness: None,
            source_session_id: None,
            bound_provider_id: Some("test-provider".to_string()),
            bound_model_id: Some("test-model".to_string()),
            reuse_policy: None,
        })
        .await;

    // Wait for the supervision loop to sample + detect soft limit exceeded +
    // trigger GracefulRestart → shutdown_internal writes `sidecar_stop` event.
    // Poll for up to 15s (GracefulRestart involves prepare_hibernate_all RPC
    // + shutdown grace period, which takes longer than a simple force-kill).
    let mut stopped = false;
    for _ in 0..150 {
        let stopped_now = {
            let db_guard = supervisor.db_handle().lock().unwrap();
            let events = db_guard.subagent_list_resource_events(None, 200).unwrap();
            events.iter().any(|e| e.event_type == "sidecar_stop")
        };
        if stopped_now {
            stopped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        stopped,
        "sidecar_stop event must be written after GracefulRestart triggers shutdown_internal"
    );

    // After graceful shutdown, the responder clears the gate to Resume so the
    // next delegate() can lazy-restart the sidecar.
    let gate = supervisor.pressure_gate().expect("gate must be present");
    let mut gate_cleared = false;
    for _ in 0..50 {
        if !gate.is_paused() {
            gate_cleared = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        gate_cleared,
        "gate must be cleared (Resume) after GracefulRestart completes"
    );

    supervisor.shutdown_writer().await.unwrap();
}

// --- Phase 5 Task 3: service-owned pi_sidecar locator update (in-memory + disk) ---
//
// P1 regression guard: the service-owned update must mutate the in-memory
// Arc<Mutex<BusytokSettings>> (so the running daemon's worker pool sees the
// new locator immediately) AND persist to settings.toml (so a cold-start
// service reads it on its own startup). A direct file write would leave the
// in-memory state stale — the "file fixed, current session still can't find
// sidecar" state-drift bug.
//
// Mirrors the provider_update pattern (supervisor.rs): clone → mutate →
// save → swap. The pi_sidecar_state() test accessor reads from the SAME
// Arc<Mutex<BusytokSettings>> that pi_sidecar_locator_update swapped — if
// the swap didn't happen, the post-condition assertion fails.

#[tokio::test]
#[serial]
async fn pi_sidecar_locator_update_mutates_in_memory_and_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = BusytokSettings::default();
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let supervisor =
        BusytokSupervisor::with_adapters_and_settings(db, paths.clone(), vec![], settings);

    // Pre-condition: read the in-memory state via the test accessor
    // (the `settings` field is private; `settings_snapshot` RPC returns a
    // DTO that omits pi_sidecar.runtime_dir).
    let (pre_dir, pre_enabled) = supervisor.pi_sidecar_state();
    assert!(pre_dir.is_none());
    assert!(!pre_enabled);

    // Call the service-owned update (the method the GUI invokes via RPC).
    // Stage a COMPLETE sidecar dir (bundle.js + valid manifest.json + node
    // binary stub) so P2 validation passes (default node_runtime="bundled"
    // now requires node/<arch>/node to exist). The stub node binary is an
    // empty file — it's never executed because no providers are configured
    // (BusytokSettings::default() has empty providers list), so
    // construct_sidecar's `ensure_worker` is never called.
    let fake_dir = tmp.path().join("fake-pi-sidecar");
    std::fs::create_dir_all(&fake_dir).unwrap();
    std::fs::write(fake_dir.join("pi-sidecar.bundle.js"), "// stub").unwrap();
    let manifest = busytok_config::SidecarManifest {
        version: "1".to_string(),
        protocol_version: busytok_subagent::sidecar::protocol::PROTOCOL_VERSION,
        bundle: "pi-sidecar.bundle.js".to_string(),
        node_runtime_version: "22.6.0".to_string(),
    };
    std::fs::write(fake_dir.join("manifest.json"), manifest.to_json_string()).unwrap();
    // Stage stub node binary so P2 validation's node-arch + executability
    // checks pass (default node_runtime="bundled" requires node/<arch>/node
    // to exist AND be executable on Unix).
    let node_arch_dir = fake_dir.join("node").join(std::env::consts::ARCH);
    std::fs::create_dir_all(&node_arch_dir).unwrap();
    let node_path = node_arch_dir.join("node");
    std::fs::write(&node_path, "#!/bin/sh\n# stub\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&node_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&node_path, perms).unwrap();
    }
    let resp = supervisor
        .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
            runtime_dir: fake_dir.to_string_lossy().to_string(),
            enabled: true,
        })
        .await
        .unwrap();

    assert!(resp.in_memory_updated);
    assert_eq!(resp.runtime_dir, fake_dir.to_string_lossy());
    assert!(resp.enabled);

    // Verify in-memory state was updated (the P1 guard — no state drift).
    // pi_sidecar_state reads from the SAME Arc<Mutex<BusytokSettings>> that
    // pi_sidecar_locator_update swapped — if the swap didn't happen, this
    // assertion fails.
    let (post_dir, post_enabled) = supervisor.pi_sidecar_state();
    assert_eq!(post_dir.as_deref(), Some(fake_dir.to_str().unwrap()));
    assert!(post_enabled);

    // Verify the file was also persisted (cold-start path).
    let reloaded = BusytokSettings::load(&paths).unwrap();
    assert_eq!(
        reloaded.subagent.pi_sidecar.runtime_dir.as_deref(),
        Some(fake_dir.to_str().unwrap())
    );
    assert!(reloaded.subagent.pi_sidecar.enabled);

    supervisor.shutdown_writer().await.unwrap();
}
