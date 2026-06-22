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
use busytok_store::{Database, LogSourceRow};

fn make_source(id: &str, first_seen: i64, created_at: i64, updated_at: i64) -> LogSourceRow {
    LogSourceRow {
        id: id.to_string(),
        agent: "claude_code".to_string(),
        source_type: "jsonl".to_string(),
        root_path: "/tmp/test".to_string(),
        configured_by_user: 0,
        default_discovery_enabled: 1,
        status: "active".to_string(),
        last_scan_started_at_ms: Some(1000),
        last_scan_completed_at_ms: None,
        last_error: None,
        first_seen_at_ms: first_seen,
        last_seen_at_ms: updated_at,
        created_at_ms: created_at,
        updated_at_ms: updated_at,
    }
}

#[test]
fn upsert_log_source_preserves_first_seen_and_created_at() {
    let db = Database::open_in_memory().unwrap();

    // First insert.
    let row = make_source("src-1", 1000, 1000, 1000);
    db.upsert_log_source(&row).unwrap();

    let stored = db.list_log_sources().unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].first_seen_at_ms, 1000);
    assert_eq!(stored[0].created_at_ms, 1000);

    // Upsert with different first_seen_at_ms and created_at_ms — these must be preserved.
    let row2 = make_source("src-1", 9999, 9999, 2000);
    db.upsert_log_source(&row2).unwrap();

    let stored2 = db.list_log_sources().unwrap();
    assert_eq!(stored2.len(), 1);
    assert_eq!(
        stored2[0].first_seen_at_ms, 1000,
        "first_seen_at_ms must be preserved on upsert"
    );
    assert_eq!(
        stored2[0].created_at_ms, 1000,
        "created_at_ms must be preserved on upsert"
    );
    assert_eq!(stored2[0].updated_at_ms, 2000, "updated_at_ms must advance");
}

#[test]
fn upsert_log_source_preserves_last_scan_started_at_when_null() {
    let db = Database::open_in_memory().unwrap();

    // First insert with last_scan_started_at_ms = Some(1000).
    let mut row = make_source("src-2", 1000, 1000, 1000);
    row.last_scan_started_at_ms = Some(1000);
    db.upsert_log_source(&row).unwrap();

    let stored = db.list_log_sources().unwrap();
    assert_eq!(stored[0].last_scan_started_at_ms, Some(1000));

    // Upsert with last_scan_started_at_ms = None — must NOT null it out.
    let mut row2 = make_source("src-2", 1000, 1000, 2000);
    row2.last_scan_started_at_ms = None;
    db.upsert_log_source(&row2).unwrap();

    let stored2 = db.list_log_sources().unwrap();
    assert_eq!(
        stored2[0].last_scan_started_at_ms,
        Some(1000),
        "last_scan_started_at_ms must be preserved when upsert passes None"
    );
}

#[test]
fn upsert_log_source_updates_last_scan_started_at_when_some() {
    let db = Database::open_in_memory().unwrap();

    let mut row = make_source("src-3", 1000, 1000, 1000);
    row.last_scan_started_at_ms = Some(1000);
    db.upsert_log_source(&row).unwrap();

    // Upsert with a new scan start time — must update.
    let mut row2 = make_source("src-3", 1000, 1000, 2000);
    row2.last_scan_started_at_ms = Some(2000);
    db.upsert_log_source(&row2).unwrap();

    let stored = db.list_log_sources().unwrap();
    assert_eq!(
        stored[0].last_scan_started_at_ms,
        Some(2000),
        "last_scan_started_at_ms must update when upsert passes Some"
    );
}
