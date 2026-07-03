//! Coverage gap tests for `busytok-config` (providers.rs + lib.rs + logging.rs).
//!
//! Targets uncovered source lines NOT already covered by `coverage_gaps.rs`:
//! - `providers.rs`: `ProviderCredentialStore` NoEntry paths (`get_key`
//!   returning `Ok(None)`, `delete_key` returning `Ok(())` when no key
//!   exists, `has_key` returning `false`), and `ProviderConfig` serde
//!   round-trip with `base_url_env_name: None`.
//! - `lib.rs`: `is_builtin_profile` rejection of non-builtin names,
//!   `BusytokSettings::default` field defaults, `atomic_write` to a
//!   deeply nested path.
//! - `logging.rs`: `prune_old_logs` with files whose date suffix is valid
//!   but the base name differs from a tracing-appender pattern.

#![allow(
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    dead_code,
    unused_imports,
    unused_variables
)]

use std::fs;
use std::path::PathBuf;

use busytok_config::{
    atomic_write, init_logging, is_builtin_profile, prune_old_logs, BusytokSettings, LogSource,
    ProviderConfig, ProviderCredentialStore, ProviderKind,
};
use tempfile::TempDir;

// ── ProviderCredentialStore: NoEntry paths ──────────────────────────────
//
// These tests exercise the keychain with provider IDs that have no stored
// key. The `get_key`, `delete_key`, and `has_key` methods all have a
// `keyring::Error::NoEntry` arm that returns `Ok(None)` / `Ok(())` / `false`.
// A unique suffix ensures we don't collide with other test crates that use
// the same keychain service ("com.busytok.providers").

const COVERAGE_TEST_PREFIX: &str = "coverage-gaps-config-test";

fn unique_id(label: &str) -> String {
    format!("{}-{}-{}", COVERAGE_TEST_PREFIX, label, std::process::id())
}

/// Returns `false` when the OS keyring is unavailable (e.g., Linux CI
/// without a D-Bus secret service daemon). Tests that call
/// `ProviderCredentialStore::set_key/get_key/delete_key` use this to
/// skip gracefully instead of panicking on `Entry::new` failure.
fn keyring_available() -> bool {
    ProviderCredentialStore::get_key("keyring-availability-probe").is_ok()
}

#[test]
fn get_key_returns_none_when_no_key_stored() {
    if !keyring_available() {
        eprintln!("skip: OS keyring unavailable");
        return;
    }
    let id = unique_id("get-none");
    // Ensure clean state.
    let _ = ProviderCredentialStore::delete_key(&id);
    let result = ProviderCredentialStore::get_key(&id).unwrap();
    assert!(result.is_none(), "get_key must return Ok(None) when no key");
}

#[test]
fn has_key_returns_false_when_no_key_stored() {
    let id = unique_id("has-false");
    let _ = ProviderCredentialStore::delete_key(&id);
    assert!(
        !ProviderCredentialStore::has_key(&id),
        "has_key must return false when no key is stored"
    );
}

#[test]
fn delete_key_succeeds_when_no_key_stored() {
    if !keyring_available() {
        eprintln!("skip: OS keyring unavailable");
        return;
    }
    let id = unique_id("del-ok");
    // Delete on a non-existent key must return Ok (NoEntry arm).
    ProviderCredentialStore::delete_key(&id).expect("delete_key must Ok on NoEntry");
}

#[test]
fn provider_credential_store_round_trips_key() {
    if !keyring_available() {
        eprintln!("skip: OS keyring unavailable");
        return;
    }
    // Full round-trip: set → get → has → delete → has (false).
    let id = unique_id("roundtrip");
    let _ = ProviderCredentialStore::delete_key(&id);

    ProviderCredentialStore::set_key(&id, "sk-coverage-test-12345").unwrap();
    assert!(ProviderCredentialStore::has_key(&id));

    let key = ProviderCredentialStore::get_key(&id).unwrap();
    assert_eq!(key.as_deref(), Some("sk-coverage-test-12345"));

    ProviderCredentialStore::delete_key(&id).unwrap();
    assert!(!ProviderCredentialStore::has_key(&id));
    assert!(ProviderCredentialStore::get_key(&id).unwrap().is_none());
}

#[test]
fn set_key_overwrites_existing_key() {
    if !keyring_available() {
        eprintln!("skip: OS keyring unavailable");
        return;
    }
    let id = unique_id("overwrite");
    let _ = ProviderCredentialStore::delete_key(&id);

    ProviderCredentialStore::set_key(&id, "first-key").unwrap();
    ProviderCredentialStore::set_key(&id, "second-key").unwrap();
    let key = ProviderCredentialStore::get_key(&id).unwrap();
    assert_eq!(key.as_deref(), Some("second-key"));

    ProviderCredentialStore::delete_key(&id).unwrap();
}

