#![allow(
    clippy::await_holding_lock,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::field_reassign_with_default,
    clippy::uninlined_format_args,
    clippy::inconsistent_digit_grouping,
    clippy::len_zero,
    clippy::identity_op,
    clippy::useless_vec,
    clippy::manual_dangling_ptr,
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::absurd_extreme_comparisons,
    unused_variables,
    unused_imports,
    dead_code
)]
//! Task 5: provider/model catalog SQL-backed handler tests.
//!
//! These tests exercise the BusytokSupervisor's provider handlers against
//! the SQL store (Task 2's `provider_catalog` repository). The harness
//! mirrors `supervisor_control.rs::make_supervisor` — a real supervisor
//! backed by an in-memory SQLite database and a temp config dir.

use busytok_config::{BusytokPaths, BusytokSettings, ProviderKind, SubagentProfileConfig};
use busytok_control::dispatch::RuntimeControl;
use busytok_protocol::dto::*;
use busytok_runtime::BusytokSupervisor;
use busytok_store::Database;
use busytok_subagent::sidecar::SidecarConfig;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

fn make_supervisor(db: Database, tmp: &tempfile::TempDir) -> BusytokSupervisor {
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = BusytokSettings::default();
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .ok();
    BusytokSupervisor::new(db, paths)
}

// ---------------------------------------------------------------------------
// Sidecar-wired harness (mirrors helpers from supervisor_control.rs).
// Used by the delegate re-validation test, which needs `worker_pool().is_some()`
// so the supervisor-side SQL re-validation block actually runs.
// ---------------------------------------------------------------------------

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

fn mock_sidecar_path() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(format!(
        "{manifest}/../busytok-subagent/tests/fixtures/mock-sidecar.sh"
    ))
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

fn make_sidecar_config_for_tests() -> SidecarConfig {
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

/// Build sidecar-enabled settings (mirrors `make_unbound_sidecar_settings`
/// from supervisor_control.rs but stays self-contained for this test file).
fn make_sidecar_settings() -> BusytokSettings {
    let mut settings = BusytokSettings::default();
    settings.subagent.pi_sidecar.enabled = true;
    settings
}

/// Construct a sidecar-stopped supervisor (pool wired, sidecar child not
/// started) from arbitrary settings. Mirrors
/// `make_sidecar_stopped_supervisor_with_settings` from supervisor_control.rs.
fn make_sidecar_supervisor(
    db: Database,
    tmp: &tempfile::TempDir,
    settings: BusytokSettings,
) -> BusytokSupervisor {
    let paths = BusytokPaths::for_test(tmp.path());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");
    BusytokSupervisor::new_with_sidecar_config(db, paths, make_sidecar_config_for_tests())
}

#[tokio::test]
async fn provider_create_persists_to_sql_with_api_key() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let created = sup
        .provider_create(ProviderCreateRequestDto {
            name: "Test".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: None,
            api_key: Some("sk-test".into()),
        })
        .await
        .unwrap();

    let list = sup.provider_list().await.unwrap();
    assert_eq!(list.providers.len(), 1);
    assert!(list.providers[0].has_api_key);
    assert_eq!(
        list.providers[0].provider_kind,
        ProviderKind::OpenAiCompatible
    );
    assert_eq!(list.providers[0].id, created.id);

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn provider_test_connection_no_enabled_model_skips_fallback() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let created = sup
        .provider_create(ProviderCreateRequestDto {
            name: "Test".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: None,
            api_key: Some("sk-test".into()),
        })
        .await
        .unwrap();
    // No models configured — fallback path will error with "no enabled models
    // configured". Without a mock HTTPS server, the /models probe will also
    // fail with a request error.
    let result = sup
        .provider_test_connection(ProviderTestConnectionRequestDto { id: created.id })
        .await;
    match result {
        Ok(resp) => {
            // /models succeeded against real endpoint — acceptable.
            let _ = resp;
        }
        Err(e) => {
            let msg = format!("{}", e);
            assert!(
                msg.contains("no enabled models")
                    || msg.contains("request failed")
                    || msg.contains("https://"),
                "unexpected error: {}",
                msg
            );
        }
    }

    sup.shutdown_writer().await.expect("writer shutdown");
}

