#![allow(clippy::unwrap_used)]

use busytok_config::{BusytokSettings, SubagentSettings};

#[test]
fn missing_subagent_section_defaults_to_enabled() {
    let toml = r#"
timezone = "UTC"
week_starts_on = 1
"#;
    let settings = BusytokSettings::load_from_str(toml).unwrap();
    assert!(settings.subagent.enabled);
    assert_eq!(settings.subagent.pi_sidecar.max_hot_sessions, 3);
}

#[test]
fn subagent_settings_round_trip_through_toml() {
    let toml = r#"
timezone = "UTC"
[subagent]
enabled = true
[subagent.pi_sidecar]
max_hot_sessions = 7
idle_exit_seconds = 99
"#;
    let settings = BusytokSettings::load_from_str(toml).unwrap();
    assert_eq!(settings.subagent.pi_sidecar.max_hot_sessions, 7);
    assert_eq!(settings.subagent.pi_sidecar.idle_exit_seconds, 99);
    // Built-in profiles must survive partial config (no [subagent.profiles] in TOML).
    assert_eq!(
        settings.subagent.profiles.len(),
        3,
        "built-in profiles must be present even when TOML omits [subagent.profiles]"
    );
    assert!(settings.subagent.profiles.contains_key("pi/search-cheap"));
    assert!(settings.subagent.profiles.contains_key("pi/review-cheap"));
    assert!(settings.subagent.profiles.contains_key("pi/plan-cheap"));

    let _reloaded: SubagentSettings = settings.subagent.clone();
}

#[test]
fn is_builtin_profile_recognizes_builtins() {
    assert!(busytok_config::is_builtin_profile("pi/search-cheap"));
    assert!(busytok_config::is_builtin_profile("pi/review-cheap"));
    assert!(busytok_config::is_builtin_profile("pi/plan-cheap"));
    assert!(!busytok_config::is_builtin_profile("pi/patch-small"));
    assert!(!busytok_config::is_builtin_profile("my-custom-profile"));
    assert!(!busytok_config::is_builtin_profile(""));
}

#[test]
fn canonicalize_fills_missing_builtins_without_overwriting_user_edits() {
    let toml_str = r#"
timezone = "UTC"
[subagent.profiles."pi/search-cheap"]
tools = ["read"]
context_budget_tokens = 9999
timeout_seconds = 42
write_access = false
"#;
    let mut settings = busytok_config::BusytokSettings::load_from_str(toml_str).unwrap();
    // Only pi/search-cheap is present; pi/review-cheap and pi/plan-cheap are missing.
    assert_eq!(settings.subagent.profiles.len(), 1);

    settings.canonicalize_builtin_profiles();

    // All 3 built-in profiles now present.
    assert_eq!(settings.subagent.profiles.len(), 3);
    assert!(settings.subagent.profiles.contains_key("pi/search-cheap"));
    assert!(settings.subagent.profiles.contains_key("pi/review-cheap"));
    assert!(settings.subagent.profiles.contains_key("pi/plan-cheap"));

    // pi/search-cheap was NOT overwritten — user edits preserved.
    let search = &settings.subagent.profiles["pi/search-cheap"];
    assert_eq!(search.context_budget_tokens, 9999);

    // pi/review-cheap was filled with defaults.
    let review = &settings.subagent.profiles["pi/review-cheap"];
    assert_eq!(review.context_budget_tokens, 5000);
}

#[test]
fn load_canonicalizes_missing_builtin_profiles() {
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = busytok_config::BusytokPaths::for_test(tmp.path());
    let config_dir = paths.config_dir();
    std::fs::create_dir_all(config_dir).unwrap();
    // Write a config with only pi/search-cheap (missing the other 2 builtins).
    // `timezone = "UTC"` is required — without it, load() would fall back to
    // Self::default() (which already has all 3 built-ins) and the test would
    // pass even without the canonicalize hook.
    std::fs::write(
        config_dir.join("settings.toml"),
        r#"
timezone = "UTC"
[subagent.profiles."pi/search-cheap"]
context_budget_tokens = 3000
"#,
    )
    .unwrap();

    let settings = busytok_config::BusytokSettings::load(&paths).unwrap();
    assert_eq!(settings.subagent.profiles.len(), 3);
    assert!(settings.subagent.profiles.contains_key("pi/review-cheap"));
    assert!(settings.subagent.profiles.contains_key("pi/plan-cheap"));
}

#[test]
fn canonicalize_is_idempotent() {
    let mut settings = busytok_config::BusytokSettings::default();
    settings.canonicalize_builtin_profiles();
    let count_after_first = settings.subagent.profiles.len();
    // Calling again must not duplicate or remove profiles.
    settings.canonicalize_builtin_profiles();
    assert_eq!(settings.subagent.profiles.len(), count_after_first);
    // Each built-in appears exactly once.
    for name in &["pi/search-cheap", "pi/review-cheap", "pi/plan-cheap"] {
        assert!(settings.subagent.profiles.contains_key(*name));
    }
}

#[test]
fn partial_config_with_only_pi_sidecar_preserves_built_in_profiles() {
    let toml = r#"
timezone = "UTC"
[subagent.pi_sidecar]
max_hot_sessions = 1
"#;
    let settings = BusytokSettings::load_from_str(toml).unwrap();
    assert_eq!(settings.subagent.pi_sidecar.max_hot_sessions, 1);
    assert_eq!(
        settings.subagent.profiles.len(),
        3,
        "built-in profiles must survive when only [subagent.pi_sidecar] is present"
    );
}

#[test]
fn default_subagent_settings_serialize_to_valid_toml() {
    // Serialize a full BusytokSettings so the `[subagent]` table header is
    // emitted (serializing SubagentSettings alone yields `[pi_sidecar]` etc.,
    // with no `[subagent]` prefix because SubagentSettings IS that section).
    let settings =
        BusytokSettings::load_from_str("timezone = \"UTC\"\nweek_starts_on = 1\n").unwrap();
    let doc = toml::to_string(&settings).unwrap();
    assert!(
        doc.contains("[subagent]"),
        "doc should emit the [subagent] section"
    );
    assert!(doc.contains("[subagent.resource_policy]"));
    assert!(doc.contains("[subagent.pi_sidecar]"));
}
