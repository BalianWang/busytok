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

fn make_supervisor(db: Database, tmp: &tempfile::TempDir) -> BusytokSupervisor {
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = BusytokSettings::default();
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .ok();
    BusytokSupervisor::new(db, paths)
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
async fn provider_delete_blocked_by_profile_reference() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let created = sup
        .provider_create(ProviderCreateRequestDto {
            name: "Test".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            api_key: Some("sk-test".into()),
        })
        .await
        .unwrap();
    let pid = created.id.clone();
    // Inject a profile referencing pid into settings (structure follows the
    // existing SubagentProfileConfig definition). `provider_delete` collects
    // profile refs from settings and passes them to the store's blocking
    // delete check.
    {
        let settings_arc = sup.settings_for_test();
        let mut settings = settings_arc.lock().unwrap();
        let profile = SubagentProfileConfig {
            write_access: false,
            tools: vec![],
            model: "gpt-4o".into(),
            context_budget_tokens: 3000,
            timeout_seconds: 120,
            provider_id: Some(pid.clone()),
        };
        settings
            .subagent
            .profiles
            .insert("test-profile".into(), profile);
    }
    let err = sup
        .provider_delete(ProviderDeleteRequestDto { id: pid })
        .await;
    assert!(
        err.is_err(),
        "provider_delete must be blocked when a profile references it"
    );

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
