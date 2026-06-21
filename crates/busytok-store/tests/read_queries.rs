//! Integration tests for read query modules.
//!
//! Tests seeding data through the `Database` write surface then querying
//! via the `read_queries` module functions.

use busytok_domain::{now_ms, AgentKind, NormalizedUsageEvent};
use busytok_store::db::Database;
use busytok_store::read_models::{BreakdownDimension, OverviewExactWindow, RangeWindow};
use busytok_store::read_queries;
use time::{Date, Time, UtcOffset};

// ── Service state ────────────────────────────────────────────────────────────

fn seeded_db_with_service_state() -> Database {
    let db = Database::open_in_memory().unwrap();
    let now = now_ms();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO service_state \
             (id, writer_queue_depth, aggregate_lag_ms, readiness, \
              active_generation_id, last_exact_rebuild_at_ms, updated_at_ms) \
             VALUES (1, 0, 0, 'starting', NULL, NULL, ?1)",
            rusqlite::params![now],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO event_sequence_state \
             (id, latest_event_seq, latest_event_timestamp_ms, updated_at_ms) \
             VALUES (1, 42, NULL, ?1)",
            rusqlite::params![now],
        )
        .unwrap();
    db
}

#[test]
fn read_service_state_returns_latest_event_sequence() {
    let db = seeded_db_with_service_state();
    let state = read_queries::read_service_state(db.conn()).unwrap();
    assert_eq!(state.latest_event_seq, Some(42));
}

#[test]
fn read_service_state_returns_default_readiness_for_seeded_row() {
    let db = seeded_db_with_service_state();
    let state = read_queries::read_service_state(db.conn()).unwrap();
    assert_eq!(state.readiness.as_deref(), Some("starting"));
    assert_eq!(state.writer_queue_depth, 0);
    assert_eq!(state.aggregate_lag_ms, 0);
}

#[test]
fn read_service_state_returns_defaults_when_no_service_state_row() {
    let db = Database::open_in_memory().unwrap();
    let state = read_queries::read_service_state(db.conn()).unwrap();
    // Fresh install: readiness defaults to "starting", no generation, no seq
    assert_eq!(state.readiness.as_deref(), Some("starting"));
    assert_eq!(state.active_generation_id, None);
    assert_eq!(state.latest_event_seq, None);
    assert_eq!(state.writer_queue_depth, 0);
    assert_eq!(state.aggregate_lag_ms, 0);
}

// ── Overview summary ─────────────────────────────────────────────────────────

fn seed_events_in_generation(db: &Database, gen_id: &str) {
    for i in 0..3 {
        let id = format!("{}-evt-{}", gen_id, i);
        let mut event = NormalizedUsageEvent::minimal_for_test(&id, AgentKind::ClaudeCode);
        event.timestamp_ms = 1000 + i * 1000;
        event.total_tokens = 100 + i * 50;
        event.cost_usd = Some(0.01 * (i + 1) as f64);
        // Write the generation_id and dedupe_key into the event via raw SQL
        // since the domain event doesn't carry these fields.
        db.write_usage_event(&event, busytok_domain::UsageWritePolicy::InsertOnce)
            .unwrap();
        db.conn()
            .execute(
                "UPDATE usage_events SET generation_id = ?1, dedupe_key = ?2 WHERE id = ?3",
                rusqlite::params![gen_id, event.id, event.id],
            )
            .unwrap();
    }
}

#[test]
fn read_overview_summary_aggregates_events_in_range() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_hour
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_usd,
              cost_status, event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-1', 0, 'claude_code', 'sonnet', 100, 0.01, 'exact', 1, 1000, 1000),
                    ('gen-1', 1000, 'claude_code', 'sonnet', 150, 0.02, 'exact', 1, 1000, 1000),
                    ('gen-1', 2000, 'claude_code', 'sonnet', 200, 0.03, 'exact', 1, 1000, 1000)",
            [],
        )
        .unwrap();

    let range = RangeWindow::new(0, 3001);
    let summary = read_queries::read_overview_summary(db.conn(), "gen-1", &range).unwrap();
    assert_eq!(summary.total_tokens, 450); // 100 + 150 + 200
    assert_eq!(summary.event_count, 3);
    assert!(summary.has_cost);
    assert!(!summary.has_no_cost);
}

#[test]
fn read_overview_summary_respects_generation_filter() {
    let db = Database::open_in_memory().unwrap();
    for generation_id in ["gen-1", "gen-2"] {
        db.conn()
            .execute(
                "INSERT INTO usage_buckets_hour
                 (generation_id, bucket_start_ms, agent, model, total_tokens,
                  cost_status, event_count, created_at_ms, updated_at_ms)
                 VALUES (?1, 0, 'claude_code', 'sonnet', 300, 'unavailable', 3, 1000, 1000)",
                rusqlite::params![generation_id],
            )
            .unwrap();
    }

    let range = RangeWindow::new(0, 10000);
    let summary_gen1 = read_queries::read_overview_summary(db.conn(), "gen-1", &range).unwrap();
    let summary_gen2 = read_queries::read_overview_summary(db.conn(), "gen-2", &range).unwrap();
    assert_eq!(summary_gen1.event_count, 3);
    assert_eq!(summary_gen2.event_count, 3);
}

