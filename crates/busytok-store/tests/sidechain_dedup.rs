#![allow(
    clippy::uninlined_format_args,
    clippy::unwrap_used,
    clippy::too_many_arguments,
    unused_variables
)]
//! Tests for the sidechain-aware, dedupe-key-based usage-event write path.
//!
//! These mirror ccusage's `should_replace_deduped_entry` semantics: a
//! non-sidechain entry beats a sidechain replay of the same `message_id`,
//! and within a class the higher-total entry wins.

use busytok_domain::{AgentKind, NormalizedUsageEvent, UsageWritePolicy};
use busytok_store::db::Database;
use busytok_store::write_queries::upsert_usage_events_dedup_aware;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Build a Claude usage event keyed on `message_id` for sidechain dedup.
fn claude_event(
    id: &str,
    message_id: &str,
    is_sidechain: bool,
    total: i64,
) -> NormalizedUsageEvent {
    let mut e = NormalizedUsageEvent::minimal_for_test(id, AgentKind::ClaudeCode);
    e.message_id = Some(message_id.to_string());
    e.dedupe_key = Some(format!("claude:msg:{message_id}"));
    e.is_sidechain = is_sidechain;
    e.total_tokens = total;
    e.input_tokens = total;
    e.timestamp_ms = 1_000_000;
    e
}

#[derive(Debug, PartialEq)]
struct SurvivingRow {
    id: String,
    is_sidechain: i64,
    total_tokens: i64,
}

fn surviving_row(
    conn: &rusqlite::Connection,
    generation_id: &str,
    dedupe_key: &str,
) -> SurvivingRow {
    conn.query_row(
        "SELECT id, is_sidechain, total_tokens FROM usage_events \
         WHERE generation_id = ?1 AND dedupe_key = ?2",
        rusqlite::params![generation_id, dedupe_key],
        |row| {
            Ok(SurvivingRow {
                id: row.get(0)?,
                is_sidechain: row.get(1)?,
                total_tokens: row.get(2)?,
            })
        },
    )
    .unwrap()
}

fn count_for_dedupe(conn: &rusqlite::Connection, generation_id: &str, dedupe_key: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM usage_events WHERE generation_id = ?1 AND dedupe_key = ?2",
        rusqlite::params![generation_id, dedupe_key],
        |row| row.get(0),
    )
    .unwrap()
}

fn replace_policies(n: usize) -> Vec<UsageWritePolicy> {
    vec![UsageWritePolicy::Replace; n]
}

// ── Winner selection ─────────────────────────────────────────────────────────

#[test]
fn non_sidechain_beats_sidechain_within_batch() {
    let db = Database::open_in_memory().unwrap();
    let replay = claude_event("claude:msg-1:req-r", "msg-1", true, 100);
    let parent = claude_event("claude:msg-1:req-p", "msg-1", false, 50);
    let outcome = upsert_usage_events_dedup_aware(
        db.conn(),
        &[replay, parent],
        &replace_policies(2),
        "gen-1",
    )
    .unwrap();

    let row = surviving_row(db.conn(), "gen-1", "claude:msg:msg-1");
    assert_eq!(row.is_sidechain, 0, "parent (non-sidechain) must win");
    assert_eq!(row.id, "claude:msg-1:req-p");
    assert_eq!(row.total_tokens, 50);
    assert_eq!(count_for_dedupe(db.conn(), "gen-1", "claude:msg:msg-1"), 1);
    assert_eq!(
        (outcome.inserted, outcome.replaced, outcome.dropped),
        (1, 1, 0)
    );
}

#[test]
fn sidechain_cannot_replace_parent_within_batch() {
    let db = Database::open_in_memory().unwrap();
    let parent = claude_event("claude:msg-1:req-p", "msg-1", false, 100);
    let replay = claude_event("claude:msg-1:req-r", "msg-1", true, 200);
    let outcome = upsert_usage_events_dedup_aware(
        db.conn(),
        &[parent, replay],
        &replace_policies(2),
        "gen-1",
    )
    .unwrap();

    let row = surviving_row(db.conn(), "gen-1", "claude:msg:msg-1");
    assert_eq!(row.is_sidechain, 0, "parent survives even with lower total");
    assert_eq!(row.total_tokens, 100);
    assert_eq!(outcome.dropped, 1);
    assert_eq!(outcome.inserted, 1);
}

