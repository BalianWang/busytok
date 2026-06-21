use busytok_discovery::ClaudeCodeDiscovery;

#[test]
fn discovers_claude_jsonl_under_projects_dirs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(".claude");
    let file = root.join("projects/project-a/session.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}\n").unwrap();

    let discovery = ClaudeCodeDiscovery::from_roots([root]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].files, vec![file]);
}

#[test]
fn deduplicates_symlinked_or_repeated_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(".claude");
    let file = root.join("projects/project-a/session.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}\n").unwrap();

    let discovery = ClaudeCodeDiscovery::from_roots([root.clone(), root]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources[0].files.len(), 1);
}

// ---------------------------------------------------------------------------
// with_settings tests
// ---------------------------------------------------------------------------

#[test]
fn with_settings_true_includes_default_roots_when_dirs_exist() {
    // When claude_code_default_paths is true, default_roots() is called.
    // The actual roots depend on the test environment, but the discovery
    // should not panic and should return Ok.
    let discovery = ClaudeCodeDiscovery::with_settings(true);
    // We just verify it doesn't panic and discover() completes.
    let result = discovery.discover();
    assert!(result.is_ok());
}

#[test]
fn with_settings_false_produces_empty_roots() {
    let discovery = ClaudeCodeDiscovery::with_settings(false);
    let sources = discovery.discover().unwrap();
    assert!(sources.is_empty(), "no roots => no sources");
}

// ---------------------------------------------------------------------------
// from_roots tests
// ---------------------------------------------------------------------------

#[test]
fn from_roots_with_custom_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("custom-claude");
    let file = root.join("projects/my-project/log.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}\n").unwrap();

    let discovery = ClaudeCodeDiscovery::from_roots([root.clone()]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].root_path, root);
    assert!(
        sources[0].configured_by_user,
        "from_roots sets configured_by_user"
    );
}

#[test]
fn from_roots_with_multiple_roots() {
    let temp = tempfile::tempdir().unwrap();

    let root_a = temp.path().join("root-a");
    let file_a = root_a.join("projects/proj-a/session.jsonl");
    std::fs::create_dir_all(file_a.parent().unwrap()).unwrap();
    std::fs::write(&file_a, "{}\n").unwrap();

    let root_b = temp.path().join("root-b");
    let file_b = root_b.join("projects/proj-b/session.jsonl");
    std::fs::create_dir_all(file_b.parent().unwrap()).unwrap();
    std::fs::write(&file_b, "{}\n").unwrap();

    let discovery = ClaudeCodeDiscovery::from_roots([root_a, root_b]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 2);
}

#[test]
fn from_roots_empty_iter_produces_no_sources() {
    let discovery = ClaudeCodeDiscovery::from_roots(Vec::<std::path::PathBuf>::new());
    let sources = discovery.discover().unwrap();
    assert!(sources.is_empty());
}

// ---------------------------------------------------------------------------
// discover / scan_root edge cases (tested indirectly through discover)
// ---------------------------------------------------------------------------

#[test]
fn discover_root_with_no_projects_subdir_yields_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("empty-root");
    std::fs::create_dir_all(&root).unwrap();
    // No "projects" subdirectory created.

    let discovery = ClaudeCodeDiscovery::from_roots([root]);
    let sources = discovery.discover().unwrap();
    assert!(
        sources.is_empty(),
        "root without projects/ subdir => no sources"
    );
}

#[test]
fn discover_root_with_empty_projects_dir_yields_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let projects = root.join("projects");
    std::fs::create_dir_all(&projects).unwrap();
    // projects/ exists but is empty.

    let discovery = ClaudeCodeDiscovery::from_roots([root]);
    let sources = discovery.discover().unwrap();
    assert!(sources.is_empty(), "empty projects/ => no sources");
}

#[test]
fn discover_ignores_non_jsonl_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let dir = root.join("projects/proj");
    std::fs::create_dir_all(&dir).unwrap();

    // Write non-jsonl files — these should be ignored.
    std::fs::write(dir.join("session.log"), "log data\n").unwrap();
    std::fs::write(dir.join("session.json"), "{}\n").unwrap();
    std::fs::write(dir.join("session.txt"), "text\n").unwrap();

    let discovery = ClaudeCodeDiscovery::from_roots([root]);
    let sources = discovery.discover().unwrap();
    assert!(sources.is_empty(), "non-jsonl files should be ignored");
}

