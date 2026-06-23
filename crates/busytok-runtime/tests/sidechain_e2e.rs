#![allow(clippy::uninlined_format_args, clippy::unwrap_used)]
//! End-to-end sidechain collapse: real Claude Code JSONL → adapter → store.
//!
//! A parent line and its `/btw` sidechain replay share a `message_id` but carry
//! different `request_id`s (the replay is marked `isSidechain: true`). After
//! ingestion the parent's usage must be counted exactly once — matching
//! ccusage, which keeps the parent and drops the replay.

use busytok_adapters::{AgentLogAdapter, ClaudeCodeAdapter};
use busytok_domain::{ParseContext, ParsedLogEvent, UsageWritePolicy};
use busytok_store::db::Database;
use busytok_store::write_queries::upsert_usage_events_dedup_aware;

fn parse_events(
    adapter: &ClaudeCodeAdapter,
    lines: &[&str],
) -> Vec<busytok_domain::NormalizedUsageEvent> {
    lines
        .iter()
        .enumerate()
        .flat_map(|(i, line)| {
            let ctx = ParseContext::for_test("src", "/tmp/s.jsonl", i as u64 + 1, 0, 100);
            adapter
                .parse_line(&ctx, line)
                .unwrap()
                .into_iter()
                .filter_map(|e| match e {
                    ParsedLogEvent::Normalized(ne) => ne.into_usage(),
                    _ => None,
                })
        })
        .collect()
}

#[test]
fn parent_and_sidechain_replay_collapse_to_parent_total() {
    let adapter = ClaudeCodeAdapter;
    // Same message_id "msg-7", identical usage (input=100, output=50). The
    // replay carries isSidechain:true and a different request_id.
    let parent = r#"{"requestId":"req-parent","sessionId":"sess-1","timestamp":"2026-06-01T08:00:00Z","message":{"id":"msg-7","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":20}}}"#;
    let replay = r#"{"isSidechain":true,"requestId":"req-replay","sessionId":"sess-1","timestamp":"2026-06-01T08:00:05Z","message":{"id":"msg-7","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":20}}}"#;

    let events = parse_events(&adapter, &[parent, replay]);
    assert_eq!(events.len(), 2, "both lines parse to usage events");

    let policies = vec![UsageWritePolicy::Replace; events.len()];
    let db = Database::open_in_memory().unwrap();
    let outcome = upsert_usage_events_dedup_aware(db.conn(), &events, &policies, "gen-1").unwrap();

    let (count, total, is_sidechain): (i64, i64, i64) = db
        .conn()
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(total_tokens), 0), \
                    (SELECT is_sidechain FROM usage_events WHERE message_id = 'msg-7') \
             FROM usage_events WHERE message_id = 'msg-7'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    assert_eq!(count, 1, "parent + replay collapse to one row");
    assert_eq!(
        total,
        100 + 50 + 10 + 20,
        "total counted once (parent formula), not doubled"
    );
    assert_eq!(is_sidechain, 0, "the surviving row is the parent");
    assert_eq!(outcome.dropped, 1, "the replay was dropped");
}

#[test]
fn replay_first_then_parent_still_collapses() {
    // Ingestion order is not guaranteed; replay arriving first must still
    // resolve to the parent once it arrives.
    let adapter = ClaudeCodeAdapter;
    let replay = r#"{"isSidechain":true,"requestId":"req-r","sessionId":"s","timestamp":"2026-06-01T08:00:00Z","message":{"id":"msg-9","model":"claude-sonnet-4-20250514","usage":{"input_tokens":40,"output_tokens":10}}}"#;
    let parent = r#"{"requestId":"req-p","sessionId":"s","timestamp":"2026-06-01T08:00:01Z","message":{"id":"msg-9","model":"claude-sonnet-4-20250514","usage":{"input_tokens":40,"output_tokens":10}}}"#;

    let db = Database::open_in_memory().unwrap();
    let replay_ev = parse_events(&adapter, &[replay]);
    upsert_usage_events_dedup_aware(db.conn(), &replay_ev, &[UsageWritePolicy::Replace], "gen-1")
        .unwrap();

    let parent_ev = parse_events(&adapter, &[parent]);
    let outcome = upsert_usage_events_dedup_aware(
        db.conn(),
        &parent_ev,
        &[UsageWritePolicy::Replace],
        "gen-1",
    )
    .unwrap();

    let (count, total): (i64, i64) = db
        .conn()
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(total_tokens), 0) FROM usage_events WHERE message_id = 'msg-9'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(total, 50, "net total is the single parent usage, not 100");
    assert_eq!(outcome.replaced, 1);
}
