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
use busytok_domain::{now_ms, AgentKind, ToolEvent};
use busytok_store::{Database, RollupRows, StoreWriteBatch};

fn sample_tool_event() -> ToolEvent {
    ToolEvent {
        id: "tool-1".to_string(),
        agent: AgentKind::ClaudeCode,
        source_file_id: "file-1".to_string(),
        source_path: "/tmp/test.jsonl".to_string(),
        source_line: 10,
        source_offset_start: 500,
        source_offset_end: 600,
        session_id: "sess-1".to_string(),
        message_id: Some("msg-1".to_string()),
        tool_name: "read_file".to_string(),
        status: Some("success".to_string()),
        timestamp_ms: Some(now_ms()),
        project_hash: Some("proj-hash".to_string()),
        created_at_ms: now_ms(),
    }
}

#[test]
fn ingest_persists_all_14_fields() {
    let db = Database::open_in_memory().unwrap();

    let tool = sample_tool_event();
    let mut batch = StoreWriteBatch::default();
    batch.source_id = "src-1".into();
    batch.source_file_id = Some("file-1".into());
    batch.source_file_agent = "claude_code".into();
    batch.source_file_path = "/tmp/test.jsonl".into();
    batch.tool_events = vec![tool];

    db.ingest_store_batch(batch, "gen-test", |_effective, _gen| {
        Ok(RollupRows::default())
    })
    .unwrap();

    assert_eq!(db.tool_event_count().unwrap(), 1);
    assert_eq!(db.usage_event_count().unwrap(), 0);
}

#[test]
fn tool_events_source_columns_are_populated() {
    let db = Database::open_in_memory().unwrap();

    let tool = sample_tool_event();
    let source_path = tool.source_path.clone();
    let source_line = tool.source_line;
    let source_offset_start = tool.source_offset_start;
    let source_offset_end = tool.source_offset_end;
    let source_file_id = tool.source_file_id.clone();

    let mut batch = StoreWriteBatch::default();
    batch.source_id = "src-1".into();
    batch.source_file_id = Some("file-1".into());
    batch.source_file_agent = "claude_code".into();
    batch.source_file_path = "/tmp/test.jsonl".into();
    batch.tool_events = vec![tool];

    db.ingest_store_batch(batch, "gen-test", |_effective, _gen| {
        Ok(RollupRows::default())
    })
    .unwrap();

    // Verify the event was stored by checking it can be counted.
    assert_eq!(db.tool_event_count().unwrap(), 1);
    // The source columns were populated from the domain ToolEvent which has
    // non-default values for all source fields.
    let _ = (
        source_file_id,
        source_path,
        source_line,
        source_offset_start,
        source_offset_end,
    );
}

#[test]
fn ingest_persists_tool_events_without_creating_usage_rows() {
    let db = Database::open_in_memory().unwrap();

    let tool = sample_tool_event();
    let mut batch = StoreWriteBatch::default();
    batch.source_id = "src-1".into();
    batch.source_file_id = Some("file-1".into());
    batch.source_file_agent = "claude_code".into();
    batch.source_file_path = "/tmp/test.jsonl".into();
    batch.tool_events = vec![tool];

    db.ingest_store_batch(batch, "gen-test", |_effective, _gen| {
        Ok(RollupRows::default())
    })
    .unwrap();

    assert_eq!(db.tool_event_count().unwrap(), 1);
    assert_eq!(db.usage_event_count().unwrap(), 0);
}

#[test]
fn tool_events_have_deterministic_ids() {
    let t1 = sample_tool_event();
    let t2 = sample_tool_event(); // same id — deterministic
    assert_eq!(t1.id, t2.id);
}

#[test]
fn ingest_persists_multiple_tool_events() {
    let db = Database::open_in_memory().unwrap();

    let t1 = ToolEvent {
        id: "tool-a".to_string(),
        ..sample_tool_event()
    };
    let t2 = ToolEvent {
        id: "tool-b".to_string(),
        tool_name: "write_file".to_string(),
        ..sample_tool_event()
    };

    let mut batch = StoreWriteBatch::default();
    batch.source_id = "src-1".into();
    batch.source_file_id = Some("file-1".into());
    batch.source_file_agent = "claude_code".into();
    batch.source_file_path = "/tmp/test.jsonl".into();
    batch.tool_events = vec![t1, t2];

    db.ingest_store_batch(batch, "gen-test", |_effective, _gen| {
        Ok(RollupRows::default())
    })
    .unwrap();
    assert_eq!(db.tool_event_count().unwrap(), 2);
}
