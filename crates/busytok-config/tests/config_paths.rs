use busytok_config::BusytokPaths;

#[test]
fn data_dir_is_busytok_under_xdg() {
    let paths = BusytokPaths::new();
    let data = paths.data_dir();
    assert!(data.to_string_lossy().contains("busytok"));
    assert!(!data.to_string_lossy().contains("autoken"));
}

#[test]
fn db_path_is_busytok_db() {
    let paths = BusytokPaths::new();
    let db = paths.db_path();
    assert!(db.to_string_lossy().contains("busytok"));
    assert!(db.to_string_lossy().ends_with(".db"));
}

#[test]
fn socket_path_is_busytok() {
    let paths = BusytokPaths::new();
    let endpoint = paths.control_endpoint().expect("control_endpoint should resolve");
    assert!(endpoint.contains("busytok"));
    assert!(!endpoint.contains("autoken"));
}

#[test]
fn log_dir_is_busytok() {
    let paths = BusytokPaths::new();
    let log = paths.log_dir();
    assert!(log.to_string_lossy().contains("busytok"));
}

#[test]
fn control_endpoint_is_a_named_pipe_on_windows_and_socket_path_on_unix() {
    let paths = BusytokPaths::new();
    let endpoint = paths.control_endpoint().expect("control_endpoint should resolve");
    assert!(endpoint.contains("busytok"), "endpoint should contain 'busytok': {endpoint}");
    assert!(!endpoint.contains("autoken"), "endpoint must not contain 'autoken': {endpoint}");

    #[cfg(unix)]
    {
        // Unix endpoints are filesystem socket paths that end with .sock
        // and live under the runtime dir.
        assert!(
            endpoint.ends_with("busytok.sock"),
            "unix endpoint should end with .sock: {endpoint}"
        );
        assert!(
            endpoint.contains(paths.runtime_dir().to_str().unwrap_or("")),
            "unix endpoint should live under runtime_dir: {endpoint}"
        );
    }

    #[cfg(windows)]
    {
        // Windows endpoints are named-pipe paths of the form
        // \\.\pipe\busytok-{user-sid}, where {user-sid} is non-empty and
        // begins with "S-1-".
        assert!(
            endpoint.starts_with(r"\\.\pipe\busytok-"),
            "windows endpoint should be a named pipe: {endpoint}"
        );
        let sid = endpoint.strip_prefix(r"\\.\pipe\busytok-").unwrap();
        assert!(
            sid.starts_with("S-1-") && sid.len() > "S-1-".len(),
            "windows endpoint should embed a non-empty SDDL SID: {endpoint}"
        );
    }
}
