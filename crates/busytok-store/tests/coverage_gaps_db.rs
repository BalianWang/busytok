#![allow(
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    dead_code,
    unused_imports,
    unused_variables
)]

//! Coverage gap tests for `crates/busytok-store/src/db.rs`.
//!
//! Each test exercises a code path that previously had zero hits in the
//! `cargo test -p busytok-store` lcov report. Tests are self-contained and
//! independent — they build their own in-memory (or temp-file) database.

use std::collections::HashMap;

use busytok_domain::{
    AgentKind, NormalizedUsageEvent, OperationalDiagnosticEvent, ReportingTimezone,
    UsageWritePolicy,
};
use busytok_store::{
    CodexTokenSnapshotRow, DailyUsageRow, Database, LogSourceRow, ModelSummaryRow, ModelUsageRow,
    OldEventTokens, ProjectRow, RollupRows, SessionRow, StoreWriteBatch, SubagentHarnessBindingRow,
    SubagentLogicalSubagentRow,
};

// ── Helpers ─────────────────────────────────────────────────────────────────

fn make_test_event(id: &str, agent: AgentKind, total_tokens: i64) -> NormalizedUsageEvent {
    let mut event = NormalizedUsageEvent::minimal_for_test(id, agent);
    event.total_tokens = total_tokens;
    event.input_tokens = total_tokens / 2;
    event.output_tokens = total_tokens - (total_tokens / 2);
    event.timestamp_ms = busytok_domain::now_ms();
    event
}

fn make_codex_snapshot(
    id: &str,
    source_file_id: &str,
    session_id: &str,
    turn_id: Option<&str>,
    ordinal: i64,
    model: Option<&str>,
    total_tokens: i64,
) -> CodexTokenSnapshotRow {
    let now = busytok_domain::now_ms();
    CodexTokenSnapshotRow {
        id: id.to_string(),
        source_file_id: source_file_id.to_string(),
        source_line: 1,
        source_offset_start: 0,
        source_offset_end: 100,
        session_id: session_id.to_string(),
        turn_id: turn_id.map(|s| s.to_string()),
        token_event_ordinal: ordinal,
        input_tokens: total_tokens / 2,
        cached_input_tokens: 0,
        output_tokens: total_tokens - (total_tokens / 2),
        reasoning_tokens: 0,
        total_tokens,
        model: model.map(|s| s.to_string()),
        raw_usage_json: "{}".to_string(),
        emitted_event_id: Some(format!("evt-{id}")),
        created_at_ms: now,
        updated_at_ms: now,
    }
}

fn make_log_source(id: &str) -> LogSourceRow {
    let now = 1_000;
    LogSourceRow {
        id: id.to_string(),
        agent: "claude_code".to_string(),
        source_type: "jsonl".to_string(),
        root_path: "/tmp/test".to_string(),
        configured_by_user: 1,
        default_discovery_enabled: 1,
        status: "active".to_string(),
        last_scan_started_at_ms: Some(now - 100),
        last_scan_completed_at_ms: Some(now),
        last_error: None,
        first_seen_at_ms: now,
        last_seen_at_ms: now,
        created_at_ms: now,
        updated_at_ms: now,
    }
}

fn make_session_row(id: &str, total_tokens: i64) -> SessionRow {
    SessionRow {
        id: id.to_string(),
        agent: "claude_code".to_string(),
        project_hash: Some("proj-hash".to_string()),
        started_at_ms: 1_000,
        last_seen_at_ms: 2_000,
        model_list_json: r#"["claude-sonnet-4"]"#.to_string(),
        total_tokens,
        total_cost_usd: Some(0.5),
        event_count: 1,
        is_active: 1,
        created_at_ms: 1_000,
        updated_at_ms: 2_000,
    }
}

fn make_project_row(hash: &str, total_tokens: i64) -> ProjectRow {
    ProjectRow {
        id: hash.to_string(),
        project_hash: hash.to_string(),
        project_path: Some("/tmp/proj".to_string()),
        agent: Some("claude_code".to_string()),
        display_name: Some("proj".to_string()),
        first_seen_at_ms: 1_000,
        last_seen_at_ms: 2_000,
        total_tokens,
        total_cost_usd: Some(0.5),
        session_count: 1,
        created_at_ms: 1_000,
        updated_at_ms: 2_000,
    }
}

fn make_model_summary_row(model: &str, total_tokens: i64) -> ModelSummaryRow {
    ModelSummaryRow {
        model: model.to_string(),
        total_tokens,
        total_cost_usd: Some(0.5),
        event_count: 1,
    }
}

fn make_model_usage_row(model: &str, total_tokens: i64) -> ModelUsageRow {
    ModelUsageRow {
        model: model.to_string(),
        agent: "claude_code".to_string(),
        timezone: "UTC".to_string(),
        date: "2026-07-02".to_string(),
        input_tokens: total_tokens / 2,
        output_tokens: total_tokens - (total_tokens / 2),
        total_tokens,
        cached_input_tokens: 0,
        reasoning_tokens: 0,
        cost_usd: Some(0.5),
        event_count: 1,
    }
}

// ── OldEventTokens::compute_delta (lines 48-73) ─────────────────────────────

#[test]
fn old_event_tokens_compute_delta_handles_none_cost_paths() {
    // Cover the (Some(n), None) and (None, _) arms of cost_usd and
    // estimated_cost_usd in `compute_delta` (lines 62-71).
    let old = OldEventTokens {
        event_id: "evt-1".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        total_tokens: 150,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        reasoning_tokens: 0,
        thoughts_tokens: 0,
        tool_tokens: 0,
        cost_usd: None,
        estimated_cost_usd: None,
    };

    let mut new = NormalizedUsageEvent::minimal_for_test("evt-1", AgentKind::ClaudeCode);
    new.input_tokens = 200;
    new.output_tokens = 80;
    new.total_tokens = 280;
    new.cost_usd = Some(1.5);
    new.estimated_cost_usd = Some(0.7);

    let delta = old.compute_delta(&new);
    // (Some(n), None) arm: cost_usd should be n (not n - 0).
    assert_eq!(delta.cost_usd, Some(1.5));
    assert_eq!(delta.estimated_cost_usd, Some(0.7));
    assert_eq!(delta.input_tokens, 100);
    assert_eq!(delta.output_tokens, 30);
    assert_eq!(delta.total_tokens, 130);
}

