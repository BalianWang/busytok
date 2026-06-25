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
use crate::updater::{init_updater_logging, parse_manifest_endpoint, parse_versions_manifest};

#[test]
fn init_updater_logging_does_not_panic() {
    init_updater_logging();
}

#[test]
fn parse_versions_manifest_parses_entries_in_order() {
    let body = r#"{"versions":[
        {"version":"v0.0.2","date":"2026-06-25T04:41:40Z","notes":"Busytok 0.0.2","manifest_url":"https://github.com/x/y/releases/download/v0.0.2/latest.json"},
        {"version":"v0.0.1","date":"2026-06-24T14:22:10Z","notes":"Busytok 0.0.1","manifest_url":"https://github.com/x/y/releases/download/v0.0.1/latest.json"}
    ]}"#;
    let entries = parse_versions_manifest(body).expect("valid manifest");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].version, "v0.0.2");
    assert_eq!(
        entries[0].manifest_url,
        "https://github.com/x/y/releases/download/v0.0.2/latest.json"
    );
    assert_eq!(entries[1].version, "v0.0.1");
    assert_eq!(entries[1].notes, "Busytok 0.0.1");
}

#[test]
fn parse_versions_manifest_empty_list() {
    let entries = parse_versions_manifest(r#"{"versions":[]}"#).expect("empty list");
    assert!(entries.is_empty());
}

#[test]
fn parse_versions_manifest_missing_versions_key_defaults_to_empty() {
    // Defensive: a manifest object without a "versions" key yields an empty
    // list rather than erroring (so a malformed manifest degrades to "0
    // versions" instead of "Unavailable").
    let entries = parse_versions_manifest(r#"{}"#).expect("defaults to empty");
    assert!(entries.is_empty());
}

#[test]
fn parse_versions_manifest_rejects_malformed_json() {
    assert!(parse_versions_manifest("not json").is_err());
}

#[test]
fn parse_versions_manifest_rejects_non_object_top_level() {
    assert!(parse_versions_manifest(r#"[1,2,3]"#).is_err());
}

#[test]
fn parse_versions_manifest_rejects_wrong_field_type() {
    // A field with the wrong JSON type (number where a string is expected)
    // must be rejected by serde rather than silently coerced.
    assert!(parse_versions_manifest(r#"{"versions": [{"version": 123}]}"#).is_err());
}

#[test]
fn parse_manifest_endpoint_accepts_https_url() {
    let eps =
        parse_manifest_endpoint("https://github.com/x/y/releases/download/v0.1.0/latest.json")
            .expect("valid url");
    assert_eq!(eps.len(), 1);
    assert_eq!(
        eps[0].as_str(),
        "https://github.com/x/y/releases/download/v0.1.0/latest.json"
    );
}

#[test]
fn parse_manifest_endpoint_rejects_garbage() {
    assert!(parse_manifest_endpoint("not a url").is_err());
}

#[test]
fn parse_manifest_endpoint_accepts_http_url() {
    // url::Url parses "http://..." fine; the updater's endpoints() validator
    // enforces HTTPS. We only assert parsing here; the HTTPS gate is the plugin's.
    let eps = parse_manifest_endpoint("http://insecure.example/latest.json").expect("parses");
    assert_eq!(eps.len(), 1);
}
