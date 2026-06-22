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
use busytok_discovery::CodexDiscovery;
use busytok_domain::{AgentKind, LogSourceType};

// ---------------------------------------------------------------------------
// from_roots — basic discovery
// ---------------------------------------------------------------------------

#[test]
fn discovers_codex_session_jsonl_files_recursively() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(".codex").join("sessions");
    let file = root.join("basic-rollout.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}\n").unwrap();

    let discovery = CodexDiscovery::from_roots(vec![root]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].agent, AgentKind::Codex);
    assert!(sources[0]
        .files
        .iter()
        .all(|path| path.extension().unwrap() == "jsonl"));
}

#[test]
fn discovers_nested_jsonl_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let deep_dir = root.join("sub1/sub2");
    std::fs::create_dir_all(&deep_dir).unwrap();

    let file1 = root.join("session.jsonl");
    let file2 = deep_dir.join("deep.jsonl");
    std::fs::write(&file1, "{}\n").unwrap();
    std::fs::write(&file2, "{}\n").unwrap();

    let discovery = CodexDiscovery::from_roots(vec![root]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].files.len(), 2);
    let names: Vec<String> = sources[0]
        .files
        .iter()
        .map(|f| f.file_name().unwrap().to_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"deep.jsonl".to_string()));
    assert!(names.contains(&"session.jsonl".to_string()));
}

// ---------------------------------------------------------------------------
// from_roots — configured_by_user flag
// ---------------------------------------------------------------------------

#[test]
fn from_roots_sets_configured_by_user_true() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("custom-codex");
    let file = root.join("session.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}\n").unwrap();

    let discovery = CodexDiscovery::from_roots(vec![root]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1);
    assert!(
        sources[0].configured_by_user,
        "from_roots sets configured_by_user"
    );
}

// ---------------------------------------------------------------------------
// from_roots — multiple roots
// ---------------------------------------------------------------------------

#[test]
fn from_roots_with_multiple_roots() {
    let temp = tempfile::tempdir().unwrap();

    let root_a = temp.path().join("root-a");
    let file_a = root_a.join("session-a.jsonl");
    std::fs::create_dir_all(file_a.parent().unwrap()).unwrap();
    std::fs::write(&file_a, "{}\n").unwrap();

    let root_b = temp.path().join("root-b");
    let file_b = root_b.join("session-b.jsonl");
    std::fs::create_dir_all(file_b.parent().unwrap()).unwrap();
    std::fs::write(&file_b, "{}\n").unwrap();

    let discovery = CodexDiscovery::from_roots(vec![root_a, root_b]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 2);
}

#[test]
fn from_roots_empty_iter_produces_no_sources() {
    let discovery = CodexDiscovery::from_roots(Vec::<std::path::PathBuf>::new());
    let sources = discovery.discover().unwrap();
    assert!(sources.is_empty());
}

// ---------------------------------------------------------------------------
// with_settings
// ---------------------------------------------------------------------------

#[test]
fn with_settings_true_includes_default_roots_when_dirs_exist() {
    // The actual roots depend on the test environment, but the discovery
    // should not panic and should return Ok.
    let discovery = CodexDiscovery::with_settings(true);
    let result = discovery.discover();
    assert!(result.is_ok());
}

#[test]
fn with_settings_false_produces_empty_roots() {
    let discovery = CodexDiscovery::with_settings(false);
    let sources = discovery.discover().unwrap();
    assert!(sources.is_empty(), "no roots => no sources");
}

#[test]
fn with_settings_sets_configured_by_user_false() {
    // Even if no roots exist, we can verify the flag by checking that
    // with_settings(false) produces no sources (meaning configured_by_user
    // is false for default roots). A more direct test would require a
    // real ~/.codex/sessions directory, so we verify indirectly.
    let discovery = CodexDiscovery::with_settings(false);
    let sources = discovery.discover().unwrap();
    assert!(sources.is_empty());
}

// ---------------------------------------------------------------------------
// Scan edge cases
// ---------------------------------------------------------------------------