#[test]
fn read_overview_summary_returns_zero_for_no_events() {
    let db = Database::open_in_memory().unwrap();
    let range = RangeWindow::new(0, 10000);
    let summary = read_queries::read_overview_summary(db.conn(), "gen-empty", &range).unwrap();
    assert_eq!(summary.total_tokens, 0);
    assert_eq!(summary.event_count, 0);
    assert!(!summary.has_cost);
    assert!(!summary.has_no_cost);
}

#[test]
fn read_overview_summary_exact_reports_mixed_cost_availability() {
    let db = Database::open_in_memory().unwrap();

    let mut priced = NormalizedUsageEvent::minimal_for_test("evt-priced", AgentKind::ClaudeCode);
    priced.timestamp_ms = 1_000;
    priced.total_tokens = 120;
    priced.cost_usd = Some(0.4);
    db.write_usage_event(&priced, busytok_domain::UsageWritePolicy::InsertOnce)
        .unwrap();
    db.conn()
        .execute(
            "UPDATE usage_events
             SET generation_id = 'gen-1', dedupe_key = 'evt-priced'
             WHERE id = 'evt-priced'",
            [],
        )
        .unwrap();

    let mut unpriced =
        NormalizedUsageEvent::minimal_for_test("evt-unpriced", AgentKind::ClaudeCode);
    unpriced.timestamp_ms = 2_000;
    unpriced.total_tokens = 80;
    unpriced.cost_usd = None;
    db.write_usage_event(&unpriced, busytok_domain::UsageWritePolicy::InsertOnce)
        .unwrap();
    db.conn()
        .execute(
            "UPDATE usage_events
             SET generation_id = 'gen-1', dedupe_key = 'evt-unpriced'
             WHERE id = 'evt-unpriced'",
            [],
        )
        .unwrap();

    let mut other_generation =
        NormalizedUsageEvent::minimal_for_test("evt-other-generation", AgentKind::ClaudeCode);
    other_generation.timestamp_ms = 1_500;
    other_generation.total_tokens = 999;
    other_generation.cost_usd = Some(9.9);
    db.write_usage_event(
        &other_generation,
        busytok_domain::UsageWritePolicy::InsertOnce,
    )
    .unwrap();
    db.conn()
        .execute(
            "UPDATE usage_events
             SET generation_id = 'gen-2', dedupe_key = 'evt-other-generation'
             WHERE id = 'evt-other-generation'",
            [],
        )
        .unwrap();

    let summary =
        read_queries::read_overview_summary_exact(db.conn(), "gen-1", &RangeWindow::new(0, 3_000))
            .unwrap();

    assert_eq!(summary.total_tokens, 200);
    assert_eq!(summary.total_cost_usd, Some(0.4));
    assert_eq!(summary.event_count, 2);
    assert!(summary.has_cost);
    assert!(summary.has_no_cost);
}

#[test]
fn overview_summary_reads_buckets_not_raw_events() {
    let db = Database::open_in_memory().unwrap();
    seed_events_in_generation(&db, "gen-1");

    let range = RangeWindow::new(0, 10_000);
    let summary = read_queries::read_overview_summary(db.conn(), "gen-1", &range).unwrap();

    assert_eq!(
        summary.total_tokens, 0,
        "summary should not scan usage_events"
    );
    assert_eq!(summary.event_count, 0);
}

#[test]
fn overview_summary_reports_partial_cost_from_bucket_rows() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_hour
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_usd,
              cost_status, event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-1', 0, 'claude_code', 'priced', 100, 0.25, 'exact', 1, 1000, 1000),
                    ('gen-1', 0, 'claude_code', 'unpriced', 200, NULL, 'unavailable', 1, 1000, 1000)",
            [],
        )
        .unwrap();

    let summary =
        read_queries::read_overview_summary(db.conn(), "gen-1", &RangeWindow::new(0, 3_600_000))
            .unwrap();

    assert_eq!(summary.total_tokens, 300);
    assert_eq!(summary.event_count, 2);
    assert_eq!(summary.total_cost_usd, Some(0.25));
    assert!(summary.has_cost);
    assert!(summary.has_no_cost);
}

#[test]
fn overview_summary_reports_partial_cost_from_partial_bucket_status() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_hour
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_usd,
              cost_status, event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-1', 0, 'claude_code', 'mixed', 300, 0.25, 'partial', 2, 1000, 1000)",
            [],
        )
        .unwrap();

    let summary =
        read_queries::read_overview_summary(db.conn(), "gen-1", &RangeWindow::new(0, 3_600_000))
            .unwrap();

    assert_eq!(summary.total_tokens, 300);
    assert!(summary.has_cost);
    assert!(summary.has_no_cost);
}

