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
//! Coverage gap tests for `read_queries.rs`.
//!
//! Each test targets a specific uncovered branch identified by the lcov
//! report. Tests stay small and focused: one branch per test where possible.

use busytok_domain::{now_ms, AgentKind, NormalizedUsageEvent, UsageWritePolicy};
use busytok_store::db::Database;
use busytok_store::read_models::{
    BreakdownDimension, BreakdownFilterField, OverviewExactWindow, RangeWindow,
};
use busytok_store::read_queries;

// ── Overview summary second match arm (L289) ────────────────────────────────
//
// `(Some(_), true) => Some(0.0)` triggers when at least one row has a non-null
// cost_usd (so has_cost=true) but the SUM(cost_usd) is 0.0 (because the only
// non-null cost_usd value was 0.0). Without this test the arm is never hit.

#[test]
fn read_overview_summary_reports_zero_cost_when_priced_row_has_zero_cost() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_hour
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_usd,
              cost_status, event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-zero-cost', 0, 'claude_code', 'sonnet', 100, 0.0, 'exact', 1, 1000, 1000)",
            [],
        )
        .unwrap();

    let summary = read_queries::read_overview_summary(
        db.conn(),
        "gen-zero-cost",
        &RangeWindow::new(0, 3_600_000),
    )
    .unwrap();
    assert_eq!(summary.total_tokens, 100);
    assert!(summary.has_cost);
    assert!(!summary.has_no_cost);
    assert_eq!(summary.total_cost_usd, Some(0.0));
}

// ── read_overview_window_aggregates_exact empty windows (L1619) ──────────────

#[test]
fn read_overview_window_aggregates_exact_empty_windows_returns_empty_vec() {
    let db = Database::open_in_memory().unwrap();
    let rows =
        read_queries::read_overview_window_aggregates_exact(db.conn(), "gen-empty", &[]).unwrap();
    assert!(rows.is_empty());
}

// ── read_breakdown_window_aggregates_exact (L1695-1780) ──────────────────────
//
// Entire function is uncovered. Seed usage_events with project_hash and verify
// the window aggregates filter by the dimension field.

#[test]
fn read_breakdown_window_aggregates_exact_filters_by_project_hash() {
    let db = Database::open_in_memory().unwrap();

    let mut priced =
        NormalizedUsageEvent::minimal_for_test("evt-bd-window-1", AgentKind::ClaudeCode);
    priced.timestamp_ms = 1_000;
    priced.total_tokens = 120;
    priced.cost_usd = Some(0.4);
    priced.project_hash = Some("proj-A".to_string());
    priced.project_path = Some("/repo/A".to_string());
    db.write_usage_event(&priced, UsageWritePolicy::InsertOnce)
        .unwrap();
    db.conn()
        .execute(
            "UPDATE usage_events SET generation_id = 'gen-bd', dedupe_key = 'evt-bd-window-1' \
             WHERE id = 'evt-bd-window-1'",
            [],
        )
        .unwrap();

    let mut other_proj =
        NormalizedUsageEvent::minimal_for_test("evt-bd-window-2", AgentKind::ClaudeCode);
    other_proj.timestamp_ms = 1_500;
    other_proj.total_tokens = 80;
    other_proj.cost_usd = None;
    other_proj.project_hash = Some("proj-B".to_string());
    other_proj.project_path = Some("/repo/B".to_string());
    db.write_usage_event(&other_proj, UsageWritePolicy::InsertOnce)
        .unwrap();
    db.conn()
        .execute(
            "UPDATE usage_events SET generation_id = 'gen-bd', dedupe_key = 'evt-bd-window-2' \
             WHERE id = 'evt-bd-window-2'",
            [],
        )
        .unwrap();

    let windows = vec![
        OverviewExactWindow {
            key: "filled".to_string(),
            start_ms: 0,
            end_ms: 2_000,
        },
        OverviewExactWindow {
            key: "empty".to_string(),
            start_ms: 5_000,
            end_ms: 6_000,
        },
    ];
    let rows = read_queries::read_breakdown_window_aggregates_exact(
        db.conn(),
        "gen-bd",
        BreakdownFilterField::Project,
        "proj-A",
        &windows,
    )
    .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].key, "filled");
    assert_eq!(rows[0].tokens, 120); // only proj-A event qualifies
    assert_eq!(rows[0].cost_usd, Some(0.4));
    assert_eq!(rows[0].event_count, 1);
    assert!(rows[0].has_cost);
    assert!(!rows[0].has_no_cost);
    assert_eq!(rows[1].key, "empty");
    assert_eq!(rows[1].tokens, 0);
    assert!(!rows[1].has_cost);
    assert!(!rows[1].has_no_cost);
}