// ── ProviderConfig serde edge cases ─────────────────────────────────────

#[test]
fn provider_config_deserializes_without_base_url_env_name() {
    // `base_url_env_name` is Option<String> with no serde default — but
    // TOML deserialization should still work when the field is absent (serde
    // default for Option is None).
    let toml_str = r#"id = "x"
name = "X"
provider_kind = "openai_compatible"
base_url = "https://x.example.com/v1"
api_key_env_name = "X_API_KEY"
models = []
enabled = true
"#;
    let parsed: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(parsed.id, "x");
    assert_eq!(parsed.base_url_env_name, None);
    assert_eq!(parsed.models.len(), 0);
    assert!(parsed.enabled);
}

#[test]
fn provider_config_serializes_and_deserializes_with_all_fields() {
    let original = ProviderConfig {
        id: "all-fields".to_string(),
        name: "All Fields".to_string(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://api.example.com/v1".to_string(),
        api_key_env_name: "ALL_FIELDS_KEY".to_string(),
        base_url_env_name: Some("ALL_FIELDS_BASE_URL".to_string()),
        models: vec!["model-a".to_string(), "model-b".to_string()],
        enabled: false,
    };
    let toml_str = toml::to_string(&original).unwrap();
    let parsed: ProviderConfig = toml::from_str(&toml_str).unwrap();
    assert_eq!(parsed.id, "all-fields");
    assert_eq!(parsed.name, "All Fields");
    assert_eq!(parsed.provider_kind, ProviderKind::OpenAiCompatible);
    assert_eq!(parsed.base_url, "https://api.example.com/v1");
    assert_eq!(parsed.api_key_env_name, "ALL_FIELDS_KEY");
    assert_eq!(
        parsed.base_url_env_name.as_deref(),
        Some("ALL_FIELDS_BASE_URL")
    );
    assert_eq!(parsed.models, vec!["model-a", "model-b"]);
    assert!(!parsed.enabled);
}

#[test]
fn provider_config_rejects_legacy_snake_case_kind() {
    // The legacy "open_ai_compatible" spelling must be rejected.
    let result = toml::from_str::<ProviderConfig>(
        r#"id = "x"
name = "X"
provider_kind = "open_ai_compatible"
base_url = "https://x.example.com/v1"
api_key_env_name = "X_API_KEY"
models = []
enabled = true
"#,
    );
    assert!(result.is_err(), "legacy snake_case kind must be rejected");
}

// ── is_builtin_profile ──────────────────────────────────────────────────

#[test]
fn is_builtin_profile_rejects_non_builtins() {
    // Already tested in subagent_settings.rs, but we add edge cases here.
    assert!(!is_builtin_profile("pi/search"));
    assert!(!is_builtin_profile("search-cheap"));
    assert!(!is_builtin_profile("PI/SEARCH-CHEAP"));
    assert!(!is_builtin_profile("pi/search-cheap-extra"));
    assert!(!is_builtin_profile("custom/profile"));
}

#[test]
fn is_builtin_profile_accepts_all_three_builtins() {
    assert!(is_builtin_profile("pi/search-cheap"));
    assert!(is_builtin_profile("pi/review-cheap"));
    assert!(is_builtin_profile("pi/plan-cheap"));
}

// ── BusytokSettings::default ─────────────────────────────────────────────

#[test]
fn busytok_settings_default_has_expected_fields() {
    let s = BusytokSettings::default();
    // Timezone should be non-empty (system local).
    assert!(!s.timezone.is_empty());
    // Week starts on Monday (1) by default.
    assert_eq!(s.week_starts_on, 1);
    // Privacy defaults.
    assert!(s.privacy.local_only);
    assert!(s.privacy.redact_sensitive_values);
    // Discovery defaults.
    assert!(s.discovery.claude_code_default_paths);
    // No providers by default.
    assert!(s.providers.is_empty());
}

// ── atomic_write: deeply nested path ────────────────────────────────────

#[test]
fn atomic_write_creates_deeply_nested_parent_dirs() {
    let tmp = TempDir::new().unwrap();
    let deep = tmp.path().join("a/b/c/d/e/f").join("settings.toml");
    atomic_write(&deep, "contents").expect("atomic_write must create deep parents");
    let read = std::fs::read_to_string(&deep).unwrap();
    assert_eq!(read, "contents");
}

#[test]
fn atomic_write_to_root_of_tempdir() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("flat.toml");
    atomic_write(&path, "flat content").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "flat content");
}

