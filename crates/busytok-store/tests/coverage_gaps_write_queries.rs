#![recursion_limit = "512"]
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
//! Coverage gap tests for `write_queries.rs`.
//!
//! Targets the dedup-aware upsert branches (drop, sidechain replacement,
//! higher-total replacement) and the affected-source-ids fallback path.

use busytok_domain::{AgentKind, NormalizedUsageEvent, UsageWritePolicy};
use busytok_store::db::Database;
use busytok_store::write_queries;
use busytok_store::LogSourceRow;

fn make_replace_event(id: &str, dedupe_key: &str, total_tokens: i64) -> NormalizedUsageEvent {
    let mut event = NormalizedUsageEvent::minimal_for_test(id, AgentKind::ClaudeCode);
    event.timestamp_ms = 1_000;
    event.dedupe_key = Some(dedupe_key.to_string());
    event.total_tokens = total_tokens;
    event.input_tokens = total_tokens / 2;
    event.output_tokens = total_tokens - (total_tokens / 2);
    event
}

// ── Drop path: existing row wins on equal sidechain class + lower total ────
//
// Triggers `!new_wins` with `existing=Some`, exercising the debug! call at the
// "dropped usage event: existing row won" branch.

#[test]
fn upsert_dedup_aware_drops_new_event_when_existing_wins() {
    let db = Database::open_in_memory().unwrap();
    let existing = make_replace_event("evt-a", "claude:msg:dk-1", 1000);
    write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[existing],
        &[UsageWritePolicy::Replace],
        "gen-drop",
    )
    .unwrap();

    let loser = make_replace_event("evt-b", "claude:msg:dk-1", 500); // lower total → loses
    let outcome = write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[loser],
        &[UsageWritePolicy::Replace],
        "gen-drop",
    )
    .unwrap();
    assert_eq!(outcome.dropped, 1);
    assert_eq!(outcome.inserted, 0);
    assert_eq!(outcome.replaced, 0);

    // The persisted row is still evt-a (the original winner).
    let persisted_id: String = db
        .conn()
        .query_row(
            "SELECT id FROM usage_events WHERE dedupe_key = 'claude:msg:dk-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_id, "evt-a");
}

// ── Sidechain replacement: parent (non-sidechain) replaces sidechain ────────
//
// Triggers the `info!("parent usage replaced sidechain replay")` branch when
// a non-sidechain event wins against an existing sidechain row.

#[test]
fn upsert_dedup_aware_replaces_sidechain_with_parent_event() {
    let db = Database::open_in_memory().unwrap();
    let mut sidechain = make_replace_event("evt-sc", "claude:msg:dk-sc", 1000);
    sidechain.is_sidechain = true;
    write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[sidechain],
        &[UsageWritePolicy::Replace],
        "gen-sc",
    )
    .unwrap();

    // Parent event: non-sidechain, replaces the sidechain row regardless of
    // total_tokens (non-sidechain always wins).
    let parent = make_replace_event("evt-parent", "claude:msg:dk-sc", 100);
    let outcome = write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[parent],
        &[UsageWritePolicy::Replace],
        "gen-sc",
    )
    .unwrap();

    assert_eq!(outcome.replaced, 1);
    assert_eq!(outcome.inserted, 0);
    assert_eq!(outcome.dropped, 0);

    // The persisted row id is now the parent's.
    let persisted_id: String = db
        .conn()
        .query_row(
            "SELECT id FROM usage_events WHERE dedupe_key = 'claude:msg:dk-sc'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_id, "evt-parent");
}

// ── Higher-total replacement within the same sidechain class ─────────────────
//
// Two non-sidechain events with the same dedupe key, the second has a strictly
// higher total_tokens → replaces via the `debug!("replaced usage event with
// higher-total entry")` branch.

#[test]
fn upsert_dedup_aware_replaces_with_higher_total_event() {
    let db = Database::open_in_memory().unwrap();
    let first = make_replace_event("evt-lo", "claude:msg:dk-replace", 1000);
    write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[first],
        &[UsageWritePolicy::Replace],
        "gen-replace",
    )
    .unwrap();

    // Different id, same dedupe key, higher total → wins.
    let winner = make_replace_event("evt-hi", "claude:msg:dk-replace", 2000);
    let outcome = write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[winner],
        &[UsageWritePolicy::Replace],
        "gen-replace",
    )
    .unwrap();

    assert_eq!(outcome.replaced, 1);
    assert_eq!(outcome.inserted, 0);

    // The displaced first row is gone; only the higher-total winner remains.
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM usage_events WHERE dedupe_key = 'claude:msg:dk-replace'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    let persisted_id: String = db
        .conn()
        .query_row(
            "SELECT id FROM usage_events WHERE dedupe_key = 'claude:msg:dk-replace'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_id, "evt-hi");
}

// ── affected_source_ids_for_inserted_events empty-source-file path (L718) ────
//
// Events with empty `source_file_id` and a non-empty fallback list — exercises
// the early-return branch where `source_file_ids` is empty.