#[test]
fn read_breakdown_window_aggregates_exact_empty_windows_returns_empty_vec() {
    let db = Database::open_in_memory().unwrap();
    let rows = read_queries::read_breakdown_window_aggregates_exact(
        db.conn(),
        "gen-bd",
        BreakdownFilterField::Model,
        "any-model",
        &[],
    )
    .unwrap();
    assert!(rows.is_empty());
}

// ── read_breakdown_list with Model dimension (L1253, L1347-1429) ─────────────
//
// `breakdown_dimension_spec(Model)` and `populate_model_breakdown_extra_values`
// are both uncovered. Seed usage_by_model_day with multiple projects so the
// extra-values top-project lookup also has data.

#[test]
fn read_breakdown_list_groups_by_model_and_populates_extra_values() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_by_model_day
             (generation_id, date, agent, model, total_tokens, cost_usd, cost_status,
              event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES
             ('gen-md', '2026-05-28', 'claude_code', 'sonnet', 950, 0.5, 'exact', 5, 1000, 1000, 1000),
             ('gen-md', '2026-05-28', 'claude_code', 'haiku',  300, NULL, 'unavailable', 2, 1000, 1000, 1000)",
            [],
        )
        .unwrap();
    // Seed usage_by_project_day so populate_model_breakdown_extra_values' ranked
    // project lookup returns a row.
    db.conn()
        .execute(
            "INSERT INTO usage_by_project_day
             (generation_id, date, project_id, project_path, agent, model,
              total_tokens, cost_usd, cost_status, event_count, last_active_at_ms,
              created_at_ms, updated_at_ms)
             VALUES
             ('gen-md', '2026-05-28', 'proj-1', '/repo/1', 'claude_code', 'sonnet', 800, 0.4, 'exact', 4, 1000, 1000, 1000),
             ('gen-md', '2026-05-28', 'proj-2', '/repo/2', 'claude_code', 'sonnet', 150, 0.1, 'exact', 1, 1000, 1000, 1000)",
            [],
        )
        .unwrap();

    let rows = read_queries::read_breakdown_list(
        db.conn(),
        "gen-md",
        BreakdownDimension::Model,
        "2026-05-01",
        "2026-06-01",
        50,
        None,
    )
    .unwrap();

    assert_eq!(rows.items.len(), 2);
    // Ordered by total_tokens DESC: sonnet (950) > haiku (300).
    assert_eq!(rows.items[0].group_key, "sonnet");
    assert_eq!(rows.items[0].total_tokens, 950);
    assert!(rows.items[0].has_cost);
    assert!(!rows.items[0].has_no_cost);
    // extra_values[0] = agent label, extra_values[1] = top project label
    assert_eq!(rows.items[0].extra_values.len(), 2);
    assert_eq!(
        rows.items[0].extra_values[0],
        Some("claude_code".to_string())
    );
    assert_eq!(rows.items[0].extra_values[1], Some("/repo/1".to_string()));

    assert_eq!(rows.items[1].group_key, "haiku");
    assert_eq!(rows.items[1].total_tokens, 300);
    assert!(!rows.items[1].has_cost);
    assert!(rows.items[1].has_no_cost);
}

// ── read_breakdown_list with Session dimension (L1253-1258, L1271, L1431-1482)

