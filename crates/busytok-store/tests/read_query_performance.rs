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
use std::time::{Duration, Instant};

use busytok_domain::{AgentKind, NormalizedUsageEvent};
use busytok_store::db::Database;
use busytok_store::read_models::{BreakdownDimension, RangeWindow};
use busytok_store::{read_queries, write_queries};
use rusqlite::params;
use time::OffsetDateTime;

const GENERATION_ID: &str = "gen-perf";
const MILLIS_PER_DAY: i64 = 86_400_000;
const RECENT_DAY_START_MS: i64 = 1_764_806_400_000;
const SOURCE_COUNT: usize = 120;

fn utc_date(epoch_ms: i64) -> String {
    OffsetDateTime::from_unix_timestamp(epoch_ms.div_euclid(1000))
        .unwrap()
        .date()
        .to_string()
}

fn usage_range() -> RangeWindow {
    RangeWindow::new(
        RECENT_DAY_START_MS - (365 * MILLIS_PER_DAY),
        RECENT_DAY_START_MS + MILLIS_PER_DAY,
    )
}

fn project_date_range(range: &RangeWindow) -> (String, String) {
    (utc_date(range.start_ms), utc_date(range.end_ms))
}

fn source_agent(index: usize) -> &'static str {
    match index % 2 {
        0 => "claude_code",
        _ => "codex",
    }
}

fn source_client_kind(index: usize) -> &'static str {
    match index % 2 {
        0 => "claude_code",
        _ => "codex_cli",
    }
}

fn event_agent(index: usize) -> AgentKind {
    match index % 2 {
        0 => AgentKind::ClaudeCode,
        _ => AgentKind::Codex,
    }
}

fn seeded_event(index: usize) -> NormalizedUsageEvent {
    let agent = event_agent(index);
    let mut event = NormalizedUsageEvent::minimal_for_test(&format!("evt-{index:06}"), agent);
    let recent_offset_ms = (index % 86_400) as i64 * 1000;
    let historical_day_offset_ms = (index % 365) as i64 * MILLIS_PER_DAY;
    let historical_second_offset_ms = ((index / 17) % 86_400) as i64 * 1000;
    event.timestamp_ms = if index % 4 == 0 {
        RECENT_DAY_START_MS + recent_offset_ms
    } else {
        RECENT_DAY_START_MS - historical_day_offset_ms + historical_second_offset_ms
    };
    event.total_tokens = 50 + (index % 500) as i64;
    event.input_tokens = event.total_tokens / 2;
    event.output_tokens = event.total_tokens - event.input_tokens;
    event.cached_input_tokens = (index % 21) as i64;
    event.reasoning_tokens = (index % 13) as i64;
    event.cost_usd = if index % 5 == 0 {
        None
    } else {
        Some(((index % 20) as f64 + 1.0) / 10_000.0)
    };
    event.project_hash = Some(format!("project-{}", index % 100));
    event.project_path = Some(format!("/workspace/project-{}", index % 100));
    event.model = Some(format!("model-{}", index % 50));
    event.session_id = format!("session-{}", index % 10_000);
    event.client_kind = Some(source_client_kind(index).to_string());
    event.source_file_id = format!("file-{}", index % SOURCE_COUNT);
    event.source_path = format!(
        "/logs/{}/source-{}.jsonl",
        source_agent(index),
        index % SOURCE_COUNT
    );
    event
}

fn seed_usage_events(db: &Database, usage_count: usize) {
    let mut batch = Vec::with_capacity(1000);
    for index in 0..usage_count {
        batch.push(seeded_event(index));
        if batch.len() == 1000 {
            write_queries::insert_usage_events_batch(db.conn(), &batch, GENERATION_ID).unwrap();
            write_queries::update_materialized_aggregates_from_events(
                db.conn(),
                &batch,
                GENERATION_ID,
            )
            .unwrap();
            batch.clear();
        }
    }

    if !batch.is_empty() {
        write_queries::insert_usage_events_batch(db.conn(), &batch, GENERATION_ID).unwrap();
        write_queries::update_materialized_aggregates_from_events(db.conn(), &batch, GENERATION_ID)
            .unwrap();
    }
}

