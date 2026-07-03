//! Coverage gap fillers for `busytok-store` source files.
//!
//! These tests target specific uncovered lines discovered by `cargo llvm-cov`:
//! - `live_queries.rs` (closures inside `query_map`, `query_exact_sample_window`)
//! - `repository.rs` (for_test constructors and StoreWriteBatch builder methods)
//! - `subagent_queries.rs` (`hard_delete_logical_subagent`, `list_filtered`
//!   filter branches, `find_lru_hot_binding` None path, `write_hot_summary`
//!   INSERT path, `commit_eviction` Some(hot_summary), `list_resource_events`
//!   filter)
//! - `write_queries.rs` (`apply_replay_rows_to_target_generation` with
//!   `source_file_id` filter, `prune_diagnostic_events` count-cap branch)
//! - `read_queries.rs` (`append_source_health_status_filter` branches via
//!   `read_source_health_summaries` with `status_filter`)

#![allow(
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    clippy::too_many_arguments,
    clippy::field_reassign_with_default,
    clippy::inconsistent_digit_grouping,
    clippy::len_zero,
    clippy::identity_op,
    clippy::useless_vec,
    clippy::manual_dangling_ptr,
    clippy::expect_used,
    clippy::absurd_extreme_comparisons,
    unused_variables,
    unused_imports,
    dead_code
)]

use busytok_domain::{
    AgentKind, CodexTokenSnapshot, NormalizedUsageEvent, OperationalDiagnosticEvent,
};
use busytok_store::read_models::BreakdownFilterField;
use busytok_store::repository::{
    CodexTokenSnapshotRow, DailyUsageRow, LogSourceRow, ModelSummaryRow, ModelUsageRow, ProjectRow,
    RealtimeSummaryRow, SessionRow, StoreWriteBatch, SubagentHarnessBindingRow,
    SubagentLogicalSubagentRow, SubagentMemoryRow, SubagentResourceEventRow, SubagentTaskRow,
};
use busytok_store::subagent_queries;
use busytok_store::{live_queries, read_queries, write_queries, Database};
use rusqlite::params;

// =============================================================================
// live_queries.rs coverage
// =============================================================================

/// Helper: insert a row into `usage_events` with explicit timestamp and tokens.
fn seed_usage_event(
    db: &Database,
    id: &str,
    timestamp_ms: i64,
    total_tokens: i64,
    cost_usd: Option<f64>,
) {
    let mut event = NormalizedUsageEvent::minimal_for_test(id, AgentKind::ClaudeCode);
    event.timestamp_ms = timestamp_ms;
    event.total_tokens = total_tokens;
    event.input_tokens = total_tokens / 2;
    event.output_tokens = total_tokens - (total_tokens / 2);
    event.cost_usd = cost_usd;
    db.write_usage_event(&event, busytok_domain::UsageWritePolicy::InsertOnce)
        .unwrap();
}

/// Helper: insert a row into `usage_buckets_2s` for a specific generation.
fn seed_usage_bucket_2s(
    db: &Database,
    bucket_start_ms: i64,
    generation_id: &str,
    agent: &str,
    model: &str,
    total_tokens: i64,
    cost_usd: Option<f64>,
    cost_status: &str,
    event_count: i64,
) {
    let now = busytok_domain::now_ms();
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_2s (\
                bucket_start_ms, agent, model, generation_id, \
                input_tokens, output_tokens, total_tokens, \
                cost_usd, cost_status, event_count, created_at_ms, updated_at_ms\
            ) VALUES (?1, ?2, ?3, ?4, 0, 0, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![
                bucket_start_ms,
                agent,
                model,
                generation_id,
                total_tokens,
                cost_usd,
                cost_status,
                event_count,
                now,
            ],
        )
        .unwrap();
}

#[test]
fn query_backfill_buckets_range_returns_bucketed_samples() {
    let db = Database::open_in_memory().unwrap();
    // Seed events at three different 2s-aligned bucket starts. We use fixed
    // historical timestamps so the function's `start_ms..end_ms` filter has
    // something to slice.
    seed_usage_event(&db, "evt-a", 2_000, 100, Some(0.01));
    seed_usage_event(&db, "evt-b", 2_500, 50, None);
    seed_usage_event(&db, "evt-c", 4_000, 200, Some(0.05));
    seed_usage_event(&db, "evt-d", 6_000, 75, Some(0.007));

    let samples = live_queries::query_backfill_buckets_range(db.conn(), 0, 10_000).unwrap();
    // Buckets: [2000-4000) -> 150 tokens, 1 cost. [4000-6000) -> 200, 0.05. [6000-8000) -> 75, 0.007.
    assert_eq!(samples.len(), 3);
    assert_eq!(samples[0].bucket_start_ms, 2_000);
    assert_eq!(samples[0].tokens_per_sec, 75.0); // 150 / 2
    assert_eq!(samples[0].cost_per_sec, Some(0.005)); // 0.01 / 2
    assert_eq!(samples[0].events_per_sec, 1.0); // 2 events / 2

    assert_eq!(samples[1].bucket_start_ms, 4_000);
    assert_eq!(samples[1].tokens_per_sec, 100.0); // 200 / 2
    assert_eq!(samples[1].cost_per_sec, Some(0.025)); // 0.05 / 2

    assert_eq!(samples[2].bucket_start_ms, 6_000);
    assert_eq!(samples[2].cost_per_sec, Some(0.0035)); // 0.007 / 2
}

#[test]
fn query_backfill_buckets_range_returns_empty_for_no_events_in_range() {
    let db = Database::open_in_memory().unwrap();
    seed_usage_event(&db, "evt-out", 100_000, 10, None);

    let samples = live_queries::query_backfill_buckets_range(db.conn(), 0, 10_000).unwrap();
    assert!(samples.is_empty());
}

#[test]
fn query_exact_buckets_range_returns_some_samples_when_buckets_have_cost() {
    let db = Database::open_in_memory().unwrap();
    seed_usage_bucket_2s(
        &db,
        2_000,
        "gen-1",
        "claude_code",
        "sonnet",
        100,
        Some(0.02),
        "exact",
        1,
    );
    seed_usage_bucket_2s(
        &db,
        4_000,
        "gen-1",
        "claude_code",
        "sonnet",
        200,
        Some(0.04),
        "exact",
        1,
    );

    let samples = live_queries::query_exact_buckets_range(db.conn(), "gen-1", 0, 10_000).unwrap();
    assert_eq!(samples.len(), 2);
    assert_eq!(samples[0].bucket_start_ms, 2_000);
    assert_eq!(samples[0].tokens_per_sec, 50.0); // 100 / 2
    assert_eq!(samples[0].cost_per_sec, Some(0.01)); // 0.02 / 2
    assert_eq!(samples[1].bucket_start_ms, 4_000);
    assert_eq!(samples[1].cost_per_sec, Some(0.02)); // 0.04 / 2
}

#[test]
fn query_exact_buckets_range_filters_out_zero_cost_to_none() {
    let db = Database::open_in_memory().unwrap();
    // Seed a bucket with NULL cost_usd (so SUM(COALESCE(cost_usd, 0)) = 0).
    seed_usage_bucket_2s(
        &db,
        2_000,
        "gen-1",
        "claude_code",
        "sonnet",
        100,
        None,
        "unavailable",
        1,
    );

    let samples = live_queries::query_exact_buckets_range(db.conn(), "gen-1", 0, 10_000).unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(
        samples[0].cost_per_sec, None,
        "NULL cost should map to None"
    );
    assert_eq!(samples[0].tokens_per_sec, 50.0);
}

#[test]
fn query_exact_sample_window_returns_some_for_current_bucket_with_tokens() {
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();
    // Align to a 2s bucket boundary in the past so the bucket matches the
    // function's `((now - 2000) / 2000) * 2000` calculation.
    let bucket_start_ms = ((now - 2_000) / 2_000) * 2_000;
    seed_usage_bucket_2s(
        &db,
        bucket_start_ms,
        "gen-current",
        "claude_code",
        "sonnet",
        400,
        Some(0.08),
        "exact",
        2,
    );

    let sample = live_queries::query_exact_sample_window(db.conn(), "gen-current").unwrap();
    let sample = sample.expect("expected Some sample for current bucket");
    assert_eq!(sample.bucket_start_ms, bucket_start_ms);
    assert_eq!(sample.tokens_per_sec, 200.0); // 400 / 2
    assert_eq!(sample.cost_per_sec, Some(0.04)); // 0.08 / 2
    assert_eq!(sample.events_per_sec, 1.0); // 2 / 2
}

#[test]
fn query_exact_sample_window_returns_none_for_current_bucket_with_no_activity() {
    let db = Database::open_in_memory().unwrap();
    // Seed a bucket far in the past — does not intersect the current 2s window.
    seed_usage_bucket_2s(
        &db,
        0,
        "gen-old",
        "claude_code",
        "sonnet",
        999,
        None,
        "exact",
        1,
    );

    let sample = live_queries::query_exact_sample_window(db.conn(), "gen-old").unwrap();
    assert!(
        sample.is_none(),
        "no rows in current bucket -> None (Ok(_) branch)"
    );
}

