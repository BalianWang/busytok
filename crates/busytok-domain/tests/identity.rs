use busytok_domain::{
    derive_project_hash, metadata_event_hash, normalize_project_path, MetadataFingerprint,
};

#[test]
fn normalizes_project_paths_before_hashing() {
    let a = normalize_project_path("/tmp/project/../project").unwrap();
    let b = normalize_project_path("/tmp/project").unwrap();
    assert_eq!(a, b);
    assert_eq!(derive_project_hash(&a), derive_project_hash(&b));
}

#[test]
fn fallback_session_id_is_source_file_id() {
    assert_eq!(
        busytok_domain::derive_session_id(None, "source-1"),
        "source-1"
    );
    assert_eq!(
        busytok_domain::derive_session_id(Some("s1"), "source-1"),
        "s1"
    );
}

#[test]
fn event_fingerprint_uses_metadata_only() {
    let a = MetadataFingerprint::new("claude-code", "session-a")
        .request_id("req-a")
        .message_id("msg-a")
        .tokens(100, 50);
    let b = a
        .clone()
        .ignored_content("secret prompt that must not affect the hash");
    assert_eq!(metadata_event_hash(&a), metadata_event_hash(&b));
}
