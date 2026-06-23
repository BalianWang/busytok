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
use busytok_domain::{AgentKind, OperationalDiagnosticEvent};
use busytok_store::{Database, RollupRows, StoreWriteBatch};

fn sample_diag(severity: &str) -> OperationalDiagnosticEvent {
    OperationalDiagnosticEvent {
        id: format!("diag-{}", severity),
        agent: Some(AgentKind::ClaudeCode),
        source_id: Some("src-1".into()),
        source_file_id: Some("file-1".into()),
        source_path: None,
        source_line: None,
        category: "parse_error".into(),
        severity: severity.to_string(),
        message: "test diagnostic".into(),
        detail_json: None,
        happened_at_ms: 1_000_000,
        created_at_ms: 1_000_000,
    }
}

#[test]
fn checkpoint_commit_persists_diagnostics_in_same_transaction() {
    let db = Database::open_in_memory().unwrap();
    let mut batch = StoreWriteBatch::default();
    batch.source_id = "src-1".into();
    batch.source_file_id = Some("file-1".into());
    batch.source_file_agent = "claude_code".into();
    batch.source_file_path = "/tmp/test.jsonl".into();
    batch.checkpoint_offset = Some(42);
    batch.diagnostic_events = vec![sample_diag("warning")];

    db.ingest_store_batch(batch, "gen-test", |_effective, _gen| {
        Ok(RollupRows::default())
    })
    .unwrap();

    assert_eq!(db.diagnostic_event_count().unwrap(), 1);
    assert_eq!(db.get_log_file("file-1").unwrap().unwrap().offset_bytes, 42);
}

#[test]
fn diagnostic_events_list_newest_first() {
    let db = Database::open_in_memory().unwrap();
    let early = OperationalDiagnosticEvent {
        id: "early".into(),
        agent: None,
        source_id: None,
        source_file_id: None,
        source_path: None,
        source_line: None,
        category: "parse_error".into(),
        severity: "warning".into(),
        message: "early".into(),
        detail_json: None,
        happened_at_ms: 1_000_000,
        created_at_ms: 1_000_000,
    };
    let late = OperationalDiagnosticEvent {
        id: "late".into(),
        agent: None,
        source_id: None,
        source_file_id: None,
        source_path: None,
        source_line: None,
        category: "store_health".into(),
        severity: "error".into(),
        message: "late".into(),
        detail_json: None,
        happened_at_ms: 2_000_000,
        created_at_ms: 2_000_000,
    };

    let mut batch = StoreWriteBatch::default();
    batch.source_id = "src-1".into();
    batch.source_file_agent = "claude_code".into();
    batch.source_file_path = "/tmp/test.jsonl".into();
    batch.diagnostic_events = vec![early, late];

    db.ingest_store_batch(batch, "gen-test", |_effective, _gen| {
        Ok(RollupRows::default())
    })
    .unwrap();

    let events = db.list_diagnostic_events("parse_error", 100).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].message, "early");

    let all = db.list_all_diagnostic_events(100).unwrap();
    assert_eq!(all.len(), 2);
    // newest first
    assert_eq!(all[0].message, "late");
    assert_eq!(all[1].message, "early");
}