#[test]
fn read_breakdown_list_groups_by_session_and_populates_extra_values() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_by_session_day
             (generation_id, date, session_id, agent, client_kind, project_path,
              project_hash, model, total_tokens, cost_usd, cost_status, event_count,
              last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES
             ('gen-sd', '2026-05-28', 'sess-1', 'claude_code', 'cli', '/repo/a', 'ph-1',
              'sonnet', 700, 0.3, 'exact', 2, 1000, 1000, 1000),
             ('gen-sd', '2026-05-28', 'sess-2', 'claude_code', 'cli', '/repo/b', 'ph-2',
              'haiku',  250, NULL, 'unavailable', 1, 1000, 1000, 1000)",
            [],
        )
        .unwrap();

    let rows = read_queries::read_breakdown_list(
        db.conn(),
        "gen-sd",
        BreakdownDimension::Session,
        "2026-05-01",
        "2026-06-01",
        50,
        None,
    )
    .unwrap();

    assert_eq!(rows.items.len(), 2);
    // Sessions order by last_active_at_ms DESC then group_key DESC. Both rows
    // share last_active_at_ms=1000 so session_id DESC applies.
    assert_eq!(rows.items[0].group_key, "sess-2");
    assert_eq!(rows.items[0].total_tokens, 250);
    assert_eq!(rows.items[1].group_key, "sess-1");
    assert_eq!(rows.items[1].total_tokens, 700);
    // extra_values = [client_kind, project_path, project_hash]
    assert_eq!(rows.items[1].extra_values.len(), 3);
    assert_eq!(rows.items[1].extra_values[0], Some("cli".to_string()));
    assert_eq!(rows.items[1].extra_values[1], Some("/repo/a".to_string()));
    assert_eq!(rows.items[1].extra_values[2], Some("ph-1".to_string()));
}

// ── read_project_top_sessions (L1176-1230) ───────────────────────────────────
//
// Entire function uncovered.

#[test]
fn read_project_top_sessions_returns_top_n_ordered_by_tokens_desc() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_by_session_day
             (generation_id, date, session_id, agent, client_kind, project_path,
              project_hash, model, total_tokens, cost_usd, cost_status, event_count,
              last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES
             ('gen-pst', '2026-05-28', 'sess-low',  'claude_code', 'cli', '/repo/p', 'ph-x', 'sonnet', 100, 0.1, 'exact',        1, 1000, 1000, 1000),
             ('gen-pst', '2026-05-28', 'sess-high', 'claude_code', 'cli', '/repo/p', 'ph-x', 'sonnet', 900, 0.9, 'exact',        4, 1000, 1000, 1000),
             ('gen-pst', '2026-05-28', 'sess-mid',  'claude_code', 'cli', '/repo/p', 'ph-x', 'sonnet', 500, NULL,'unavailable', 2, 1000, 1000, 1000),
             ('gen-pst', '2026-05-28', 'sess-other','claude_code', 'cli', '/repo/p', 'ph-y', 'sonnet', 999, 0.1, 'exact',        1, 1000, 1000, 1000)",
            [],
        )
        .unwrap();

    let rows = read_queries::read_project_top_sessions(
        db.conn(),
        "gen-pst",
        "ph-x",
        "2026-05-01",
        "2026-06-01",
        10,
    )
    .unwrap();

    assert_eq!(rows.len(), 3);
    // Ordered by total_tokens DESC, session_id DESC.
    assert_eq!(rows[0].group_key, "sess-high");
    assert_eq!(rows[0].total_tokens, 900);
    assert_eq!(rows[1].group_key, "sess-mid");
    assert_eq!(rows[1].total_tokens, 500);
    assert!(!rows[1].has_cost);
    assert!(rows[1].has_no_cost);
    assert_eq!(rows[2].group_key, "sess-low");
    // extra_values = [client_kind, project_path]
    assert_eq!(rows[0].extra_values.len(), 2);
    assert_eq!(rows[0].extra_values[0], Some("cli".to_string()));
    assert_eq!(rows[0].extra_values[1], Some("/repo/p".to_string()));
}

// ── read_session_source_context (L939-966) ───────────────────────────────────
//
// Entire function uncovered. Joins usage_events → log_files → log_sources.

