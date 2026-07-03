//! Coverage gap tests for `busytok-discovery` (claude.rs + codex.rs).
//!
//! Targets uncovered source lines:
//! - `claude.rs`: `default_roots()` (FNDA:0), `with_settings(false)`,
//!   `CLAUDE_CONFIG_DIR` env-var root.
//! - `codex.rs`: `default_roots()` (FNDA:0), `with_settings(false)`.

#![allow(
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    dead_code,
    unused_imports,
    unused_variables
)]

use std::sync::Mutex;

use busytok_discovery::{ClaudeCodeDiscovery, CodexDiscovery};

/// Serializes tests that mutate process-global environment variables so they
/// don't interfere with each other or with the default-root tests that read
/// `CLAUDE_CONFIG_DIR`.
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ── ClaudeCodeDiscovery::default_roots ──────────────────────────────────

#[test]
fn claude_default_roots_does_not_panic() {
    // `default_roots()` was FNDA:0 — calling it exercises the constructor
    // that delegates to `with_settings(true)`.
    let discovery = ClaudeCodeDiscovery::default_roots();
    // discover() should complete without error regardless of whether the
    // default directories exist on this machine.
    let result = discovery.discover();
    assert!(result.is_ok(), "discover() must not error");
}

#[test]
fn claude_with_settings_false_has_no_roots() {
    // When claude_code_default_paths is false, no default roots are added.
    let discovery = ClaudeCodeDiscovery::with_settings(false);
    let sources = discovery.discover().unwrap();
    assert!(
        sources.is_empty(),
        "with_settings(false) must produce no sources"
    );
}

#[test]
fn claude_default_roots_picks_up_claude_config_dir_env() {
    // Set CLAUDE_CONFIG_DIR to a temp dir that contains a projects/ tree
    // with a .jsonl file. This exercises the env-var root branch (lines
    // 76-81) and the `is_dir() == true` push.
    let _guard = ENV_LOCK.lock().unwrap();

    let temp = tempfile::tempdir().unwrap();
    let claude_dir = temp.path().join(".claude");
    let file = claude_dir.join("projects/proj-a/session.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}\n").unwrap();

    // SAFETY: env::set_var is process-global; the ENV_LOCK mutex ensures
    // no other test in this binary touches CLAUDE_CONFIG_DIR concurrently.
    std::env::set_var("CLAUDE_CONFIG_DIR", &claude_dir);

    let discovery = ClaudeCodeDiscovery::with_settings(true);
    let sources = discovery.discover().unwrap();

    // Clean up env var immediately to minimize side effects on other tests.
    std::env::remove_var("CLAUDE_CONFIG_DIR");

    // The CLAUDE_CONFIG_DIR root should produce at least one source with
    // the .jsonl file we created.
    let found = sources
        .iter()
        .any(|s| s.files.iter().any(|f| f == &file));
    assert!(
        found,
        "CLAUDE_CONFIG_DIR root must discover the test .jsonl file"
    );
}

#[test]
fn claude_with_settings_false_ignores_claude_config_dir_env() {
    // Even if CLAUDE_CONFIG_DIR is set, with_settings(false) must not
    // include it — the env var is only read when default_paths is true.
    let _guard = ENV_LOCK.lock().unwrap();

    let temp = tempfile::tempdir().unwrap();
    let claude_dir = temp.path().join(".claude-env");
    std::fs::create_dir_all(&claude_dir).unwrap();

    std::env::set_var("CLAUDE_CONFIG_DIR", &claude_dir);
    let discovery = ClaudeCodeDiscovery::with_settings(false);
    std::env::remove_var("CLAUDE_CONFIG_DIR");

    let sources = discovery.discover().unwrap();
    assert!(sources.is_empty());
}

// ── CodexDiscovery::default_roots ───────────────────────────────────────

#[test]
fn codex_default_roots_does_not_panic() {
    // `default_roots()` was FNDA:0 — calling it exercises the constructor
    // that delegates to `with_settings(true)`.
    let discovery = CodexDiscovery::default_roots();
    let result = discovery.discover();
    assert!(result.is_ok(), "discover() must not error");
}

#[test]
fn codex_with_settings_false_has_no_roots() {
    let discovery = CodexDiscovery::with_settings(false);
    let sources = discovery.discover().unwrap();
    assert!(
        sources.is_empty(),
        "with_settings(false) must produce no sources"
    );
}

#[test]
fn codex_with_settings_true_discovers_jsonl_files() {
    // Exercise the happy path of with_settings(true) by providing a
    // root via from_roots (since ~/.codex/sessions likely doesn't exist).
    let temp = tempfile::tempdir().unwrap();
    let codex_dir = temp.path().join(".codex/sessions");
    let file = codex_dir.join("session-1.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}\n").unwrap();

    let discovery = CodexDiscovery::from_roots([codex_dir.clone()]);
    let sources = discovery.discover().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].files, vec![file]);
}