#[test]
fn refresh_source_summaries_for_inserted_events_uses_fallback_when_no_source_files() {
    let db = Database::open_in_memory().unwrap();
    let now_ms = 1_000;
    // Seed sources via the same upsert path production uses, matching the
    // existing `refresh_source_summaries_for_inserted_events_uses_log_file_source_ids`
    // test setup.
    for (id, root) in [("src-real", "/logs"), ("src-fallback", "/fallback")] {
        db.upsert_log_source(&LogSourceRow {
            id: id.to_string(),
            agent: "claude_code".to_string(),
            source_type: "jsonl".to_string(),
            root_path: root.to_string(),
            configured_by_user: 1,
            default_discovery_enabled: 1,
            status: "active".to_string(),
            last_scan_started_at_ms: Some(now_ms - 100),
            last_scan_completed_at_ms: Some(now_ms),
            last_error: None,
            first_seen_at_ms: now_ms,
            last_seen_at_ms: now_ms,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
        .unwrap();
    }
    // Seed a log_file so the refresh INSERT...SELECT has a join partner for
    // file_count (the fallback path must still materialize the summary row).
    db.conn()
        .execute(
            "INSERT INTO log_files \
             (id, source_id, agent, path, inode, size_bytes, offset_bytes, last_mtime_ms, \
              first_seen_at_ms, last_seen_at_ms, state, last_error, created_at_ms, updated_at_ms) \
             VALUES ('file-real', 'src-real', 'claude_code', '/logs/real.jsonl', NULL, 128, 128, \
                     ?1, ?1, ?1, 'active', NULL, ?1, ?1)",
            rusqlite::params![now_ms],
        )
        .unwrap();

    // No prior rebuild — the summary row must be created solely by the
    // function under test, so the assertion actually verifies the fallback
    // path rather than a pre-existing row.

    // Event with empty source_file_id — falls back to the provided list.
    let mut event = NormalizedUsageEvent::minimal_for_test("evt-no-source", AgentKind::ClaudeCode);
    event.timestamp_ms = now_ms;
    event.total_tokens = 50;
    event.source_file_id = String::new(); // empty → falls back to fallback list
    write_queries::insert_usage_events_batch(db.conn(), &[event.clone()], "gen-fb").unwrap();

    let tx = db.conn().unchecked_transaction().unwrap();
    write_queries::refresh_source_summaries_for_inserted_events_tx(
        &tx,
        "gen-fb",
        std::slice::from_ref(&event),
        &["src-fallback"],
    )
    .unwrap();
    tx.commit().unwrap();

    // The fallback source should have a refreshed summary row (exercises the
    // L717 early-return path in affected_source_ids_for_inserted_events).
    let fallback_exists: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM source_health_summary \
             WHERE generation_id = 'gen-fb' AND source_id = 'src-fallback'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fallback_exists, 1, "fallback source should have a summary row");

    // src-real is NOT in the fallback list and the event has no source_file_id
    // pointing to it, so it must NOT get a summary row — proving only the
    // fallback source was refreshed, not all sources.
    let real_exists: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM source_health_summary \
             WHERE generation_id = 'gen-fb' AND source_id = 'src-real'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(real_exists, 0, "non-fallback source should not have a summary row");
}

// ── remove_other_dedupe_rows no-op when only the keep_id matches ──────────────
//
// Triggers the case where the DELETE matches zero rows (the only row sharing
// the dedupe key is the keep_id itself). This exercises the `Ok(deleted)`
// return with `deleted = 0`.

#[test]
fn upsert_dedup_aware_with_same_id_rewrite_does_not_invoke_eviction_path() {
    let db = Database::open_in_memory().unwrap();
    let first = make_replace_event("evt-same-id", "claude:msg:dk-same", 1000);
    write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[first],
        &[UsageWritePolicy::Replace],
        "gen-same",
    )
    .unwrap();

    // Rewrite the SAME id with the SAME dedupe key but higher tokens. The
    // `needs_delete` check (`existing.id != event.id`) is false, so the eviction
    // / cache-metric-violation delete path is skipped.
    let mut rewrite = make_replace_event("evt-same-id", "claude:msg:dk-same", 2000);
    rewrite.is_sidechain = false;
    let outcome = write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[rewrite],
        &[UsageWritePolicy::Replace],
        "gen-same",
    )
    .unwrap();

    assert_eq!(outcome.replaced, 1);
    assert_eq!(outcome.inserted, 0);
    assert_eq!(outcome.dropped, 0);

    let persisted_total: i64 = db
        .conn()
        .query_row(
            "SELECT total_tokens FROM usage_events WHERE id = 'evt-same-id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_total, 2000);
}

// ── insert_usage_events_batch_with_empty_slice_is_noop ───────────────────────
//
// The empty-batch path: no events means no work, count is zero.

#[test]
fn insert_usage_events_batch_with_empty_slice_returns_zero() {
    let db = Database::open_in_memory().unwrap();
    let count = write_queries::insert_usage_events_batch(db.conn(), &[], "gen-empty").unwrap();
    assert_eq!(count, 0);
}

// ── update_materialized_aggregates_from_events empty-slice early return ──────
//
// `if events.is_empty() { return Ok(()); }` early-return path.

#[test]
fn update_materialized_aggregates_from_events_empty_is_noop() {
    let db = Database::open_in_memory().unwrap();
    write_queries::update_materialized_aggregates_from_events(db.conn(), &[], "gen-empty").unwrap();

    let count: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM usage_buckets_2s", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