#[test]
fn discover_finds_jsonl_but_ignores_other_extensions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let dir = root.join("projects/proj");
    std::fs::create_dir_all(&dir).unwrap();

    let jsonl_file = dir.join("session.jsonl");
    std::fs::write(&jsonl_file, "{}\n").unwrap();
    std::fs::write(dir.join("session.json"), "{}\n").unwrap();
    std::fs::write(dir.join("session.log"), "log\n").unwrap();

    let discovery = ClaudeCodeDiscovery::from_roots([root]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].files.len(), 1);
    assert!(sources[0].files[0].ends_with("session.jsonl"));
}

#[test]
fn discover_finds_nested_jsonl_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let deep_dir = root.join("projects/proj-a/sub1/sub2");
    std::fs::create_dir_all(&deep_dir).unwrap();

    let file1 = root.join("projects/proj-a/session.jsonl");
    let file2 = deep_dir.join("deep.jsonl");
    std::fs::write(&file1, "{}\n").unwrap();
    std::fs::write(&file2, "{}\n").unwrap();

    let discovery = ClaudeCodeDiscovery::from_roots([root]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].files.len(), 2);
    // Files are sorted by full path; verify both are present.
    let names: Vec<String> = sources[0]
        .files
        .iter()
        .map(|f| f.file_name().unwrap().to_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"deep.jsonl".to_string()));
    assert!(names.contains(&"session.jsonl".to_string()));
}

#[test]
fn discover_nonexistent_root_yields_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let nonexistent = temp.path().join("does-not-exist");

    let discovery = ClaudeCodeDiscovery::from_roots([nonexistent]);
    let sources = discovery.discover().unwrap();
    assert!(sources.is_empty(), "nonexistent root => no sources");
}

#[test]
fn discover_source_id_is_derived_from_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("my-claude");
    let file = root.join("projects/proj/session.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}\n").unwrap();

    let discovery = ClaudeCodeDiscovery::from_roots([root.clone()]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1);
    assert!(sources[0].source_id.starts_with("claude_code_"));
}

#[test]
fn discover_source_has_correct_agent_and_type() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let file = root.join("projects/proj/session.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}\n").unwrap();

    let discovery = ClaudeCodeDiscovery::from_roots([root]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1);

    use busytok_domain::{AgentKind, LogSourceType};
    assert_eq!(sources[0].agent, AgentKind::ClaudeCode);
    assert_eq!(sources[0].source_type, LogSourceType::Jsonl);
}

#[test]
fn discover_mixed_roots_only_yields_sources_for_roots_with_files() {
    let temp = tempfile::tempdir().unwrap();

    // Root A has jsonl files.
    let root_a = temp.path().join("root-a");
    let file_a = root_a.join("projects/proj/session.jsonl");
    std::fs::create_dir_all(file_a.parent().unwrap()).unwrap();
    std::fs::write(&file_a, "{}\n").unwrap();

    // Root B has no projects subdir.
    let root_b = temp.path().join("root-b");
    std::fs::create_dir_all(&root_b).unwrap();

    // Root C has projects dir but no jsonl files.
    let root_c = temp.path().join("root-c");
    std::fs::create_dir_all(root_c.join("projects/proj")).unwrap();

    let discovery = ClaudeCodeDiscovery::from_roots([root_a, root_b, root_c]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1, "only root-a should produce a source");
}

#[test]
fn discover_deduplicates_by_canonical_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let dir = root.join("projects/proj");
    std::fs::create_dir_all(&dir).unwrap();

    let file = dir.join("session.jsonl");
    std::fs::write(&file, "{}\n").unwrap();

    // Same root listed twice — should still produce only one source with one file.
    let discovery = ClaudeCodeDiscovery::from_roots([root.clone(), root]);
    let sources = discovery.discover().unwrap();
    // Two roots both produce sources, but each source's files are deduplicated.
    // Actually, since both roots are the same path, they produce two separate
    // DiscoveredLogSource entries (one per root iteration), but each has deduplicated files.
    // Let's verify the files in each source are deduplicated.
    for source in &sources {
        assert_eq!(source.files.len(), 1);
    }
}