fn seed_source_health_summaries(db: &Database, usage_count: usize) {
    let event_count_per_source = (usage_count / SOURCE_COUNT).max(1) as i64;
    for index in 0..SOURCE_COUNT {
        let source_id = format!("source-{}", index);
        let agent = source_agent(index);
        let status = match index % 8 {
            0 => "error",
            1 | 2 => "warning",
            3 | 4 => "active",
            _ => "idle",
        };
        let last_scan_at_ms = RECENT_DAY_START_MS + index as i64 * 60_000;
        let latest_activity_at_ms = last_scan_at_ms + 30_000;
        db.conn()
            .execute(
                "INSERT INTO source_health_summary
                 (generation_id, source_id, agent, root_path, source_type, status,
                  configured_by_user, last_scan_at_ms, file_count, parsed_file_count,
                  event_count, last_error, latest_activity_at_ms, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 'default', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
                params![
                    GENERATION_ID,
                    source_id,
                    agent,
                    format!("/workspace/roots/{}", index % 12),
                    status,
                    if index % 2 == 0 { 1 } else { 0 },
                    last_scan_at_ms,
                    4 + (index % 7) as i64,
                    3 + (index % 5) as i64,
                    event_count_per_source + index as i64,
                    if status == "error" {
                        Some(format!("source-{index} failed"))
                    } else {
                        None
                    },
                    latest_activity_at_ms,
                    RECENT_DAY_START_MS,
                ],
            )
            .unwrap();
    }
}

fn seed_diagnostics(db: &Database, diagnostic_count: usize) {
    let mut stmt = db
        .conn()
        .prepare(
            "INSERT INTO diagnostic_events
             (id, agent, source_id, source_file_id, source_path, source_line, severity, code,
              message, details_json, happened_at_ms, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, NULL, ?9, ?9)",
        )
        .unwrap();

    for index in 0..diagnostic_count {
        let severity = match index % 10 {
            0 => "error",
            1 | 2 => "warning",
            _ => "info",
        };
        let source_index = index % SOURCE_COUNT;
        let happened_at_ms = RECENT_DAY_START_MS - ((index % 365) as i64 * MILLIS_PER_DAY)
            + (index % 86_400) as i64 * 1000;
        stmt.execute(params![
            format!("diag-{index:06}"),
            source_agent(source_index),
            format!("source-{}", source_index),
            format!("file-{}", source_index),
            format!(
                "/logs/{}/source-{}.jsonl",
                source_agent(source_index),
                source_index
            ),
            severity,
            format!("code-{}", index % 8),
            format!("diagnostic event {}", index),
            happened_at_ms,
        ])
        .unwrap();
    }
}

fn seed_perf_fixture(db: &Database, usage_count: usize, diagnostic_count: usize) {
    db.conn().execute_batch("BEGIN IMMEDIATE").unwrap();
    seed_usage_events(db, usage_count);
    seed_source_health_summaries(db, usage_count);
    seed_diagnostics(db, diagnostic_count);
    db.conn().execute_batch("COMMIT").unwrap();
}

struct QueryBudgets {
    summary: Duration,
    trend: Duration,
    hourly: Duration,
    heatmap: Duration,
    rankings: Duration,
    clients: Duration,
    breakdown: Duration,
    facts: Duration,
}

fn record_budget<T, F>(label: &str, budget: Duration, failures: &mut Vec<String>, query: F) -> T
where
    F: FnOnce() -> T,
{
    let started = Instant::now();
    let result = query();
    let elapsed = started.elapsed();
    eprintln!("{label}: {elapsed:?}");
    if elapsed > budget {
        failures.push(format!("{label} exceeded budget: {elapsed:?} > {budget:?}"));
    }
    result
}

fn assert_query_budgets(db: &Database, budgets: QueryBudgets) {
    let range = usage_range();
    let day_range = RangeWindow::new(RECENT_DAY_START_MS, RECENT_DAY_START_MS + MILLIS_PER_DAY);
    let (start_date, end_date) = project_date_range(&range);
    let mut failures = Vec::new();

    let summary = record_budget("overview.summary", budgets.summary, &mut failures, || {
        read_queries::read_overview_summary(db.conn(), GENERATION_ID, &range).unwrap()
    });
    assert!(summary.event_count > 0);

    let trend = record_budget("overview.trend", budgets.trend, &mut failures, || {
        read_queries::read_overview_trend(db.conn(), GENERATION_ID, range.start_ms, range.end_ms)
            .unwrap()
    });
    assert!(!trend.is_empty());

    let hourly = record_budget(
        "overview.trend.hourly",
        budgets.hourly,
        &mut failures,
        || {
            read_queries::read_overview_trend_hourly(
                db.conn(),
                GENERATION_ID,
                day_range.start_ms,
                day_range.end_ms,
            )
            .unwrap()
        },
    );
    assert!(!hourly.is_empty());

    let heatmap = record_budget("overview.heatmap", budgets.heatmap, &mut failures, || {
        read_queries::read_overview_heatmap(db.conn(), GENERATION_ID, range.start_ms, range.end_ms)
            .unwrap()
    });
    assert!(!heatmap.is_empty());

    let rankings = record_budget(
        "overview.rankings.projects",
        budgets.rankings,
        &mut failures,
        || {
            read_queries::read_overview_rankings_projects(
                db.conn(),
                GENERATION_ID,
                range.start_ms,
                range.end_ms,
                10,
            )
            .unwrap()
        },
    );
    assert!(!rankings.is_empty());

    let client_page = record_budget(
        "clients.snapshot.list",
        budgets.clients,
        &mut failures,
        || {
            read_queries::read_source_health_summaries(
                db.conn(),
                GENERATION_ID,
                50,
                None,
                None,
                None,
            )
            .unwrap()
        },
    );
    assert!(!client_page.items.is_empty());

    let client_totals = record_budget(
        "clients.snapshot.totals",
        budgets.clients,
        &mut failures,
        || {
            read_queries::read_source_health_summary_totals(db.conn(), GENERATION_ID, None, None)
                .unwrap()
        },
    );
    assert!(client_totals.source_count > 0);

    let client_rollups = record_budget(
        "clients.snapshot.rollups",
        budgets.clients,
        &mut failures,
        || read_queries::read_client_rollups(db.conn(), GENERATION_ID).unwrap(),
    );
    assert!(!client_rollups.is_empty());

    let breakdown = record_budget(
        "breakdown.projects.list",
        budgets.breakdown,
        &mut failures,
        || {
            read_queries::read_breakdown_list(
                db.conn(),
                GENERATION_ID,
                BreakdownDimension::Project,
                &start_date,
                &end_date,
                50,
                None,
            )
            .unwrap()
        },
    );
    assert!(!breakdown.items.is_empty());

    let breakdown_totals = record_budget(
        "breakdown.projects.totals",
        budgets.breakdown,
        &mut failures,
        || {
            read_queries::read_breakdown_totals(
                db.conn(),
                GENERATION_ID,
                BreakdownDimension::Project,
                &start_date,
                &end_date,
            )
            .unwrap()
        },
    );
    assert!(breakdown_totals.grouped_count > 0);

    let activity = record_budget("activity.list", budgets.facts, &mut failures, || {
        read_queries::read_activity_list(
            db.conn(),
            GENERATION_ID,
            range.start_ms,
            range.end_ms,
            100,
            None,
        )
        .unwrap()
    });
    assert!(!activity.is_empty());

    let recent = record_budget("activity.recent", budgets.facts, &mut failures, || {
        read_queries::read_activity_recent(
            db.conn(),
            GENERATION_ID,
            day_range.start_ms,
            day_range.end_ms,
            20,
        )
        .unwrap()
    });
    assert!(!recent.is_empty());

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn read_model_queries_stay_under_small_fixture_budget() {
    let db = Database::open_in_memory().unwrap();
    seed_perf_fixture(&db, 10_000, 10_000);
    assert_query_budgets(
        &db,
        QueryBudgets {
            summary: Duration::from_millis(150),
            trend: Duration::from_millis(500),
            hourly: Duration::from_millis(250),
            heatmap: Duration::from_millis(250),
            rankings: Duration::from_millis(500),
            clients: Duration::from_millis(150),
            breakdown: Duration::from_millis(400),
            facts: Duration::from_millis(300),
        },
    );
}

#[test]
#[ignore = "large deterministic performance budget; run before release"]
fn read_model_queries_stay_under_500k_budget() {
    let db = Database::open_in_memory().unwrap();
    seed_perf_fixture(&db, 500_000, 500_000);
    assert_query_budgets(
        &db,
        QueryBudgets {
            summary: Duration::from_secs(2),
            trend: Duration::from_secs(2),
            hourly: Duration::from_secs(2),
            heatmap: Duration::from_secs(2),
            rankings: Duration::from_secs(2),
            clients: Duration::from_secs(2),
            breakdown: Duration::from_secs(2),
            facts: Duration::from_secs(5),
        },
    );
}