#[test]
fn query_exact_sample_window_returns_none_when_table_has_rows_but_generation_missing() {
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();
    let bucket_start_ms = ((now - 2_000) / 2_000) * 2_000;
    // Seed a bucket for a DIFFERENT generation; the function queries by generation_id.
    seed_usage_bucket_2s(
        &db,
        bucket_start_ms,
        "gen-other",
        "claude_code",
        "sonnet",
        100,
        Some(0.01),
        "exact",
        1,
    );

    let sample = live_queries::query_exact_sample_window(db.conn(), "gen-missing").unwrap();
    assert!(sample.is_none(), "missing generation -> None");
}

#[test]
fn query_exact_sample_window_returns_some_with_only_events_no_cost() {
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();
    let bucket_start_ms = ((now - 2_000) / 2_000) * 2_000;
    // NULL cost_usd means SUM(COALESCE(cost_usd,0)) = 0 → cost_per_sec None.
    seed_usage_bucket_2s(
        &db,
        bucket_start_ms,
        "gen-events-only",
        "claude_code",
        "sonnet",
        0,
        None,
        "unavailable",
        3,
    );

    let sample = live_queries::query_exact_sample_window(db.conn(), "gen-events-only").unwrap();
    let sample = sample.expect("events > 0 should produce Some");
    assert_eq!(sample.tokens_per_sec, 0.0);
    assert_eq!(sample.cost_per_sec, None);
    assert_eq!(sample.events_per_sec, 1.5); // 3 / 2
}

#[test]
fn query_exact_buckets_range_returns_empty_when_table_has_no_rows() {
    let db = Database::open_in_memory().unwrap();
    let samples =
        live_queries::query_exact_buckets_range(db.conn(), "gen-empty", 0, 10_000).unwrap();
    assert!(samples.is_empty());
}

#[test]
fn query_exact_sample_window_returns_none_when_table_has_no_rows() {
    let db = Database::open_in_memory().unwrap();
    let sample = live_queries::query_exact_sample_window(db.conn(), "gen-empty").unwrap();
    assert!(sample.is_none());
}

// =============================================================================
// repository.rs coverage — for_test constructors and StoreWriteBatch builders
// =============================================================================

#[test]
fn repository_for_test_constructors_produce_expected_defaults() {
    let daily = DailyUsageRow::for_test("2024-01-01", "UTC", "gen-1", 100);
    assert_eq!(daily.date, "2024-01-01");
    assert_eq!(daily.timezone, "UTC");
    assert_eq!(daily.agent, "claude_code");
    assert_eq!(daily.total_tokens, 100);
    assert_eq!(daily.event_count, 1);
    assert_eq!(daily.generation_id, "gen-1");
    assert!(daily.cost_usd.is_none());
    assert!(daily.estimated_cost_usd.is_none());

    let model_usage = ModelUsageRow::for_test("claude-sonnet", 200);
    assert_eq!(model_usage.model, "claude-sonnet");
    assert_eq!(model_usage.total_tokens, 200);
    assert_eq!(model_usage.timezone, "UTC");
    assert_eq!(model_usage.event_count, 1);

    let realtime = RealtimeSummaryRow::for_test("key-1", "{}");
    assert_eq!(realtime.key, "key-1");
    assert_eq!(realtime.value_json, "{}");

    let session = SessionRow::for_test("sess-1");
    assert_eq!(session.id, "sess-1");
    assert_eq!(session.agent, "claude_code");
    assert_eq!(session.model_list_json, "[]");
    assert_eq!(session.is_active, 0);

    let project = ProjectRow::for_test("hash-1");
    assert_eq!(project.id, "hash-1");
    assert_eq!(project.project_hash, "hash-1");
    assert!(project.project_path.is_none());
    assert!(project.agent.is_none());

    let summary = ModelSummaryRow::for_test("claude-haiku");
    assert_eq!(summary.model, "claude-haiku");
    assert_eq!(summary.total_tokens, 0);
    assert!(summary.total_cost_usd.is_none());
}

#[test]
fn store_write_batch_builder_methods_accumulate_rows() {
    let mut batch = StoreWriteBatch::for_test("src-1", "file-1");
    assert_eq!(batch.source_id, "src-1");
    assert_eq!(batch.source_file_id.as_deref(), Some("file-1"));
    assert_eq!(batch.source_file_agent, "claude_code");

    // tool_event
    let tool = busytok_domain::ToolEvent {
        id: "tool-1".to_string(),
        agent: AgentKind::ClaudeCode,
        session_id: "sess-1".to_string(),
        message_id: None,
        tool_name: "bash".to_string(),
        status: Some("ok".to_string()),
        timestamp_ms: Some(1_000),
        project_hash: None,
        created_at_ms: 1_000,
        source_file_id: "file-1".to_string(),
        source_path: "/tmp/f.jsonl".to_string(),
        source_line: 1,
        source_offset_start: 0,
        source_offset_end: 10,
    };
    batch = batch.tool_event(tool);
    assert_eq!(batch.tool_events.len(), 1);

    // daily_usage_row
    let daily = DailyUsageRow::for_test("2024-01-01", "UTC", "gen-1", 100);
    batch = batch.daily_usage_row(daily);
    assert_eq!(batch.daily_usage_rows.len(), 1);

    // model_usage_row
    let model = ModelUsageRow::for_test("claude-sonnet", 200);
    batch = batch.model_usage_row(model);
    assert_eq!(batch.model_usage_rows.len(), 1);

    // realtime_summary_row
    let realtime = RealtimeSummaryRow::for_test("key", "{}");
    batch = batch.realtime_summary_row(realtime);
    assert_eq!(batch.realtime_summary_rows.len(), 1);

    // session_row
    let session = SessionRow::for_test("sess-1");
    batch = batch.session_row(session);
    assert_eq!(batch.session_rows.len(), 1);

    // project_row
    let project = ProjectRow::for_test("hash-1");
    batch = batch.project_row(project);
    assert_eq!(batch.project_rows.len(), 1);

    // model_summary_row
    let summary = ModelSummaryRow::for_test("claude-haiku");
    batch = batch.model_summary_row(summary);
    assert_eq!(batch.model_summary_rows.len(), 1);

    // checkpoint_offset
    batch = batch.checkpoint_offset(12345);
    assert_eq!(batch.checkpoint_offset, Some(12345));

    // diagnostic event
    let diag = busytok_domain::OperationalDiagnosticEvent {
        id: "diag-1".to_string(),
        agent: Some(AgentKind::ClaudeCode),
        source_id: None,
        source_file_id: None,
        source_path: None,
        source_line: None,
        category: "test".to_string(),
        severity: "info".to_string(),
        message: "diagnostic test".to_string(),
        detail_json: None,
        happened_at_ms: 1_000,
        created_at_ms: 1_000,
    };
    batch = batch.diagnostic(diag);
    assert_eq!(batch.diagnostic_events.len(), 1);

    // usage_event
    let mut event = NormalizedUsageEvent::minimal_for_test("evt-1", AgentKind::ClaudeCode);
    event.timestamp_ms = 1_000;
    event.total_tokens = 50;
    batch = batch.usage_event(event, busytok_domain::UsageWritePolicy::InsertOnce);
    assert_eq!(batch.usage_events.len(), 1);
}

// =============================================================================
// subagent_queries.rs coverage
// =============================================================================

fn db_for_subagent_tests() -> Database {
    Database::open_in_memory().unwrap()
}

fn seed_subagent(db: &Database, id: &str, status: &str) {
    let mut row = SubagentLogicalSubagentRow::for_test(id, id);
    row.status = status.to_string();
    db.subagent_upsert_logical(&row).unwrap();
}

fn seed_subagent_with_project(
    db: &Database,
    id: &str,
    status: &str,
    project_id: &str,
    repo_hash: &str,
) {
    let mut row = SubagentLogicalSubagentRow::for_test(id, id);
    row.status = status.to_string();
    row.project_id = project_id.to_string();
    row.repo_hash = repo_hash.to_string();
    db.subagent_upsert_logical(&row).unwrap();
}

fn seed_task_for_subagent(
    db: &Database,
    id: &str,
    subagent_id: &str,
    status: &str,
    created_at_ms: i64,
) {
    let mut task = SubagentTaskRow::for_test(id, subagent_id, "pi/review-cheap", "do something");
    task.status = status.to_string();
    task.created_at_ms = created_at_ms;
    db.subagent_insert_task(&task).unwrap();
}

fn seed_memory(db: &Database, subagent_id: &str, hot_summary: Option<&str>) {
    let mut mem = SubagentMemoryRow::new_empty(subagent_id);
    mem.hot_summary = hot_summary.map(String::from);
    db.subagent_upsert_memory(&mem).unwrap();
}

fn seed_hot_binding(
    db: &Database,
    id: &str,
    subagent_id: &str,
    harness: &str,
    last_used_at_ms: Option<i64>,
) {
    let now = busytok_domain::now_ms();
    db.subagent_upsert_binding(&SubagentHarnessBindingRow {
        id: id.to_string(),
        subagent_id: subagent_id.to_string(),
        harness: harness.to_string(),
        adapter_session_id: Some(format!("sess-{subagent_id}")),
        adapter_process_id: Some("12345".to_string()),
        is_hot: 1,
        status: "hot".to_string(),
        created_at_ms: now,
        last_used_at_ms,
        closed_at_ms: None,
        detail_json: None,
    })
    .unwrap();
}