#[test]
fn overview_summary_uses_day_buckets_for_day_aligned_long_ranges() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_hour
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_status,
              event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-1', 0, 'claude_code', 'hour-row', 999, 'unavailable', 9, 1000, 1000)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_day
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_status,
              event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-1', 0, 'claude_code', 'day-row', 123, 'unavailable', 4, 1000, 1000)",
            [],
        )
        .unwrap();

    let forty_days_ms = 40 * 86_400_000;
    let summary = read_queries::read_overview_summary(
        db.conn(),
        "gen-1",
        &RangeWindow::new(0, forty_days_ms),
    )
    .unwrap();

    assert_eq!(summary.total_tokens, 123);
    assert_eq!(summary.event_count, 4);
}

#[test]
fn overview_summary_uses_hour_buckets_for_non_day_aligned_long_ranges() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_hour
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_status,
              event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-1', 3600000, 'claude_code', 'hour-row', 999, 'unavailable', 9, 1000, 1000)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_day
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_status,
              event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-1', 0, 'claude_code', 'day-row', 123, 'unavailable', 4, 1000, 1000)",
            [],
        )
        .unwrap();

    let forty_days_ms = 40 * 86_400_000;
    let summary = read_queries::read_overview_summary(
        db.conn(),
        "gen-1",
        &RangeWindow::new(3_600_000, forty_days_ms + 3_600_000),
    )
    .unwrap();

    assert_eq!(summary.total_tokens, 999);
    assert_eq!(summary.event_count, 9);
}

// ── Live window exact ────────────────────────────────────────────────────────

#[test]
fn read_live_window_exact_returns_empty_for_no_data() {
    let db = Database::open_in_memory().unwrap();
    let rows = read_queries::read_live_window_exact(db.conn(), "gen-1", 0, 10000).unwrap();
    assert!(rows.is_empty());
}

#[test]
fn read_live_window_exact_returns_bucket_rows() {
    let db = Database::open_in_memory().unwrap();
    // Manually seed a 2s bucket
    let now = now_ms();
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_2s (\
                bucket_start_ms, agent, model, generation_id, \
                input_tokens, output_tokens, total_tokens, \
                cost_usd, cost_status, event_count, \
                created_at_ms, updated_at_ms\
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                2000i64,
                "claude_code",
                "claude-sonnet",
                "gen-1",
                100i64,
                200i64,
                300i64,
                0.05f64,
                "exact",
                2i64,
                now,
                now,
            ],
        )
        .unwrap();

    let rows = read_queries::read_live_window_exact(db.conn(), "gen-1", 0, 10000).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].start_ms, 2000);
    assert_eq!(rows[0].tokens, 300);
    assert_eq!(rows[0].event_count, 2);
    assert!(rows[0].has_cost);
}

// ── Materialized aggregate lists ─────────────────────────────────────────────

#[test]
fn breakdown_project_list_reads_dimension_table() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_by_project_day
             (generation_id, date, project_id, project_path, agent, model,
              total_tokens, cost_usd, event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES ('gen-1', '2026-05-28', 'p1', '/repo/p1', 'claude_code', 'sonnet',
                     1200, 0.2, 3, 1000, 1000, 1000)",
            [],
        )
        .unwrap();

    let rows = read_queries::read_breakdown_list(
        db.conn(),
        "gen-1",
        BreakdownDimension::Project,
        "2026-05-01",
        "2026-06-01",
        50,
        None,
    )
    .unwrap();

    assert_eq!(rows.items[0].group_key, "p1");
    assert_eq!(rows.items[0].label.as_deref(), Some("/repo/p1"));
    assert_eq!(rows.items[0].total_tokens, 1200);
    assert_eq!(rows.items[0].event_count, 3);
}

#[test]
fn breakdown_project_list_reports_partial_cost_status() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_by_project_day
             (generation_id, date, project_id, project_path, agent, model,
              total_tokens, cost_usd, cost_status, event_count, last_active_at_ms,
              created_at_ms, updated_at_ms)
             VALUES ('gen-1', '2026-05-28', 'p1', '/repo/p1', 'claude_code', 'sonnet',
                     1200, 0.2, 'partial', 3, 1000, 1000, 1000)",
            [],
        )
        .unwrap();

    let rows = read_queries::read_breakdown_list(
        db.conn(),
        "gen-1",
        BreakdownDimension::Project,
        "2026-05-01",
        "2026-06-01",
        50,
        None,
    )
    .unwrap();

    assert!(rows.items[0].has_cost);
    assert!(rows.items[0].has_no_cost);
}