#[test]
fn higher_total_wins_within_same_sidechain_class() {
    let db = Database::open_in_memory().unwrap();
    let low = claude_event("claude:msg-1:req-a", "msg-1", false, 100);
    let high = claude_event("claude:msg-1:req-b", "msg-1", false, 200);
    let outcome =
        upsert_usage_events_dedup_aware(db.conn(), &[low, high], &replace_policies(2), "gen-1")
            .unwrap();
    let row = surviving_row(db.conn(), "gen-1", "claude:msg:msg-1");
    assert_eq!(row.total_tokens, 200);
    assert_eq!(row.id, "claude:msg-1:req-b");
    assert_eq!(outcome.replaced, 1);
}

#[test]
fn lower_total_loses_within_same_sidechain_class() {
    let db = Database::open_in_memory().unwrap();
    let high = claude_event("claude:msg-1:req-a", "msg-1", false, 200);
    let low = claude_event("claude:msg-1:req-b", "msg-1", false, 100);
    let outcome =
        upsert_usage_events_dedup_aware(db.conn(), &[high, low], &replace_policies(2), "gen-1")
            .unwrap();
    let row = surviving_row(db.conn(), "gen-1", "claude:msg:msg-1");
    assert_eq!(row.total_tokens, 200, "existing higher total is kept");
    assert_eq!(outcome.dropped, 1);
}

#[test]
fn equal_total_tie_keeps_existing() {
    let db = Database::open_in_memory().unwrap();
    let first = claude_event("claude:msg-1:req-a", "msg-1", false, 150);
    let second = claude_event("claude:msg-1:req-b", "msg-1", false, 150);
    upsert_usage_events_dedup_aware(db.conn(), &[first, second], &replace_policies(2), "gen-1")
        .unwrap();
    let row = surviving_row(db.conn(), "gen-1", "claude:msg:msg-1");
    assert_eq!(row.id, "claude:msg-1:req-a", "ties keep the existing row");
}

#[test]
fn distinct_message_ids_stay_separate() {
    let db = Database::open_in_memory().unwrap();
    let a = claude_event("claude:msg-a:req-1", "msg-a", false, 100);
    let b = claude_event("claude:msg-b:req-1", "msg-b", false, 100);
    let outcome =
        upsert_usage_events_dedup_aware(db.conn(), &[a, b], &replace_policies(2), "gen-1").unwrap();
    assert_eq!(outcome.inserted, 2);
    assert_eq!(count_for_dedupe(db.conn(), "gen-1", "claude:msg:msg-a"), 1);
    assert_eq!(count_for_dedupe(db.conn(), "gen-1", "claude:msg:msg-b"), 1);
}

// ── Cross-batch collapse ─────────────────────────────────────────────────────

#[test]
fn parent_displaces_replay_across_batches() {
    // Replay ingested first (batch 1), parent arrives later (batch 2).
    let db = Database::open_in_memory().unwrap();
    let replay = claude_event("claude:msg-1:req-r", "msg-1", true, 100);
    upsert_usage_events_dedup_aware(db.conn(), &[replay], &replace_policies(1), "gen-1").unwrap();

    let parent = claude_event("claude:msg-1:req-p", "msg-1", false, 80);
    let outcome =
        upsert_usage_events_dedup_aware(db.conn(), &[parent], &replace_policies(1), "gen-1")
            .unwrap();

    let row = surviving_row(db.conn(), "gen-1", "claude:msg:msg-1");
    assert_eq!(row.is_sidechain, 0);
    assert_eq!(row.id, "claude:msg-1:req-p");
    assert_eq!(row.total_tokens, 80);
    assert_eq!(
        (outcome.inserted, outcome.replaced, outcome.dropped),
        (0, 1, 0)
    );
}