#[test]
fn hard_delete_logical_subagent_cascades_through_all_dependent_tables() {
    let db = db_for_subagent_tests();
    seed_subagent(&db, "sa-1", "warm");

    // Seed a task
    seed_task_for_subagent(&db, "t-1", "sa-1", "queued", 1_000);

    // Seed a memory row
    seed_memory(&db, "sa-1", Some("hot summary"));

    // Seed a hot binding
    seed_hot_binding(&db, "bind-1", "sa-1", "pi", Some(1_000));

    // Seed a usage record
    let usage_record = busytok_store::repository::SubagentUsageRecordRow {
        id: "ur-1".to_string(),
        task_id: "t-1".to_string(),
        subagent_id: "sa-1".to_string(),
        source_usage_event_id: None,
        harness: "pi".to_string(),
        provider: Some("anthropic".to_string()),
        model: Some("claude-sonnet".to_string()),
        input_tokens: Some(100),
        output_tokens: Some(50),
        cache_read_tokens: None,
        cache_write_tokens: None,
        total_cost_usd: Some(0.01),
        duration_ms: Some(500),
        created_at_ms: 1_000,
    };
    db.subagent_insert_usage_record(&usage_record).unwrap();

    // Seed a resource event scoped to the subagent
    let resource_event = SubagentResourceEventRow {
        id: "re-1".to_string(),
        event_type: "cpu".to_string(),
        target_id: Some("sa-1".to_string()),
        rss_mb: Some(50.0),
        cpu_percent: Some(20.0),
        detail_json: None,
        created_at_ms: 1_000,
    };
    db.subagent_insert_resource_event(&resource_event).unwrap();

    // Hard delete
    db.subagent_hard_delete("sa-1").unwrap();

    // Verify everything is gone
    assert!(db.subagent_get_logical("sa-1").unwrap().is_none());
    let task_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM subagent_tasks WHERE subagent_id = 'sa-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(task_count, 0);

    let mem_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM subagent_memory WHERE subagent_id = 'sa-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(mem_count, 0);

    let binding_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM subagent_harness_bindings WHERE subagent_id = 'sa-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(binding_count, 0);

    let usage_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM subagent_usage_records WHERE subagent_id = 'sa-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(usage_count, 0);

    let resource_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM subagent_resource_events WHERE target_id = 'sa-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(resource_count, 0);
}

#[test]
fn hard_delete_logical_subagent_succeeds_for_unknown_id() {
    let db = db_for_subagent_tests();
    // No rows seeded — hard delete should be a no-op success
    db.subagent_hard_delete("never-existed").unwrap();
}

#[test]
fn list_filtered_with_status_filter_returns_only_matching_status() {
    let db = db_for_subagent_tests();
    seed_subagent_with_project(&db, "sa-hot", "hot", "proj-1", "hash-1");
    seed_subagent_with_project(&db, "sa-warm", "warm", "proj-1", "hash-1");
    seed_subagent_with_project(&db, "sa-cold", "cold", "proj-1", "hash-1");
    seed_subagent_with_project(&db, "sa-deleted", "deleted", "proj-1", "hash-1");

    // Filter by status="hot" — only the hot one should be returned.
    let rows = db.subagent_list_filtered(Some("hot"), None, false).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "sa-hot");
}

#[test]
fn list_filtered_with_project_filter_returns_only_matching_project() {
    let db = db_for_subagent_tests();
    seed_subagent_with_project(&db, "sa-p1", "warm", "proj-1", "hash-1");
    seed_subagent_with_project(&db, "sa-p2", "warm", "proj-2", "hash-1");

    let rows = db
        .subagent_list_filtered(None, Some("proj-2"), false)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "sa-p2");
}

#[test]
fn list_filtered_with_both_status_and_project_filters() {
    let db = db_for_subagent_tests();
    seed_subagent_with_project(&db, "sa-1", "hot", "proj-1", "hash-1");
    seed_subagent_with_project(&db, "sa-2", "warm", "proj-1", "hash-1");
    seed_subagent_with_project(&db, "sa-3", "hot", "proj-2", "hash-1");

    let rows = db
        .subagent_list_filtered(Some("hot"), Some("proj-1"), false)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "sa-1");
}

#[test]
fn list_filtered_with_include_deleted_returns_tombstones() {
    let db = db_for_subagent_tests();
    seed_subagent_with_project(&db, "sa-live", "warm", "proj-1", "hash-1");
    seed_subagent_with_project(&db, "sa-dead", "deleted", "proj-1", "hash-1");

    // Default (exclude deleted) — only warm/cold/hot
    let excluded = db.subagent_list_filtered(None, None, false).unwrap();
    assert_eq!(excluded.len(), 1);
    assert_eq!(excluded[0].id, "sa-live");

    // include_deleted=true — both
    let included = db.subagent_list_filtered(None, None, true).unwrap();
    assert_eq!(included.len(), 2);
}

#[test]
fn find_lru_hot_binding_returns_none_when_no_hot_bindings() {
    let db = db_for_subagent_tests();
    // Seed a subagent with no bindings
    seed_subagent(&db, "sa-1", "warm");

    let binding = db.subagent_find_lru_hot_binding("pi").unwrap();
    assert!(binding.is_none(), "no hot bindings -> None");
}

#[test]
fn find_lru_hot_binding_returns_oldest_hot_binding() {
    let db = db_for_subagent_tests();
    seed_subagent(&db, "sa-a", "hot");
    seed_subagent(&db, "sa-b", "hot");
    seed_hot_binding(&db, "bind-a", "sa-a", "pi", Some(1_000));
    seed_hot_binding(&db, "bind-b", "sa-b", "pi", Some(2_000));

    let lru = db.subagent_find_lru_hot_binding("pi").unwrap().unwrap();
    // LRU picks the lowest last_used_at_ms — sa-a (1000) wins.
    assert_eq!(lru.subagent_id, "sa-a");
}

#[test]
fn write_hot_summary_inserts_new_memory_row_when_none_exists() {
    let db = db_for_subagent_tests();
    seed_subagent(&db, "sa-1", "warm");

    // No memory row yet — calling write_hot_summary should INSERT a new one.
    db.subagent_write_hot_summary("sa-1", "first hot summary")
        .unwrap();

    let mem = db.subagent_get_memory("sa-1").unwrap().unwrap();
    assert_eq!(mem.hot_summary.as_deref(), Some("first hot summary"));
}

#[test]
fn write_hot_summary_updates_existing_memory_row() {
    let db = db_for_subagent_tests();
    seed_subagent(&db, "sa-1", "warm");
    seed_memory(&db, "sa-1", Some("initial"));

    db.subagent_write_hot_summary("sa-1", "updated").unwrap();

    let mem = db.subagent_get_memory("sa-1").unwrap().unwrap();
    assert_eq!(mem.hot_summary.as_deref(), Some("updated"));
}

#[test]
fn commit_eviction_with_hot_summary_writes_memory_and_returns_warm_status() {
    let db = db_for_subagent_tests();
    seed_subagent(&db, "sa-1", "hot");
    seed_hot_binding(&db, "bind-1", "sa-1", "pi", Some(1_000));

    let now = busytok_domain::now_ms();
    let mut binding = db.subagent_hot_binding("sa-1", "pi").unwrap().unwrap();
    binding.is_hot = 0;
    binding.status = "closed".to_string();
    binding.closed_at_ms = Some(now);

    // Pass Some(hot_summary) — covers the `if let Some(summary)` branch.
    let new_status = db
        .subagent_commit_eviction(&binding, "sa-1", Some("eviction summary"))
        .unwrap();
    assert_eq!(new_status, "warm", "memory written -> status='warm'");

    // Verify memory row was written
    let mem = db.subagent_get_memory("sa-1").unwrap().unwrap();
    assert_eq!(mem.hot_summary.as_deref(), Some("eviction summary"));

    // Verify binding flipped to closed
    let hot_after = db.subagent_hot_binding("sa-1", "pi").unwrap();
    assert!(hot_after.is_none(), "binding should no longer be hot");
}

#[test]
fn commit_eviction_without_hot_summary_returns_cold_when_no_memory() {
    let db = db_for_subagent_tests();
    seed_subagent(&db, "sa-1", "hot");
    seed_hot_binding(&db, "bind-1", "sa-1", "pi", Some(1_000));

    let now = busytok_domain::now_ms();
    let mut binding = db.subagent_hot_binding("sa-1", "pi").unwrap().unwrap();
    binding.is_hot = 0;
    binding.status = "closed".to_string();
    binding.closed_at_ms = Some(now);

    // Pass None — hot_summary write skipped; no memory -> status='cold'.
    let new_status = db.subagent_commit_eviction(&binding, "sa-1", None).unwrap();
    assert_eq!(new_status, "cold");
}