#[test]
fn read_session_source_context_returns_distinct_sources_for_session() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO log_sources
             (id, agent, source_type, root_path, configured_by_user, default_discovery_enabled,
              status, first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms)
             VALUES ('src-1', 'claude_code', 'session_root', '/repo/demo', 1, 0,
                     'active', 1000, 1000, 1000, 1000),
                    ('src-2', 'codex', 'session_root', '/other', 1, 0,
                     'active', 1000, 1000, 1000, 1000)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO log_files
             (id, source_id, agent, path, inode, offset_bytes, state,
              first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms)
             VALUES ('file-1', 'src-1', 'claude_code', '/repo/demo/a.jsonl', NULL, 0, 'active', 1000, 1000, 1000, 1000),
                    ('file-2', 'src-2', 'codex',         '/other/b.jsonl',     NULL, 0, 'active', 1000, 1000, 1000, 1000)",
            [],
        )
        .unwrap();
    // Two usage_events on different files but same session → DISTINCT collapses to two source rows.
    db.conn()
        .execute(
            "INSERT INTO usage_events
             (id, agent, source_file_id, source_path, source_line, source_offset_start,
              source_offset_end, session_id, timestamp_ms, model, total_tokens, cost_usd,
              cost_source, raw_event_hash, is_error, generation_id, dedupe_key,
              created_at_ms, updated_at_ms)
             VALUES
             ('evt-sess-1', 'claude_code', 'file-1', '/repo/demo/a.jsonl', 0, 0, 0, 'sess-A', 1000, 'sonnet', 100, NULL, 'unknown', '', 0, 'gen-sc', 'dk-1', 1000, 1000),
             ('evt-sess-2', 'codex',         'file-2', '/other/b.jsonl',     0, 0, 0, 'sess-A', 2000, 'gpt-5',  100, NULL, 'unknown', '', 0, 'gen-sc', 'dk-2', 1000, 1000)",
            [],
        )
        .unwrap();

    let rows =
        read_queries::read_session_source_context(db.conn(), "gen-sc", "sess-A", 20).unwrap();
    assert_eq!(rows.len(), 2);
    let source_ids: Vec<&str> = rows.iter().map(|r| r.source_id.as_str()).collect();
    assert!(source_ids.contains(&"src-1"));
    assert!(source_ids.contains(&"src-2"));
}

// ── read_breakdown_activity_list (L555) ──────────────────────────────────────
//
// `read_breakdown_activity_list` is uncovered. Tests filter by project, model,
// session to exercise the dynamic column branch in `breakdown_filter_column`.

#[test]
fn read_breakdown_activity_list_filters_by_project_dimension() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_events
             (id, agent, source_file_id, source_path, source_line, source_offset_start,
              source_offset_end, session_id, timestamp_ms, model, total_tokens, cost_usd,
              cost_source, raw_event_hash, is_error, generation_id, dedupe_key,
              project_hash, project_path, created_at_ms, updated_at_ms)
             VALUES
             ('evt-1', 'claude_code', '', '', 0, 0, 0, 'sess-A', 1000, 'sonnet', 100, 0.1, 'unknown', '', 0, 'gen-ba', 'dk-1', 'ph-A', '/repo/A', 1000, 1000),
             ('evt-2', 'claude_code', '', '', 0, 0, 0, 'sess-B', 2000, 'sonnet', 200, NULL, 'unknown', '', 0, 'gen-ba', 'dk-2', 'ph-B', '/repo/B', 1000, 1000)",
            [],
        )
        .unwrap();

    let rows = read_queries::read_breakdown_activity_list(
        db.conn(),
        "gen-ba",
        BreakdownFilterField::Project,
        "ph-A",
        0,
        10_000,
        10,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "evt-1");
    assert_eq!(rows[0].project_hash.as_deref(), Some("ph-A"));
}

#[test]
fn read_breakdown_activity_list_filters_by_model_dimension() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_events
             (id, agent, source_file_id, source_path, source_line, source_offset_start,
              source_offset_end, session_id, timestamp_ms, model, total_tokens, cost_usd,
              cost_source, raw_event_hash, is_error, generation_id, dedupe_key,
              created_at_ms, updated_at_ms)
             VALUES
             ('evt-m1', 'claude_code', '', '', 0, 0, 0, 'sess-A', 1000, 'sonnet', 100, 0.1, 'unknown', '', 0, 'gen-bm', 'dk-1', 1000, 1000),
             ('evt-m2', 'claude_code', '', '', 0, 0, 0, 'sess-B', 2000, 'haiku',  200, NULL, 'unknown', '', 0, 'gen-bm', 'dk-2', 1000, 1000)",
            [],
        )
        .unwrap();

    let rows = read_queries::read_breakdown_activity_list(
        db.conn(),
        "gen-bm",
        BreakdownFilterField::Model,
        "haiku",
        0,
        10_000,
        10,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "evt-m2");
    assert_eq!(rows[0].model.as_deref(), Some("haiku"));
}

