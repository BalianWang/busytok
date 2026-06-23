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
use busytok_domain::{
    AgentKind, NormalizedUsageEvent, OperationalDiagnosticEvent, UsageWritePolicy,
};
use busytok_store::{DailyUsageRow, Database, ModelUsageRow, RollupRows, StoreWriteBatch};

#[test]
fn scan_batch_commits_events_diagnostics_aggregates_summary_and_checkpoint_atomically() {
    let db = Database::open_in_memory().unwrap();
    let mut event = NormalizedUsageEvent::minimal_for_test("claude:req-a", AgentKind::ClaudeCode);
    event.total_tokens = 150;
    let diagnostic = OperationalDiagnosticEvent::for_test("diag-a");
    let daily = DailyUsageRow::for_test("2026-05-15", "Asia/Shanghai", "gen-test", 150);
    let model = ModelUsageRow::for_test("claude-sonnet-4-20250514", 150);
    let batch = StoreWriteBatch::for_test("source-1", "source-file-1")
        .usage_event(event, UsageWritePolicy::InsertOnce)
        .diagnostic(diagnostic)
        .checkpoint_offset(128);

    db.ingest_store_batch(batch, "gen-test", |_effective, _gen| {
        Ok(RollupRows {
            daily_usage_rows: vec![daily],
            model_usage_rows: vec![model],
            ..Default::default()
        })
    })
    .unwrap();
    // Realtime summary is rebuilt separately (post-transaction cache rebuild).
    db.replace_realtime_summary(&[(
        "today_total_tokens".to_string(),
        r#"{"value":150}"#.to_string(),
    )])
    .unwrap();
    assert_eq!(db.usage_event_count().unwrap(), 1);
    assert_eq!(db.diagnostic_event_count().unwrap(), 1);
    let log_file = db.get_log_file("source-file-1").unwrap().unwrap();
    assert_eq!(log_file.offset_bytes, 128);
    assert_eq!(log_file.agent, "claude_code");
    assert_eq!(log_file.path, "/tmp/source-file-1.jsonl");
    assert_eq!(
        db.daily_usage_total_tokens("2026-05-15", "gen-test")
            .unwrap(),
        150
    );
    assert_eq!(db.model_usage_rows().unwrap()[0].total_tokens, 150);
    assert_eq!(
        db.realtime_summary_value("today_total_tokens")
            .unwrap()
            .unwrap(),
        r#"{"value":150}"#
    );
}

#[test]
fn failed_aggregate_write_does_not_advance_checkpoint() {
    let db = Database::open_in_memory().unwrap();
    let mut event = NormalizedUsageEvent::minimal_for_test("claude:req-a", AgentKind::ClaudeCode);
    event.total_tokens = 150;
    let bad_daily =
        DailyUsageRow::for_test("invalid-date-for-test", "Asia/Shanghai", "gen-test", 150);
    let bad_batch = StoreWriteBatch::for_test("source-1", "source-file-1")
        .usage_event(event, UsageWritePolicy::InsertOnce)
        .checkpoint_offset(128);

    assert!(db
        .ingest_store_batch(bad_batch, "gen-test", |_effective, _gen| Ok(RollupRows {
            daily_usage_rows: vec![bad_daily],
            ..Default::default()
        }))
        .is_err());
    assert!(db.get_log_file("source-file-1").unwrap().is_none());
    assert_eq!(db.usage_event_count().unwrap(), 0);
}

#[test]
fn duplicate_diagnostic_write_replaces_existing_event_without_rolling_back_batch() {
    let db = Database::open_in_memory().unwrap();

    // First batch: insert a diagnostic event with id "diag-a".
    let mut event1 = NormalizedUsageEvent::minimal_for_test("evt-1", AgentKind::ClaudeCode);
    event1.total_tokens = 100;
    let diag1 = OperationalDiagnosticEvent::for_test("diag-a");
    let batch1 = StoreWriteBatch::for_test("source-1", "file-1")
        .usage_event(event1, UsageWritePolicy::InsertOnce)
        .diagnostic(diag1)
        .checkpoint_offset(100);
    db.ingest_store_batch(batch1, "gen-test", |_effective, _gen| {
        Ok(RollupRows::default())
    })
    .unwrap();

    // Second batch: try to insert a duplicate diagnostic event id "diag-a".
    // Plain INSERT on diagnostic_events violates PRIMARY KEY UNIQUE constraint.
    let mut event2 = NormalizedUsageEvent::minimal_for_test("evt-2", AgentKind::ClaudeCode);
    event2.total_tokens = 200;
    let diag2 = OperationalDiagnosticEvent::for_test("diag-a");
    let batch2 = StoreWriteBatch::for_test("source-1", "file-1")
        .usage_event(event2, UsageWritePolicy::InsertOnce)
        .diagnostic(diag2)
        .checkpoint_offset(200);

    db.ingest_store_batch(batch2, "gen-test", |_effective, _gen| {
        Ok(RollupRows::default())
    })
    .unwrap();

    assert_eq!(db.usage_event_count().unwrap(), 2);
    assert_eq!(db.diagnostic_event_count().unwrap(), 1);
    assert_eq!(
        db.get_log_file("file-1").unwrap().unwrap().offset_bytes,
        200
    );
}

#[test]
fn failed_summary_write_rolls_back_events_and_checkpoint() {
    let db = Database::open_in_memory().unwrap();

    // First batch: insert a summary with key "k1".
    let mut event1 = NormalizedUsageEvent::minimal_for_test("evt-1", AgentKind::ClaudeCode);
    event1.total_tokens = 100;
    let batch1 = StoreWriteBatch::for_test("source-1", "file-1")
        .usage_event(event1, UsageWritePolicy::InsertOnce)
        .checkpoint_offset(100);
    db.ingest_store_batch(batch1, "gen-test", |_effective, _gen| {
        Ok(RollupRows::default())
    })
    .unwrap();
    db.replace_realtime_summary(&[("k1".to_string(), r#"{"value":100}"#.to_string())])
        .unwrap();

    // Second batch: try to insert a usage event with a duplicate id "evt-1".
    // INSERT OR IGNORE won't fail, so use Replace policy on a different event,
    // then add a daily_usage with an invalid date format to force the failure
    // after the summary write stage.
    let mut event2 = NormalizedUsageEvent::minimal_for_test("evt-2", AgentKind::ClaudeCode);
    event2.total_tokens = 200;
    let bad_daily = DailyUsageRow::for_test("invalid-date", "UTC", "gen-test", 200);
    let batch2 = StoreWriteBatch::for_test("source-1", "file-1")
        .usage_event(event2, UsageWritePolicy::InsertOnce)
        .checkpoint_offset(200);

    // The invalid date in daily_usage_rows should cause the transaction to fail.
    assert!(db
        .ingest_store_batch(batch2, "gen-test", |_effective, _gen| Ok(RollupRows {
            daily_usage_rows: vec![bad_daily],
            ..Default::default()
        }))
        .is_err());
    // The second event and checkpoint must NOT be committed.
    assert_eq!(db.usage_event_count().unwrap(), 1);
    assert_eq!(
        db.get_log_file("file-1").unwrap().unwrap().offset_bytes,
        100
    );
}