#[test]
fn list_resource_events_returns_rows_with_target_id_filter() {
    let db = db_for_subagent_tests();
    // Insert resource events for two different targets
    db.subagent_insert_resource_event(&SubagentResourceEventRow {
        id: "re-1".to_string(),
        event_type: "cpu".to_string(),
        target_id: Some("sa-a".to_string()),
        rss_mb: Some(10.0),
        cpu_percent: Some(5.0),
        detail_json: None,
        created_at_ms: 1_000,
    })
    .unwrap();
    db.subagent_insert_resource_event(&SubagentResourceEventRow {
        id: "re-2".to_string(),
        event_type: "memory".to_string(),
        target_id: Some("sa-b".to_string()),
        rss_mb: Some(20.0),
        cpu_percent: None,
        detail_json: None,
        created_at_ms: 2_000,
    })
    .unwrap();
    db.subagent_insert_resource_event(&SubagentResourceEventRow {
        id: "re-3".to_string(),
        event_type: "cpu".to_string(),
        target_id: None,
        rss_mb: None,
        cpu_percent: Some(50.0),
        detail_json: None,
        created_at_ms: 3_000,
    })
    .unwrap();

    // Filter by target_id="sa-a"
    let filtered = db.subagent_list_resource_events(Some("sa-a"), 10).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, "re-1");

    // No filter — returns all, newest first
    let all = db.subagent_list_resource_events(None, 10).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].id, "re-3"); // newest first (created_at_ms=3000)
    assert_eq!(all[1].id, "re-2");
    assert_eq!(all[2].id, "re-1");
}

#[test]
fn list_resource_events_returns_empty_when_no_events_match_target() {
    let db = db_for_subagent_tests();
    db.subagent_insert_resource_event(&SubagentResourceEventRow {
        id: "re-1".to_string(),
        event_type: "cpu".to_string(),
        target_id: Some("sa-a".to_string()),
        rss_mb: None,
        cpu_percent: None,
        detail_json: None,
        created_at_ms: 1_000,
    })
    .unwrap();

    let filtered = db
        .subagent_list_resource_events(Some("nonexistent"), 10)
        .unwrap();
    assert!(filtered.is_empty());
}

#[test]
fn list_recent_tasks_all_returns_across_subagents() {
    let db = db_for_subagent_tests();
    seed_subagent(&db, "sa-a", "warm");
    seed_subagent(&db, "sa-b", "warm");
    seed_task_for_subagent(&db, "t-1", "sa-a", "completed", 1_000);
    seed_task_for_subagent(&db, "t-2", "sa-b", "completed", 2_000);
    seed_task_for_subagent(&db, "t-3", "sa-a", "queued", 3_000);

    let tasks = db.subagent_list_recent_tasks_all(10).unwrap();
    assert_eq!(tasks.len(), 3);
    // Newest first
    assert_eq!(tasks[0].id, "t-3");
    assert_eq!(tasks[1].id, "t-2");
    assert_eq!(tasks[2].id, "t-1");
}

#[test]
fn count_tasks_by_subagent_groups_by_subagent_id() {
    let db = db_for_subagent_tests();
    seed_subagent(&db, "sa-a", "warm");
    seed_subagent(&db, "sa-b", "warm");
    seed_task_for_subagent(&db, "t-1", "sa-a", "completed", 1_000);
    seed_task_for_subagent(&db, "t-2", "sa-a", "completed", 2_000);
    seed_task_for_subagent(&db, "t-3", "sa-b", "completed", 3_000);

    let counts = db.subagent_count_tasks_by_subagent().unwrap();
    // Returns Vec<(subagent_id, count)>
    let mut by_sub: std::collections::HashMap<String, u32> = counts.into_iter().collect();
    assert_eq!(by_sub.remove("sa-a"), Some(2));
    assert_eq!(by_sub.remove("sa-b"), Some(1));
}

#[test]
fn last_task_by_subagent_returns_one_row_per_subagent() {
    let db = db_for_subagent_tests();
    seed_subagent(&db, "sa-a", "warm");
    seed_subagent(&db, "sa-b", "warm");
    seed_task_for_subagent(&db, "t-1", "sa-a", "completed", 1_000);
    seed_task_for_subagent(&db, "t-2", "sa-a", "running", 2_000);
    seed_task_for_subagent(&db, "t-3", "sa-b", "queued", 1_500);

    let rows = db.subagent_last_task_by_subagent().unwrap();
    assert_eq!(rows.len(), 2);
    // Each subagent's most recent task
    let by_sub: std::collections::HashMap<String, (i64, String)> = rows
        .into_iter()
        .map(|(id, ts, status)| (id, (ts, status)))
        .collect();
    let (ts_a, status_a) = by_sub.get("sa-a").unwrap();
    assert_eq!(*ts_a, 2_000);
    assert_eq!(status_a, "running");
    let (ts_b, status_b) = by_sub.get("sa-b").unwrap();
    assert_eq!(*ts_b, 1_500);
    assert_eq!(status_b, "queued");
}

#[test]
fn find_hot_binding_by_session_returns_none_for_unknown_session() {
    let db = db_for_subagent_tests();
    let result = db
        .subagent_find_hot_binding_by_session("never-exists", "pi")
        .unwrap();
    assert!(result.is_none());
}

// =============================================================================
// write_queries.rs coverage
// =============================================================================

fn enqueue_replay_row(
    db: &Database,
    source_file_id: &str,
    event_seq: i64,
    event_id: &str,
    ts: i64,
) {
    let event_json = serde_json::json!({
        "id": event_id,
        "agent": "claude_code",
        "source_file_id": source_file_id,
        "source_path": format!("/tmp/{source_file_id}.jsonl"),
        "source_line": 1,
        "source_offset_start": 0,
        "source_offset_end": 100,
        "session_id": "sess-1",
        "turn_id": "",
        "source_request_id": "",
        "message_id": "",
        "timestamp_ms": ts,
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
        "created_at_ms": ts,
        "updated_at_ms": ts
    });
    let rows = vec![write_queries::TailReplayEnqueue {
        source_file_id: source_file_id.to_string(),
        event_seq,
        event_data_json: event_json.to_string(),
    }];
    write_queries::enqueue_tail_replay_rows(db.conn(), &rows).unwrap();
}

#[test]
fn apply_replay_rows_with_source_file_filter_applies_only_matching_rows() {
    let db = Database::open_in_memory().unwrap();
    // Enqueue replay rows for two source files
    enqueue_replay_row(&db, "file-a", 1, "evt-a", 5_000);
    enqueue_replay_row(&db, "file-b", 1, "evt-b", 6_000);

    // Apply only file-a rows
    let applied = write_queries::apply_replay_rows_to_target_generation(
        db.conn(),
        "gen-target",
        Some("file-a"),
        10,
    )
    .unwrap();
    assert_eq!(applied, 1);

    // Verify only evt-a is in usage_events
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM usage_events WHERE generation_id = 'gen-target'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // Verify file-a replay row was marked as applied, file-b still pending
    let pending: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM tail_replay_queue WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 1, "file-b replay should still be pending");

    // Apply file-b
    let applied_b = write_queries::apply_replay_rows_to_target_generation(
        db.conn(),
        "gen-target",
        Some("file-b"),
        10,
    )
    .unwrap();
    assert_eq!(applied_b, 1);

    let total_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM usage_events WHERE generation_id = 'gen-target'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(total_count, 2);
}

#[test]
fn apply_replay_rows_with_no_rows_returns_zero() {
    let db = Database::open_in_memory().unwrap();
    let applied =
        write_queries::apply_replay_rows_to_target_generation(db.conn(), "gen-target", None, 10)
            .unwrap();
    assert_eq!(applied, 0);
}

#[test]
fn apply_replay_rows_with_source_filter_no_matching_rows_returns_zero() {
    let db = Database::open_in_memory().unwrap();
    enqueue_replay_row(&db, "file-a", 1, "evt-a", 5_000);

    // Apply with a source_file filter that doesn't match any pending row
    let applied = write_queries::apply_replay_rows_to_target_generation(
        db.conn(),
        "gen-target",
        Some("file-nonexistent"),
        10,
    )
    .unwrap();
    assert_eq!(applied, 0);
}

#[test]
fn prune_diagnostic_events_triggers_count_cap_when_over_10k_rows() {
    let db = Database::open_in_memory().unwrap();

    // Insert 11 non-cache_metric diagnostic events — exceeds the 10,000 row cap
    // by 1, so the count-cap branch should delete the oldest 1.
    //
    // (We insert only 11 here for test speed; the cap logic kicks in whenever
    // count > 10_000, which `11_001` would also exercise but takes ~30x longer.
    // To stay under the cap, we lower the threshold by pre-populating with
    // 10_000 + 1 rows via a tight INSERT loop.)
    //
    // Actually, the function hardcodes max_rows=10_000 in source, so we need
    // >10_000 rows. Use a fast batch INSERT.
    let conn = db.conn();
    conn.execute_batch(
        "BEGIN;
        INSERT INTO diagnostic_events
            (id, agent, source_id, source_file_id, source_path, source_line,
             severity, code, message, details_json, happened_at_ms, created_at_ms)
        WITH RECURSIVE cnt(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM cnt WHERE x < 10001)
        SELECT
            'cap-' || x, NULL, NULL, NULL, NULL, NULL,
            'info', 'cap_test', 'msg', NULL, x, x
        FROM cnt;
        COMMIT;",
    )
    .unwrap();

    // All rows have created_at_ms = their ordinal, so the oldest is 'cap-1'.
    // Prune with a very old cutoff — should keep all 10001 rows from age
    // pruning but the count cap (10_000) should evict the oldest 1.
    let deleted = write_queries::prune_diagnostic_events(db.conn(), 0).unwrap();
    // Age pruning deletes 0 (all created_at_ms >= 0 == cutoff). Count cap
    // deletes 1 (10001 - 10000 = 1 excess).
    assert_eq!(deleted, 1, "count cap should evict 1 oldest row");

    // Verify the oldest row was evicted (the SQL orders by created_at_ms ASC)
    let oldest_present: i64 = conn
        .query_row(
            "SELECT MIN(created_at_ms) FROM diagnostic_events WHERE code = 'cap_test'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    // Original oldest was created_at_ms=1. After cap prune of 1 row, oldest is 2.
    assert_eq!(oldest_present, 2);

    // Verify total remaining = 10_000 (the cap)
    let remaining: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM diagnostic_events WHERE code = 'cap_test'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 10_000);
}