#[test]
fn old_event_tokens_compute_delta_both_some_subtracts() {
    // Cover the (Some(n), Some(o)) arm.
    let old = OldEventTokens {
        event_id: "evt-2".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        total_tokens: 150,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        reasoning_tokens: 0,
        thoughts_tokens: 0,
        tool_tokens: 0,
        cost_usd: Some(0.5),
        estimated_cost_usd: Some(0.2),
    };
    let mut new = NormalizedUsageEvent::minimal_for_test("evt-2", AgentKind::ClaudeCode);
    new.cost_usd = Some(1.5);
    new.estimated_cost_usd = Some(0.7);
    let delta = old.compute_delta(&new);
    // 1.5 - 0.5 is not exactly 1.0 in f64 — use approximate comparison.
    let cost = delta.cost_usd.expect("cost_usd should be Some");
    assert!((cost - 1.0).abs() < 1e-9, "cost_usd delta: {cost}");
    let est = delta
        .estimated_cost_usd
        .expect("estimated_cost_usd should be Some");
    assert!((est - 0.5).abs() < 1e-9, "estimated_cost_usd delta: {est}");
}

#[test]
fn old_event_tokens_compute_delta_new_none_returns_none() {
    // Cover the (None, _) arm: when new.cost_usd is None, the result is None
    // regardless of old.
    let old = OldEventTokens {
        event_id: "evt-3".to_string(),
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        reasoning_tokens: 0,
        thoughts_tokens: 0,
        tool_tokens: 0,
        cost_usd: Some(1.0),
        estimated_cost_usd: Some(2.0),
    };
    let new = NormalizedUsageEvent::minimal_for_test("evt-3", AgentKind::Codex);
    let delta = old.compute_delta(&new);
    assert!(delta.cost_usd.is_none());
    assert!(delta.estimated_cost_usd.is_none());
}

// ── Database::open / open_readonly / reopen / path_buf (lines 155-284) ──────

#[test]
fn in_memory_reopen_and_reopen_readonly_return_none() {
    // In-memory databases have no file path, so reopen() and
    // reopen_readonly() must return Ok(None) (lines 254, 268).
    let db = Database::open_in_memory().unwrap();
    assert!(
        db.reopen().unwrap().is_none(),
        "in-memory reopen returns None"
    );
    assert!(
        db.reopen_readonly().unwrap().is_none(),
        "in-memory reopen_readonly returns None"
    );
    assert!(db.path_buf().is_none(), "in-memory path_buf returns None");
}

