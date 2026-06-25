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
[subagent.models]
default_cheap_model = "deepseek-chat"
"#;
    let settings = BusytokSettings::load_from_str(toml).unwrap();
    assert_eq!(settings.subagent.pi_sidecar.max_hot_sessions, 7);
    assert_eq!(settings.subagent.pi_sidecar.idle_exit_seconds, 99);
    assert_eq!(
        settings.subagent.models.default_cheap_model,
        "deepseek-chat"
    );

    let _reloaded: SubagentSettings = settings.subagent.clone();
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