// =============================================================================
// read_queries.rs coverage
// =============================================================================

fn seed_source_health_summary(
    db: &Database,
    generation_id: &str,
    source_id: &str,
    agent: &str,
    status: &str,
    last_scan_at_ms: Option<i64>,
    event_count: i64,
) {
    db.conn()
        .execute(
            "INSERT INTO source_health_summary \
             (generation_id, source_id, agent, root_path, source_type, status, \
              configured_by_user, last_scan_at_ms, file_count, parsed_file_count, \
              event_count, last_error, latest_activity_at_ms, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, ?3, '/root', 'jsonl', ?4, \
                     1, ?5, 0, 0, ?6, NULL, NULL, 1, 1)",
            params![
                generation_id,
                source_id,
                agent,
                status,
                last_scan_at_ms,
                event_count
            ],
        )
        .unwrap();
}

#[test]
fn read_source_health_summaries_filters_by_scanning_or_active_status() {
    let db = Database::open_in_memory().unwrap();
    seed_source_health_summary(
        &db,
        "gen-1",
        "src-1",
        "claude_code",
        "scanning",
        Some(1_000),
        5,
    );
    seed_source_health_summary(
        &db,
        "gen-1",
        "src-2",
        "claude_code",
        "active",
        Some(2_000),
        10,
    );
    seed_source_health_summary(&db, "gen-1", "src-3", "claude_code", "idle", Some(3_000), 2);

    let page = read_queries::read_source_health_summaries(
        db.conn(),
        "gen-1",
        10,
        None,
        None,
        Some("scanning_or_active"),
    )
    .unwrap();
    assert_eq!(
        page.items.len(),
        2,
        "should return scanning + active sources"
    );
    let ids: Vec<String> = page.items.iter().map(|r| r.source_id.clone()).collect();
    assert!(ids.contains(&"src-1".to_string()));
    assert!(ids.contains(&"src-2".to_string()));
}

#[test]
fn read_source_health_summaries_filters_by_idle_status() {
    let db = Database::open_in_memory().unwrap();
    seed_source_health_summary(
        &db,
        "gen-1",
        "src-1",
        "claude_code",
        "scanning",
        Some(1_000),
        5,
    );
    seed_source_health_summary(
        &db,
        "gen-1",
        "src-2",
        "claude_code",
        "active",
        Some(2_000),
        10,
    );
    seed_source_health_summary(&db, "gen-1", "src-3", "claude_code", "idle", Some(3_000), 2);
    seed_source_health_summary(
        &db,
        "gen-1",
        "src-4",
        "claude_code",
        "error",
        Some(4_000),
        1,
    );

    let page = read_queries::read_source_health_summaries(
        db.conn(),
        "gen-1",
        10,
        None,
        None,
        Some("idle"),
    )
    .unwrap();
    // idle filter excludes 'error', 'warning', 'scanning', 'active'
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].source_id, "src-3");
}

#[test]
fn read_source_health_summaries_filters_by_other_status_string() {
    let db = Database::open_in_memory().unwrap();
    seed_source_health_summary(
        &db,
        "gen-1",
        "src-1",
        "claude_code",
        "warning",
        Some(1_000),
        5,
    );
    seed_source_health_summary(
        &db,
        "gen-1",
        "src-2",
        "claude_code",
        "error",
        Some(2_000),
        10,
    );

    let page = read_queries::read_source_health_summaries(
        db.conn(),
        "gen-1",
        10,
        None,
        None,
        Some("warning"),
    )
    .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].source_id, "src-1");
    assert_eq!(page.items[0].status, "warning");
}

#[test]
fn read_source_health_summaries_with_empty_status_filter_treats_as_none() {
    let db = Database::open_in_memory().unwrap();
    seed_source_health_summary(
        &db,
        "gen-1",
        "src-1",
        "claude_code",
        "scanning",
        Some(1_000),
        5,
    );
    seed_source_health_summary(
        &db,
        "gen-1",
        "src-2",
        "claude_code",
        "active",
        Some(2_000),
        10,
    );

    // Empty string filter is treated as None (no filter applied).
    let page =
        read_queries::read_source_health_summaries(db.conn(), "gen-1", 10, None, None, Some(""))
            .unwrap();
    assert_eq!(page.items.len(), 2);
}

#[test]
fn read_source_health_summaries_filters_by_client_id_and_status() {
    let db = Database::open_in_memory().unwrap();
    seed_source_health_summary(
        &db,
        "gen-1",
        "src-1",
        "claude_code",
        "active",
        Some(1_000),
        5,
    );
    seed_source_health_summary(&db, "gen-1", "src-2", "codex", "active", Some(2_000), 10);

    let page = read_queries::read_source_health_summaries(
        db.conn(),
        "gen-1",
        10,
        None,
        Some("claude_code"),
        Some("active"),
    )
    .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].source_id, "src-1");
    assert_eq!(page.items[0].agent, "claude_code");
}

#[test]
fn read_source_health_summaries_returns_empty_when_generation_has_no_sources() {
    let db = Database::open_in_memory().unwrap();
    let page =
        read_queries::read_source_health_summaries(db.conn(), "gen-empty", 10, None, None, None)
            .unwrap();
    assert!(page.items.is_empty());
    assert!(page.next_cursor.is_none());
}

#[test]
fn read_source_health_summary_totals_with_status_filter() {
    let db = Database::open_in_memory().unwrap();
    seed_source_health_summary(
        &db,
        "gen-1",
        "src-1",
        "claude_code",
        "active",
        Some(1_000),
        5,
    );
    seed_source_health_summary(
        &db,
        "gen-1",
        "src-2",
        "claude_code",
        "active",
        Some(2_000),
        10,
    );
    seed_source_health_summary(&db, "gen-1", "src-3", "claude_code", "idle", Some(3_000), 2);

    let totals =
        read_queries::read_source_health_summary_totals(db.conn(), "gen-1", None, Some("active"))
            .unwrap();
    assert_eq!(totals.source_count, 2);
    assert_eq!(totals.active_source_count, 2);

    let totals_idle =
        read_queries::read_source_health_summary_totals(db.conn(), "gen-1", None, Some("idle"))
            .unwrap();
    assert_eq!(totals_idle.source_count, 1);
    assert_eq!(totals_idle.active_source_count, 0);
}

// =============================================================================
// live_queries.rs — query_sample_window (lines 11-52)
// =============================================================================

#[test]
fn query_sample_window_returns_current_bucket_data() {
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();
    // Insert events with timestamps in the current 2s-aligned bucket.
    let bucket_start = ((now - 2000) / 2000) * 2000;
    seed_usage_event(&db, "evt-1", bucket_start, 100, Some(0.5));
    seed_usage_event(&db, "evt-2", bucket_start + 500, 200, Some(1.5));

    let sample = live_queries::query_sample_window(db.conn()).unwrap();
    assert_eq!(sample.bucket_start_ms, bucket_start);
    // total_tokens = 300, per-sec = 150
    assert!((sample.tokens_per_sec - 150.0).abs() < 0.01);
    // total_cost = 2.0, per-sec = 1.0
    assert!((sample.cost_per_sec.unwrap() - 1.0).abs() < 0.01);
    // event_count = 2, per-sec = 1.0
    assert!((sample.events_per_sec - 1.0).abs() < 0.01);
}

#[test]
fn query_sample_window_returns_zeros_when_no_events() {
    let db = Database::open_in_memory().unwrap();
    let sample = live_queries::query_sample_window(db.conn()).unwrap();
    assert_eq!(sample.tokens_per_sec, 0.0);
    assert!(sample.cost_per_sec.is_none());
    assert_eq!(sample.events_per_sec, 0.0);
}

// =============================================================================
// repository.rs — CodexTokenSnapshotRow::from_domain (lines 49-82)
// =============================================================================