#[test]
fn file_backed_open_round_trip_and_path_buf_returns_path() {
    // Cover Database::open with a real file path (lines 155-163), open_readonly
    // (lines 169-180), and path_buf / reopen returning Some for file-backed DBs.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    // Drop the temp handle — we want to create the DB file fresh.
    drop(tmp);

    {
        let db = Database::open(&path).unwrap();
        // macOS symlinks /var -> /private/var, so compare canonical paths.
        let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        assert_eq!(
            db.path_buf()
                .map(|p| { std::fs::canonicalize(&p).unwrap_or(p) }),
            Some(canonical.clone())
        );
        // reopen should succeed and return a fresh Database to the same path.
        let reopened = db
            .reopen()
            .unwrap()
            .expect("file-backed reopen returns Some");
        assert_eq!(
            reopened
                .path_buf()
                .map(|p| std::fs::canonicalize(&p).unwrap_or(p)),
            Some(canonical.clone())
        );
        // reopen_readonly should succeed for a file-backed DB.
        let ro = db
            .reopen_readonly()
            .unwrap()
            .expect("file-backed reopen_readonly returns Some");
        // Read-only connection cannot write — verify by attempting a write that
        // should fail.
        let write_result = ro
            .conn()
            .execute("CREATE TABLE _should_fail (x INTEGER)", []);
        assert!(
            write_result.is_err(),
            "read-only connection must reject writes"
        );
    }
    // Clean up the DB file and sidecar files.
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

#[test]
fn open_readonly_directly_works_for_file_backed_db() {
    // Cover Database::open_readonly directly (lines 169-180).
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    drop(tmp);

    // First open with full read-write to create the schema + WAL files.
    {
        let db = Database::open(&path).unwrap();
        // Touch a write so we know the file is real.
        db.conn()
            .execute("CREATE TABLE IF NOT EXISTS _x (id INTEGER)", [])
            .unwrap();
    }
    // Now open read-only directly.
    let ro = Database::open_readonly(&path).unwrap();
    // Confirm we can run a SELECT (read works).
    let _: i64 = ro
        .conn()
        .query_row("SELECT 1", [], |row| row.get(0))
        .unwrap();
    // Confirm writes fail.
    let err = ro
        .conn()
        .execute("CREATE TABLE _should_fail (id INTEGER)", []);
    assert!(err.is_err());

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

#[test]
fn checkpoint_wal_runs_without_error() {
    // checkpoint_wal() previously had zero hits (lines 289-294).
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("cp.db");
    let db = Database::open(&path).unwrap();
    // Insert a row so the WAL has something to checkpoint.
    db.conn()
        .execute(
            "INSERT INTO log_sources (id, agent, source_type, root_path, configured_by_user, \
             default_discovery_enabled, status, last_scan_started_at_ms, last_scan_completed_at_ms, \
             last_error, first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms) \
             VALUES ('s1','claude_code','jsonl','/tmp',1,1,'active',NULL,NULL,NULL,1,1,1,1)",
            [],
        )
        .unwrap();
    db.checkpoint_wal().unwrap();
}

// ── get_usage_event / all_usage_events / usage_events_for_generation ────────

#[test]
fn get_usage_event_returns_none_for_unknown_id() {
    // Cover the `.optional()` None branch in get_usage_event (line 465).
    let db = Database::open_in_memory().unwrap();
    let got = db.get_usage_event("does-not-exist").unwrap();
    assert!(got.is_none(), "unknown id must return None");
}

#[test]
fn all_usage_events_empty_returns_empty_vec() {
    // Cover the empty result path in all_usage_events (line 501).
    let db = Database::open_in_memory().unwrap();
    let rows = db.all_usage_events().unwrap();
    assert!(rows.is_empty(), "fresh db has no usage events");
}

#[test]
fn usage_events_for_generation_empty_returns_empty_vec() {
    // Cover the empty result path in usage_events_for_generation (line 534).
    let db = Database::open_in_memory().unwrap();
    let rows = db.usage_events_for_generation("gen-unknown").unwrap();
    assert!(rows.is_empty(), "unknown generation has no events");
}

#[test]
fn usage_events_for_generation_returns_only_matching_events() {
    let db = Database::open_in_memory().unwrap();
    let event = make_test_event("evt-1", AgentKind::ClaudeCode, 100);
    let batch = StoreWriteBatch::for_test("src-1", "file-1")
        .usage_event(event, UsageWritePolicy::InsertOnce)
        .checkpoint_offset(50);
    db.ingest_store_batch(batch, "gen-A", |_e, _g| Ok(RollupRows::default()))
        .unwrap();

    let in_gen = db.usage_events_for_generation("gen-A").unwrap();
    assert_eq!(in_gen.len(), 1);
    let not_in_gen = db.usage_events_for_generation("gen-B").unwrap();
    assert!(not_in_gen.is_empty());
}

// ── Diagnostic events ────────────────────────────────────────────────────────

#[test]
fn list_diagnostic_events_returns_empty_when_no_match() {
    // Cover the empty path of list_diagnostic_events (line 607).
    let db = Database::open_in_memory().unwrap();
    let rows = db.list_diagnostic_events("missing-category", 100).unwrap();
    assert!(rows.is_empty());
}

#[test]
fn list_all_diagnostic_events_returns_empty_when_no_diags() {
    // Cover the empty path of list_all_diagnostic_events (line 632).
    let db = Database::open_in_memory().unwrap();
    let rows = db.list_all_diagnostic_events(100).unwrap();
    assert!(rows.is_empty());
}

#[test]
fn list_diagnostic_events_filters_by_category_and_severity() {
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();
    // Insert a warning diag for category "cache_metric" and an info diag for
    // the same category (info should be filtered out by severity IN (...)).
    let warning = OperationalDiagnosticEvent {
        id: "d-warn".to_string(),
        agent: None,
        source_id: None,
        source_file_id: None,
        source_path: None,
        source_line: None,
        category: "cache_metric".to_string(),
        severity: "warning".to_string(),
        message: "warn msg".to_string(),
        detail_json: None,
        happened_at_ms: now,
        created_at_ms: now,
    };
    let error = OperationalDiagnosticEvent {
        id: "d-err".to_string(),
        severity: "error".to_string(),
        category: "cache_metric".to_string(),
        message: "err msg".to_string(),
        ..warning.clone()
    };
    let info = OperationalDiagnosticEvent {
        id: "d-info".to_string(),
        severity: "info".to_string(),
        ..warning.clone()
    };
    db.record_diagnostic_event(&warning).unwrap();
    db.record_diagnostic_event(&error).unwrap();
    db.record_diagnostic_event(&info).unwrap();

    // Only warning + error match the severity filter, ordered by happened_at DESC.
    let rows = db.list_diagnostic_events("cache_metric", 100).unwrap();
    assert_eq!(rows.len(), 2, "info must be filtered out");
    let all = db.list_all_diagnostic_events(100).unwrap();
    assert_eq!(all.len(), 3, "list_all returns all severities");
}

// ── Codex snapshot helpers ───────────────────────────────────────────────────

#[test]
fn next_codex_ordinal_returns_one_when_no_rows_empty_turn() {
    // Cover the empty-turn_id branch of next_codex_ordinal (lines 720-727).
    let db = Database::open_in_memory().unwrap();
    let next = db.next_codex_ordinal("file-1", "sess-1", "").unwrap();
    assert_eq!(next, 1, "no rows -> ordinal 1");
}

#[test]
fn next_codex_ordinal_returns_one_when_no_rows_nonempty_turn() {
    let db = Database::open_in_memory().unwrap();
    let next = db.next_codex_ordinal("file-1", "sess-1", "turn-1").unwrap();
    assert_eq!(next, 1);
}

#[test]
fn next_codex_ordinal_increments_per_scope_empty_turn() {
    // Cover both insertion paths in next_codex_ordinal and IS NULL comparison.
    let db = Database::open_in_memory().unwrap();
    let snap1 = make_codex_snapshot("snap-1", "file-1", "sess-1", None, 1, None, 100);
    db.upsert_codex_snapshot(&snap1).unwrap();
    let next = db.next_codex_ordinal("file-1", "sess-1", "").unwrap();
    assert_eq!(next, 2, "after one row -> ordinal 2");

    // Different scope (non-empty turn_id) is independent.
    let next_other = db.next_codex_ordinal("file-1", "sess-1", "turn-A").unwrap();
    assert_eq!(next_other, 1);
}

#[test]
fn get_latest_codex_snapshot_returns_none_when_empty_empty_turn() {
    // Cover the empty-turn_id branch of get_latest_codex_snapshot
    // (lines 752-762, 768).
    let db = Database::open_in_memory().unwrap();
    let got = db
        .get_latest_codex_snapshot("file-1", "sess-1", "")
        .unwrap();
    assert!(got.is_none());
}

#[test]
fn get_latest_codex_snapshot_returns_none_when_empty_nonempty_turn() {
    let db = Database::open_in_memory().unwrap();
    let got = db
        .get_latest_codex_snapshot("file-1", "sess-1", "turn-1")
        .unwrap();
    assert!(got.is_none());
}

#[test]
fn get_latest_codex_snapshot_returns_highest_ordinal_empty_turn() {
    let db = Database::open_in_memory().unwrap();
    let snap_a = make_codex_snapshot("snap-a", "file-1", "sess-1", None, 1, None, 100);
    let snap_b = make_codex_snapshot("snap-b", "file-1", "sess-1", None, 5, None, 500);
    db.upsert_codex_snapshot(&snap_a).unwrap();
    db.upsert_codex_snapshot(&snap_b).unwrap();
    let latest = db
        .get_latest_codex_snapshot("file-1", "sess-1", "")
        .unwrap()
        .expect("should return the latest snapshot");
    assert_eq!(latest.token_event_ordinal, 5);
    assert_eq!(latest.total_tokens, 500);
}

#[test]
fn get_latest_codex_snapshot_returns_highest_ordinal_nonempty_turn() {
    let db = Database::open_in_memory().unwrap();
    let snap_a = make_codex_snapshot("snap-a", "file-1", "sess-1", Some("turn-1"), 1, None, 100);
    let snap_b = make_codex_snapshot("snap-b", "file-1", "sess-1", Some("turn-1"), 3, None, 300);
    db.upsert_codex_snapshot(&snap_a).unwrap();
    db.upsert_codex_snapshot(&snap_b).unwrap();
    let latest = db
        .get_latest_codex_snapshot("file-1", "sess-1", "turn-1")
        .unwrap()
        .expect("should return the latest snapshot");
    assert_eq!(latest.token_event_ordinal, 3);
}

#[test]
fn latest_codex_snapshot_model_for_source_file_returns_none_when_empty() {
    // Cover latest_codex_snapshot_model_for_source_file None path (line 811).
    let db = Database::open_in_memory().unwrap();
    let got = db
        .latest_codex_snapshot_model_for_source_file("file-1")
        .unwrap();
    assert!(got.is_none());
}

#[test]
fn latest_codex_snapshot_model_for_source_file_returns_latest_non_empty() {
    let db = Database::open_in_memory().unwrap();
    let snap = make_codex_snapshot("snap-1", "file-1", "sess-1", None, 1, Some("codex-1"), 100);
    db.upsert_codex_snapshot(&snap).unwrap();
    let got = db
        .latest_codex_snapshot_model_for_source_file("file-1")
        .unwrap();
    assert_eq!(got.as_deref(), Some("codex-1"));
}

#[test]
fn latest_usage_event_model_for_source_file_returns_none_when_empty() {
    // Cover latest_usage_event_model_for_source_file None path (line 829).
    let db = Database::open_in_memory().unwrap();
    let got = db
        .latest_usage_event_model_for_source_file("file-1", "claude_code")
        .unwrap();
    assert!(got.is_none());
}

#[test]
fn latest_usage_event_model_for_source_file_returns_latest_non_empty() {
    let db = Database::open_in_memory().unwrap();
    let mut event = make_test_event("evt-1", AgentKind::ClaudeCode, 100);
    event.source_file_id = "file-1".to_string();
    event.model = Some("claude-sonnet-4".to_string());
    db.write_usage_event(&event, UsageWritePolicy::InsertOnce)
        .unwrap();
    let got = db
        .latest_usage_event_model_for_source_file("file-1", "claude_code")
        .unwrap();
    assert_eq!(got.as_deref(), Some("claude-sonnet-4"));
}

#[test]
fn distinct_codex_models_for_session_returns_empty_when_no_data() {
    // Cover distinct_codex_models_for_session empty path (line 851).
    let db = Database::open_in_memory().unwrap();
    let got = db
        .distinct_codex_models_for_session("file-1", "sess-1")
        .unwrap();
    assert!(got.is_empty());
}

#[test]
fn distinct_codex_models_for_session_unions_events_and_snapshots() {
    let db = Database::open_in_memory().unwrap();
    let snap = make_codex_snapshot(
        "snap-1",
        "file-1",
        "sess-1",
        None,
        1,
        Some("codex-snap-model"),
        100,
    );
    db.upsert_codex_snapshot(&snap).unwrap();
    let mut event = make_test_event("evt-1", AgentKind::Codex, 100);
    event.source_file_id = "file-1".to_string();
    event.session_id = "sess-1".to_string();
    event.model = Some("codex-event-model".to_string());
    db.write_usage_event(&event, UsageWritePolicy::InsertOnce)
        .unwrap();
    let models = db
        .distinct_codex_models_for_session("file-1", "sess-1")
        .unwrap();
    assert!(models.contains(&"codex-snap-model".to_string()));
    assert!(models.contains(&"codex-event-model".to_string()));
}

#[test]
fn codex_sessions_with_null_model_returns_empty_when_no_data() {
    // Cover codex_sessions_with_null_model (lines 903-918).
    let db = Database::open_in_memory().unwrap();
    let got = db.codex_sessions_with_null_model("file-1").unwrap();
    assert!(got.is_empty());
}

#[test]
fn codex_sessions_with_null_model_returns_sessions_with_null_model() {
    let db = Database::open_in_memory().unwrap();
    // Snapshot with NULL model and session "sess-A".
    let snap = make_codex_snapshot("snap-1", "file-1", "sess-A", None, 1, None, 100);
    db.upsert_codex_snapshot(&snap).unwrap();
    // Codex event with NULL model and session "sess-B".
    let mut event = make_test_event("evt-1", AgentKind::Codex, 100);
    event.source_file_id = "file-1".to_string();
    event.session_id = "sess-B".to_string();
    event.model = None;
    db.write_usage_event(&event, UsageWritePolicy::InsertOnce)
        .unwrap();
    let sessions = db.codex_sessions_with_null_model("file-1").unwrap();
    assert!(sessions.contains(&"sess-A".to_string()));
    assert!(sessions.contains(&"sess-B".to_string()));
}

#[test]
fn backfill_codex_model_for_session_updates_null_models() {
    // Cover backfill_codex_model_for_session (lines 866-896).
    let db = Database::open_in_memory().unwrap();
    let snap = make_codex_snapshot("snap-1", "file-1", "sess-A", None, 1, None, 100);
    db.upsert_codex_snapshot(&snap).unwrap();
    let mut event = make_test_event("evt-1", AgentKind::Codex, 100);
    event.source_file_id = "file-1".to_string();
    event.session_id = "sess-A".to_string();
    event.model = None;
    db.write_usage_event(&event, UsageWritePolicy::InsertOnce)
        .unwrap();
    let updated = db
        .backfill_codex_model_for_session("file-1", "sess-A", "gpt-5")
        .unwrap();
    assert_eq!(updated, 2, "one event + one snapshot should be updated");
    let snap_after = db
        .get_latest_codex_snapshot("file-1", "sess-A", "")
        .unwrap()
        .unwrap();
    assert_eq!(snap_after.model.as_deref(), Some("gpt-5"));
    let event_after = db.get_usage_event("evt-1").unwrap().unwrap();
    assert_eq!(event_after.model.as_deref(), Some("gpt-5"));
}

#[test]
fn backfill_codex_model_for_session_returns_zero_when_already_set() {
    // Cover the case where no NULL/empty models exist — both UPDATEs match 0 rows.
    let db = Database::open_in_memory().unwrap();
    let snap = make_codex_snapshot(
        "snap-1",
        "file-1",
        "sess-A",
        None,
        1,
        Some("already-set"),
        100,
    );
    db.upsert_codex_snapshot(&snap).unwrap();
    let updated = db
        .backfill_codex_model_for_session("file-1", "sess-A", "gpt-5")
        .unwrap();
    assert_eq!(updated, 0);
}

// ── delete_daily_usage / replace_daily_usage ────────────────────────────────

#[test]
fn delete_daily_usage_clears_table() {
    // Cover delete_daily_usage (lines 923-926).
    let db = Database::open_in_memory().unwrap();
    let batch = StoreWriteBatch::for_test("src-1", "file-1")
        .usage_event(
            make_test_event("evt-1", AgentKind::ClaudeCode, 100),
            UsageWritePolicy::InsertOnce,
        )
        .checkpoint_offset(50);
    db.ingest_store_batch(batch, "gen-A", |events, gen| {
        Ok(RollupRows {
            daily_usage_rows: vec![DailyUsageRow::for_test(
                "2026-07-02",
                "UTC",
                gen,
                events.iter().map(|e| e.total_tokens).sum(),
            )],
            ..Default::default()
        })
    })
    .unwrap();
    assert_eq!(db.daily_usage_rows().unwrap().len(), 1);
    db.delete_daily_usage().unwrap();
    assert!(db.daily_usage_rows().unwrap().is_empty());
}

#[test]
fn replace_daily_usage_rebuilds_atomically() {
    // Cover replace_daily_usage (lines 932-951).
    let db = Database::open_in_memory().unwrap();
    let rtz = ReportingTimezone::utc();
    let event = make_test_event("evt-1", AgentKind::ClaudeCode, 100);
    db.replace_daily_usage(&[event], &rtz, "gen-A").unwrap();
    let rows = db.daily_usage_rows().unwrap();
    assert!(!rows.is_empty());
    let total: i64 = db
        .daily_usage_total_tokens(rows[0].date.as_str(), "gen-A")
        .unwrap();
    assert_eq!(total, 100);
}

// ── log files: checkpoint_log_file, get_log_file, count_log_files_by_source ─

#[test]
fn checkpoint_log_file_round_trips_via_get_log_file() {
    // Cover checkpoint_log_file (lines 956-973) and get_log_file (None + Some).
    let db = Database::open_in_memory().unwrap();
    // get_log_file on unknown id returns None (line 979, 983-985, 994).
    assert!(db.get_log_file("unknown").unwrap().is_none());

    db.checkpoint_log_file(
        "file-1",
        1024,
        "claude_code",
        "src-1",
        "/tmp/x.jsonl",
        Some("inode-1"),
    )
    .unwrap();
    let got = db
        .get_log_file("file-1")
        .unwrap()
        .expect("log file should exist");
    assert_eq!(got.source_id, "src-1");
    assert_eq!(got.agent, "claude_code");
    assert_eq!(got.path, "/tmp/x.jsonl");
    assert_eq!(got.offset_bytes, 1024);
    assert_eq!(got.inode.as_deref(), Some("inode-1"));
    assert_eq!(got.state, "active");

    // Upsert (second call) — should update offset_bytes only.
    db.checkpoint_log_file("file-1", 2048, "claude_code", "src-1", "/tmp/x.jsonl", None)
        .unwrap();
    let updated = db.get_log_file("file-1").unwrap().unwrap();
    assert_eq!(updated.offset_bytes, 2048);
    assert_eq!(updated.inode.as_deref(), None);
}

#[test]
fn count_log_files_by_source_returns_zero_for_unknown() {
    // Cover count_log_files_by_source (lines 1465-1483).
    let db = Database::open_in_memory().unwrap();
    let (total, parsed) = db.count_log_files_by_source("unknown-src").unwrap();
    assert_eq!(total, 0);
    assert_eq!(parsed, 0);
}

#[test]
fn count_log_files_by_source_distinguishes_parsed_and_unparsed() {
    let db = Database::open_in_memory().unwrap();
    // Two log files for the same source: one with offset > 0 (parsed), one with offset == 0.
    db.checkpoint_log_file("file-1", 0, "claude_code", "src-1", "/tmp/a.jsonl", None)
        .unwrap();
    db.checkpoint_log_file("file-2", 1000, "claude_code", "src-1", "/tmp/b.jsonl", None)
        .unwrap();
    let (total, parsed) = db.count_log_files_by_source("src-1").unwrap();
    assert_eq!(total, 2);
    assert_eq!(parsed, 1, "only file-2 has offset_bytes > 0");
}

// ── read_chip_hydration_counts (lines 1026-1036) ────────────────────────────

#[test]
fn read_chip_hydration_counts_returns_zero_on_fresh_db() {
    let db = Database::open_in_memory().unwrap();
    let (total, sources) = db.read_chip_hydration_counts().unwrap();
    assert_eq!(total, 0);
    assert_eq!(sources, 0);
}

#[test]
fn read_chip_hydration_counts_returns_counts_after_data() {
    let db = Database::open_in_memory().unwrap();
    // Seed two log sources — one 'active', one 'removed'.
    db.upsert_log_source(&make_log_source("src-active"))
        .unwrap();
    let mut removed = make_log_source("src-removed");
    removed.status = "removed".to_string();
    db.upsert_log_source(&removed).unwrap();
    // Seed a usage event.
    db.write_usage_event(
        &make_test_event("evt-1", AgentKind::ClaudeCode, 100),
        UsageWritePolicy::InsertOnce,
    )
    .unwrap();

    let (total, sources) = db.read_chip_hydration_counts().unwrap();
    assert_eq!(total, 1, "one usage event");
    assert_eq!(
        sources, 1,
        "only the active source counts (status != 'removed')"
    );
}

// ── list_log_sources / list_all_diagnostic_events etc ───────────────────────

#[test]
fn list_log_sources_returns_empty_initially() {
    // Cover the empty path of list_log_sources (line 1090).
    let db = Database::open_in_memory().unwrap();
    assert!(db.list_log_sources().unwrap().is_empty());
}

#[test]
fn list_log_sources_returns_inserted_sources_ordered_by_id() {
    let db = Database::open_in_memory().unwrap();
    db.upsert_log_source(&make_log_source("src-b")).unwrap();
    db.upsert_log_source(&make_log_source("src-a")).unwrap();
    let rows = db.list_log_sources().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "src-a", "ordered by id ASC");
    assert_eq!(rows[1].id, "src-b");
}

// ── daily_usage_rows / session_rows / project_rows / model_* ─────────────────

#[test]
fn daily_usage_rows_returns_empty_initially() {
    // Cover empty path of daily_usage_rows (line 1142).
    let db = Database::open_in_memory().unwrap();
    assert!(db.daily_usage_rows().unwrap().is_empty());
}

#[test]
fn session_rows_returns_empty_initially() {
    // Cover session_rows empty path (lines 1177-1209).
    let db = Database::open_in_memory().unwrap();
    assert!(db.session_rows().unwrap().is_empty());
}

#[test]
fn session_rows_returns_inserted_rows() {
    let db = Database::open_in_memory().unwrap();
    db.replace_sessions(&[
        make_session_row("sess-A", 100),
        make_session_row("sess-B", 200),
    ])
    .unwrap();
    let rows = db.session_rows().unwrap();
    assert_eq!(rows.len(), 2);
    // started_at_ms == 1_000 for both → ordering is implementation-defined.
    let total: i64 = rows.iter().map(|r| r.total_tokens).sum();
    assert_eq!(total, 300);
}

#[test]
fn replace_sessions_clears_and_inserts() {
    // Cover replace_sessions (lines 1212-1249).
    let db = Database::open_in_memory().unwrap();
    db.replace_sessions(&[make_session_row("sess-old", 100)])
        .unwrap();
    assert_eq!(db.session_rows().unwrap().len(), 1);
    // Replace with new set — old must be gone.
    db.replace_sessions(&[
        make_session_row("sess-new1", 50),
        make_session_row("sess-new2", 75),
    ])
    .unwrap();
    let rows = db.session_rows().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.id != "sess-old"));
}