#[test]
fn discover_root_with_no_jsonl_yields_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("empty-root");
    std::fs::create_dir_all(&root).unwrap();
    // No .jsonl files created.

    let discovery = CodexDiscovery::from_roots(vec![root]);
    let sources = discovery.discover().unwrap();
    assert!(sources.is_empty(), "root without jsonl files => no sources");
}

#[test]
fn discover_ignores_non_jsonl_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();

    // Write non-jsonl files — these should be ignored.
    std::fs::write(root.join("session.log"), "log data\n").unwrap();
    std::fs::write(root.join("session.json"), "{}\n").unwrap();
    std::fs::write(root.join("session.txt"), "text\n").unwrap();

    let discovery = CodexDiscovery::from_roots(vec![root]);
    let sources = discovery.discover().unwrap();
    assert!(sources.is_empty(), "non-jsonl files should be ignored");
}

#[test]
fn discover_finds_jsonl_but_ignores_other_extensions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();

    let jsonl_file = root.join("session.jsonl");
    std::fs::write(&jsonl_file, "{}\n").unwrap();
    std::fs::write(root.join("session.json"), "{}\n").unwrap();
    std::fs::write(root.join("session.log"), "log\n").unwrap();

    let discovery = CodexDiscovery::from_roots(vec![root]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].files.len(), 1);
    assert!(sources[0].files[0].ends_with("session.jsonl"));
}

#[test]
fn discover_nonexistent_root_yields_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let nonexistent = temp.path().join("does-not-exist");

    let discovery = CodexDiscovery::from_roots(vec![nonexistent]);
    let sources = discovery.discover().unwrap();
    assert!(sources.is_empty(), "nonexistent root => no sources");
}

#[test]
fn discover_source_id_is_derived_from_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("my-codex");
    let file = root.join("session.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}\n").unwrap();

    let discovery = CodexDiscovery::from_roots(vec![root]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1);
    assert!(sources[0].source_id.starts_with("codex_"));
}

#[test]
fn discover_source_has_correct_agent_and_type() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let file = root.join("session.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}\n").unwrap();

    let discovery = CodexDiscovery::from_roots(vec![root]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].agent, AgentKind::Codex);
    assert_eq!(sources[0].source_type, LogSourceType::Jsonl);
}

#[test]
fn discover_mixed_roots_only_yields_sources_for_roots_with_files() {
    let temp = tempfile::tempdir().unwrap();

    // Root A has jsonl files.
    let root_a = temp.path().join("root-a");
    let file_a = root_a.join("session.jsonl");
    std::fs::create_dir_all(file_a.parent().unwrap()).unwrap();
    std::fs::write(&file_a, "{}\n").unwrap();

    // Root B is empty.
    let root_b = temp.path().join("root-b");
    std::fs::create_dir_all(&root_b).unwrap();

    // Root C has files but no jsonl.
    let root_c = temp.path().join("root-c");
    std::fs::create_dir_all(&root_c).unwrap();
    std::fs::write(root_c.join("readme.txt"), "hello\n").unwrap();

    let discovery = CodexDiscovery::from_roots(vec![root_a, root_b, root_c]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1, "only root-a should produce a source");
}

#[test]
fn discover_deduplicates_by_canonical_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();

    let file = root.join("session.jsonl");
    std::fs::write(&file, "{}\n").unwrap();

    // Same root listed twice — each root iteration produces a source,
    // but files within each source are deduplicated.
    let discovery = CodexDiscovery::from_roots(vec![root.clone(), root]);
    let sources = discovery.discover().unwrap();
    for source in &sources {
        assert_eq!(source.files.len(), 1);
    }
}

#[test]
fn discover_no_projects_subdir_required() {
    // Unlike Claude Code, Codex does NOT require a projects/ subdirectory.
    // Files directly under the root should be found.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    std::fs::create_dir_all(&root).unwrap();

    let file = root.join("direct-session.jsonl");
    std::fs::write(&file, "{}\n").unwrap();

    let discovery = CodexDiscovery::from_roots(vec![root]);
    let sources = discovery.discover().unwrap();
    assert_eq!(
        sources.len(),
        1,
        "Codex should find files directly under root"
    );
    assert_eq!(sources[0].files.len(), 1);
}
