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
use busytok_store::generation_commands;
use busytok_store::Database;

#[test]
fn persist_ready_exact_writes_service_state() {
    let db = Database::open_in_memory().expect("db");
    let result = generation_commands::persist_ready_exact_for_generation(db.conn(), "gen-1")
        .expect("persist");
    assert_eq!(result.generation_id, "gen-1");
    assert!(result.updated_at_ms > 0);

    let readiness: String = db
        .conn()
        .query_row(
            "SELECT readiness FROM service_state WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .expect("read");
    assert_eq!(readiness, "ready_exact");
}

#[test]
fn persist_ready_degraded_writes_service_state() {
    let db = Database::open_in_memory().expect("db");
    let result = generation_commands::persist_ready_degraded_for_generation(db.conn(), "gen-2")
        .expect("persist");
    assert_eq!(result.generation_id, "gen-2");

    let readiness: String = db
        .conn()
        .query_row(
            "SELECT readiness FROM service_state WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .expect("read");
    assert_eq!(readiness, "ready_degraded");
}

#[test]
fn recover_missing_generation_metadata_returns_none_on_empty_db() {
    let db = Database::open_in_memory().expect("db");
    let result =
        generation_commands::recover_missing_generation_metadata(&db, "UTC").expect("recover");
    assert!(result.is_none());
}

#[test]
fn transition_readiness_from_starting_to_ready_degraded() {
    let db = Database::open_in_memory().expect("db");
    let now = busytok_domain::now_ms();
    // Seed starting state.
    db.conn()
        .execute(
            "INSERT INTO service_state \
             (id, readiness, active_generation_id, writer_queue_depth, aggregate_lag_ms, \
              updated_at_ms) \
             VALUES (1, 'starting', 'gen-1', 0, 0, ?1)",
            rusqlite::params![now],
        )
        .expect("seed starting state");

    let transitioned =
        generation_commands::transition_readiness(db.conn(), "ready_degraded", "gen-1")
            .expect("transition");
    assert!(transitioned, "starting → ready_degraded should succeed");

    let readiness: String = db
        .conn()
        .query_row(
            "SELECT readiness FROM service_state WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .expect("read");
    assert_eq!(readiness, "ready_degraded");
}

#[test]
fn transition_readiness_rejects_ready_exact_without_promoted_generation() {
    let db = Database::open_in_memory().expect("db");
    let now = busytok_domain::now_ms();
    db.conn()
        .execute(
            "INSERT INTO service_state \
             (id, readiness, active_generation_id, writer_queue_depth, aggregate_lag_ms, \
              updated_at_ms) \
             VALUES (1, 'starting', 'gen-missing', 0, 0, ?1)",
            rusqlite::params![now],
        )
        .expect("seed");

    let transitioned =
        generation_commands::transition_readiness(db.conn(), "ready_exact", "gen-missing")
            .expect("transition");
    assert!(
        !transitioned,
        "ready_exact without promoted generation should be rejected"
    );
}

#[test]
fn transition_readiness_from_starting_to_ready_exact_with_promoted_generation() {
    let db = Database::open_in_memory().expect("db");
    let now = busytok_domain::now_ms();
    // Seed promoted generation.
    db.conn()
        .execute(
            "INSERT INTO audit_generations \
             (generation_id, state, started_at_ms, promoted_at_ms, is_active, \
              created_at_ms, updated_at_ms) \
             VALUES ('gen-1', 'promoted', ?1, ?1, 1, ?1, ?1)",
            rusqlite::params![now],
        )
        .expect("seed generation");
    // Seed starting state.
    db.conn()
        .execute(
            "INSERT INTO service_state \
             (id, readiness, active_generation_id, writer_queue_depth, aggregate_lag_ms, \
              updated_at_ms) \
             VALUES (1, 'starting', 'gen-1', 0, 0, ?1)",
            rusqlite::params![now],
        )
        .expect("seed state");

    let transitioned = generation_commands::transition_readiness(db.conn(), "ready_exact", "gen-1")
        .expect("transition");
    assert!(
        transitioned,
        "starting → ready_exact with promoted gen should succeed"
    );

    let readiness: String = db
        .conn()
        .query_row(
            "SELECT readiness FROM service_state WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .expect("read");
    assert_eq!(readiness, "ready_exact");
}

#[test]
fn transition_readiness_from_ready_degraded_to_ready_exact() {
    let db = Database::open_in_memory().expect("db");
    let now = busytok_domain::now_ms();
    db.conn()
        .execute(
            "INSERT INTO audit_generations \
             (generation_id, state, started_at_ms, promoted_at_ms, is_active, \
              created_at_ms, updated_at_ms) \
             VALUES ('gen-1', 'promoted', ?1, ?1, 1, ?1, ?1)",
            rusqlite::params![now],
        )
        .expect("seed generation");
    db.conn()
        .execute(
            "INSERT INTO service_state \
             (id, readiness, active_generation_id, writer_queue_depth, aggregate_lag_ms, \
              updated_at_ms) \
             VALUES (1, 'ready_degraded', 'gen-1', 0, 0, ?1)",
            rusqlite::params![now],
        )
        .expect("seed state");

    let transitioned = generation_commands::transition_readiness(db.conn(), "ready_exact", "gen-1")
        .expect("transition");
    assert!(
        transitioned,
        "ready_degraded → ready_exact with promoted gen should succeed"
    );

    let readiness: String = db
        .conn()
        .query_row(
            "SELECT readiness FROM service_state WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .expect("read");
    assert_eq!(readiness, "ready_exact");
}

#[test]
fn transition_readiness_rejects_when_already_ready_exact() {
    let db = Database::open_in_memory().expect("db");
    let now = busytok_domain::now_ms();
    db.conn()
        .execute(
            "INSERT INTO audit_generations \
             (generation_id, state, started_at_ms, promoted_at_ms, is_active, \
              created_at_ms, updated_at_ms) \
             VALUES ('gen-1', 'promoted', ?1, ?1, 1, ?1, ?1)",
            rusqlite::params![now],
        )
        .expect("seed generation");
    db.conn()
        .execute(
            "INSERT INTO service_state \
             (id, readiness, active_generation_id, writer_queue_depth, aggregate_lag_ms, \
              updated_at_ms) \
             VALUES (1, 'ready_exact', 'gen-1', 0, 0, ?1)",
            rusqlite::params![now],
        )
        .expect("seed state");

    let transitioned =
        generation_commands::transition_readiness(db.conn(), "ready_degraded", "gen-1")
            .expect("transition");
    assert!(
        !transitioned,
        "ready_exact → ready_degraded should be rejected by CAS guard"
    );
}

#[test]
fn recover_missing_generation_metadata_performs_bootstrap_recovery() {
    let db = Database::open_in_memory().expect("db");
    let now = 2000i64;
    // Seed usage events without generation_id — triggers bootstrap recovery.
    db.conn()
        .execute(
            "INSERT INTO usage_events \
             (id, agent, source_file_id, source_path, source_line, source_offset_start, \
              source_offset_end, session_id, raw_event_hash, timestamp_ms, created_at_ms, \
              updated_at_ms) \
             VALUES ('ev1', 'claude_code', 'f1', '/p.jsonl', 1, 0, 10, 's1', 'h1', ?1, ?1, ?1)",
            rusqlite::params![now],
        )
        .expect("seed event");

    let result =
        generation_commands::recover_missing_generation_metadata(&db, "UTC").expect("recover");
    assert!(result.is_some(), "should recover from orphaned events");
    let r = result.unwrap();
    assert!(r.generation_id.starts_with("gen-bootstrap-"));
    assert!(r.repaired, "bootstrap recovery should set repaired=true");
    assert!(
        matches!(r.readiness, generation_commands::StoreReadiness::ReadyExact),
        "bootstrap recovery should result in ReadyExact"
    );

    // Verify generation was created in audit_generations.
    let gen_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM audit_generations WHERE generation_id = ?1 AND state = 'promoted' AND is_active = 1",
            rusqlite::params![r.generation_id],
            |row| row.get(0),
        )
        .expect("check");
    assert_eq!(
        gen_count, 1,
        "recovered generation should be promoted and active"
    );
}

#[test]
fn recover_missing_generation_metadata_uses_iana_timezone_for_daily_usage() {
    let db = Database::open_in_memory().expect("db");
    // 2026-06-11 20:00 UTC = 2026-06-12 04:00 Asia/Shanghai
    let ts_ms = 1_781_246_400_000i64;
    db.conn()
        .execute(
            "INSERT INTO usage_events \
             (id, agent, source_file_id, source_path, source_line, source_offset_start, \
              source_offset_end, session_id, raw_event_hash, timestamp_ms, total_tokens, \
              input_tokens, output_tokens, created_at_ms, updated_at_ms) \
             VALUES ('ev-iana', 'claude_code', 'f1', '/p.jsonl', 1, 0, 10, 's1', 'h1', \
              ?1, 100, 50, 50, ?1, ?1)",
            rusqlite::params![ts_ms],
        )
        .expect("seed event");

    let result = generation_commands::recover_missing_generation_metadata(&db, "Asia/Shanghai")
        .expect("recover");
    let r = result.expect("should recover");

    // Verify daily_usage was rebuilt with correct IANA date and timezone.
    let (date, tz, gen_id, tokens): (String, String, String, i64) = db
        .conn()
        .query_row(
            "SELECT date, timezone, generation_id, total_tokens FROM daily_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("daily_usage row");
    assert_eq!(
        date, "2026-06-12",
        "date must use Asia/Shanghai boundary, not UTC"
    );
    assert_eq!(
        tz, "Asia/Shanghai",
        "timezone must match the recovery timezone"
    );
    assert_eq!(
        gen_id, r.generation_id,
        "generation_id must match recovered generation"
    );
    assert_eq!(tokens, 100);
}