#[test]
fn read_breakdown_activity_list_filters_by_session_dimension() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_events
             (id, agent, source_file_id, source_path, source_line, source_offset_start,
              source_offset_end, session_id, timestamp_ms, model, total_tokens, cost_usd,
              cost_source, raw_event_hash, is_error, generation_id, dedupe_key,
              created_at_ms, updated_at_ms)
             VALUES
             ('evt-s1', 'claude_code', '', '', 0, 0, 0, 'sess-A', 1000, 'sonnet', 100, 0.1, 'unknown', '', 0, 'gen-bs', 'dk-1', 1000, 1000),
             ('evt-s2', 'claude_code', '', '', 0, 0, 0, 'sess-B', 2000, 'sonnet', 200, NULL, 'unknown', '', 0, 'gen-bs', 'dk-2', 1000, 1000)",
            [],
        )
        .unwrap();

    let rows = read_queries::read_breakdown_activity_list(
        db.conn(),
        "gen-bs",
        BreakdownFilterField::Session,
        "sess-B",
        0,
        10_000,
        10,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "evt-s2");
    assert_eq!(rows[0].session_id, "sess-B");
}

// ── read_source_health_summaries cursor pagination (L688-697, L735) ──────────
//
// Two uncovered paths: (1) the `if let Some(cursor)` branch with the indexed
// parameter binding, and (2) the `next_cursor` encode path when items overflow
// page_size.

#[test]
fn read_source_health_summaries_cursor_pagination_returns_second_page() {
    let db = Database::open_in_memory().unwrap();
    // Three sources with distinct last_scan_at_ms so the cursor sort key
    // (last_scan_at_ms DESC, source_id DESC) gives a deterministic order.
    for (source_id, scan_ms) in [("src-1", 1000), ("src-2", 2000), ("src-3", 3000)] {
        db.conn()
            .execute(
                "INSERT INTO source_health_summary
                 (generation_id, source_id, agent, root_path, source_type, status,
                  configured_by_user, last_scan_at_ms, file_count, parsed_file_count,
                  event_count, last_error, latest_activity_at_ms, created_at_ms, updated_at_ms)
                 VALUES ('gen-sh', ?1, 'claude_code', '/repo', 'default', 'active',
                         1, ?2, 0, 0, 0, NULL, NULL, 1000, 1000)",
                rusqlite::params![source_id, scan_ms],
            )
            .unwrap();
    }

    // Page 1: limit=1, should return one row (src-3, the highest scan) and a
    // next_cursor that encodes (scan=3000, source_id="src-3").
    let page1 =
        read_queries::read_source_health_summaries(db.conn(), "gen-sh", 1, None, None, None)
            .unwrap();
    assert_eq!(page1.items.len(), 1);
    assert_eq!(page1.items[0].source_id, "src-3");
    let cursor = page1
        .next_cursor
        .expect("first page should produce a cursor");

    // Page 2: cursor set, exercises the `idx, idx2, idx3` parameter binding.
    let page2 = read_queries::read_source_health_summaries(
        db.conn(),
        "gen-sh",
        1,
        Some(cursor),
        None,
        None,
    )
    .unwrap();
    assert_eq!(page2.items.len(), 1);
    assert_eq!(page2.items[0].source_id, "src-2");
}

// ── read_source_health_summary_totals status/client filters (L760-762) ────────
//
// `append_source_health_status_filter` has multiple branches that are not
// exercised: `client_id_filter` with status_filter combos and the `idle` /
// `other` status filter arms. Drive each through the totals path.