#[test]
fn breakdown_project_list_cursor_returns_second_page_with_collision_safe_cursor() {
    let db = Database::open_in_memory().unwrap();
    for (project, tokens) in [("p3", 300), ("p:2", 200), ("p1", 100)] {
        db.conn()
            .execute(
                "INSERT INTO usage_by_project_day
                 (generation_id, date, project_id, project_path, agent, model,
                  total_tokens, event_count, last_active_at_ms, created_at_ms, updated_at_ms)
                 VALUES ('gen-1', '2026-05-28', ?1, ?1, 'claude_code', 'sonnet',
                         ?2, 1, ?2, 1000, 1000)",
                rusqlite::params![project, tokens],
            )
            .unwrap();
    }

    let first = read_queries::read_breakdown_list(
        db.conn(),
        "gen-1",
        BreakdownDimension::Project,
        "2026-05-01",
        "2026-06-01",
        1,
        None,
    )
    .unwrap();
    assert_eq!(first.items[0].group_key, "p3");
    let cursor = first.next_cursor.expect("expected cursor");
    assert!(
        !cursor.contains("p:2"),
        "cursor should encode group keys instead of embedding raw separators"
    );

    let second = read_queries::read_breakdown_list(
        db.conn(),
        "gen-1",
        BreakdownDimension::Project,
        "2026-05-01",
        "2026-06-01",
        1,
        Some(cursor),
    )
    .unwrap();
    assert_eq!(second.items[0].group_key, "p:2");
}