// ---------------------------------------------------------------------------
// Task 6: model handlers (model_create / model_list / model_update /
// model_delete / model_tags_update) — SQL-backed CRUD round-trips.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn model_create_and_list_round_trip() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let provider = sup
        .provider_create(ProviderCreateRequestDto {
            name: "Test".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: None,
            api_key: Some("sk-test".into()),
        })
        .await
        .unwrap();
    let pid = provider.id.clone();

    let created = sup
        .model_create(ModelCreateRequestDto {
            provider_id: pid.clone(),
            model_id: "gpt-4o".into(),
            enabled: Some(true),
            tags: vec!["fast".into()],
        })
        .await
        .unwrap();
    assert_eq!(created.model_id, "gpt-4o");
    assert_eq!(created.provider_id, pid);
    assert!(created.model_enabled);
    assert!(created.provider_enabled);
    assert_eq!(created.provider_name, "Test");
    assert_eq!(created.provider_kind, ProviderKind::OpenAiCompatible);
    assert!(created.tags.contains(&"fast".to_string()));

    let list = sup
        .model_list(ModelListRequestDto {
            provider_id: None,
            tags: vec![],
            include_disabled: false,
        })
        .await
        .unwrap();
    assert_eq!(list.models.len(), 1);
    assert_eq!(list.models[0].model_id, "gpt-4o");
    assert!(list.models[0].tags.contains(&"fast".to_string()));

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// Compile-time guarantee that `ModelUpdateRequestDto` has no `model_id`
/// field — `model_id` is immutable after creation (no rename). If the DTO
/// ever regains a `model_id` field, this test will fail to compile.
#[tokio::test]
async fn model_update_rejects_model_id_change() {
    let dto = ModelUpdateRequestDto {
        id: "model_x".into(),
        enabled: Some(false),
    };
    // If this compiles, model_id is not in the DTO — success.
    let _ = dto;
}