#[test]
fn read_source_health_summary_totals_filters_by_client_and_status() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO source_health_summary
             (generation_id, source_id, agent, root_path, source_type, status,
              configured_by_user, last_scan_at_ms, file_count, parsed_file_count,
              event_count, last_error, latest_activity_at_ms, created_at_ms, updated_at_ms)
             VALUES
             ('gen-totals', 'src-1', 'claude_code', '/a', 'default', 'active',    1, 1000, 0, 0, 5, NULL, NULL, 1000, 1000),
             ('gen-totals', 'src-2', 'codex',         '/b', 'default', 'scanning',  1, 1000, 0, 0, 3, NULL, NULL, 1000, 1000),
             ('gen-totals', 'src-3', 'claude_code', '/c', 'default', 'error',     1, 1000, 0, 0, 2, NULL, NULL, 1000, 1000),
             ('gen-totals', 'src-4', 'claude_code', '/d', 'default', 'idle',      1, 1000, 0, 0, 1, NULL, NULL, 1000, 1000)",
            [],
        )
        .unwrap();

    // status_filter = "scanning_or_active" → matches src-1 and src-2 (active_source_count = 1).
    let totals_scanning_or_active = read_queries::read_source_health_summary_totals(
        db.conn(),
        "gen-totals",
        None,
        Some("scanning_or_active"),
    )
    .unwrap();
    assert_eq!(totals_scanning_or_active.source_count, 2);
    assert_eq!(totals_scanning_or_active.active_source_count, 1);

    // status_filter = "idle" → src-4 only (idle status, NOT in the excluded list).
    let totals_idle = read_queries::read_source_health_summary_totals(
        db.conn(),
        "gen-totals",
        None,
        Some("idle"),
    )
    .unwrap();
    assert_eq!(totals_idle.source_count, 1);
    assert_eq!(totals_idle.active_source_count, 0);

    // status_filter = "error" (other arm) → only src-3.
    let totals_error = read_queries::read_source_health_summary_totals(
        db.conn(),
        "gen-totals",
        None,
        Some("error"),
    )
    .unwrap();
    assert_eq!(totals_error.source_count, 1);
    assert_eq!(totals_error.active_source_count, 0);

    // client_id_filter alone → restricts to a single agent.
    let totals_cc = read_queries::read_source_health_summary_totals(
        db.conn(),
        "gen-totals",
        Some("claude_code"),
        None,
    )
    .unwrap();
    assert_eq!(totals_cc.source_count, 3); // src-1, src-3, src-4

    // Combined client + status_filter (other arm).
    let totals_cc_active = read_queries::read_source_health_summary_totals(
        db.conn(),
        "gen-totals",
        Some("claude_code"),
        Some("active"),
    )
    .unwrap();
    assert_eq!(totals_cc_active.source_count, 1);
    assert_eq!(totals_cc_active.active_source_count, 1);
}

// Same filters but exercised through `read_source_health_summaries` to ensure
// the listing query also constructs the WHERE clauses correctly.

#[test]
fn read_source_health_summaries_filters_by_client_and_status() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO source_health_summary
             (generation_id, source_id, agent, root_path, source_type, status,
              configured_by_user, last_scan_at_ms, file_count, parsed_file_count,
              event_count, last_error, latest_activity_at_ms, created_at_ms, updated_at_ms)
             VALUES
             ('gen-list', 'src-1', 'claude_code', '/a', 'default', 'active',    1, 1000, 0, 0, 5, NULL, NULL, 1000, 1000),
             ('gen-list', 'src-2', 'codex',         '/b', 'default', 'scanning',  1, 2000, 0, 0, 3, NULL, NULL, 1000, 1000),
             ('gen-list', 'src-3', 'claude_code', '/c', 'default', 'idle',      1, 3000, 0, 0, 1, NULL, NULL, 1000, 1000)",
            [],
        )
        .unwrap();

    let scanning_or_active = read_queries::read_source_health_summaries(
        db.conn(),
        "gen-list",
        50,
        None,
        None,
        Some("scanning_or_active"),
    )
    .unwrap();
    let ids: Vec<&str> = scanning_or_active
        .items
        .iter()
        .map(|r| r.source_id.as_str())
        .collect();
    assert_eq!(ids, vec!["src-2", "src-1"]); // ordered by last_scan DESC, source_id DESC

    let idle = read_queries::read_source_health_summaries(
        db.conn(),
        "gen-list",
        50,
        None,
        None,
        Some("idle"),
    )
    .unwrap();
    assert_eq!(idle.items.len(), 1);
    assert_eq!(idle.items[0].source_id, "src-3");

    let by_client = read_queries::read_source_health_summaries(
        db.conn(),
        "gen-list",
        50,
        None,
        Some("claude_code"),
        None,
    )
    .unwrap();
    assert_eq!(by_client.items.len(), 2);
}

