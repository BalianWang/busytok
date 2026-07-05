//! Residual cleanup assertions.
//! Uses walkdir + std::fs (no external `rg` binary dependency).
use std::fs;
use std::path::PathBuf;

/// Asserts that old design remnants are fully removed from the codebase.
/// This test prevents regressions where someone re-adds deleted patterns.
/// Scans `crates/**/src/**/*.rs` and `apps/**/src/**/*.rs` only.
#[test]
fn no_env_name_fields_remain() {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    // Files that may legitimately mention these strings (e.g. this test file,
    // or plan/spec docs). We match by suffix relative to workspace root.
    let excluded_suffixes: Vec<PathBuf> = vec![
        // This test file itself contains the strings for assertion
        PathBuf::from("crates/busytok-runtime/tests/residual_cleanup.rs"),
    ];
    let forbidden_patterns = [
        "api_key_env_name",
        "base_url_env_name",
        "ProviderCredentialStore",
    ];

    let mut offenders: Vec<(PathBuf, &str)> = Vec::new();
    for root in &["crates", "apps"] {
        let root_dir = workspace_root.join(root);
        if !root_dir.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&root_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            // Only scan .rs source files under a src/ directory
            let path_str = path.to_string_lossy();
            if !path_str.contains("/src/") && !path_str.contains("\\src\\") {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }

            // Skip excluded files (match by suffix relative to workspace)
            let rel = path.strip_prefix(&workspace_root).unwrap_or(path);
            let is_excluded = excluded_suffixes.iter().any(|ex| rel.ends_with(ex));
            if is_excluded {
                continue;
            }

            let content = match fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for pattern in &forbidden_patterns {
                if content.contains(pattern) {
                    offenders.push((path.to_path_buf(), *pattern));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "Forbidden patterns found in source files:\n{}",
        offenders
            .iter()
            .map(|(p, pat)| format!("  {} (pattern: {})", p.display(), pat))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Asserts no Cargo.toml under crates/ or apps/ still depends on `keyring`.
#[test]
fn no_keychain_dependency_in_cargo() {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut offenders: Vec<PathBuf> = Vec::new();

    for root in &["crates", "apps"] {
        let root_dir = workspace_root.join(root);
        if !root_dir.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&root_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.file_name().and_then(|n| n.to_str()) != Some("Cargo.toml") {
                continue;
            }

            let content = match fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            // Match `keyring` only on non-comment lines (Cargo.toml has no
            // comments traditionally, but be defensive).
            for line in content.lines() {
                let trimmed = line.trim_start();
                if trimmed.starts_with('#') {
                    continue;
                }
                if trimmed.contains("keyring") {
                    offenders.push(path.to_path_buf());
                    break;
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "keyring dependency still present in: {offenders:?}"
    );
}