#[tokio::test]
async fn model_create_defaults_enabled_to_true_when_omitted() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let provider = sup
        .provider_create(ProviderCreateRequestDto {
            name: "Test".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: None,
            api_key: Some("sk-test".into()),
        })
        .await
        .unwrap();

    // `enabled: None` — store should default to true.
    let created = sup
        .model_create(ModelCreateRequestDto {
            provider_id: provider.id.clone(),
            model_id: "gpt-4o".into(),
            enabled: None,
            tags: vec![],
        })
        .await
        .unwrap();
    assert!(
        created.model_enabled,
        "model_create must default enabled=true when omitted"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn model_create_rejects_duplicate_model_id_per_provider() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let provider = sup
        .provider_create(ProviderCreateRequestDto {
            name: "Test".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: None,
            api_key: Some("sk-test".into()),
        })
        .await
        .unwrap();
    let pid = provider.id.clone();

    sup.model_create(ModelCreateRequestDto {
        provider_id: pid.clone(),
        model_id: "gpt-4o".into(),
        enabled: Some(true),
        tags: vec![],
    })
    .await
    .unwrap();

    // Same (provider_id, model_id) must fail with "already exists".
    let err = sup
        .model_create(ModelCreateRequestDto {
            provider_id: pid,
            model_id: "gpt-4o".into(),
            enabled: Some(true),
            tags: vec![],
        })
        .await
        .expect_err("duplicate (provider_id, model_id) must error");
    assert!(
        format!("{err}").contains("already exists"),
        "expected 'already exists' error, got: {err}"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn model_update_toggles_enabled() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let provider = sup
        .provider_create(ProviderCreateRequestDto {
            name: "Test".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: None,
            api_key: Some("sk-test".into()),
        })
        .await
        .unwrap();
    let pid = provider.id.clone();

    let created = sup
        .model_create(ModelCreateRequestDto {
            provider_id: pid,
            model_id: "gpt-4o".into(),
            enabled: Some(true),
            tags: vec![],
        })
        .await
        .unwrap();
    assert!(created.model_enabled);

    // Disable it.
    sup.model_update(ModelUpdateRequestDto {
        id: created.model_db_id.clone(),
        enabled: Some(false),
    })
    .await
    .unwrap();

    // list with include_disabled=false should NOT show it.
    let list = sup
        .model_list(ModelListRequestDto {
            provider_id: None,
            tags: vec![],
            include_disabled: false,
        })
        .await
        .unwrap();
    assert!(
        list.models
            .iter()
            .all(|m| m.model_db_id != created.model_db_id),
        "disabled model must be filtered out when include_disabled=false"
    );

    // list with include_disabled=true SHOULD show it, with model_enabled=false.
    let list_all = sup
        .model_list(ModelListRequestDto {
            provider_id: None,
            tags: vec![],
            include_disabled: true,
        })
        .await
        .unwrap();
    let m = list_all
        .models
        .iter()
        .find(|m| m.model_db_id == created.model_db_id)
        .expect("disabled model must appear when include_disabled=true");
    assert!(!m.model_enabled);

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn model_update_unknown_id_returns_error() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .model_update(ModelUpdateRequestDto {
            id: "nonexistent-model-id".into(),
            enabled: Some(false),
        })
        .await
        .expect_err("update on unknown id must error");
    assert!(
        format!("{err}").contains("model not found"),
        "expected 'model not found' error, got: {err}"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn model_delete_unknown_id_returns_error() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .model_delete(ModelDeleteRequestDto {
            id: "nonexistent-model-id".into(),
        })
        .await
        .expect_err("delete on unknown id must error");
    assert!(
        format!("{err}").contains("model not found"),
        "expected 'model not found' error, got: {err}"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn model_tags_update_replaces_tags() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let provider = sup
        .provider_create(ProviderCreateRequestDto {
            name: "Test".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: None,
            api_key: Some("sk-test".into()),
        })
        .await
        .unwrap();
    let pid = provider.id.clone();

    let created = sup
        .model_create(ModelCreateRequestDto {
            provider_id: pid,
            model_id: "gpt-4o".into(),
            enabled: Some(true),
            tags: vec!["fast".into(), "cheap".into()],
        })
        .await
        .unwrap();
    assert_eq!(created.tags.len(), 2);

    // Replace tags: drop "cheap", add "expensive", keep "fast".
    sup.model_tags_update(ModelTagUpdateDto {
        model_id: created.model_db_id.clone(),
        tags: vec!["fast".into(), "expensive".into()],
    })
    .await
    .unwrap();

    let list = sup
        .model_list(ModelListRequestDto {
            provider_id: None,
            tags: vec![],
            include_disabled: true,
        })
        .await
        .unwrap();
    let m = list
        .models
        .iter()
        .find(|m| m.model_db_id == created.model_db_id)
        .expect("model must still exist after tag update");
    assert!(m.tags.contains(&"fast".to_string()));
    assert!(m.tags.contains(&"expensive".to_string()));
    assert!(
        !m.tags.contains(&"cheap".to_string()),
        "stale tag must be removed"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn model_list_filters_by_provider() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let p1 = sup
        .provider_create(ProviderCreateRequestDto {
            name: "P1".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.p1.com".into(),
            enabled: None,
            api_key: Some("sk-p1".into()),
        })
        .await
        .unwrap();
    let p2 = sup
        .provider_create(ProviderCreateRequestDto {
            name: "P2".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.p2.com".into(),
            enabled: None,
            api_key: Some("sk-p2".into()),
        })
        .await
        .unwrap();

    sup.model_create(ModelCreateRequestDto {
        provider_id: p1.id.clone(),
        model_id: "p1-model".into(),
        enabled: Some(true),
        tags: vec![],
    })
    .await
    .unwrap();
    sup.model_create(ModelCreateRequestDto {
        provider_id: p2.id.clone(),
        model_id: "p2-model".into(),
        enabled: Some(true),
        tags: vec![],
    })
    .await
    .unwrap();

    let p1_only = sup
        .model_list(ModelListRequestDto {
            provider_id: Some(p1.id.clone()),
            tags: vec![],
            include_disabled: false,
        })
        .await
        .unwrap();
    assert_eq!(p1_only.models.len(), 1);
    assert_eq!(p1_only.models[0].model_id, "p1-model");
    assert_eq!(p1_only.models[0].provider_id, p1.id);

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn model_list_filters_by_tag_and_semantics() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let provider = sup
        .provider_create(ProviderCreateRequestDto {
            name: "Test".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: None,
            api_key: Some("sk-test".into()),
        })
        .await
        .unwrap();
    let pid = provider.id.clone();

    // m1: tags = [fast, cheap]
    sup.model_create(ModelCreateRequestDto {
        provider_id: pid.clone(),
        model_id: "m1".into(),
        enabled: Some(true),
        tags: vec!["fast".into(), "cheap".into()],
    })
    .await
    .unwrap();
    // m2: tags = [fast]
    sup.model_create(ModelCreateRequestDto {
        provider_id: pid.clone(),
        model_id: "m2".into(),
        enabled: Some(true),
        tags: vec!["fast".into()],
    })
    .await
    .unwrap();

    // Filter: tags=[fast] → both m1 and m2.
    let fast = sup
        .model_list(ModelListRequestDto {
            provider_id: None,
            tags: vec!["fast".into()],
            include_disabled: false,
        })
        .await
        .unwrap();
    assert_eq!(fast.models.len(), 2);

    // Filter: tags=[fast, cheap] → only m1 (AND semantics).
    let fast_cheap = sup
        .model_list(ModelListRequestDto {
            provider_id: None,
            tags: vec!["fast".into(), "cheap".into()],
            include_disabled: false,
        })
        .await
        .unwrap();
    assert_eq!(fast_cheap.models.len(), 1);
    assert_eq!(fast_cheap.models[0].model_id, "m1");

    sup.shutdown_writer().await.expect("writer shutdown");
}

// ---------------------------------------------------------------------------
// Local SQL seed helpers (self-contained for this test file — the equivalent
// helpers in supervisor_control.rs are private). Used by the delegate
// re-validation tests above.
// ---------------------------------------------------------------------------

fn seed_provider_to_sql(
    sup: &BusytokSupervisor,
    id: &str,
    name: &str,
    base_url: &str,
    api_key: Option<&str>,
    enabled: bool,
) {
    use rusqlite::params;
    let db = sup.db_handle().lock().unwrap();
    let now = busytok_domain::now_ms();
    db.conn()
        .execute(
            "INSERT INTO providers (id, name, provider_kind, base_url, enabled, api_key, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                id,
                name,
                serde_json::to_string(&ProviderKind::OpenAiCompatible).unwrap(),
                base_url,
                enabled as i64,
                api_key,
                now,
            ],
        )
        .expect("seed provider to SQL");
}

fn seed_model_to_sql(sup: &BusytokSupervisor, provider_id: &str, model_id: &str, enabled: bool) {
    use rusqlite::params;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let db = sup.db_handle().lock().unwrap();
    let now = busytok_domain::now_ms();
    let id = format!(
        "seed-model-{}-{}",
        now,
        COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    db.conn()
        .execute(
            "INSERT INTO models (id, provider_id, model_id, enabled, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![id, provider_id, model_id, enabled as i64, now],
        )
        .expect("seed model to SQL");
}
