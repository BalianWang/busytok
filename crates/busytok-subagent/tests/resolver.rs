#![allow(clippy::unwrap_used)]

use busytok_domain::ProviderKind;
use busytok_store::{CreateModelReq, CreateProviderReq, Database};
use busytok_subagent::resolver::{lookup_by_name, resolve_by_id, resolve_by_name};
use busytok_subagent::SubagentError;

/// Seed a provider + model and return `(provider_id, model_id)`. Used by
/// creation-path tests so `validate_bound_provider_model` succeeds.
fn seed_provider_model(db: &Database) -> (String, String) {
    let provider = db
        .create_provider(CreateProviderReq {
            name: "P1".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: true,
            api_key: Some("sk-test".into()),
        })
        .unwrap();
    let model = db
        .create_model(CreateModelReq {
            provider_id: provider.id.clone(),
            model_id: "gpt-4o".into(),
            enabled: true,
            tags: vec![],
            display_name: None,
            reasoning: None,
            context_window: None,
            max_tokens: None,
        })
        .unwrap();
    (provider.id, model.model_id)
}

#[test]
fn resolve_by_id_returns_not_found_for_missing_uuid() {
    let db = Database::open_in_memory().unwrap();
    let err = resolve_by_id(&db, "no-such-uuid").unwrap_err();
    assert!(matches!(err, SubagentError::NotFound(_)));
}

#[test]
fn resolve_by_id_finds_existing_row() {
    let db = Database::open_in_memory().unwrap();
    let (pid, mid) = seed_provider_model(&db);
    let created =
        resolve_by_name(&db, "reviewer", "/tmp/repo", "pi/search-cheap", &pid, &mid).unwrap();
    let found = resolve_by_id(&db, &created.subagent.id).unwrap();
    assert_eq!(found.id, created.subagent.id);
    assert_eq!(found.name, "reviewer");
}

#[test]
fn resolve_by_name_creates_when_none_exist() {
    let db = Database::open_in_memory().unwrap();
    let (pid, mid) = seed_provider_model(&db);
    let r = resolve_by_name(&db, "reviewer", "/tmp/repo", "pi/search-cheap", &pid, &mid).unwrap();
    assert!(r.created, "should create when no match exists");
    assert_eq!(r.subagent.name, "reviewer");
}

#[test]
fn resolve_by_name_reuses_when_one_exists() {
    let db = Database::open_in_memory().unwrap();
    let (pid, mid) = seed_provider_model(&db);
    let first =
        resolve_by_name(&db, "reviewer", "/tmp/repo", "pi/search-cheap", &pid, &mid).unwrap();
    // Second call hits the reuse path — bound fields are ignored.
    let second = resolve_by_name(&db, "reviewer", "/tmp/repo", "pi/search-cheap", "", "").unwrap();
    assert!(!second.created, "should reuse existing row");
    assert_eq!(first.subagent.id, second.subagent.id);
}

#[test]
fn resolve_by_name_rejects_invalid_name() {
    let db = Database::open_in_memory().unwrap();
    // Invalid name → returns InvalidName BEFORE bound-field validation,
    // so empty bound fields are fine here.
    let err = resolve_by_name(&db, "bad name!", "/tmp/repo", "pi/search-cheap", "", "")
        .err()
        .unwrap();
    assert!(matches!(err, SubagentError::InvalidName(_)));
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
    let (pid, mid) = seed_provider_model(&db);
    resolve_by_name(&db, "reviewer", "/tmp/repo", "pi/search-cheap", &pid, &mid).unwrap();
    let found = lookup_by_name(&db, "reviewer", "/tmp/repo").unwrap();
    assert_eq!(found.name, "reviewer");
}

// --- Task 2: creation-time validation (validate_bound_provider_model) --------

#[test]
fn resolve_by_name_creates_subagent_with_valid_bound_provider_and_model() {
    let db = Database::open_in_memory().unwrap();
    let (pid, mid) = seed_provider_model(&db);
    let resolved = resolve_by_name(&db, "test-sub", "/tmp", "pi/search-cheap", &pid, &mid).unwrap();
    assert!(resolved.created);
    assert_eq!(resolved.subagent.bound_provider_id, pid);
    assert_eq!(resolved.subagent.bound_model_id, "gpt-4o");
}

#[test]
fn resolve_by_name_rejects_disabled_provider() {
    let db = Database::open_in_memory().unwrap();
    let provider = db
        .create_provider(CreateProviderReq {
            name: "P1".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: false,
            api_key: None,
        })
        .unwrap();
    let result = resolve_by_name(
        &db,
        "test-sub",
        "/tmp",
        "pi/search-cheap",
        &provider.id,
        "gpt-4o",
    );
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("provider disabled"), "got: {msg}");
}

#[test]
fn resolve_by_name_rejects_missing_model_in_provider() {
    let db = Database::open_in_memory().unwrap();
    let (pid, _mid) = seed_provider_model(&db);
    let result = resolve_by_name(
        &db,
        "test-sub",
        "/tmp",
        "pi/search-cheap",
        &pid,
        "no-such-model",
    );
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("model not found in provider"), "got: {msg}");
}

#[test]
fn resolve_by_name_rejects_unknown_provider() {
    let db = Database::open_in_memory().unwrap();
    let result = resolve_by_name(
        &db,
        "test-sub",
        "/tmp",
        "pi/search-cheap",
        "nonexistent-provider",
        "gpt-4o",
    );
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("provider not found"), "got: {msg}");
}

#[test]
fn resolve_by_name_rejects_disabled_model() {
    let db = Database::open_in_memory().unwrap();
    let provider = db
        .create_provider(CreateProviderReq {
            name: "P1".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: true,
            api_key: Some("sk-test".into()),
        })
        .unwrap();
    let model = db
        .create_model(CreateModelReq {
            provider_id: provider.id.clone(),
            model_id: "gpt-4o".into(),
            enabled: false,
            tags: vec![],
            display_name: None,
            reasoning: None,
            context_window: None,
            max_tokens: None,
        })
        .unwrap();
    let result = resolve_by_name(
        &db,
        "test-sub",
        "/tmp",
        "pi/search-cheap",
        &provider.id,
        &model.model_id,
    );
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("model disabled"), "got: {msg}");
}

// NOTE: `resolve_by_name` / `lookup_by_name` have an `AmbiguousName` branch
// (matches.len() > 1) and `row_to_model` has an `unwrap_or(Cold)` status
// fallback. Both branches are unreachable through the public DB API because
// the `subagent_logical_subagents` table enforces:
//   - UNIQUE(project_id, repo_hash, name)  →AmbiguousName impossible
//   - CHECK(status IN ('hot','warm','cold','deleted')) → bad-status impossible
// These are defensive branches; they remain intentionally uncovered.