#[test]
fn rankings_models_projects_and_sessions_read_dimension_tables() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_by_model_day
             (generation_id, date, agent, model,
              total_tokens, cost_usd, event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES ('gen-1', '2026-05-28', 'claude_code', 'sonnet',
                     950, 0.5, 5, 1000, 1000, 1000)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_by_project_day
             (generation_id, date, project_id, project_path, agent, model,
              total_tokens, cost_usd, event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES ('gen-1', '2026-05-28', 'p1', '/repo/p1', 'claude_code', 'sonnet',
                     900, 0.4, 4, 1000, 1000, 1000)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_by_session_day
             (generation_id, date, session_id, agent, model,
              total_tokens, cost_usd, event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES ('gen-1', '2026-05-28', 's1', 'claude_code', 'sonnet',
                     700, 0.3, 2, 1000, 1000, 1000)",
            [],
        )
        .unwrap();

    let start_ms = utc_midnight_ms("2026-05-01");
    let end_ms = utc_midnight_ms("2026-06-01");

    let models =
        read_queries::read_overview_rankings_models(db.conn(), "gen-1", start_ms, end_ms, 5)
            .unwrap();
    let projects =
        read_queries::read_overview_rankings_projects(db.conn(), "gen-1", start_ms, end_ms, 5)
            .unwrap();
    let sessions =
        read_queries::read_overview_rankings_sessions(db.conn(), "gen-1", start_ms, end_ms, 5)
            .unwrap();

    assert_eq!(models[0].group_key, "sonnet");
    assert_eq!(models[0].total_tokens, 950);
    assert_eq!(projects[0].group_key, "p1");
    assert_eq!(projects[0].label.as_deref(), Some("/repo/p1"));
    assert_eq!(projects[0].total_tokens, 900);
    assert_eq!(sessions[0].group_key, "s1");
    assert_eq!(sessions[0].total_tokens, 700);
}

#[test]
fn rankings_exact_range_use_raw_events_for_non_utc_aligned_ranges() {
    let db = Database::open_in_memory().unwrap();

    for (id, timestamp_ms, project_hash, project_path, model, tokens, cost) in [
        (
            "rank-edge",
            1_748_361_600_000i64,
            Some("proj-edge"),
            Some("/repo/edge"),
            Some("claude-sonnet-4"),
            900,
            Some(0.9),
        ),
        (
            "rank-midday",
            1_748_404_800_000i64,
            Some("proj-midday"),
            Some("/repo/midday"),
            Some("claude-haiku-4"),
            300,
            Some(0.3),
        ),
    ] {
        let mut event = NormalizedUsageEvent::minimal_for_test(id, AgentKind::ClaudeCode);
        event.timestamp_ms = timestamp_ms;
        event.total_tokens = tokens;
        event.cost_usd = cost;
        event.project_hash = project_hash.map(ToString::to_string);
        event.project_path = project_path.map(ToString::to_string);
        event.model = model.map(ToString::to_string);
        db.write_usage_event(&event, busytok_domain::UsageWritePolicy::InsertOnce)
            .unwrap();
        db.conn()
            .execute(
                "UPDATE usage_events SET generation_id = 'gen-1', dedupe_key = ?1 WHERE id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
    }

    db.conn()
        .execute(
            "INSERT INTO usage_by_project_day
             (generation_id, date, project_id, project_path, agent, model,
              total_tokens, cost_usd, event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES ('gen-1', '2025-05-28', 'proj-midday', '/repo/midday', 'claude_code', 'claude-haiku-4',
                     300, 0.3, 1, 1748404800000, 1000, 1000)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_by_model_day
             (generation_id, date, agent, model,
              total_tokens, cost_usd, event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES ('gen-1', '2025-05-28', 'claude_code', 'claude-haiku-4',
                     300, 0.3, 1, 1748404800000, 1000, 1000)",
            [],
        )
        .unwrap();

    let start_ms = 1_748_361_600_000i64;
    let end_ms = 1_748_448_000_000i64;

    let projects =
        read_queries::read_overview_rankings_projects(db.conn(), "gen-1", start_ms, end_ms, 5)
            .unwrap();
    let models =
        read_queries::read_overview_rankings_models(db.conn(), "gen-1", start_ms, end_ms, 5)
            .unwrap();

    assert_eq!(projects[0].group_key, "proj-edge");
    assert_eq!(projects[0].total_tokens, 900);
    assert_eq!(models[0].group_key, "claude-sonnet-4");
    assert_eq!(models[0].total_tokens, 900);
}

#[test]
fn read_overview_rankings_models_by_cost_sorts_by_cost_not_tokens() {
    let db = Database::open_in_memory().unwrap();
    // Model A: high tokens, low cost; Model B: low tokens, high cost.
    // B should rank first when ordering by cost.
    db.conn()
        .execute(
            "INSERT INTO usage_by_model_day
             (generation_id, date, agent, model,
              total_tokens, cost_usd, cost_status, event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES
             ('gen-1', '2026-05-28', 'claude_code', 'high-tok-low-cost',
              1000, 0.1, 'exact', 1, 1000, 1000, 1000),
             ('gen-1', '2026-05-28', 'claude_code', 'low-tok-high-cost',
              100, 1.0, 'exact', 1, 1000, 1000, 1000)",
            [],
        )
        .unwrap();

    let start_ms = utc_midnight_ms("2026-05-01");
    let end_ms = utc_midnight_ms("2026-06-01");

    let rows = read_queries::read_overview_rankings_models_by_cost(
        db.conn(),
        "gen-1",
        start_ms,
        end_ms,
        5,
    )
    .unwrap();

    assert_eq!(rows[0].group_key, "low-tok-high-cost");
    assert_eq!(rows[0].total_cost_usd, Some(1.0));
    assert_eq!(rows[1].group_key, "high-tok-low-cost");
    assert_eq!(rows[1].total_cost_usd, Some(0.1));
    assert!(rows[0].has_cost);
    assert!(!rows[0].has_no_cost);
}

#[test]
fn read_overview_rankings_models_by_cost_marks_partial_when_mixed_cost_status() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_by_model_day
             (generation_id, date, agent, model,
              total_tokens, cost_usd, cost_status, event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES
             ('gen-1', '2026-05-28', 'claude_code', 'partial-model',
              500, 0.5, 'exact', 3, 1000, 1000, 1000),
             ('gen-1', '2026-05-29', 'claude_code', 'partial-model',
              200, NULL, 'unavailable', 1, 1000, 1000, 1000)",
            [],
        )
        .unwrap();

    let start_ms = utc_midnight_ms("2026-05-01");
    let end_ms = utc_midnight_ms("2026-06-01");

    let rows = read_queries::read_overview_rankings_models_by_cost(
        db.conn(),
        "gen-1",
        start_ms,
        end_ms,
        5,
    )
    .unwrap();

    assert_eq!(rows.len(), 1);
    assert!(rows[0].has_cost);
    assert!(rows[0].has_no_cost);
}

#[test]
fn read_rankings_exact_range_zero_cost_events_not_marked_partial() {
    let db = Database::open_in_memory().unwrap();

    // Use a non-UTC-aligned range so raw_suffix covers a partial-day window.
    // start = 2026-05-28T00:00:00Z, end = 2026-05-28T12:00:00Z (noon).
    // materialized = empty (same-day), raw_suffix = 00:00–12:00.
    let day_start = utc_midnight_ms("2026-05-28");
    let half_day = day_start + 12 * 3_600_000;

    // Insert events with cost_usd = 0.0 (known-zero cost, e.g. zero-token
    // events where pricing was applied). These must NOT be treated as
    // "no cost available".
    db.conn()
        .execute(
            "INSERT INTO usage_events
             (id, agent, source_file_id, source_path, source_line,
              source_offset_start, source_offset_end, session_id,
              timestamp_ms, model, total_tokens, cost_usd, cost_source,
              raw_event_hash, created_at_ms, updated_at_ms, generation_id, dedupe_key)
             VALUES
             ('evt-1', 'codex', 'f1', '/tmp/a.jsonl', 1, 0, 50, 'sess-1',
              ?1, 'glm-5.1', 0, 0.0, 'estimated',
              'hash1', 1000, 1000, 'gen-1', 'dk-1'),
             ('evt-2', 'codex', 'f1', '/tmp/a.jsonl', 2, 51, 100, 'sess-1',
              ?1, 'glm-5.1', 500, 0.05, 'estimated',
              'hash2', 1000, 1000, 'gen-1', 'dk-2')",
            rusqlite::params![day_start + 3_600_000], // 01:00 UTC — inside suffix window
        )
        .unwrap();

    let rows = read_queries::read_overview_rankings_models_by_cost(
        db.conn(),
        "gen-1",
        day_start,
        half_day,
        5,
    )
    .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].group_key, "glm-5.1");
    assert!(
        rows[0].has_cost,
        "cost_usd=0 events should count as 'has cost'"
    );
    assert!(
        !rows[0].has_no_cost,
        "cost_usd=0 with known pricing must not be 'no cost'"
    );
}

fn utc_midnight_ms(date: &str) -> i64 {
    Date::parse(
        date,
        &time::format_description::parse("[year]-[month]-[day]").unwrap(),
    )
    .unwrap()
    .with_time(Time::MIDNIGHT)
    .assume_offset(UtcOffset::UTC)
    .unix_timestamp()
        * 1000
}

#[test]
fn source_and_client_summaries_read_summary_tables() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO source_health_summary
             (generation_id, source_id, agent, root_path, source_type, status,
              configured_by_user, last_scan_at_ms, file_count, parsed_file_count,
              event_count, last_error, latest_activity_at_ms, created_at_ms, updated_at_ms)
             VALUES ('gen-1', 'src-1', 'claude_code', '/repo/a', 'default', 'active',
                     1, 2000, 10, 8, 11, NULL, 3000, 1000, 1000),
                    ('gen-1', 'src-2', 'codex', '/repo/b', 'custom', 'active',
                     0, 4000, 5, 5, 13, 'boom', 5000, 1000, 1000)",
            [],
        )
        .unwrap();

    let page = read_queries::read_source_health_summaries(db.conn(), "gen-1", 1, None, None, None)
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].source_id, "src-2");

    let rollups = read_queries::read_client_rollups(db.conn(), "gen-1").unwrap();
    assert_eq!(rollups.len(), 2);
    assert_eq!(rollups[0].client_kind, "codex");
    assert_eq!(rollups[0].active_source_count, 1);
    assert_eq!(rollups[0].event_count, 13);
}