// ── read_client_source_detail (covers the optional/None path) ───────────────
//
// `read_client_source_detail` returns None when source_id has status='removed'.
// Lightly exercised path that helps cover the `optional()` boundary.

#[test]
fn read_client_source_detail_returns_none_for_removed_source() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO log_sources
             (id, agent, source_type, root_path, configured_by_user, default_discovery_enabled,
              status, first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms,
              last_scan_started_at_ms, last_scan_completed_at_ms)
             VALUES ('src-removed', 'claude_code', 'session_root', '/old', 1, 0,
                     'removed', 1000, 1000, 1000, 1000, NULL, NULL)",
            [],
        )
        .unwrap();

    let row = read_queries::read_client_source_detail(db.conn(), "src-removed").unwrap();
    assert!(row.is_none(), "removed sources must not be returned");
}

#[test]
fn read_client_source_detail_returns_row_for_active_source() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO log_sources
             (id, agent, source_type, root_path, configured_by_user, default_discovery_enabled,
              status, first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms,
              last_scan_started_at_ms, last_scan_completed_at_ms)
             VALUES ('src-active', 'claude_code', 'session_root', '/repo', 1, 0,
                     'active', 1000, 1000, 1000, 1000, 950, 1000)",
            [],
        )
        .unwrap();

    let row = read_queries::read_client_source_detail(db.conn(), "src-active")
        .unwrap()
        .expect("active source should be returned");
    assert_eq!(row.source_id, "src-active");
    assert_eq!(row.agent, "claude_code");
    assert_eq!(row.status, "active");
    assert!(row.configured_by_user);
}

// ── read_model_token_breakdown returns zeros when no events match ────────────
//
// `read_model_token_breakdown` is partially covered; the empty-result path is
// not. The COALESCE ensures zero defaults — exercise that branch.

#[test]
fn read_model_token_breakdown_returns_zeros_when_model_not_found() {
    let db = Database::open_in_memory().unwrap();
    let row = read_queries::read_model_token_breakdown(
        db.conn(),
        "gen-empty",
        "no-such-model",
        0,
        10_000,
    )
    .unwrap();
    assert_eq!(row.prompt_input_total_tokens, 0);
    assert_eq!(row.cache_read_tokens, 0);
    assert_eq!(row.input_tokens, 0);
}

// ── read_overview_trend_from_daily_usage match arm (L2114) ───────────────────
//
// `(Some(cost), true) if cost > 0.0 => Some(cost)` arm in the daily-usage
// trend. Existing test seeds non-zero cost; here we exercise the path with a
// distinct date range to ensure the match arm is reached.

#[test]
fn read_overview_trend_from_daily_usage_returns_cost_when_priced() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO daily_usage (date, timezone, agent, project_hash, model, \
             input_tokens, output_tokens, total_tokens, cached_input_tokens, \
             cache_creation_tokens, cache_read_tokens, reasoning_tokens, \
             thoughts_tokens, tool_tokens, cost_usd, estimated_cost_usd, event_count, generation_id) \
             VALUES ('2026-07-01', 'UTC', 'claude_code', '', 'sonnet', \
             10, 20, 30, 0, 0, 0, 0, 0, 0, 0.42, NULL, 1, 'gen-trend')",
            [],
        )
        .unwrap();

    let rows = read_queries::read_overview_trend_from_daily_usage(
        db.conn(),
        "UTC",
        "2026-07-01",
        "2026-07-01",
        "gen-trend",
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tokens, 30);
    assert_eq!(rows[0].cost_usd, Some(0.42));
    assert!(rows[0].has_cost);
    assert!(!rows[0].has_no_cost);
}