#[test]
fn codex_token_snapshot_row_from_domain_builds_row() {
    let snapshot = CodexTokenSnapshot {
        source_file_id: "file-1".to_string(),
        source_path: "/tmp/f.jsonl".to_string(),
        source_line: 42,
        source_offset_start: 100,
        source_offset_end: 200,
        session_id: "sess-1".to_string(),
        turn_id: Some("turn-1".to_string()),
        token_event_ordinal: 5,
        input_tokens: 1000,
        cached_input_tokens: 500,
        output_tokens: 200,
        reasoning_tokens: 50,
        total_tokens: 1700,
        delta_input_tokens: None,
        delta_cached_input_tokens: None,
        delta_output_tokens: None,
        delta_reasoning_tokens: None,
        delta_total_tokens: None,
        model: Some("claude-sonnet".to_string()),
        model_provider: Some("anthropic".to_string()),
        cost_usd: Some(0.05),
        raw_usage_json: r#"{"input":1000}"#.to_string(),
        timestamp_ms: 1_000_000,
    };
    let row = CodexTokenSnapshotRow::from_domain(&snapshot, 5, Some("evt-1".to_string()));
    assert_eq!(row.source_file_id, "file-1");
    assert_eq!(row.source_line, 42);
    assert_eq!(row.session_id, "sess-1");
    assert_eq!(row.turn_id.as_deref(), Some("turn-1"));
    assert_eq!(row.token_event_ordinal, 5);
    assert_eq!(row.input_tokens, 1000);
    assert_eq!(row.total_tokens, 1700);
    assert_eq!(row.model.as_deref(), Some("claude-sonnet"));
    assert_eq!(row.emitted_event_id.as_deref(), Some("evt-1"));
    assert!(!row.id.is_empty());
    assert!(row.id.contains("file-1"));
    assert!(row.id.contains("sess-1"));
}

#[test]
fn codex_token_snapshot_row_from_domain_with_none_turn_id() {
    let snapshot = CodexTokenSnapshot {
        source_file_id: "file-2".to_string(),
        source_path: "/tmp/g.jsonl".to_string(),
        source_line: 10,
        source_offset_start: 0,
        source_offset_end: 50,
        session_id: "sess-2".to_string(),
        turn_id: None,
        token_event_ordinal: 1,
        input_tokens: 100,
        cached_input_tokens: 0,
        output_tokens: 50,
        reasoning_tokens: 0,
        total_tokens: 150,
        delta_input_tokens: None,
        delta_cached_input_tokens: None,
        delta_output_tokens: None,
        delta_reasoning_tokens: None,
        delta_total_tokens: None,
        model: None,
        model_provider: None,
        cost_usd: None,
        raw_usage_json: "{}".to_string(),
        timestamp_ms: 500,
    };
    let row = CodexTokenSnapshotRow::from_domain(&snapshot, 1, None);
    assert_eq!(row.turn_id, None);
    assert_eq!(row.emitted_event_id, None);
    assert!(row.model.is_none());
    assert!(row.id.contains("none"));
}

// =============================================================================
// write_queries.rs — upsert_log_file_checkpoint, current_active_generation_id,
//   upsert_log_source, record_diagnostic_event
// =============================================================================

#[test]
fn upsert_log_file_checkpoint_inserts_and_updates() {
    let db = Database::open_in_memory().unwrap();
    // Insert
    write_queries::upsert_log_file_checkpoint(
        db.conn(),
        "lf-1",
        "src-1",
        "claude_code",
        "/tmp/f.jsonl",
        Some("inode-1"),
        100,
        1000,
        Some(123),
        "active",
        None,
    )
    .unwrap();

    let row: (String, i64, i64) = db
        .conn()
        .query_row(
            "SELECT state, offset_bytes, size_bytes FROM log_files WHERE id = 'lf-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row.0, "active");
    assert_eq!(row.1, 100);
    assert_eq!(row.2, 1000);

    // Update — offset and state change, first_seen_at_ms preserved.
    write_queries::upsert_log_file_checkpoint(
        db.conn(),
        "lf-1",
        "src-1",
        "claude_code",
        "/tmp/f.jsonl",
        Some("inode-1"),
        500,
        2000,
        Some(456),
        "scanning",
        Some("partial error"),
    )
    .unwrap();
    let row2: (String, i64, i64, Option<String>) = db
        .conn()
        .query_row(
            "SELECT state, offset_bytes, size_bytes, last_error FROM log_files WHERE id = 'lf-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(row2.0, "scanning");
    assert_eq!(row2.1, 500);
    assert_eq!(row2.2, 2000);
    assert_eq!(row2.3.as_deref(), Some("partial error"));
}

#[test]
fn current_active_generation_id_returns_from_service_state() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO service_state (id, active_generation_id, updated_at_ms) VALUES (1, 'gen-from-state', 0)",
            [],
        )
        .unwrap();
    let gen = write_queries::current_active_generation_id(db.conn()).unwrap();
    assert_eq!(gen.as_deref(), Some("gen-from-state"));
}

#[test]
fn current_active_generation_id_falls_back_to_audit_generations() {
    let db = Database::open_in_memory().unwrap();
    // No service_state row → falls back to audit_generations.
    db.conn()
        .execute(
            "INSERT INTO service_state (id, active_generation_id, updated_at_ms) VALUES (1, '', 0)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO audit_generations (generation_id, state, started_at_ms, is_active, created_at_ms, updated_at_ms) \
             VALUES ('gen-audit', 'promoted', 100, 1, 100, 100)",
            [],
        )
        .unwrap();
    let gen = write_queries::current_active_generation_id(db.conn()).unwrap();
    assert_eq!(gen.as_deref(), Some("gen-audit"));
}

#[test]
fn current_active_generation_id_returns_none_when_no_active() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO service_state (id, active_generation_id, updated_at_ms) VALUES (1, '', 0)",
            [],
        )
        .unwrap();
    let gen = write_queries::current_active_generation_id(db.conn()).unwrap();
    assert!(gen.is_none());
}