#[test]
fn overview_heatmap_formats_leap_year_dates() {
    let db = Database::open_in_memory().unwrap();
    let leap_day_start_ms = 1_709_164_800_000i64; // 2024-02-29T00:00:00Z
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_day
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_status,
              event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-1', ?1, 'claude_code', 'sonnet', 42, 'unavailable', 1, 1000, 1000)",
            rusqlite::params![leap_day_start_ms],
        )
        .unwrap();

    let rows = read_queries::read_overview_heatmap(
        db.conn(),
        "gen-1",
        leap_day_start_ms,
        leap_day_start_ms + 86_400_000,
    )
    .unwrap();

    assert_eq!(rows[0].date, "2024-02-29");
    assert_eq!(rows[0].tokens, 42);
}

#[test]
fn read_overview_trend_hourly_returns_hour_bucket_rows() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_hour
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_usd, cost_status,
              event_count, created_at_ms, updated_at_ms)
             VALUES
             ('gen-1', 0, 'claude_code', 'sonnet', 120, 0.4, 'exact', 2, 1000, 1000),
             ('gen-1', 3_600_000, 'codex', 'gpt-5', 80, NULL, 'unavailable', 1, 1000, 1000),
             ('gen-2', 0, 'codex', 'gpt-5', 999, 1.0, 'exact', 9, 1000, 1000)",
            [],
        )
        .unwrap();

    let rows = read_queries::read_overview_trend_hourly(db.conn(), "gen-1", 0, 7_200_000).unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].start_ms, 0);
    assert_eq!(rows[0].tokens, 120);
    assert!(rows[0].has_cost);
    assert_eq!(rows[1].start_ms, 3_600_000);
    assert_eq!(rows[1].tokens, 80);
    assert!(rows[1].has_no_cost);
}