#[test]
fn replay_arriving_after_parent_is_dropped() {
    let db = Database::open_in_memory().unwrap();
    let parent = claude_event("claude:msg-1:req-p", "msg-1", false, 100);
    upsert_usage_events_dedup_aware(db.conn(), &[parent], &replace_policies(1), "gen-1").unwrap();

    let replay = claude_event("claude:msg-1:req-r", "msg-1", true, 100);
    let outcome =
        upsert_usage_events_dedup_aware(db.conn(), &[replay], &replace_policies(1), "gen-1")
            .unwrap();

    assert_eq!(outcome.dropped, 1);
    let row = surviving_row(db.conn(), "gen-1", "claude:msg:msg-1");
    assert_eq!(row.id, "claude:msg-1:req-p");
    assert_eq!(count_for_dedupe(db.conn(), "gen-1", "claude:msg:msg-1"), 1);
}

// ── Policy & null-dedupe-key behavior ────────────────────────────────────────

#[test]
fn null_dedupe_key_falls_back_to_id_no_collapse() {
    let db = Database::open_in_memory().unwrap();
    let mut a = NormalizedUsageEvent::minimal_for_test("evt-a", AgentKind::Codex);
    a.total_tokens = 100;
    let mut b = NormalizedUsageEvent::minimal_for_test("evt-b", AgentKind::Codex);
    b.total_tokens = 100;
    // No dedupe_key set — InsertOnce semantics key on id.
    let outcome = upsert_usage_events_dedup_aware(
        db.conn(),
        &[a, b],
        &[UsageWritePolicy::InsertOnce, UsageWritePolicy::InsertOnce],
        "gen-1",
    )
    .unwrap();
    assert_eq!(outcome.inserted, 2);
    assert_eq!(outcome.replaced, 0);
}

#[test]
fn insertonce_never_replaces_existing() {
    let db = Database::open_in_memory().unwrap();
    let mut first = NormalizedUsageEvent::minimal_for_test("evt-1", AgentKind::Codex);
    first.total_tokens = 100;
    let mut again = NormalizedUsageEvent::minimal_for_test("evt-1", AgentKind::Codex);
    again.total_tokens = 999; // would "win" under Replace, but InsertOnce ignores it
    let _ = upsert_usage_events_dedup_aware(
        db.conn(),
        &[first],
        &[UsageWritePolicy::InsertOnce],
        "gen-1",
    )
    .unwrap();
    let outcome = upsert_usage_events_dedup_aware(
        db.conn(),
        &[again],
        &[UsageWritePolicy::InsertOnce],
        "gen-1",
    )
    .unwrap();
    assert_eq!(outcome.inserted, 0, "duplicate InsertOnce id is ignored");
    let total: i64 = db
        .conn()
        .query_row(
            "SELECT total_tokens FROM usage_events WHERE id = 'evt-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(total, 100, "original row untouched");
}

// ── Rollup delta correctness ─────────────────────────────────────────────────

#[test]
fn effective_events_carry_delta_for_displacement() {
    // Replay (100) inserted, then parent (80) displaces it. The effective
    // events for the second batch must net to -20 so rollups end at 80.
    let db = Database::open_in_memory().unwrap();
    let replay = claude_event("claude:msg-1:req-r", "msg-1", true, 100);
    upsert_usage_events_dedup_aware(db.conn(), &[replay], &replace_policies(1), "gen-1").unwrap();

    let parent = claude_event("claude:msg-1:req-p", "msg-1", false, 80);
    let outcome =
        upsert_usage_events_dedup_aware(db.conn(), &[parent], &replace_policies(1), "gen-1")
            .unwrap();

    assert_eq!(outcome.effective_events.len(), 1);
    assert_eq!(
        outcome.effective_events[0].total_tokens, -20,
        "delta = parent(80) - replay(100)"
    );
    // Combined effective deltas across both batches net to 80.
    let net: i64 = 100 + outcome.effective_events[0].total_tokens;
    assert_eq!(net, 80);
}
