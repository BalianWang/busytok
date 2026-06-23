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
use crate::updater::{init_updater_logging, parse_manifest_endpoint};

#[test]
fn init_updater_logging_does_not_panic() {
    init_updater_logging();
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