#[test]
fn upsert_log_source_inserts_and_updates() {
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();
    let source = LogSourceRow {
        id: "src-1".to_string(),
        agent: "claude_code".to_string(),
        source_type: "file".to_string(),
        root_path: "/tmp/logs".to_string(),
        configured_by_user: 1,
        default_discovery_enabled: 1,
        status: "active".to_string(),
        last_scan_started_at_ms: Some(now),
        last_scan_completed_at_ms: Some(now),
        last_error: None,
        first_seen_at_ms: now,
        last_seen_at_ms: now,
        created_at_ms: now,
        updated_at_ms: now,
    };
    write_queries::upsert_log_source(db.conn(), &source).unwrap();
    let agent: String = db
        .conn()
        .query_row(
            "SELECT agent FROM log_sources WHERE id = 'src-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(agent, "claude_code");

    // Update — change status.
    let mut updated = source.clone();
    updated.status = "scanning".to_string();
    write_queries::upsert_log_source(db.conn(), &updated).unwrap();
    let status: String = db
        .conn()
        .query_row(
            "SELECT status FROM log_sources WHERE id = 'src-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "scanning");
}

#[test]
fn record_diagnostic_event_inserts_row() {
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();
    let event = OperationalDiagnosticEvent {
        id: "diag-1".to_string(),
        agent: Some(AgentKind::ClaudeCode),
        source_id: Some("src-1".to_string()),
        source_file_id: Some("file-1".to_string()),
        source_path: Some("/tmp/f.jsonl".to_string()),
        source_line: Some(42),
        category: "test_category".to_string(),
        severity: "warning".to_string(),
        message: "test message".to_string(),
        detail_json: Some(r#"{"key":"val"}"#.to_string()),
        happened_at_ms: now,
        created_at_ms: now,
    };
    write_queries::record_diagnostic_event(db.conn(), &event).unwrap();
    let (code, msg): (String, String) = db
        .conn()
        .query_row(
            "SELECT code, message FROM diagnostic_events WHERE id = 'diag-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(code, "test_category");
    assert_eq!(msg, "test message");

    // Upsert (replace) with same id.
    let mut event2 = event.clone();
    event2.message = "updated message".to_string();
    write_queries::record_diagnostic_event(db.conn(), &event2).unwrap();
    let msg2: String = db
        .conn()
        .query_row(
            "SELECT message FROM diagnostic_events WHERE id = 'diag-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(msg2, "updated message");
}

// =============================================================================
// subagent_queries.rs — additional function coverage
// =============================================================================

#[test]
fn find_by_name_in_repo_returns_matching_subagents() {
    let db = Database::open_in_memory().unwrap();
    seed_subagent_with_project(&db, "sa-1", "warm", "proj-1", "hash-abc");
    seed_subagent_with_project(&db, "sa-2", "warm", "proj-1", "hash-abc");
    seed_subagent_with_project(&db, "sa-3", "warm", "proj-2", "hash-xyz");

    let rows =
        subagent_queries::find_by_name_in_repo(db.conn(), "proj-1", "hash-abc", "sa-1").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "sa-1");
    assert_eq!(rows[0].project_id, "proj-1");
    assert_eq!(rows[0].repo_hash, "hash-abc");

    // No match.
    let empty =
        subagent_queries::find_by_name_in_repo(db.conn(), "proj-1", "hash-abc", "nonexistent")
            .unwrap();
    assert!(empty.is_empty());
}

#[test]
fn list_active_by_repo_returns_non_deleted_subagents() {
    let db = Database::open_in_memory().unwrap();
    seed_subagent_with_project(&db, "sa-1", "hot", "proj-1", "hash-abc");
    seed_subagent_with_project(&db, "sa-2", "warm", "proj-1", "hash-abc");
    seed_subagent_with_project(&db, "sa-3", "deleted", "proj-1", "hash-abc");

    let rows = subagent_queries::list_active_by_repo(db.conn(), "hash-abc").unwrap();
    assert_eq!(rows.len(), 2); // deleted is excluded
    assert!(rows.iter().all(|r| r.status != "deleted"));
}

#[test]
fn get_logical_subagent_returns_some_for_existing() {
    let db = Database::open_in_memory().unwrap();
    seed_subagent(&db, "sa-1", "warm");
    let row = subagent_queries::get_logical_subagent(db.conn(), "sa-1").unwrap();
    assert!(row.is_some());
    assert_eq!(row.unwrap().id, "sa-1");

    let none = subagent_queries::get_logical_subagent(db.conn(), "nonexistent").unwrap();
    assert!(none.is_none());
}

#[test]
fn list_tasks_returns_tasks_for_subagent() {
    let db = Database::open_in_memory().unwrap();
    seed_subagent(&db, "sa-1", "warm");
    seed_task_for_subagent(&db, "task-1", "sa-1", "completed", 100);
    seed_task_for_subagent(&db, "task-2", "sa-1", "running", 200);
    seed_task_for_subagent(&db, "task-3", "sa-1", "queued", 300);

    let tasks = subagent_queries::list_tasks(db.conn(), "sa-1", 10).unwrap();
    assert_eq!(tasks.len(), 3);
    // Ordered by created_at_ms DESC.
    assert_eq!(tasks[0].id, "task-3");
    assert_eq!(tasks[1].id, "task-2");
    assert_eq!(tasks[2].id, "task-1");
}

#[test]
fn set_task_status_updates_status_and_completed_at() {
    let db = Database::open_in_memory().unwrap();
    seed_subagent(&db, "sa-1", "warm");
    seed_task_for_subagent(&db, "task-1", "sa-1", "running", 100);

    subagent_queries::set_task_status(
        db.conn(),
        "task-1",
        "completed",
        Some("done".to_string()),
        None,
    )
    .unwrap();

    let task = subagent_queries::get_task(db.conn(), "task-1")
        .unwrap()
        .unwrap();
    assert_eq!(task.status, "completed");
    assert_eq!(task.result_summary.as_deref(), Some("done"));
    assert!(task.completed_at_ms.is_some());
}

#[test]
fn set_task_error_kind_updates_error_kind() {
    let db = Database::open_in_memory().unwrap();
    seed_subagent(&db, "sa-1", "warm");
    seed_task_for_subagent(&db, "task-1", "sa-1", "failed", 100);

    subagent_queries::set_task_error_kind(db.conn(), "task-1", Some("timeout")).unwrap();
    let task = subagent_queries::get_task(db.conn(), "task-1")
        .unwrap()
        .unwrap();
    assert_eq!(task.error_kind.as_deref(), Some("timeout"));

    // Clear error_kind.
    subagent_queries::set_task_error_kind(db.conn(), "task-1", None).unwrap();
    let task = subagent_queries::get_task(db.conn(), "task-1")
        .unwrap()
        .unwrap();
    assert!(task.error_kind.is_none());
}

#[test]
fn count_tasks_since_counts_tasks_after_timestamp() {
    let db = Database::open_in_memory().unwrap();
    seed_subagent(&db, "sa-1", "warm");
    seed_task_for_subagent(&db, "task-1", "sa-1", "completed", 100);
    seed_task_for_subagent(&db, "task-2", "sa-1", "completed", 200);
    seed_task_for_subagent(&db, "task-3", "sa-1", "completed", 300);

    let count = subagent_queries::count_tasks_since(db.conn(), "sa-1", 150).unwrap();
    assert_eq!(count, 2);

    let count_all = subagent_queries::count_tasks_since(db.conn(), "sa-1", 0).unwrap();
    assert_eq!(count_all, 3);
}

#[test]
fn has_running_task_returns_correct_bool() {
    let db = Database::open_in_memory().unwrap();
    seed_subagent(&db, "sa-1", "warm");
    seed_task_for_subagent(&db, "task-1", "sa-1", "completed", 100);

    let has = subagent_queries::has_running_task(db.conn(), "sa-1").unwrap();
    assert!(!has);

    seed_task_for_subagent(&db, "task-2", "sa-1", "running", 200);
    let has = subagent_queries::has_running_task(db.conn(), "sa-1").unwrap();
    assert!(has);
}

#[test]
fn task_counts_by_status_returns_queued_and_running() {
    let db = Database::open_in_memory().unwrap();
    seed_subagent(&db, "sa-1", "warm");
    seed_task_for_subagent(&db, "task-1", "sa-1", "queued", 100);
    seed_task_for_subagent(&db, "task-2", "sa-1", "queued", 200);
    seed_task_for_subagent(&db, "task-3", "sa-1", "running", 300);
    seed_task_for_subagent(&db, "task-4", "sa-1", "completed", 400);

    let (queued, running) = subagent_queries::task_counts_by_status(db.conn()).unwrap();
    assert_eq!(queued, 2);
    assert_eq!(running, 1);
}

#[test]
fn pick_oldest_queued_task_picks_and_flips_to_running() {
    let db = Database::open_in_memory().unwrap();
    seed_subagent(&db, "sa-1", "warm");
    seed_task_for_subagent(&db, "task-1", "sa-1", "queued", 100);
    seed_task_for_subagent(&db, "task-2", "sa-1", "queued", 200);

    // First pick: gets task-1 (oldest), flips to running.
    let picked = subagent_queries::pick_oldest_queued_task(db.conn())
        .unwrap()
        .unwrap();
    assert_eq!(picked.id, "task-1");
    assert_eq!(picked.status, "running");
    assert!(picked.started_at_ms.is_some());

    // sa-1 now has a running task → next pick should NOT pick task-2 (per-subagent FIFO).
    let none = subagent_queries::pick_oldest_queued_task(db.conn()).unwrap();
    assert!(none.is_none());

    // Mark task-1 as completed, then task-2 should be picked.
    subagent_queries::set_task_status(db.conn(), "task-1", "completed", None, None).unwrap();
    let picked2 = subagent_queries::pick_oldest_queued_task(db.conn())
        .unwrap()
        .unwrap();
    assert_eq!(picked2.id, "task-2");
    assert_eq!(picked2.status, "running");
}

#[test]
fn pick_oldest_queued_task_returns_none_when_no_queued() {
    let db = Database::open_in_memory().unwrap();
    let none = subagent_queries::pick_oldest_queued_task(db.conn()).unwrap();
    assert!(none.is_none());
}

#[test]
fn get_memory_returns_memory_for_subagent() {
    let db = Database::open_in_memory().unwrap();
    seed_subagent(&db, "sa-1", "warm");
    seed_memory(&db, "sa-1", Some("hot summary text"));

    let mem = subagent_queries::get_memory(db.conn(), "sa-1").unwrap();
    assert!(mem.is_some());
    assert_eq!(
        mem.unwrap().hot_summary.as_deref(),
        Some("hot summary text")
    );

    let none = subagent_queries::get_memory(db.conn(), "nonexistent").unwrap();
    assert!(none.is_none());
}

#[test]
fn reconcile_sidecar_crash_reconciles_state() {
    let db = Database::open_in_memory().unwrap();
    seed_subagent(&db, "sa-1", "warm");
    seed_subagent(&db, "sa-2", "warm");
    // Seed memory with hot_summary so the status rolls back to "warm" (not "cold").
    seed_memory(&db, "sa-1", Some("hot summary text"));
    seed_memory(&db, "sa-2", Some("hot summary text"));
    seed_hot_binding(&db, "bind-1", "sa-1", "pi", Some(100));
    seed_hot_binding(&db, "bind-2", "sa-2", "pi", Some(200));
    seed_task_for_subagent(&db, "task-1", "sa-1", "running", 100);
    seed_task_for_subagent(&db, "task-2", "sa-2", "running", 200);

    let counts = subagent_queries::reconcile_sidecar_crash(db.conn(), "pi").unwrap();
    assert_eq!(counts.tasks_failed, 2);
    assert_eq!(counts.bindings_released, 2);
    assert_eq!(counts.status_rolled_back, 2);

    // Tasks should be failed.
    let task1 = subagent_queries::get_task(db.conn(), "task-1")
        .unwrap()
        .unwrap();
    assert_eq!(task1.status, "failed");
    assert_eq!(task1.error.as_deref(), Some("SIDECAR_CRASHED"));

    // Bindings should be released (is_hot=0, status='crashed').
    let binding_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM subagent_harness_bindings WHERE harness = 'pi' AND is_hot = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(binding_count, 0);

    // Subagents should roll back to warm (memory exists with hot_summary).
    let sa1 = subagent_queries::get_logical_subagent(db.conn(), "sa-1")
        .unwrap()
        .unwrap();
    assert_eq!(sa1.status, "warm");
}

#[test]
fn reconcile_sidecar_crash_returns_default_when_no_bindings() {
    let db = Database::open_in_memory().unwrap();
    let counts = subagent_queries::reconcile_sidecar_crash(db.conn(), "pi").unwrap();
    assert_eq!(counts.tasks_failed, 0);
    assert_eq!(counts.bindings_released, 0);
    assert_eq!(counts.status_rolled_back, 0);
}

#[test]
fn reconcile_sidecar_crash_rolls_back_to_cold_when_no_memory() {
    let db = Database::open_in_memory().unwrap();
    seed_subagent(&db, "sa-1", "warm");
    seed_hot_binding(&db, "bind-1", "sa-1", "pi", Some(100));
    // No memory seeded → should roll back to 'cold'.

    let counts = subagent_queries::reconcile_sidecar_crash(db.conn(), "pi").unwrap();
    assert_eq!(counts.status_rolled_back, 1);

    let sa = subagent_queries::get_logical_subagent(db.conn(), "sa-1")
        .unwrap()
        .unwrap();
    assert_eq!(sa.status, "cold");
}

// =============================================================================
// read_queries.rs — additional function coverage
// =============================================================================

#[test]
fn read_breakdown_activity_list_returns_filtered_events() {
    let db = Database::open_in_memory().unwrap();
    // Seed usage_events with a specific generation_id and project_hash.
    db.conn()
        .execute(
            "INSERT INTO usage_events (id, agent, source_file_id, source_path, source_line, \
             source_offset_start, source_offset_end, session_id, timestamp_ms, project_hash, \
             model, total_tokens, input_tokens, output_tokens, raw_event_hash, created_at_ms, \
             updated_at_ms, generation_id) \
             VALUES ('evt-1','claude_code','file-1','',0,0,0,'sess-1',1000,'hash-1','claude-sonnet',\
             100,50,50,'hash',1000,1000,'gen-1')",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_events (id, agent, source_file_id, source_path, source_line, \
             source_offset_start, source_offset_end, session_id, timestamp_ms, project_hash, \
             model, total_tokens, input_tokens, output_tokens, raw_event_hash, created_at_ms, \
             updated_at_ms, generation_id) \
             VALUES ('evt-2','claude_code','file-2','',0,0,0,'sess-2',2000,'hash-2','claude-haiku',\
             200,100,100,'hash',2000,2000,'gen-1')",
            [],
        )
        .unwrap();

    let rows = read_queries::read_breakdown_activity_list(
        db.conn(),
        "gen-1",
        BreakdownFilterField::Project,
        "hash-1",
        0,
        3000,
        10,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "evt-1");
    assert_eq!(rows[0].project_hash.as_deref(), Some("hash-1"));
    assert_eq!(rows[0].total_tokens, 100);

    // Filter by model.
    let rows_model = read_queries::read_breakdown_activity_list(
        db.conn(),
        "gen-1",
        BreakdownFilterField::Model,
        "claude-haiku",
        0,
        3000,
        10,
    )
    .unwrap();
    assert_eq!(rows_model.len(), 1);
    assert_eq!(rows_model[0].id, "evt-2");

    // Filter by session.
    let rows_session = read_queries::read_breakdown_activity_list(
        db.conn(),
        "gen-1",
        BreakdownFilterField::Session,
        "sess-1",
        0,
        3000,
        10,
    )
    .unwrap();
    assert_eq!(rows_session.len(), 1);
    assert_eq!(rows_session[0].id, "evt-1");
}

#[test]
fn read_breakdown_activity_list_returns_empty_when_no_match() {
    let db = Database::open_in_memory().unwrap();
    let rows = read_queries::read_breakdown_activity_list(
        db.conn(),
        "gen-1",
        BreakdownFilterField::Project,
        "nonexistent",
        0,
        3000,
        10,
    )
    .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn read_client_rollups_returns_grouped_clients() {
    let db = Database::open_in_memory().unwrap();
    seed_source_health_summary(
        &db,
        "gen-1",
        "src-1",
        "claude_code",
        "active",
        Some(1_000),
        5,
    );
    seed_source_health_summary(
        &db,
        "gen-1",
        "src-2",
        "claude_code",
        "idle",
        Some(2_000),
        10,
    );
    seed_source_health_summary(&db, "gen-1", "src-3", "codex", "active", Some(3_000), 15);

    let rollups = read_queries::read_client_rollups(db.conn(), "gen-1").unwrap();
    assert_eq!(rollups.len(), 2); // claude_code and codex
                                  // claude_code has 1 active source, 15 events total.
    let cc = rollups
        .iter()
        .find(|r| r.client_kind == "claude_code")
        .unwrap();
    assert_eq!(cc.active_source_count, 1);
    assert_eq!(cc.event_count, 15);

    let cd = rollups.iter().find(|r| r.client_kind == "codex").unwrap();
    assert_eq!(cd.active_source_count, 1);
    assert_eq!(cd.event_count, 15);
}

#[test]
fn read_client_rollups_returns_empty_for_unknown_generation() {
    let db = Database::open_in_memory().unwrap();
    let rollups = read_queries::read_client_rollups(db.conn(), "gen-empty").unwrap();
    assert!(rollups.is_empty());
}

#[test]
fn read_activity_source_info_returns_source_for_file() {
    let db = Database::open_in_memory().unwrap();
    // Seed log_sources and log_files.
    let now = busytok_domain::now_ms();
    write_queries::upsert_log_source(
        db.conn(),
        &LogSourceRow {
            id: "src-1".to_string(),
            agent: "claude_code".to_string(),
            source_type: "file".to_string(),
            root_path: "/tmp/logs".to_string(),
            configured_by_user: 1,
            default_discovery_enabled: 1,
            status: "active".to_string(),
            last_scan_started_at_ms: None,
            last_scan_completed_at_ms: None,
            last_error: None,
            first_seen_at_ms: now,
            last_seen_at_ms: now,
            created_at_ms: now,
            updated_at_ms: now,
        },
    )
    .unwrap();
    write_queries::upsert_log_file_checkpoint(
        db.conn(),
        "lf-1",
        "src-1",
        "claude_code",
        "/tmp/logs/f.jsonl",
        None,
        0,
        100,
        None,
        "active",
        None,
    )
    .unwrap();

    let info = read_queries::read_activity_source_info(db.conn(), "lf-1").unwrap();
    assert!(info.is_some());
    let info = info.unwrap();
    assert_eq!(info.source_id, "src-1");
    assert_eq!(info.agent, "claude_code");
    assert_eq!(info.root_path, "/tmp/logs");
}

#[test]
fn read_activity_source_info_returns_none_for_unknown_file() {
    let db = Database::open_in_memory().unwrap();
    let info = read_queries::read_activity_source_info(db.conn(), "nonexistent").unwrap();
    assert!(info.is_none());
}

#[test]
fn read_client_source_detail_returns_detail_for_source() {
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();
    write_queries::upsert_log_source(
        db.conn(),
        &LogSourceRow {
            id: "src-1".to_string(),
            agent: "claude_code".to_string(),
            source_type: "file".to_string(),
            root_path: "/tmp/logs".to_string(),
            configured_by_user: 1,
            default_discovery_enabled: 1,
            status: "active".to_string(),
            last_scan_started_at_ms: None,
            last_scan_completed_at_ms: Some(now),
            last_error: None,
            first_seen_at_ms: now,
            last_seen_at_ms: now,
            created_at_ms: now,
            updated_at_ms: now,
        },
    )
    .unwrap();
    write_queries::upsert_log_file_checkpoint(
        db.conn(),
        "lf-1",
        "src-1",
        "claude_code",
        "/tmp/logs/f.jsonl",
        None,
        100,
        1000,
        None,
        "active",
        None,
    )
    .unwrap();

    let detail = read_queries::read_client_source_detail(db.conn(), "src-1").unwrap();
    assert!(detail.is_some());
    let detail = detail.unwrap();
    assert_eq!(detail.source_id, "src-1");
    assert_eq!(detail.agent, "claude_code");
    assert_eq!(detail.status, "active");
    assert_eq!(detail.file_count, 1);
}

#[test]
fn read_client_source_detail_returns_none_for_unknown_source() {
    let db = Database::open_in_memory().unwrap();
    let detail = read_queries::read_client_source_detail(db.conn(), "nonexistent").unwrap();
    assert!(detail.is_none());
}

#[test]
fn read_overview_summary_exact_returns_totals() {
    let db = Database::open_in_memory().unwrap();
    // Seed usage_events with generation_id.
    db.conn()
        .execute(
            "INSERT INTO usage_events (id, agent, source_file_id, source_path, source_line, \
             source_offset_start, source_offset_end, session_id, timestamp_ms, \
             total_tokens, input_tokens, output_tokens, cost_usd, raw_event_hash, \
             created_at_ms, updated_at_ms, generation_id) \
             VALUES ('evt-1','claude_code','file-1','',0,0,0,'sess-1',1000,100,50,50,0.5,'hash',1000,1000,'gen-1')",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_events (id, agent, source_file_id, source_path, source_line, \
             source_offset_start, source_offset_end, session_id, timestamp_ms, \
             total_tokens, input_tokens, output_tokens, cost_usd, raw_event_hash, \
             created_at_ms, updated_at_ms, generation_id) \
             VALUES ('evt-2','claude_code','file-2','',0,0,0,'sess-2',2000,200,100,100,1.5,'hash',2000,2000,'gen-1')",
            [],
        )
        .unwrap();

    use busytok_store::read_models::RangeWindow;
    let range = RangeWindow {
        start_ms: 0,
        end_ms: 3000,
    };
    let summary = read_queries::read_overview_summary_exact(db.conn(), "gen-1", &range).unwrap();
    assert_eq!(summary.total_tokens, 300);
    assert!((summary.total_cost_usd.unwrap() - 2.0).abs() < 0.01);
    assert_eq!(summary.event_count, 2);
}
