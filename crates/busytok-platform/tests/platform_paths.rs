use busytok_platform::PlatformPaths;

#[test]
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
