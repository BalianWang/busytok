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

#[test]
fn usage_event_round_trips_unified_cache_fields() {
    let db = Database::open_in_memory().unwrap();
    let mut e = NormalizedUsageEvent::minimal_for_test("u1", AgentKind::ClaudeCode);
    e.provider_payload_shape =
        busytok_domain::cache_metrics::ProviderPayloadShape::AnthropicCompatibleNonCachedInput;
    e.prompt_input_total_tokens = 1000;
    e.prompt_input_non_cached_tokens = 10;
    e.cache_read_tokens = 990;
    db.write_usage_event(&e, UsageWritePolicy::Replace).unwrap();
    let got = db.get_usage_event("u1").unwrap().unwrap();
    assert_eq!(got.prompt_input_total_tokens, 1000);
    assert_eq!(got.prompt_input_non_cached_tokens, 10);
    assert_eq!(got.provider_payload_shape, e.provider_payload_shape);
}

#[test]
fn cache_metric_diagnostic_lifecycle_records_and_recovers() {
    let db = Database::open_in_memory().unwrap();
    let mut e = NormalizedUsageEvent::minimal_for_test("bad", AgentKind::ClaudeCode);
    // Impossible combination: total(10) < read(800)+write(200)+non_cached(10).
    e.prompt_input_total_tokens = 10;
    e.prompt_input_non_cached_tokens = 10;
    e.cache_read_tokens = 800;
    e.cache_creation_tokens = 200;
    db.write_usage_event(&e, UsageWritePolicy::Replace).unwrap();
    let diags = db.list_all_diagnostic_events(100).unwrap();
    // `DiagnosticEventRow.code` holds the category value ("cache_metric").
    assert!(
        diags.iter().any(|d| d.code == "cache_metric"),
        "violating event must record a cache_metric diagnostic, got: {:?}",
        diags
    );

    // Same event id rewritten with a valid combination ⇒ diagnostic is cleared
    // (no stale warning). total(1010) == non_cached(10)+read(800)+write(200).
    e.prompt_input_total_tokens = 1010;
    db.write_usage_event(&e, UsageWritePolicy::Replace).unwrap();
    let diags = db.list_all_diagnostic_events(100).unwrap();
    assert!(
        diags.iter().all(|d| d.code != "cache_metric"),
        "recovered event must leave no stale warning, got: {:?}",
        diags
    );
}
