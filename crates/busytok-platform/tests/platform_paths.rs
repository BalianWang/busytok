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
use busytok_platform::PlatformPaths;

#[test]
#[ignore = "CI runners have a different home/data layout; passes on macOS dev"]
fn service_identifier_is_busytok() {
    let paths = PlatformPaths::new();
    assert_eq!(paths.service_identifier(), "com.busytok.service");
}

#[test]
fn busytok_data_dir_contains_busytok() {
    let paths = PlatformPaths::new();
    let dir = paths.busytok_data_dir();
    assert!(dir.to_string_lossy().contains("busytok"));
}

#[test]
fn platform_paths_match_shared_busytok_paths() {
    let platform = PlatformPaths::new();
    let shared = busytok_config::BusytokPaths::new();

    assert_eq!(platform.busytok_data_dir(), *shared.data_dir());
    assert_eq!(platform.busytok_db_path(), shared.db_path());
}