#[test]
fn project_rows_returns_empty_initially() {
    // Cover project_rows empty path (lines 1254-1284).
    let db = Database::open_in_memory().unwrap();
    assert!(db.project_rows().unwrap().is_empty());
}

#[test]
fn project_rows_returns_inserted_rows() {
    let db = Database::open_in_memory().unwrap();
    db.replace_projects(&[make_project_row("hash-A", 100)])
        .unwrap();
    let rows = db.project_rows().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].project_hash, "hash-A");
}

#[test]
fn replace_projects_clears_and_inserts() {
    // Cover replace_projects (lines 1287-1324).
    let db = Database::open_in_memory().unwrap();
    db.replace_projects(&[make_project_row("hash-old", 100)])
        .unwrap();
    db.replace_projects(&[make_project_row("hash-new", 200)])
        .unwrap();
    let rows = db.project_rows().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].project_hash, "hash-new");
}

#[test]
fn model_summary_rows_returns_empty_initially() {
    let db = Database::open_in_memory().unwrap();
    assert!(db.model_summary_rows().unwrap().is_empty());
}

#[test]
fn replace_model_summaries_clears_and_inserts() {
    // Cover replace_model_summaries (lines 1352-1374).
    let db = Database::open_in_memory().unwrap();
    db.replace_model_summaries(&[make_model_summary_row("claude-3", 100)])
        .unwrap();
    db.replace_model_summaries(&[
        make_model_summary_row("claude-3", 50),
        make_model_summary_row("gpt-5", 75),
    ])
    .unwrap();
    let rows = db.model_summary_rows().unwrap();
    assert_eq!(rows.len(), 2);
    let total: i64 = rows.iter().map(|r| r.total_tokens).sum();
    assert_eq!(total, 125);
}