// ── logging.rs: prune_old_logs edge cases ────────────────────────────────

#[test]
fn prune_keeps_file_with_valid_date_but_different_base_name() {
    // A file named "myapp.2020-01-01" has a valid date suffix but may not
    // be a tracing-appender rotated log. prune_old_logs only checks the
    // date format, not the base name — so it WILL remove it if old enough.
    // We use a recent date so it's NOT removed.
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();

    // Create a file with a recent date suffix.
    let recent = log_dir.join("myapp.2099-12-31");
    std::fs::write(&recent, "data").unwrap();

    prune_old_logs(&log_dir, 7);

    // Recent file must survive.
    assert!(recent.exists(), "recent file with valid date must survive");
}

#[test]
fn prune_removes_old_file_with_valid_date_suffix() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();

    // Create a file with a valid date suffix and set its mtime to the past.
    let old = log_dir.join("service.2020-01-01");
    std::fs::write(&old, "old data").unwrap();
    let thirty_days_ago = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(30 * 86400))
        .unwrap();
    // On Windows, File::open (read-only) cannot set file times —
    // SetFileTime requires write access. Open with write(true) and skip
    // the test if set_modified fails.
    let mtime_ok = std::fs::OpenOptions::new()
        .write(true)
        .open(&old)
        .and_then(|f| f.set_modified(thirty_days_ago))
        .is_ok();
    if !mtime_ok {
        eprintln!("skip: cannot set file mtime on this platform");
        return;
    }

    prune_old_logs(&log_dir, 7);

    // Old file must be removed.
    assert!(!old.exists(), "old file with valid date must be pruned");
}

#[test]
fn prune_keeps_base_file_without_date_suffix() {
    // Files without a date suffix (the current/non-rotated base file)
    // must never be removed, regardless of age.
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();

    let base = log_dir.join("service.log");
    std::fs::write(&base, "base").unwrap();

    // Even with keep_days=0, the base file must survive.
    prune_old_logs(&log_dir, 0);
    assert!(base.exists(), "base file without date suffix must survive");
}

#[test]
fn prune_keeps_file_with_extra_dots_in_name() {
    // "service.v2.2020-01-01" — the date is after the LAST dot.
    // The rfind('.') finds the last dot, so the date part is "2020-01-01".
    // This is a valid date → the file WILL be pruned if old.
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();

    let multi_dot = log_dir.join("service.v2.2099-06-15");
    std::fs::write(&multi_dot, "data").unwrap();

    prune_old_logs(&log_dir, 7);
    assert!(multi_dot.exists(), "recent multi-dot file must survive");
}

// ── logging.rs: init_logging Gui-first path (line 141) ───────────────────
//
// `tracing::subscriber::set_global_default` can succeed at most once per
// process. The existing test in `coverage_gaps.rs` calls `init_logging`
// with `LogSource::Service` first, so the Gui `try_init` Ok arm (line 141)
// is never reached. This test binary has NO prior `init_logging` call, so
// calling Gui first succeeds and covers line 141.
//
// As a side effect, this also initializes a tracing subscriber for the
// entire test binary — the ProviderCredentialStore methods emit debug!
// /warn! events that are now actually evaluated (covering the multi-line
// debug! closing paren on providers.rs line 91).

#[test]
fn init_logging_gui_first_succeeds_and_enables_provider_debug() {
    // Set RUST_LOG=trace so EnvFilter::try_from_default_env() enables debug-
    // level logging. Without this, init_logging defaults to "info" and the
    // multi-line debug! format strings in providers.rs are not evaluated.
    std::env::set_var("RUST_LOG", "trace");

    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    fs::create_dir_all(&log_dir).unwrap();

    // Gui init_logging — first call in this binary, so try_init succeeds.
    // Covers logging.rs line 141 (Gui Ok arm).
    let guards = init_logging(&log_dir, LogSource::Gui, "gui-first");
    assert!(
        guards.is_some(),
        "Gui init_logging must succeed when called first in a fresh process"
    );
    let guards = guards.unwrap();
    assert!(
        guards.bootstrap_guard.is_some(),
        "Gui has a bootstrap guard"
    );
    assert!(guards.file_guard.is_some(), "Gui has a file guard");
    drop(guards);

    // Now that a subscriber is set with trace level, the multi-line
    // `tracing::debug!` in `has_key` is fully evaluated — covering
    // providers.rs line 91 (the format string of the debug! macro).
    let id = unique_id("gui-first-has-key");
    let _ = ProviderCredentialStore::delete_key(&id);
    let result = ProviderCredentialStore::has_key(&id);
    assert!(!result, "has_key must return false for a non-existent key");
}