#[test]
fn read_overview_window_aggregates_exact_returns_requested_windows() {
    let db = Database::open_in_memory().unwrap();

    let mut priced =
        NormalizedUsageEvent::minimal_for_test("evt-window-priced", AgentKind::ClaudeCode);
    priced.timestamp_ms = 1_000;
    priced.total_tokens = 120;
    priced.cost_usd = Some(0.4);
    db.write_usage_event(&priced, busytok_domain::UsageWritePolicy::InsertOnce)
        .unwrap();
    db.conn()
        .execute(
            "UPDATE usage_events
             SET generation_id = 'gen-1', dedupe_key = 'evt-window-priced'
             WHERE id = 'evt-window-priced'",
            [],
        )
        .unwrap();

    let mut unpriced =
        NormalizedUsageEvent::minimal_for_test("evt-window-unpriced", AgentKind::ClaudeCode);
    unpriced.timestamp_ms = 1_500;
    unpriced.total_tokens = 80;
    unpriced.cost_usd = None;
    db.write_usage_event(&unpriced, busytok_domain::UsageWritePolicy::InsertOnce)
        .unwrap();
    db.conn()
        .execute(
            "UPDATE usage_events
             SET generation_id = 'gen-1', dedupe_key = 'evt-window-unpriced'
             WHERE id = 'evt-window-unpriced'",
            [],
        )
        .unwrap();

    let rows = read_queries::read_overview_window_aggregates_exact(
        db.conn(),
        "gen-1",
        &[
            OverviewExactWindow {
                key: "filled".to_string(),
                start_ms: 0,
                end_ms: 2_000,
            },
            OverviewExactWindow {
                key: "empty".to_string(),
                start_ms: 2_000,
                end_ms: 3_000,
            },
        ],
    )
    .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].key, "filled");
    assert_eq!(rows[0].tokens, 200);
    assert_eq!(rows[0].cost_usd, Some(0.4));
    assert_eq!(rows[0].event_count, 2);
    assert!(rows[0].has_cost);
    assert!(rows[0].has_no_cost);
    assert_eq!(rows[1].key, "empty");
    assert_eq!(rows[1].tokens, 0);
    assert_eq!(rows[1].cost_usd, None);
    assert_eq!(rows[1].event_count, 0);
    assert!(!rows[1].has_cost);
    assert!(!rows[1].has_no_cost);
}

#[test]
fn read_overview_window_aggregates_exact_chunks_and_respects_generation_scope() {
    let db = Database::open_in_memory().unwrap();

    let mut first =
        NormalizedUsageEvent::minimal_for_test("evt-window-first", AgentKind::ClaudeCode);
    first.timestamp_ms = 500;
    first.total_tokens = 120;
    first.cost_usd = Some(0.4);
    db.write_usage_event(&first, busytok_domain::UsageWritePolicy::InsertOnce)
        .unwrap();
    db.conn()
        .execute(
            "UPDATE usage_events
             SET generation_id = 'gen-1', dedupe_key = 'evt-window-first'
             WHERE id = 'evt-window-first'",
            [],
        )
        .unwrap();

    let mut last = NormalizedUsageEvent::minimal_for_test("evt-window-last", AgentKind::ClaudeCode);
    last.timestamp_ms = 304_500;
    last.total_tokens = 80;
    last.cost_usd = None;
    db.write_usage_event(&last, busytok_domain::UsageWritePolicy::InsertOnce)
        .unwrap();
    db.conn()
        .execute(
            "UPDATE usage_events
             SET generation_id = 'gen-1', dedupe_key = 'evt-window-last'
             WHERE id = 'evt-window-last'",
            [],
        )
        .unwrap();

    let mut other_generation = NormalizedUsageEvent::minimal_for_test(
        "evt-window-other-generation",
        AgentKind::ClaudeCode,
    );
    other_generation.timestamp_ms = 150_500;
    other_generation.total_tokens = 999;
    other_generation.cost_usd = Some(9.9);
    db.write_usage_event(
        &other_generation,
        busytok_domain::UsageWritePolicy::InsertOnce,
    )
    .unwrap();
    db.conn()
        .execute(
            "UPDATE usage_events
             SET generation_id = 'gen-2', dedupe_key = 'evt-window-other-generation'
             WHERE id = 'evt-window-other-generation'",
            [],
        )
        .unwrap();

    let windows = (0..305)
        .map(|idx| OverviewExactWindow {
            key: format!("window-{idx:03}"),
            start_ms: idx * 1_000,
            end_ms: (idx + 1) * 1_000,
        })
        .collect::<Vec<_>>();

    let rows =
        read_queries::read_overview_window_aggregates_exact(db.conn(), "gen-1", &windows).unwrap();

    assert_eq!(rows.len(), 305);
    assert_eq!(rows[0].key, "window-000");
    assert_eq!(rows[0].tokens, 120);
    assert_eq!(rows[0].cost_usd, Some(0.4));
    assert_eq!(rows[0].event_count, 1);
    assert!(rows[0].has_cost);
    assert!(!rows[0].has_no_cost);

    assert_eq!(rows[150].key, "window-150");
    assert_eq!(rows[150].tokens, 0);
    assert_eq!(rows[150].event_count, 0);

    assert_eq!(rows[304].key, "window-304");
    assert_eq!(rows[304].tokens, 80);
    assert_eq!(rows[304].cost_usd, None);
    assert_eq!(rows[304].event_count, 1);
    assert!(!rows[304].has_cost);
    assert!(rows[304].has_no_cost);
}

// ── Activity list ────────────────────────────────────────────────────────────

#[test]
fn read_activity_list_returns_events_in_desc_order() {
    let db = Database::open_in_memory().unwrap();
    seed_events_in_generation(&db, "gen-1");

    let rows = read_queries::read_activity_list(db.conn(), "gen-1", 0, 10000, 10, None).unwrap();
    assert_eq!(rows.len(), 3);
    // Ordered by timestamp_ms DESC
    assert_eq!(rows[0].happened_at_ms, 3000);
    assert_eq!(rows[1].happened_at_ms, 2000);
    assert_eq!(rows[2].happened_at_ms, 1000);
}

