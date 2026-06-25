#![allow(clippy::unwrap_used)]

use busytok_store::Database;
use busytok_subagent::resolver::{lookup_by_name, resolve_by_id, resolve_by_name};
use busytok_subagent::SubagentError;

#[test]
fn resolve_by_id_returns_not_found_for_missing_uuid() {
    let db = Database::open_in_memory().unwrap();
    let err = resolve_by_id(&db, "no-such-uuid").unwrap_err();
    assert!(matches!(err, SubagentError::NotFound(_)));
}

#[test]
fn resolve_by_id_finds_existing_row() {
    let db = Database::open_in_memory().unwrap();
    let created = resolve_by_name(&db, "reviewer", "/tmp/repo", "pi/search-cheap", None).unwrap();
    let found = resolve_by_id(&db, &created.subagent.id).unwrap();
    assert_eq!(found.id, created.subagent.id);
    assert_eq!(found.name, "reviewer");
}

#[test]
fn resolve_by_name_creates_when_none_exist() {
    let db = Database::open_in_memory().unwrap();
    let r = resolve_by_name(&db, "reviewer", "/tmp/repo", "pi/search-cheap", None).unwrap();
    assert!(r.created, "should create when no match exists");
    assert_eq!(r.subagent.name, "reviewer");
}

#[test]
fn resolve_by_name_reuses_when_one_exists() {
    let db = Database::open_in_memory().unwrap();
    let first = resolve_by_name(&db, "reviewer", "/tmp/repo", "pi/search-cheap", None).unwrap();
    let second = resolve_by_name(&db, "reviewer", "/tmp/repo", "pi/search-cheap", None).unwrap();
    assert!(!second.created, "should reuse existing row");
    assert_eq!(first.subagent.id, second.subagent.id);
}

#[test]
fn resolve_by_name_rejects_invalid_name() {
    let db = Database::open_in_memory().unwrap();
    let err = resolve_by_name(&db, "bad name!", "/tmp/repo", "pi/search-cheap", None)
        .err()
        .unwrap();
    assert!(matches!(err, SubagentError::InvalidName(_)));
}

#[test]
fn resolve_by_name_with_default_model_seeds_row() {
    let db = Database::open_in_memory().unwrap();
    let r = resolve_by_name(
        &db,
        "reviewer",
        "/tmp/repo",
        "pi/search-cheap",
        Some("claude-default"),
    )
    .unwrap();
    assert_eq!(r.subagent.default_model.as_deref(), Some("claude-default"));
}

#[test]
fn lookup_by_name_returns_not_found_when_missing() {
    let db = Database::open_in_memory().unwrap();
    let err = lookup_by_name(&db, "ghost", "/tmp/repo").unwrap_err();
    assert!(matches!(err, SubagentError::NotFound(_)));
}

#[test]
fn lookup_by_name_rejects_invalid_name() {
    let db = Database::open_in_memory().unwrap();
    let err = lookup_by_name(&db, ".hidden", "/tmp/repo").unwrap_err();
    assert!(matches!(err, SubagentError::InvalidName(_)));
}

#[test]
fn lookup_by_name_finds_existing_row() {
    let db = Database::open_in_memory().unwrap();
    resolve_by_name(&db, "reviewer", "/tmp/repo", "pi/search-cheap", None).unwrap();
    let found = lookup_by_name(&db, "reviewer", "/tmp/repo").unwrap();
    assert_eq!(found.name, "reviewer");
}

// NOTE: `resolve_by_name` / `lookup_by_name` have an `AmbiguousName` branch
// (matches.len() > 1) and `row_to_model` has an `unwrap_or(Cold)` status
// fallback. Both branches are unreachable through the public DB API because
// the `subagent_logical_subagents` table enforces:
//   - UNIQUE(project_id, repo_hash, name)  →AmbiguousName impossible
//   - CHECK(status IN ('hot','warm','cold','deleted')) → bad-status impossible
// These are defensive branches; they remain intentionally uncovered.
