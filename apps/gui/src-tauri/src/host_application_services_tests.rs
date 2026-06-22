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
use crate::host_application_services::HostServices;

#[test]
fn host_services_stores_endpoint() {
    let services = HostServices::new("/tmp/busytok-test.sock".into());
    assert_eq!(services.endpoint(), "/tmp/busytok-test.sock");
}

#[test]
fn invoke_meta_defaults() {
    let meta: crate::host_application_services::InvokeMeta = serde_json::from_str("{}").unwrap();
    assert!(meta.correlation_id().is_none());
    assert!(meta.session_id().is_none());
}

#[test]
fn invoke_meta_parses_fields() {
    let meta: crate::host_application_services::InvokeMeta =
        serde_json::from_str(r#"{"correlation_id":"abc","session_id":"xyz"}"#).unwrap();
    assert_eq!(meta.correlation_id().unwrap(), "abc");
    assert_eq!(meta.session_id().unwrap(), "xyz");
}
