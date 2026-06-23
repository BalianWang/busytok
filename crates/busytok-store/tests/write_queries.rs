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
//! Integration tests for write query modules.
//!
//! Tests exercise the write helpers directly against an in-memory database,
//! then verify state via raw SQL queries.

use busytok_domain::{AgentKind, NormalizedUsageEvent};
use busytok_store::db::Database;
use busytok_store::outbox_queries;
use busytok_store::write_queries;
use busytok_store::LogSourceRow;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_test_event(id: &str, timestamp_ms: i64, tokens: i64) -> NormalizedUsageEvent {
    let mut event = NormalizedUsageEvent::minimal_for_test(id, AgentKind::ClaudeCode);
    event.timestamp_ms = timestamp_ms;
    event.total_tokens = tokens;
    event.input_tokens = tokens / 2;
    event.output_tokens = tokens - (tokens / 2);
    event
}

fn seed_log_source_for_test(db: &Database, id: &str, agent: &str, root_path: &str, status: &str) {
    let now_ms = 1_000;
    db.upsert_log_source(&LogSourceRow {
        id: id.to_string(),
        agent: agent.to_string(),
        source_type: "jsonl".to_string(),
        root_path: root_path.to_string(),
        configured_by_user: 1,
        default_discovery_enabled: 1,
        status: status.to_string(),
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

fn seed_log_file_for_test(
    db: &Database,
    id: &str,
    source_id: &str,
    agent: &str,
    path: &str,
    offset_bytes: i64,
) {
    let now_ms = 1_000;
    db.conn()
        .execute(
            "INSERT INTO log_files \
             (id, source_id, agent, path, inode, size_bytes, offset_bytes, last_mtime_ms, \
              first_seen_at_ms, last_seen_at_ms, state, last_error, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?5, ?6, ?7, ?7, 'active', NULL, ?7, ?7)",
            rusqlite::params![id, source_id, agent, path, offset_bytes, now_ms, now_ms],
        )
        .unwrap();
}

fn seed_diagnostic_for_test(db: &Database, id: &str, source_id: &str, severity: &str) {
    let happened_at_ms = 2_000;
    db.conn()
        .execute(
            "INSERT INTO diagnostic_events \
             (id, agent, source_id, source_file_id, source_path, source_line, severity, code, \
              message, details_json, happened_at_ms, created_at_ms) \
             VALUES (?1, NULL, ?2, NULL, NULL, NULL, ?3, 'test', 'diagnostic', NULL, ?4, ?4)",
            rusqlite::params![id, source_id, severity, happened_at_ms],
        )
        .unwrap();
}

// ── Usage events batch ───────────────────────────────────────────────────────

#[test]
fn insert_usage_events_batch_inserts_and_counts() {
    let db = Database::open_in_memory().unwrap();
    let events = vec![
        make_test_event("evt-a", 1000, 100),
        make_test_event("evt-b", 2000, 200),
    ];
    let count = write_queries::insert_usage_events_batch(db.conn(), &events, "gen-1").unwrap();
    assert_eq!(count, 2);

    let db_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM usage_events WHERE generation_id = 'gen-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(db_count, 2);
}

#[test]
fn insert_usage_events_batch_ignores_duplicate_ids() {
    let db = Database::open_in_memory().unwrap();
    let evt = make_test_event("evt-dup", 1000, 100);
    let count1 =
        write_queries::insert_usage_events_batch(db.conn(), &[evt.clone()], "gen-1").unwrap();
    assert_eq!(count1, 1);
    let count2 = write_queries::insert_usage_events_batch(db.conn(), &[evt], "gen-1").unwrap();
    assert_eq!(
        count2, 0,
        "duplicate event should not be counted as inserted"
    );
}

#[test]
fn insert_usage_events_batch_stores_generation_id() {
    let db = Database::open_in_memory().unwrap();
    let evt = make_test_event("evt-gen", 1000, 100);
    write_queries::insert_usage_events_batch(db.conn(), &[evt], "gen-specific").unwrap();

    let stored_gen: String = db
        .conn()
        .query_row(
            "SELECT generation_id FROM usage_events WHERE id = 'evt-gen'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_gen, "gen-specific");
}

#[test]
fn rebuild_source_health_summary_materializes_source_counts() {
    let db = Database::open_in_memory().unwrap();
    seed_log_source_for_test(&db, "source-1", "claude_code", "/logs", "active");
    seed_log_file_for_test(
        &db,
        "file-1",
        "source-1",
        "claude_code",
        "/logs/a.jsonl",
        128,
    );
    let mut event = make_test_event("evt-source", 1000, 50);
    event.source_file_id = "file-1".to_string();
    write_queries::insert_usage_events_batch(db.conn(), &[event], "gen-1").unwrap();

    write_queries::rebuild_source_summaries(db.conn(), "gen-1").unwrap();

    let count: i64 = db
        .conn()
        .query_row(
            "SELECT event_count FROM source_health_summary \
             WHERE generation_id = 'gen-1' AND source_id = 'source-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn rebuild_source_health_summary_ignores_diagnostic_events() {
    let db = Database::open_in_memory().unwrap();
    seed_log_source_for_test(&db, "source-1", "claude_code", "/logs", "active");
    seed_diagnostic_for_test(&db, "diag-1", "source-1", "warning");
    seed_diagnostic_for_test(&db, "diag-2", "source-1", "error");

    write_queries::rebuild_source_summaries(db.conn(), "gen-1").unwrap();

    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM source_health_summary \
             WHERE generation_id = 'gen-1' AND source_id = 'source-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn refresh_source_summaries_updates_one_source_after_ingest() {
    let db = Database::open_in_memory().unwrap();
    seed_log_source_for_test(&db, "source-1", "claude_code", "/logs", "active");
    seed_log_source_for_test(&db, "source-2", "codex", "/other", "active");
    seed_log_file_for_test(
        &db,
        "file-1",
        "source-1",
        "claude_code",
        "/logs/a.jsonl",
        128,
    );

    write_queries::rebuild_source_summaries(db.conn(), "gen-1").unwrap();

    let mut event = make_test_event("evt-source", 1000, 50);
    event.source_file_id = "file-1".to_string();
    write_queries::insert_usage_events_batch(db.conn(), &[event], "gen-1").unwrap();
    write_queries::refresh_source_summaries_for_sources(db.conn(), "gen-1", &["source-1"]).unwrap();

    let source_1_count: i64 = db
        .conn()
        .query_row(
            "SELECT event_count FROM source_health_summary \
             WHERE generation_id = 'gen-1' AND source_id = 'source-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let source_2_count: i64 = db
        .conn()
        .query_row(
            "SELECT event_count FROM source_health_summary \
             WHERE generation_id = 'gen-1' AND source_id = 'source-2'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(source_1_count, 1);
    assert_eq!(source_2_count, 0);
}

#[test]
fn refresh_source_summaries_for_inserted_events_uses_log_file_source_ids() {
    let db = Database::open_in_memory().unwrap();
    seed_log_source_for_test(&db, "source-real", "claude_code", "/logs", "active");
    seed_log_source_for_test(&db, "source-fallback", "claude_code", "/fallback", "active");
    seed_log_file_for_test(
        &db,
        "file-real",
        "source-real",
        "claude_code",
        "/logs/real.jsonl",
        128,
    );

    write_queries::rebuild_source_summaries(db.conn(), "gen-1").unwrap();

    let mut event = make_test_event("evt-real", 1000, 50);
    event.source_file_id = "file-real".to_string();
    write_queries::insert_usage_events_batch(db.conn(), &[event.clone()], "gen-1").unwrap();

    let tx = db.conn().unchecked_transaction().unwrap();
    write_queries::refresh_source_summaries_for_inserted_events_tx(
        &tx,
        "gen-1",
        std::slice::from_ref(&event),
        &["source-fallback"],
    )
    .unwrap();
    tx.commit().unwrap();

    let source_real_count: i64 = db
        .conn()
        .query_row(
            "SELECT event_count FROM source_health_summary \
             WHERE generation_id = 'gen-1' AND source_id = 'source-real'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let source_fallback_count: i64 = db
        .conn()
        .query_row(
            "SELECT event_count FROM source_health_summary \
             WHERE generation_id = 'gen-1' AND source_id = 'source-fallback'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(source_real_count, 1);
    assert_eq!(source_fallback_count, 0);
}

#[test]
fn rebuild_source_summaries_clears_stale_rows_for_removed_sources() {
    let db = Database::open_in_memory().unwrap();
    seed_log_source_for_test(&db, "source-active", "claude_code", "/logs", "active");
    seed_log_source_for_test(&db, "source-removed", "claude_code", "/old", "removed");

    db.conn()
        .execute(
            "INSERT INTO source_health_summary \
             (generation_id, source_id, agent, root_path, source_type, status, \
              configured_by_user, last_scan_at_ms, file_count, parsed_file_count, \
              event_count, last_error, latest_activity_at_ms, created_at_ms, updated_at_ms) \
             VALUES ('gen-1', 'source-removed', 'claude_code', '/old', 'jsonl', 'removed', \
                     1, NULL, 0, 0, 0, NULL, NULL, 1, 1)",
            [],
        )
        .unwrap();
    write_queries::rebuild_source_summaries(db.conn(), "gen-1").unwrap();

    let removed_source_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM source_health_summary \
             WHERE generation_id = 'gen-1' AND source_id = 'source-removed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(removed_source_count, 0);
}

// ── Source file checkpoints ──────────────────────────────────────────────────

#[test]
fn upsert_source_file_checkpoint_replaces_offset() {
    let db = Database::open_in_memory().unwrap();

    write_queries::upsert_source_file_checkpoint(
        db.conn(),
        "cp-1",
        "src-1",
        "claude_code",
        "/tmp/f.jsonl",
        Some("inode-1"),
        100,
        500,
        Some(1000),
        "active",
        None,
    )
    .unwrap();

    let offset1: i64 = db
        .conn()
        .query_row(
            "SELECT offset_bytes FROM source_file_checkpoints WHERE id = 'cp-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(offset1, 100);

    // Update
    write_queries::upsert_source_file_checkpoint(
        db.conn(),
        "cp-1",
        "src-1",
        "claude_code",
        "/tmp/f.jsonl",
        Some("inode-1"),
        250,
        600,
        Some(2000),
        "active",
        None,
    )
    .unwrap();

    let offset2: i64 = db
        .conn()
        .query_row(
            "SELECT offset_bytes FROM source_file_checkpoints WHERE id = 'cp-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(offset2, 250);
}

// ── Generation observations ──────────────────────────────────────────────────

#[test]
fn insert_generation_observation_replaces_previous() {
    let db = Database::open_in_memory().unwrap();

    write_queries::insert_generation_observation(
        db.conn(),
        "gen-1",
        "file-1",
        100,
        500,
        Some(1000),
        Some("ok"),
        None,
    )
    .unwrap();

    // Second observation for same gen+file replaces
    write_queries::insert_generation_observation(
        db.conn(),
        "gen-1",
        "file-1",
        200,
        600,
        Some(2000),
        Some("ok"),
        None,
    )
    .unwrap();

    let (offset, size): (i64, i64) = db
        .conn()
        .query_row(
            "SELECT offset_bytes, size_bytes FROM generation_file_observations \
             WHERE generation_id = 'gen-1' AND source_file_id = 'file-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(offset, 200);
    assert_eq!(size, 600);
}

// ── Tail replay queue ────────────────────────────────────────────────────────

#[test]
fn enqueue_tail_replay_persists_pending_rows() {
    let db = Database::open_in_memory().unwrap();

    let rows = vec![
        write_queries::TailReplayEnqueue {
            source_file_id: "file-a".to_string(),
            event_seq: 1,
            event_data_json: r#"{"id":"evt-1","total_tokens":100}"#.to_string(),
        },
        write_queries::TailReplayEnqueue {
            source_file_id: "file-a".to_string(),
            event_seq: 2,
            event_data_json: r#"{"id":"evt-2","total_tokens":200}"#.to_string(),
        },
    ];
    write_queries::enqueue_tail_replay_rows(db.conn(), &rows).unwrap();

    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM tail_replay_queue WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn apply_replay_rows_writes_to_target_generation() {
    let db = Database::open_in_memory().unwrap();

    // Enqueue a replay row with full event data
    let event_json = serde_json::json!({
        "id": "evt-replay",
        "agent": "claude_code",
        "source_file_id": "sf-1",
        "source_path": "/tmp/test.jsonl",
        "source_line": 1,
        "source_offset_start": 0,
        "source_offset_end": 100,
        "session_id": "sess-1",
        "turn_id": "",
        "source_request_id": "",
        "message_id": "",
        "timestamp_ms": 5000,
        "project_path": "/project",
        "project_hash": "abc",
        "cwd": "/project",
        "model": "claude-sonnet",
        "model_provider": "anthropic",
        "agent_version": "1.0",
        "client_kind": "cli",
        "input_tokens": 50,
        "output_tokens": 50,
        "total_tokens": 100,
        "cached_input_tokens": 0,
        "cache_creation_tokens": 0,
        "cache_read_tokens": 0,
        "reasoning_tokens": 0,
        "thoughts_tokens": 0,
        "tool_tokens": 0,
        "cost_usd": null,
        "estimated_cost_usd": null,
        "cost_currency": "USD",
        "cost_source": "unknown",
        "price_catalog_version": "",
        "is_error": 0,
        "error_type": null,
        "raw_event_hash": "",
        "usage_limit_reset_time_ms": null,
        "created_at_ms": 5000,
        "updated_at_ms": 5000
    });
    let rows = vec![write_queries::TailReplayEnqueue {
        source_file_id: "file-a".to_string(),
        event_seq: 1,
        event_data_json: event_json.to_string(),
    }];
    write_queries::enqueue_tail_replay_rows(db.conn(), &rows).unwrap();

    // Apply replay to target generation
    let applied =
        write_queries::apply_replay_rows_to_target_generation(db.conn(), "gen-target", None, 10)
            .unwrap();
    assert_eq!(applied, 1);

    // Verify event exists in usage_events
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM usage_events WHERE generation_id = 'gen-target'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // Verify replay row was marked as applied
    let pending: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM tail_replay_queue WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 0);
}

// ── Diagnostic pruning ───────────────────────────────────────────────────────

#[test]
fn prune_diagnostic_events_removes_old_events() {
    let db = Database::open_in_memory().unwrap();
    // Insert diagnostic events with different timestamps
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO diagnostic_events \
             (id, agent, source_id, source_file_id, source_path, source_line, \
              severity, code, message, details_json, happened_at_ms, created_at_ms) \
             VALUES ('d1', NULL, NULL, NULL, NULL, NULL, 'info', 'test', 'msg1', NULL, 1000, 1000)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO diagnostic_events \
             (id, agent, source_id, source_file_id, source_path, source_line, \
              severity, code, message, details_json, happened_at_ms, created_at_ms) \
             VALUES ('d2', NULL, NULL, NULL, NULL, NULL, 'info', 'test', 'msg2', NULL, 5000, 5000)",
            [],
        )
        .unwrap();

    let deleted = write_queries::prune_diagnostic_events(db.conn(), 3000).unwrap();
    assert_eq!(deleted, 1);

    let remaining: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM diagnostic_events", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(remaining, 1);
}

// ── Event sequence state (outbox) ────────────────────────────────────────────

#[test]
fn allocate_event_sequence_batch_gives_contiguous_range() {
    let db = Database::open_in_memory().unwrap();
    let (start, end) = outbox_queries::allocate_event_sequence_batch(db.conn(), 5).unwrap();
    assert_eq!(start, 1);
    assert_eq!(end, 5);
}

#[test]
fn allocate_event_sequence_accumulates_across_calls() {
    let db = Database::open_in_memory().unwrap();
    outbox_queries::allocate_event_sequence_batch(db.conn(), 3).unwrap();
    outbox_queries::allocate_event_sequence_batch(db.conn(), 4).unwrap();
    let seq = outbox_queries::read_latest_event_seq(db.conn()).unwrap();
    assert_eq!(seq, 7);
}

// ── Property-style: aggregate consistency ────────────────────────────────────

#[test]
fn materialized_bucket_totals_equal_summed_canonical_usage_event_totals() {
    let db = Database::open_in_memory().unwrap();
    let generation_id = "gen-property";

    // Generate varied event batches with different agents, models, timestamps
    let batches: Vec<Vec<NormalizedUsageEvent>> = vec![
        vec![
            {
                let mut e = make_test_event("prop-a1", 1000, 100);
                e.cost_usd = Some(0.01);
                e.model = Some("model-x".to_string());
                e
            },
            {
                let mut e = make_test_event("prop-a2", 1500, 200);
                e.cost_usd = None;
                e.model = Some("model-y".to_string());
                e
            },
        ],
        vec![
            {
                let mut e = make_test_event("prop-b1", 3600_000, 50);
                e.cost_usd = Some(0.005);
                e.model = Some("model-x".to_string());
                e
            },
            {
                let mut e = make_test_event("prop-b2", 7200_000, 300);
                e.cost_usd = Some(0.03);
                e.model = Some("model-y".to_string());
                e
            },
            {
                let mut e = make_test_event("prop-b3", 10800_000, 150);
                e.cost_usd = None;
                e.model = Some("model-z".to_string());
                e
            },
        ],
    ];

    // Write via batch insert
    for batch in &batches {
        write_queries::insert_usage_events_batch(db.conn(), batch, generation_id).unwrap();
    }

    // Query canonical totals from usage_events
    let canonical: (i64, i64, i64, i64) = db
        .conn()
        .query_row(
            "SELECT \
                COALESCE(SUM(total_tokens), 0), \
                COALESCE(SUM(input_tokens), 0), \
                COALESCE(SUM(output_tokens), 0), \
                COUNT(*) \
             FROM usage_events \
             WHERE generation_id = ?1",
            rusqlite::params![generation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();

    // Now materialize aggregates
    let all_events: Vec<NormalizedUsageEvent> = batches.into_iter().flatten().collect();
    write_queries::update_materialized_aggregates_from_events(
        db.conn(),
        &all_events,
        generation_id,
    )
    .unwrap();

    // Each bucket table should have totals matching canonical usage_events
    for (table, label) in &[
        ("usage_buckets_2s", "2s"),
        ("usage_buckets_hour", "hour"),
        ("usage_buckets_day", "day"),
    ] {
        let bucket_totals: (i64, i64, i64, i64) = db
            .conn()
            .query_row(
                &format!(
                    "SELECT \
                        COALESCE(SUM(total_tokens), 0), \
                        COALESCE(SUM(input_tokens), 0), \
                        COALESCE(SUM(output_tokens), 0), \
                        COALESCE(SUM(event_count), 0) \
                     FROM {table} \
                     WHERE generation_id = ?1"
                ),
                rusqlite::params![generation_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .unwrap_or_else(|e| {
                panic!("failed to query {label} bucket totals: {e}");
            });

        assert_eq!(
            bucket_totals.0, canonical.0,
            "{label} bucket total_tokens ({}) must equal canonical total_tokens ({})",
            bucket_totals.0, canonical.0,
        );
        assert_eq!(
            bucket_totals.1, canonical.1,
            "{label} bucket input_tokens ({}) must equal canonical input_tokens ({})",
            bucket_totals.1, canonical.1,
        );
        assert_eq!(
            bucket_totals.2, canonical.2,
            "{label} bucket output_tokens ({}) must equal canonical output_tokens ({})",
            bucket_totals.2, canonical.2,
        );
        assert_eq!(
            bucket_totals.3, canonical.3,
            "{label} bucket event_count ({}) must equal canonical event_count ({})",
            bucket_totals.3, canonical.3,
        );
    }
}

#[test]
fn materialized_buckets_keep_generations_isolated() {
    let db = Database::open_in_memory().unwrap();
    let event = make_test_event("evt-a", 86_400_000, 100);

    write_queries::update_materialized_aggregates_from_events(db.conn(), &[event.clone()], "gen-1")
        .unwrap();
    write_queries::update_materialized_aggregates_from_events(db.conn(), &[event], "gen-2")
        .unwrap();

    for table in [
        "usage_buckets_2s",
        "usage_buckets_hour",
        "usage_buckets_day",
    ] {
        let count: i64 = db
            .conn()
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE bucket_start_ms = ?1"),
                rusqlite::params![86_400_000i64],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2, "{table} must not merge generations");
    }
}

#[test]
fn materialized_dimension_tables_store_generation_and_labels() {
    let db = Database::open_in_memory().unwrap();
    let mut event = make_test_event("evt-project", 86_400_000, 300);
    event.project_hash = Some("project-hash".to_string());
    event.project_path = Some("/workspace/project".to_string());
    event.client_kind = Some("claude_code".to_string());
    event.model = Some("claude-sonnet".to_string());
    event.session_id = "session-1".to_string();

    write_queries::update_materialized_aggregates_from_events(db.conn(), &[event], "gen-1")
        .unwrap();

    let row: (String, String, String, String) = db
        .conn()
        .query_row(
            "SELECT generation_id, project_id, project_path, model FROM usage_by_project_day",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (
            "gen-1".to_string(),
            "project-hash".to_string(),
            "/workspace/project".to_string(),
            "claude-sonnet".to_string(),
        )
    );
}

#[test]
fn materialized_bucket_cost_status_becomes_partial_after_unavailable_then_priced_event() {
    let db = Database::open_in_memory().unwrap();
    let mut unpriced = make_test_event("evt-unpriced", 86_400_000, 100);
    unpriced.model = Some("model-cost".to_string());
    unpriced.cost_usd = None;
    let mut priced = make_test_event("evt-priced", 86_400_500, 200);
    priced.model = Some("model-cost".to_string());
    priced.cost_usd = Some(0.25);

    write_queries::update_materialized_aggregates_from_events(db.conn(), &[unpriced], "gen-1")
        .unwrap();
    write_queries::update_materialized_aggregates_from_events(db.conn(), &[priced], "gen-1")
        .unwrap();

    for table in [
        "usage_buckets_2s",
        "usage_buckets_hour",
        "usage_buckets_day",
    ] {
        let status: String = db
            .conn()
            .query_row(
                &format!("SELECT cost_status FROM {table} WHERE generation_id = 'gen-1'"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "partial",
            "{table} should report mixed cost coverage"
        );
    }
}

#[test]
fn materialized_bucket_cost_status_becomes_partial_after_priced_then_unavailable_event() {
    let db = Database::open_in_memory().unwrap();
    let mut priced = make_test_event("evt-priced-first", 86_400_000, 100);
    priced.model = Some("model-cost".to_string());
    priced.cost_usd = Some(0.25);
    let mut unpriced = make_test_event("evt-unpriced-second", 86_400_500, 200);
    unpriced.model = Some("model-cost".to_string());
    unpriced.cost_usd = None;

    write_queries::update_materialized_aggregates_from_events(db.conn(), &[priced], "gen-1")
        .unwrap();
    write_queries::update_materialized_aggregates_from_events(db.conn(), &[unpriced], "gen-1")
        .unwrap();

    for table in [
        "usage_buckets_2s",
        "usage_buckets_hour",
        "usage_buckets_day",
    ] {
        let status: String = db
            .conn()
            .query_row(
                &format!("SELECT cost_status FROM {table} WHERE generation_id = 'gen-1'"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "partial",
            "{table} should report mixed cost coverage"
        );
    }
}

#[test]
fn materialized_dimension_cost_status_becomes_partial_after_mixed_cost_events() {
    let db = Database::open_in_memory().unwrap();
    let mut unpriced = make_test_event("evt-dim-unpriced", 86_400_000, 100);
    unpriced.project_hash = Some("project-cost".to_string());
    unpriced.project_path = Some("/workspace/project-cost".to_string());
    unpriced.client_kind = Some("claude_code".to_string());
    unpriced.model = Some("model-cost".to_string());
    unpriced.session_id = "session-cost".to_string();
    unpriced.cost_usd = None;

    let mut priced = unpriced.clone();
    priced.id = "evt-dim-priced".to_string();
    priced.timestamp_ms = 86_400_500;
    priced.total_tokens = 200;
    priced.input_tokens = 100;
    priced.output_tokens = 100;
    priced.cost_usd = Some(0.25);

    write_queries::update_materialized_aggregates_from_events(db.conn(), &[unpriced], "gen-1")
        .unwrap();
    write_queries::update_materialized_aggregates_from_events(db.conn(), &[priced], "gen-1")
        .unwrap();

    for table in [
        "usage_by_project_day",
        "usage_by_model_day",
        "usage_by_session_day",
        "usage_by_client_day",
    ] {
        let status: String = db
            .conn()
            .query_row(
                &format!("SELECT cost_status FROM {table} WHERE generation_id = 'gen-1'"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "partial",
            "{table} should preserve mixed cost coverage"
        );
    }
}

#[test]
fn materialized_dimension_tables_keep_generations_isolated() {
    let db = Database::open_in_memory().unwrap();
    let mut event = make_test_event("evt-dim", 86_400_000, 300);
    event.project_hash = Some("project-hash".to_string());
    event.project_path = Some("/workspace/project".to_string());
    event.client_kind = Some("claude_code".to_string());
    event.model = Some("claude-sonnet".to_string());
    event.session_id = "session-1".to_string();

    write_queries::update_materialized_aggregates_from_events(db.conn(), &[event.clone()], "gen-1")
        .unwrap();
    write_queries::update_materialized_aggregates_from_events(db.conn(), &[event], "gen-2")
        .unwrap();

    for table in [
        "usage_by_project_day",
        "usage_by_model_day",
        "usage_by_session_day",
        "usage_by_client_day",
    ] {
        let count: i64 = db
            .conn()
            .query_row(
                &format!("SELECT COUNT(DISTINCT generation_id) FROM {table}"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2, "{table} must not merge generations");
    }
}

// ── Durable event sequence persistence ─────────────────────────────────────

#[test]
fn durable_event_sequence_survives_restart() {
    let db = Database::open_in_memory().unwrap();

    // Phase 1: allocate event sequences.
    let (start1, end1) = outbox_queries::allocate_event_sequence_batch(db.conn(), 5).unwrap();
    assert_eq!(start1, 1);
    assert_eq!(end1, 5);

    // Checkpoint writer metrics with latest seq.
    outbox_queries::checkpoint_writer_metrics(db.conn(), 10, 500, Some(5)).unwrap();

    // Phase 2: append durable outbox events for sequences 1-5.
    let envelopes: Vec<(i64, String)> = (1..=5)
        .map(|i| {
            (
                i as i64,
                format!(
                    r#"{{"event_id":"evt-{}","event_seq":{},"generation_id":"gen-1"}}"#,
                    i, i
                ),
            )
        })
        .collect();
    outbox_queries::append_durable_outbox_events(db.conn(), &envelopes).unwrap();

    // Phase 3: allocate more sequences after "restart" (same DB).
    let (start2, end2) = outbox_queries::allocate_event_sequence_batch(db.conn(), 3).unwrap();
    assert_eq!(start2, 6);
    assert_eq!(
        end2, 8,
        "sequence counter must survive across checkpoint cycles"
    );

    // Phase 4: verify outbox log entries are readable.
    let rows = outbox_queries::read_outbox_since(db.conn(), 0, 10).unwrap();
    assert_eq!(rows.len(), 5);
    assert!(rows[0].1.contains("evt-1"));
    assert!(rows[4].1.contains("evt-5"));

    // Phase 5: verify service_state has correct metrics.
    let (qd, lag): (i64, i64) = db
        .conn()
        .query_row(
            "SELECT writer_queue_depth, aggregate_lag_ms FROM service_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(qd, 10);
    assert_eq!(lag, 500);

    // Phase 6: verify latest_event_seq survives across allocations.
    let latest = outbox_queries::read_latest_event_seq(db.conn()).unwrap();
    assert_eq!(
        latest, 8,
        "latest_event_seq should be 8 after second allocation"
    );
}

// ── Cache-metric diagnostic lifecycle (F1/F2/F3/F4/F5) ───────────────────────
//
// Contract: a `cache_metric` diagnostic (id `cache-metric-violation:{event_id}`)
// exists ⇔ that event's CURRENT persisted unified fields violate the
// cache-metric invariant. These tests pin each mutation path to the contract.

use busytok_domain::UsageWritePolicy;

/// Count cache_metric diagnostics for a given event id.
fn count_cache_metric_diag(db: &Database, event_id: &str) -> i64 {
    db.conn()
        .query_row(
            "SELECT COUNT(*) FROM diagnostic_events \
             WHERE id = ?1 AND code = 'cache_metric'",
            rusqlite::params![format!("cache-metric-violation:{event_id}")],
            |row| row.get(0),
        )
        .unwrap()
}

/// A Claude event with VIOLATING unified fields (cache exceeds total). The
/// invariant is `cache_read + cache_write + non_cached == total`; here
/// 800 + 0 + 10 = 810 != 10.
fn violating_event(id: &str) -> NormalizedUsageEvent {
    let mut e = NormalizedUsageEvent::minimal_for_test(id, AgentKind::ClaudeCode);
    e.dedupe_key = Some(format!("claude:msg:{id}"));
    e.provider_payload_shape = busytok_domain::cache_metrics::ProviderPayloadShape::AnthropicNative;
    e.prompt_input_total_tokens = 10;
    e.prompt_input_non_cached_tokens = 10;
    e.cache_read_tokens = 800;
    e.cache_creation_tokens = 0;
    e.total_tokens = 1000;
    e
}

/// A Claude event with VALID unified fields (1000 == 800 + 0 + 200).
fn valid_event(id: &str) -> NormalizedUsageEvent {
    let mut e = NormalizedUsageEvent::minimal_for_test(id, AgentKind::ClaudeCode);
    e.dedupe_key = Some(format!("claude:msg:{id}"));
    e.provider_payload_shape = busytok_domain::cache_metrics::ProviderPayloadShape::AnthropicNative;
    e.prompt_input_total_tokens = 1000;
    e.prompt_input_non_cached_tokens = 200;
    e.cache_read_tokens = 800;
    e.cache_creation_tokens = 0;
    e.total_tokens = 1000;
    e
}

// F2: InsertOnce no-op must not sync the diagnostic against an unpersisted event.
#[test]
fn f2_insertonce_noop_does_not_sync_diagnostic() {
    let db = Database::open_in_memory().unwrap();

    // First write: VALID event persisted → no diagnostic.
    write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[valid_event("evt-x")],
        &[UsageWritePolicy::InsertOnce],
        "gen-1",
    )
    .unwrap();
    assert_eq!(count_cache_metric_diag(&db, "evt-x"), 0);

    // Second write with the SAME id but VIOLATING fields is an INSERT OR IGNORE
    // no-op (changes == 0): the persisted row is still the valid first write,
    // so NO diagnostic must appear.
    write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[violating_event("evt-x")],
        &[UsageWritePolicy::InsertOnce],
        "gen-1",
    )
    .unwrap();
    assert_eq!(
        count_cache_metric_diag(&db, "evt-x"),
        0,
        "InsertOnce no-op must not sync diagnostic to unpersisted event"
    );

    // Sanity: a genuinely new id with violating fields DOES produce a diagnostic
    // (sync runs only when changes > 0).
    write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[violating_event("evt-y")],
        &[UsageWritePolicy::InsertOnce],
        "gen-1",
    )
    .unwrap();
    assert_eq!(count_cache_metric_diag(&db, "evt-y"), 1);
}

// F1: dedupe eviction must delete the evicted row's diagnostic.
#[test]
fn f1_dedupe_eviction_deletes_evicted_diagnostic() {
    let db = Database::open_in_memory().unwrap();

    // Write A (violating) with a dedupe key. Replace policy so it persists and
    // gets a diagnostic.
    write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[violating_event("id-a")],
        &[UsageWritePolicy::Replace],
        "gen-1",
    )
    .unwrap();
    assert_eq!(count_cache_metric_diag(&db, "id-a"), 1);

    // Write B: different id, SAME dedupe key, higher total_tokens so it wins,
    // and VALID unified fields so it must have NO diagnostic. A is evicted.
    let mut b = valid_event("id-b");
    b.dedupe_key = Some("claude:msg:id-a".to_string()); // collide with A's key
    b.total_tokens = 5000; // strictly higher than A's 1000 → wins
    write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[b],
        &[UsageWritePolicy::Replace],
        "gen-1",
    )
    .unwrap();

    assert_eq!(
        count_cache_metric_diag(&db, "id-a"),
        0,
        "evicted id-a's diagnostic must be deleted"
    );
    assert_eq!(
        count_cache_metric_diag(&db, "id-b"),
        0,
        "valid replacement id-b must have no diagnostic"
    );
}

// F3: replay must sync the diagnostic for the replayed event.
#[test]
fn f3_replay_syncs_diagnostic_on_violation() {
    let db = Database::open_in_memory().unwrap();

    // Build a replay JSON mirroring the real tail-replay snapshot shape.
    let base = || -> serde_json::Value {
        serde_json::json!({
            "id": "evt-replay",
            "agent": "claude_code",
            "source_file_id": "sf-1",
            "source_path": "/tmp/test.jsonl",
            "source_line": 1,
            "source_offset_start": 0,
            "source_offset_end": 100,
            "session_id": "sess-1",
            "turn_id": "",
            "source_request_id": "",
            "message_id": "",
            "timestamp_ms": 5000,
            "project_path": "/project",
            "project_hash": "abc",
            "cwd": "/project",
            "model": "claude-sonnet-4-5",
            "model_provider": "anthropic",
            "agent_version": "1.0",
            "client_kind": "cli",
            "input_tokens": 50,
            "output_tokens": 50,
            "total_tokens": 100,
            "cached_input_tokens": 0,
            "cache_creation_tokens": 0,
            "cache_read_tokens": 0,
            "reasoning_tokens": 0,
            "thoughts_tokens": 0,
            "tool_tokens": 0,
            "cost_usd": null,
            "estimated_cost_usd": null,
            "cost_currency": "USD",
            "cost_source": "unknown",
            "price_catalog_version": "",
            "is_error": 0,
            "error_type": null,
            "raw_event_hash": "",
            "usage_limit_reset_time_ms": null,
            "created_at_ms": 5000,
            "updated_at_ms": 5000,
            "provider_payload_shape": "anthropic_native",
            "prompt_input_total_tokens": 0,
            "prompt_input_non_cached_tokens": 0,
        })
    };

    // Violating replay: cache_read exceeds total (800 + 0 + 10 != 10).
    let mut v = base();
    v["id"] = serde_json::json!("evt-bad");
    v["prompt_input_total_tokens"] = serde_json::json!(10);
    v["prompt_input_non_cached_tokens"] = serde_json::json!(10);
    v["cache_read_tokens"] = serde_json::json!(800);
    write_queries::enqueue_tail_replay_rows(
        db.conn(),
        &[write_queries::TailReplayEnqueue {
            source_file_id: "file-a".to_string(),
            event_seq: 1,
            event_data_json: v.to_string(),
        }],
    )
    .unwrap();
    write_queries::apply_replay_rows_to_target_generation(db.conn(), "gen-target", None, 10)
        .unwrap();
    assert_eq!(
        count_cache_metric_diag(&db, "evt-bad"),
        1,
        "replayed violating event must produce a diagnostic"
    );

    // Valid replay: 1000 == 800 + 0 + 200. No diagnostic.
    let mut g = base();
    g["id"] = serde_json::json!("evt-good");
    g["prompt_input_total_tokens"] = serde_json::json!(1000);
    g["prompt_input_non_cached_tokens"] = serde_json::json!(200);
    g["cache_read_tokens"] = serde_json::json!(800);
    write_queries::enqueue_tail_replay_rows(
        db.conn(),
        &[write_queries::TailReplayEnqueue {
            source_file_id: "file-b".to_string(),
            event_seq: 2,
            event_data_json: g.to_string(),
        }],
    )
    .unwrap();
    write_queries::apply_replay_rows_to_target_generation(db.conn(), "gen-target", None, 10)
        .unwrap();
    assert_eq!(
        count_cache_metric_diag(&db, "evt-good"),
        0,
        "replayed valid event must produce no diagnostic"
    );
}

// F4: usage prune must delete cache_metric diagnostics for pruned ids.
#[test]
fn f4_usage_prune_deletes_diagnostics_for_pruned_ids() {
    let db = Database::open_in_memory().unwrap();

    // A violating event with an OLD timestamp (before the 24h cutoff).
    let mut old = violating_event("evt-old");
    old.timestamp_ms = busytok_domain::now_ms() - 86_400_000 - 1000; // > 24h ago
    write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[old],
        &[UsageWritePolicy::Replace],
        "gen-1",
    )
    .unwrap();
    assert_eq!(count_cache_metric_diag(&db, "evt-old"), 1);

    write_queries::prune_usage_events(db.conn(), "gen-1").unwrap();

    assert_eq!(
        count_cache_metric_diag(&db, "evt-old"),
        0,
        "diagnostic for pruned usage event must be deleted"
    );
}

// F5: diagnostic prune must NOT age/count-prune cache_metric diagnostics.
#[test]
fn f5_diagnostic_prune_excludes_cache_metric() {
    let db = Database::open_in_memory().unwrap();

    // An OLD cache_metric diagnostic (far in the past) for a live event.
    let mut old_cm = violating_event("evt-live");
    old_cm.timestamp_ms = busytok_domain::now_ms();
    write_queries::upsert_usage_events_dedup_aware(
        db.conn(),
        &[old_cm],
        &[UsageWritePolicy::Replace],
        "gen-1",
    )
    .unwrap();
    // Force its created_at into the distant past so age-pruning would normally
    // remove it.
    db.conn()
        .execute(
            "UPDATE diagnostic_events SET created_at_ms = 1 \
             WHERE id = 'cache-metric-violation:evt-live'",
            [],
        )
        .unwrap();
    assert_eq!(count_cache_metric_diag(&db, "evt-live"), 1);

    // Also seed an OLD non-cache_metric diagnostic that SHOULD be pruned.
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO diagnostic_events \
             (id, agent, source_id, source_file_id, source_path, source_line, \
              severity, code, message, details_json, happened_at_ms, created_at_ms) \
             VALUES ('d-old', NULL, NULL, NULL, NULL, NULL, 'info', 'parse_error', \
                     'msg', NULL, 1, 1)",
            [],
        )
        .unwrap();

    // Cutoff well past both diagnostics' created_at (1).
    let deleted =
        write_queries::prune_diagnostic_events(db.conn(), busytok_domain::now_ms()).unwrap();
    assert_eq!(deleted, 1, "only the non-cache_metric diagnostic is pruned");

    assert_eq!(
        count_cache_metric_diag(&db, "evt-live"),
        1,
        "cache_metric diagnostic survives age pruning"
    );

    let non_cm: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM diagnostic_events WHERE id = 'd-old'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(non_cm, 0, "non-cache_metric diagnostic was pruned");
}