#[test]
fn read_activity_list_respects_limit() {
    let db = Database::open_in_memory().unwrap();
    seed_events_in_generation(&db, "gen-1");

    let rows = read_queries::read_activity_list(db.conn(), "gen-1", 0, 10000, 2, None).unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn read_activity_source_info_returns_joined_source_metadata() {
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO log_sources
             (id, agent, source_type, root_path, configured_by_user, default_discovery_enabled,
              status, first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms)
             VALUES ('src-1', 'claude_code', 'session_root', '/repo/demo', 1, 0,
                     'active', 1000, 1000, 1000, 1000)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO log_files
             (id, source_id, agent, path, inode, offset_bytes, state,
              first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms)
             VALUES ('file-1', 'src-1', 'claude_code', '/repo/demo/session.jsonl', NULL, 0, 'active',
                     1000, 1000, 1000, 1000)",
            [],
        )
        .unwrap();

    let source = read_queries::read_activity_source_info(db.conn(), "file-1").unwrap();

    let source = source.expect("source metadata should exist");
    assert_eq!(source.source_id, "src-1");
    assert_eq!(source.agent, "claude_code");
    assert_eq!(source.root_path, "/repo/demo");
}

#[test]
fn read_activity_list_maps_input_and_cached_tokens() {
    let db = Database::open_in_memory().unwrap();
    // Write an event with non-zero input and cached tokens via NormalizedUsageEvent
    let mut event = NormalizedUsageEvent::minimal_for_test("evt-1", AgentKind::ClaudeCode);
    event.timestamp_ms = 2000;
    event.total_tokens = 500;
    event.input_tokens = 400;
    event.cached_input_tokens = 120;
    event.model = Some("claude-sonnet-4".to_string());
    db.write_usage_event(&event, busytok_domain::UsageWritePolicy::InsertOnce)
        .unwrap();
    db.conn()
        .execute(
            "UPDATE usage_events SET generation_id = ?, dedupe_key = ? WHERE id = ?",
            rusqlite::params!["gen-1", "evt-1", "evt-1"],
        )
        .unwrap();

    let rows = read_queries::read_activity_list(db.conn(), "gen-1", 0, 10000, 10, None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].total_tokens, 500);
    assert_eq!(rows[0].input_tokens, 400);
    assert_eq!(rows[0].cached_input_tokens, 120);
}

#[test]
fn read_activity_list_maps_zero_tokens_correctly() {
    let db = Database::open_in_memory().unwrap();
    let mut event = NormalizedUsageEvent::minimal_for_test("evt-1", AgentKind::ClaudeCode);
    event.timestamp_ms = 2000;
    event.total_tokens = 100;
    event.input_tokens = 0;
    event.cached_input_tokens = 0;
    event.model = Some("claude-sonnet-4".to_string());
    db.write_usage_event(&event, busytok_domain::UsageWritePolicy::InsertOnce)
        .unwrap();
    db.conn()
        .execute(
            "UPDATE usage_events SET generation_id = ?, dedupe_key = ? WHERE id = ?",
            rusqlite::params!["gen-1", "evt-1", "evt-1"],
        )
        .unwrap();

    let rows = read_queries::read_activity_list(db.conn(), "gen-1", 0, 10000, 10, None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].input_tokens, 0);
    assert_eq!(rows[0].cached_input_tokens, 0);
}

#[test]
fn read_client_source_recent_activity_maps_input_and_cached_tokens() {
    let db = Database::open_in_memory().unwrap();

    db.conn()
        .execute(
            "INSERT INTO log_sources
             (id, agent, source_type, root_path, configured_by_user, default_discovery_enabled,
              status, first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms)
             VALUES ('src-1', 'claude_code', 'session_root', '/repo/demo', 1, 0,
                     'active', 1000, 1000, 1000, 1000)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO log_files
             (id, source_id, agent, path, inode, offset_bytes, state,
              first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms)
             VALUES ('file-1', 'src-1', 'claude_code', '/repo/demo/session.jsonl', NULL, 0, 'active',
                     1000, 1000, 1000, 1000)",
            [],
        )
        .unwrap();

    let mut event = NormalizedUsageEvent::minimal_for_test("evt-1", AgentKind::ClaudeCode);
    event.timestamp_ms = 2000;
    event.source_file_id = "file-1".to_string();
    event.total_tokens = 300;
    event.input_tokens = 200;
    event.cached_input_tokens = 60;
    event.model = Some("claude-sonnet-4".to_string());
    db.write_usage_event(&event, busytok_domain::UsageWritePolicy::InsertOnce)
        .unwrap();

    let rows = read_queries::read_client_source_recent_activity(db.conn(), "src-1", 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].input_tokens, 200);
    assert_eq!(rows[0].cached_input_tokens, 60);
}