#[test]
fn model_usage_rows_returns_empty_initially() {
    // Cover empty path of model_usage_rows (line 1385).
    let db = Database::open_in_memory().unwrap();
    assert!(db.model_usage_rows().unwrap().is_empty());
}

#[test]
fn model_usage_rows_returns_inserted_rows() {
    let db = Database::open_in_memory().unwrap();
    // Insert via ingest_store_batch so model_usage rows land in the table.
    let event = make_test_event("evt-1", AgentKind::ClaudeCode, 100);
    let batch = StoreWriteBatch::for_test("src-1", "file-1")
        .usage_event(event, UsageWritePolicy::InsertOnce)
        .checkpoint_offset(50);
    db.ingest_store_batch(batch, "gen-A", |_events, _gen| {
        Ok(RollupRows {
            model_usage_rows: vec![make_model_usage_row("claude-sonnet-4", 100)],
            ..Default::default()
        })
    })
    .unwrap();
    let rows = db.model_usage_rows().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].model, "claude-sonnet-4");
    assert_eq!(rows[0].total_tokens, 100);
}

// ── realtime_summary ─────────────────────────────────────────────────────────

#[test]
fn realtime_summary_value_returns_none_for_unknown_key() {
    let db = Database::open_in_memory().unwrap();
    assert!(db.realtime_summary_value("missing").unwrap().is_none());
}

