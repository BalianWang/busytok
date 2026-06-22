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
use busytok_domain::{AgentKind, NormalizedUsageEvent, UsageWritePolicy};
use busytok_store::Database;

#[test]
fn claude_insert_once_ignores_duplicate_identity() {
    let db = Database::open_in_memory().unwrap();
    let mut event = NormalizedUsageEvent::minimal_for_test("event-1", AgentKind::ClaudeCode);
    event.input_tokens = 100;
    db.write_usage_event(&event, UsageWritePolicy::InsertOnce)
        .unwrap();
    event.input_tokens = 150;
    db.write_usage_event(&event, UsageWritePolicy::InsertOnce)
        .unwrap();
    let stored = db.get_usage_event("event-1").unwrap().unwrap();
    assert_eq!(stored.input_tokens, 100);
    assert_eq!(db.usage_event_count().unwrap(), 1);
}

#[test]
fn replace_policy_updates_same_identity_for_late_token_sources() {
    let db = Database::open_in_memory().unwrap();
    let mut event = NormalizedUsageEvent::minimal_for_test("event-1", AgentKind::Codex);
    event.input_tokens = 0;
    db.write_usage_event(&event, UsageWritePolicy::Replace)
        .unwrap();
    let original_created_at = db
        .get_usage_event("event-1")
        .unwrap()
        .unwrap()
        .created_at_ms;
    let original_updated_at = db
        .get_usage_event("event-1")
        .unwrap()
        .unwrap()
        .updated_at_ms;

    // Simulate a delayed-token completion with a later timestamp.
    event.input_tokens = 150;
    event.updated_at_ms = original_updated_at + 1000;
    db.write_usage_event(&event, UsageWritePolicy::Replace)
        .unwrap();

    let stored = db.get_usage_event("event-1").unwrap().unwrap();
    assert_eq!(stored.input_tokens, 150);
    assert_eq!(
        stored.created_at_ms, original_created_at,
        "created_at_ms must be preserved on upsert"
    );
    assert_eq!(
        stored.updated_at_ms,
        original_updated_at + 1000,
        "updated_at_ms must advance on upsert"
    );
    assert_eq!(db.usage_event_count().unwrap(), 1);
}