#[test]
fn read_realtime_summary_returns_empty_initially() {
    let db = Database::open_in_memory().unwrap();
    assert!(db.read_realtime_summary().unwrap().is_empty());
}

#[test]
fn replace_realtime_summary_clears_and_inserts() {
    let db = Database::open_in_memory().unwrap();
    db.replace_realtime_summary(&[("k1".to_string(), r#"{"v":1}"#.to_string())])
        .unwrap();
    db.replace_realtime_summary(&[
        ("k2".to_string(), r#"{"v":2}"#.to_string()),
        ("k3".to_string(), r#"{"v":3}"#.to_string()),
    ])
    .unwrap();
    let map = db.read_realtime_summary().unwrap();
    assert_eq!(map.len(), 2);
    assert!(map.contains_key("k2"));
    assert!(map.contains_key("k3"));
    assert!(!map.contains_key("k1"));
    // realtime_summary_value reads a single key.
    assert_eq!(
        db.realtime_summary_value("k2").unwrap().as_deref(),
        Some(r#"{"v":2}"#)
    );
}

// ── update_log_source_status (lines 1486-1495) ──────────────────────────────

#[test]
fn update_log_source_status_changes_status() {
    let db = Database::open_in_memory().unwrap();
    db.upsert_log_source(&make_log_source("src-1")).unwrap();
    db.update_log_source_status("src-1", "paused").unwrap();
    let stored = db.list_log_sources().unwrap();
    assert_eq!(stored[0].status, "paused");
}

#[test]
fn update_log_source_status_no_op_for_unknown_id() {
    // The UPDATE statement matches 0 rows but should not error.
    let db = Database::open_in_memory().unwrap();
    db.update_log_source_status("does-not-exist", "paused")
        .unwrap();
}

// ── query_usage_events_paginated (lines 1503-1540) ──────────────────────────

#[test]
fn query_usage_events_paginated_returns_empty_for_empty_db() {
    let db = Database::open_in_memory().unwrap();
    let rows = db.query_usage_events_paginated(None, 10).unwrap();
    assert!(rows.is_empty());
}

#[test]
fn query_usage_events_paginated_no_cursor_returns_most_recent_first() {
    let db = Database::open_in_memory().unwrap();
    let mut e1 = make_test_event("evt-1", AgentKind::ClaudeCode, 100);
    e1.timestamp_ms = 1_000;
    let mut e2 = make_test_event("evt-2", AgentKind::ClaudeCode, 200);
    e2.timestamp_ms = 2_000;
    let mut e3 = make_test_event("evt-3", AgentKind::ClaudeCode, 300);
    e3.timestamp_ms = 3_000;
    for e in [&e1, &e2, &e3] {
        db.write_usage_event(e, UsageWritePolicy::InsertOnce)
            .unwrap();
    }

    // No cursor: return most recent first.
    let rows = db.query_usage_events_paginated(None, 10).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].id, "evt-3");
    assert_eq!(rows[1].id, "evt-2");
    assert_eq!(rows[2].id, "evt-1");
}

#[test]
fn query_usage_events_paginated_with_cursor_returns_older_events() {
    let db = Database::open_in_memory().unwrap();
    let mut e1 = make_test_event("evt-1", AgentKind::ClaudeCode, 100);
    e1.timestamp_ms = 1_000;
    let mut e2 = make_test_event("evt-2", AgentKind::ClaudeCode, 200);
    e2.timestamp_ms = 2_000;
    let mut e3 = make_test_event("evt-3", AgentKind::ClaudeCode, 300);
    e3.timestamp_ms = 3_000;
    for e in [&e1, &e2, &e3] {
        db.write_usage_event(e, UsageWritePolicy::InsertOnce)
            .unwrap();
    }

    // Cursor at (evt-3, ts=3_000): should return evt-2 and evt-1.
    let rows = db
        .query_usage_events_paginated(Some(("evt-3", 3_000)), 10)
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "evt-2");
    assert_eq!(rows[1].id, "evt-1");
}

#[test]
fn query_usage_events_paginated_respects_limit() {
    let db = Database::open_in_memory().unwrap();
    for i in 0..5 {
        let mut e = make_test_event(&format!("evt-{i}"), AgentKind::ClaudeCode, 100);
        e.timestamp_ms = 1_000 + i as i64;
        db.write_usage_event(&e, UsageWritePolicy::InsertOnce)
            .unwrap();
    }
    let rows = db.query_usage_events_paginated(None, 2).unwrap();
    assert_eq!(rows.len(), 2, "limit must be respected");
}

// ── health_check (lines 1863-1905) ──────────────────────────────────────────

#[test]
fn health_check_returns_healthy_on_fresh_db() {
    // Cover the full body of health_check (lines 1863-1905).
    let db = Database::open_in_memory().unwrap();
    let info = db.health_check().unwrap();
    assert!(info.healthy, "fresh db should be healthy");
    assert_eq!(info.integrity_message, "ok");
    assert_eq!(info.migration_version, 7);
    assert!(info.db_size_bytes > 0, "fresh db has at least one page");
    assert_eq!(info.usage_event_count, 0);
    assert!(
        info.last_log_checkpoint_ms.is_none(),
        "no log_files -> no checkpoint timestamp"
    );
}

#[test]
fn health_check_reports_counts_after_data() {
    let db = Database::open_in_memory().unwrap();
    db.write_usage_event(
        &make_test_event("evt-1", AgentKind::ClaudeCode, 100),
        UsageWritePolicy::InsertOnce,
    )
    .unwrap();
    db.checkpoint_log_file("file-1", 1000, "claude_code", "src-1", "/tmp/x.jsonl", None)
        .unwrap();
    let info = db.health_check().unwrap();
    assert!(info.healthy);
    assert_eq!(info.usage_event_count, 1);
    assert!(
        info.last_log_checkpoint_ms.is_some(),
        "log_files has one row -> MAX(updated_at_ms) is Some"
    );
}

// ── row_to_usage_event unknown-agent fallback (line 2111) ───────────────────

#[test]
fn row_to_usage_event_unknown_agent_falls_back_to_claude_code() {
    // row_to_usage_event's match arm `_ => AgentKind::ClaudeCode` (line 2111)
    // is only reachable when the agent string in the DB doesn't match
    // "claude_code" or "codex". Insert a normal event, then UPDATE the agent
    // column to an unknown string, then read it back through get_usage_event.
    let db = Database::open_in_memory().unwrap();
    db.write_usage_event(
        &make_test_event("weird-id", AgentKind::ClaudeCode, 100),
        UsageWritePolicy::InsertOnce,
    )
    .unwrap();
    // Mutate the agent column to a value that matches no AgentKind variant.
    db.conn()
        .execute(
            "UPDATE usage_events SET agent = 'unknown-agent' WHERE id = 'weird-id'",
            [],
        )
        .unwrap();
    let got = db
        .get_usage_event("weird-id")
        .unwrap()
        .expect("row should exist");
    assert_eq!(
        got.agent,
        AgentKind::ClaudeCode,
        "unknown agent string must fall back to ClaudeCode"
    );
    assert_eq!(got.id, "weird-id");
}

#[test]
fn row_to_usage_event_codex_agent_round_trips() {
    // Sanity check that "codex" maps to AgentKind::Codex.
    let db = Database::open_in_memory().unwrap();
    let event = make_test_event("evt-codex", AgentKind::Codex, 100);
    db.write_usage_event(&event, UsageWritePolicy::InsertOnce)
        .unwrap();
    let got = db.get_usage_event("evt-codex").unwrap().unwrap();
    assert_eq!(got.agent, AgentKind::Codex);
}

// ── validate_date_format error paths (lines 2166-2192) ──────────────────────

/// Ingest a batch whose rollup closure produces a daily_usage row with the
/// given date. The transaction should fail because of `validate_date_format`.
fn ingest_with_bad_date_returns_err(db: &Database, date: &str) -> anyhow::Error {
    let event = make_test_event("evt-bad", AgentKind::ClaudeCode, 100);
    let batch = StoreWriteBatch::for_test("src-1", "file-1")
        .usage_event(event, UsageWritePolicy::InsertOnce)
        .checkpoint_offset(50);
    match db.ingest_store_batch(batch, "gen-A", |_events, _gen| {
        Ok(RollupRows {
            daily_usage_rows: vec![DailyUsageRow::for_test(date, "UTC", "gen-A", 100)],
            ..Default::default()
        })
    }) {
        Ok(_) => panic!("expected ingest_store_batch to fail for date {date:?}"),
        Err(e) => e,
    }
}

#[test]
fn validate_date_format_rejects_wrong_length() {
    // Lines 2167-2168 (len != 10).
    let db = Database::open_in_memory().unwrap();
    let err = ingest_with_bad_date_returns_err(&db, "short");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("YYYY-MM-DD") || msg.contains("date"),
        "expected date format error, got: {msg}"
    );
}

#[test]
fn validate_date_format_rejects_wrong_separators() {
    // Lines 2171-2172 (bytes[4] or bytes[7] != '-').
    let db = Database::open_in_memory().unwrap();
    let err = ingest_with_bad_date_returns_err(&db, "2026/07/02");
    assert!(format!("{err:#}").to_lowercase().contains("yyyy-mm-dd"));
}

#[test]
fn validate_date_format_rejects_non_numeric_year() {
    // Lines 2174-2176 — year parse fails.
    let db = Database::open_in_memory().unwrap();
    let err = ingest_with_bad_date_returns_err(&db, "abcd-07-02");
    let msg = format!("{err:#}").to_lowercase();
    assert!(msg.contains("year"), "expected 'year' in error, got: {msg}");
}

#[test]
fn validate_date_format_rejects_non_numeric_month() {
    // Lines 2177-2179 — month parse fails.
    let db = Database::open_in_memory().unwrap();
    let err = ingest_with_bad_date_returns_err(&db, "2026-xx-02");
    let msg = format!("{err:#}").to_lowercase();
    assert!(
        msg.contains("month"),
        "expected 'month' in error, got: {msg}"
    );
}

#[test]
fn validate_date_format_rejects_non_numeric_day() {
    // Lines 2180-2182 — day parse fails.
    let db = Database::open_in_memory().unwrap();
    let err = ingest_with_bad_date_returns_err(&db, "2026-07-xx");
    let msg = format!("{err:#}").to_lowercase();
    assert!(msg.contains("day"), "expected 'day' in error, got: {msg}");
}

#[test]
fn validate_date_format_rejects_year_out_of_range() {
    // Lines 2183-2185 — year outside 2000..=2100.
    let db = Database::open_in_memory().unwrap();
    let err = ingest_with_bad_date_returns_err(&db, "1999-07-02");
    let msg = format!("{err:#}").to_lowercase();
    assert!(msg.contains("year"), "expected 'year' in error, got: {msg}");
}

#[test]
fn validate_date_format_rejects_month_out_of_range() {
    // Lines 2186-2188 — month outside 1..=12.
    let db = Database::open_in_memory().unwrap();
    let err = ingest_with_bad_date_returns_err(&db, "2026-13-02");
    let msg = format!("{err:#}").to_lowercase();
    assert!(
        msg.contains("month"),
        "expected 'month' in error, got: {msg}"
    );
}

#[test]
fn validate_date_format_rejects_day_out_of_range() {
    // Lines 2189-2191 — day outside 1..=31.
    let db = Database::open_in_memory().unwrap();
    let err = ingest_with_bad_date_returns_err(&db, "2026-07-32");
    let msg = format!("{err:#}").to_lowercase();
    assert!(msg.contains("day"), "expected 'day' in error, got: {msg}");
}

#[test]
fn validate_date_format_accepts_boundary_dates() {
    // Cover the success path with valid boundary dates.
    let db = Database::open_in_memory().unwrap();
    let event = make_test_event("evt-ok", AgentKind::ClaudeCode, 100);
    let batch = StoreWriteBatch::for_test("src-1", "file-1")
        .usage_event(event, UsageWritePolicy::InsertOnce)
        .checkpoint_offset(50);
    db.ingest_store_batch(batch, "gen-A", |_events, _gen| {
        Ok(RollupRows {
            daily_usage_rows: vec![
                DailyUsageRow::for_test("2000-01-01", "UTC", "gen-A", 50),
                DailyUsageRow::for_test("2100-12-31", "UTC", "gen-A", 50),
            ],
            ..Default::default()
        })
    })
    .unwrap();
    assert_eq!(db.daily_usage_rows().unwrap().len(), 2);
}

// ── subagent_upsert_hot_binding (lines 1997-2000) ───────────────────────────

#[test]
fn subagent_upsert_hot_binding_inserts_and_reads() {
    // Cover subagent_upsert_hot_binding (lines 1997-2000) — currently 0 hits.
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();
    let sa = SubagentLogicalSubagentRow::for_test("sa-1", "reviewer");
    db.subagent_upsert_logical(&sa).unwrap();

    let binding = SubagentHarnessBindingRow {
        id: "bind-1".to_string(),
        subagent_id: "sa-1".to_string(),
        harness: "pi".to_string(),
        adapter_session_id: Some("sess-1".to_string()),
        adapter_process_id: Some("pid-1".to_string()),
        is_hot: 1,
        status: "hot".to_string(),
        created_at_ms: now,
        last_used_at_ms: Some(now),
        closed_at_ms: None,
        detail_json: None,
    };
    db.subagent_upsert_hot_binding(&binding).unwrap();
    let got = db.subagent_hot_binding("sa-1", "pi").unwrap();
    assert!(got.is_some(), "hot binding should be readable after upsert");
    let got = got.unwrap();
    assert_eq!(got.id, "bind-1");
    assert_eq!(got.is_hot, 1);
    assert_eq!(got.status, "hot");

    // Upsert again with a NEW id but same (subagent_id, harness) and is_hot=1.
    // The ON CONFLICT(subagent_id, harness) WHERE is_hot=1 clause triggers and
    // updates the EXISTING row's adapter_session_id/status fields in place
    // (the conflicting new row with id="bind-2" is discarded).
    let mut updated = binding.clone();
    updated.id = "bind-2".to_string();
    updated.status = "hot".to_string();
    updated.adapter_session_id = Some("sess-2".to_string());
    db.subagent_upsert_hot_binding(&updated).unwrap();
    let after = db.subagent_hot_binding("sa-1", "pi").unwrap();
    assert!(
        after.is_some(),
        "hot binding should still exist after re-upsert"
    );
    let after = after.unwrap();
    // The existing row keeps id="bind-1" (ON CONFLICT DO UPDATE does not change `id`).
    assert_eq!(after.id, "bind-1");
    assert_eq!(after.adapter_session_id.as_deref(), Some("sess-2"));
}
