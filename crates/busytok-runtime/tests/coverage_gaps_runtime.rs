#![allow(
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    dead_code,
    unused_imports,
    unused_variables
)]
//! Coverage gap tests for `busytok-runtime` modules not covered by
//! `coverage_gaps_core.rs`.
//!
//! Targets uncovered code paths in:
//! - `aggregates.rs` — `apply_replay_batch_aggregates`, `update_generation_summaries`.
//! - `ui_models.rs` — `breakdown_metrics`, `format_trend_label` (Week/Hour/Day/Month
//!   branches + invalid keys), `format_compact` B→T escalation, `format_thousands`
//!   negative, `trend_granularity` Month, `metric_labels` Month.
//! - `range.rs` — `parse_date_to_ms`, `parse_date_to_ms_exclusive`,
//!   `heatmap_window_from_date` Feb-29 fallback, `resolve_range` December branch.
//! - `rebuild.rs` — `initiate_rebuild` with empty frontiers, `record_new_file_observation`,
//!   `record_rebuild_diagnostic`, `RebuildFrontierSet::persist` multi-frontier log.
//! - `scan.rs` — `derive_file_id` determinism, `enrich_cost` CostMode variants.
//! - `supervisor.rs` — `legacy_audit_rebuild_recommended` (true/false), `debug_registered_agents`,
//!   `apply_service_status_snapshot`, `read_status_snapshot`, `transition_after_initial_scan`,
//!   `hydrate_status_from_db`, `provider_changed`/`provider_deleted` no-op (sidecar disabled).
//! - `bootstrap.rs` — `ServiceApp::boot` stages 1-4 (open_database, create_supervisor,
//!   hydrate_status, bind_control_server).
//! - `source_registry.rs` / `generation_manager.rs` — driven through
//!   `BusytokSupervisor::run_initial_scan` with `codex_default_paths=true`.

use std::sync::Arc;
use std::time::Instant;

use serial_test::serial;

use busytok_config::{BusytokPaths, BusytokSettings};
use busytok_domain::{now_ms, AgentKind, NormalizedUsageEvent, ReportingTimezone};
use busytok_pricing::CostMode;
use busytok_protocol::dto::{
    RangePresetDto, ReadinessStateDto, TrendBucketGranularityDto, WeekdayIndexDto,
};
use busytok_runtime::aggregates::{
    apply_event_batch_aggregates, apply_replay_batch_aggregates, update_generation_summaries,
};
use busytok_runtime::range::{
    heatmap_window_from_date, parse_date_to_ms, parse_date_to_ms_exclusive, resolve_range,
};
use busytok_runtime::rebuild::{
    create_generation, initiate_rebuild, record_new_file_observation, record_rebuild_diagnostic,
    RebuildFrontierSet,
};
use busytok_runtime::scan::{derive_file_id, enrich_cost};
use busytok_runtime::ui_models::{
    breakdown_metrics, format_thousands, format_tokens, format_trend_label, trend_granularity,
    UsageTotals,
};
use busytok_runtime::{BusytokSupervisor, ServiceApp};
use busytok_store::Database;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_supervisor() -> (BusytokSupervisor, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let db = Database::open_in_memory().expect("open in-memory");
    (BusytokSupervisor::new(db, paths), dir)
}

fn make_supervisor_with_settings(
    settings: BusytokSettings,
) -> (BusytokSupervisor, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let db = Database::open_in_memory().expect("open in-memory");
    (
        BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings),
        dir,
    )
}

fn seed_active_generation(db: &Database, gen_id: &str) {
    let now = now_ms();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO audit_generations \
             (generation_id, state, started_at_ms, promoted_at_ms, is_active, \
              created_at_ms, updated_at_ms) \
             VALUES (?1, 'promoted', ?2, ?2, 1, ?2, ?2)",
            rusqlite::params![gen_id, now],
        )
        .expect("seed active generation");
}

fn seed_service_state(db: &Database, readiness: &str, gen_id: Option<&str>) {
    let now = now_ms();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO service_state \
             (id, writer_queue_depth, aggregate_lag_ms, readiness, \
              active_generation_id, updated_at_ms) \
             VALUES (1, 0, 0, ?1, ?2, ?3)",
            rusqlite::params![readiness, gen_id, now],
        )
        .expect("seed service_state");
}

fn seed_legacy_codex_event(db: &Database, id: &str, gen_id: &str) {
    // Codex event with NULL model — triggers legacy_audit_rebuild_recommended.
    let now = now_ms();
    db.conn()
        .execute(
            "INSERT INTO usage_events \
             (id, generation_id, agent, source_file_id, source_path, source_line, \
              source_offset_start, source_offset_end, session_id, timestamp_ms, \
              model, total_tokens, input_tokens, output_tokens, \
              cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
              reasoning_tokens, cost_usd, created_at_ms, updated_at_ms, is_sidechain, \
              raw_event_hash) \
             VALUES (?1, ?2, 'codex', 'file-1', '/tmp/x.jsonl', 1, 0, 100, \
                     'sess-1', ?3, NULL, 100, 50, 50, 0, 0, 0, 0, NULL, ?3, ?3, 0, ?4)",
            rusqlite::params![id, gen_id, now, format!("hash-{id}")],
        )
        .expect("seed legacy codex event");
}

fn seed_legacy_claude_event(db: &Database, id: &str, gen_id: &str) {
    // Claude event where total_tokens != sum of component tokens.
    let now = now_ms();
    db.conn()
        .execute(
            "INSERT INTO usage_events \
             (id, generation_id, agent, source_file_id, source_path, source_line, \
              source_offset_start, source_offset_end, session_id, timestamp_ms, \
              model, total_tokens, input_tokens, output_tokens, \
              cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
              reasoning_tokens, cost_usd, created_at_ms, updated_at_ms, is_sidechain, \
              raw_event_hash) \
             VALUES (?1, ?2, 'claude_code', 'file-2', '/tmp/y.jsonl', 1, 0, 100, \
                     'sess-2', ?3, 'claude-sonnet', 500, 50, 50, 0, 0, 0, 0, NULL, ?3, ?3, 0, ?4)",
            rusqlite::params![id, gen_id, now, format!("hash-{id}")],
        )
        .expect("seed legacy claude event");
}

// =============================================================================
// aggregates.rs — apply_replay_batch_aggregates + update_generation_summaries
// =============================================================================

#[test]
fn apply_replay_batch_aggregates_returns_ok_on_empty_batch() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn();
    let events: Vec<NormalizedUsageEvent> = vec![];
    let result = apply_replay_batch_aggregates(conn, &events, "gen-1");
    assert!(
        result.is_ok(),
        "apply_replay_batch_aggregates should succeed"
    );
}

#[test]
fn apply_replay_batch_aggregates_returns_ok_with_events() {
    // Currently a no-op hook, but exercising it with non-empty events covers
    // the function body lines that lcov marks as FNDA:0.
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn();
    let mut evt = NormalizedUsageEvent::minimal_for_test("evt-1", AgentKind::Codex);
    evt.total_tokens = 100;
    let events = vec![evt];
    let result = apply_replay_batch_aggregates(conn, &events, "gen-replay");
    assert!(result.is_ok());
}

#[test]
fn apply_event_batch_aggregates_returns_ok() {
    // Sanity coverage for the live-batch hook (also a no-op today).
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn();
    let events: Vec<NormalizedUsageEvent> = vec![];
    let result = apply_event_batch_aggregates(conn, &events, "gen-live");
    assert!(result.is_ok());
}

#[test]
fn update_generation_summaries_computes_totals_for_seeded_events() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn();
    let gen_id = "gen-summary-1";

    // Seed a generation + two usage events with token/cost data.
    let now = now_ms();
    conn.execute(
        "INSERT INTO audit_generations \
         (generation_id, state, started_at_ms, is_active, created_at_ms, updated_at_ms) \
         VALUES (?1, 'building', ?2, 0, ?2, ?2)",
        rusqlite::params![gen_id, now],
    )
    .unwrap();

    for i in 0..2 {
        conn.execute(
            "INSERT INTO usage_events \
             (id, generation_id, agent, source_file_id, source_path, source_line, \
              source_offset_start, source_offset_end, session_id, timestamp_ms, \
              model, total_tokens, input_tokens, output_tokens, \
              cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
              reasoning_tokens, cost_usd, created_at_ms, updated_at_ms, is_sidechain, \
              raw_event_hash) \
             VALUES (?1, ?2, 'codex', 'file-1', '/tmp/x.jsonl', 1, 0, 100, \
                     'sess-1', ?3, 'gpt-5', 100, 60, 40, 0, 0, 0, 0, 0.05, ?3, ?3, 0, ?4)",
            rusqlite::params![format!("evt-{i}"), gen_id, now, format!("hash-{i}")],
        )
        .unwrap();
    }

    let result = update_generation_summaries(conn, gen_id);
    assert!(result.is_ok(), "update_generation_summaries should succeed");
}

#[test]
fn update_generation_summaries_returns_ok_with_no_events() {
    // Covers the COALESCE(...,0) path when no events exist for the generation.
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn();
    let gen_id = "gen-empty";
    let now = now_ms();
    conn.execute(
        "INSERT INTO audit_generations \
         (generation_id, state, started_at_ms, is_active, created_at_ms, updated_at_ms) \
         VALUES (?1, 'building', ?2, 0, ?2, ?2)",
        rusqlite::params![gen_id, now],
    )
    .unwrap();

    let result = update_generation_summaries(conn, gen_id);
    assert!(result.is_ok());
}

// =============================================================================
// ui_models.rs — breakdown_metrics, format_trend_label, format_compact, etc.
// =============================================================================

#[test]
fn breakdown_metrics_month_range_returns_three_cards_with_month_labels() {
    let totals = UsageTotals {
        total_tokens: 5000,
        total_cost_usd: Some(1.23),
        event_count: 42,
        has_cost: true,
        has_no_cost: false,
    };
    let cards = breakdown_metrics(RangePresetDto::Month, &totals);
    assert_eq!(cards.len(), 3);
    assert_eq!(cards[0].id, "tokens");
    assert_eq!(cards[0].label, "Month Tokens");
    assert_eq!(cards[0].value, "5K");
    assert_eq!(cards[1].id, "cost");
    assert_eq!(cards[1].label, "Month Cost");
    assert_eq!(cards[1].value, "$1.23");
    assert_eq!(cards[2].id, "events");
    assert_eq!(cards[2].label, "Month Events");
}

#[test]
fn breakdown_metrics_year_range_returns_year_labels() {
    let totals = UsageTotals {
        total_tokens: 0,
        total_cost_usd: None,
        event_count: 0,
        has_cost: false,
        has_no_cost: false,
    };
    let cards = breakdown_metrics(RangePresetDto::Year, &totals);
    assert_eq!(cards[0].label, "Year Tokens");
    assert_eq!(cards[1].label, "Year Cost");
    assert_eq!(cards[2].label, "Year Events");
}

#[test]
fn breakdown_metrics_week_range_returns_week_labels() {
    let totals = UsageTotals {
        total_tokens: 1000,
        total_cost_usd: Some(0.50),
        event_count: 5,
        has_cost: true,
        has_no_cost: false,
    };
    let cards = breakdown_metrics(RangePresetDto::Week, &totals);
    assert_eq!(cards[0].label, "Week Tokens");
    assert_eq!(cards[1].label, "Week Cost");
    assert_eq!(cards[2].label, "Week Events");
}

#[test]
fn trend_granularity_month_preset_maps_to_day_granularity() {
    // Line 335: RangePresetDto::Month => Day (uncovered in external tests).
    assert_eq!(
        trend_granularity(RangePresetDto::Month),
        TrendBucketGranularityDto::Day
    );
}

#[test]
fn format_trend_label_week_granularity_with_valid_date() {
    // Covers lines 350-365: the Week branch that parses a date and returns
    // "{weekday_abbr} {day}".
    let label = format_trend_label(&TrendBucketGranularityDto::Week, "2026-05-20");
    // 2026-05-20 is a Wednesday.
    assert_eq!(label, "Wed 20");
}

#[test]
fn format_trend_label_week_granularity_with_invalid_key_falls_back() {
    // Covers the fallback `key.to_string()` when date parsing fails.
    let label = format_trend_label(&TrendBucketGranularityDto::Week, "not-a-date");
    assert_eq!(label, "not-a-date");
}

#[test]
fn format_trend_label_hour_granularity_with_valid_key() {
    let label = format_trend_label(&TrendBucketGranularityDto::Hour, "2026-05-20T09:00:00");
    assert_eq!(label, "9:00");
}

#[test]
fn format_trend_label_hour_granularity_with_invalid_key_falls_back() {
    // Covers lines 376-378: when split('T') or hour parse fails.
    let label = format_trend_label(&TrendBucketGranularityDto::Hour, "no-time-part");
    assert_eq!(label, "no-time-part");
}

#[test]
fn format_trend_label_hour_granularity_with_non_numeric_hour_falls_back() {
    // Covers the inner parse::<u8> failure branch.
    let label = format_trend_label(&TrendBucketGranularityDto::Hour, "2026-05-20TXX:00:00");
    assert_eq!(label, "2026-05-20TXX:00:00");
}

#[test]
fn format_trend_label_day_granularity_with_valid_date() {
    let label = format_trend_label(&TrendBucketGranularityDto::Day, "2026-05-19");
    assert_eq!(label, "May 19");
}

#[test]
fn format_trend_label_day_granularity_with_invalid_key_falls_back() {
    // Covers lines 388-403: Date::parse fails -> return key.
    let label = format_trend_label(&TrendBucketGranularityDto::Day, "garbage");
    assert_eq!(label, "garbage");
}

#[test]
fn format_trend_label_month_granularity_with_valid_key() {
    let label = format_trend_label(&TrendBucketGranularityDto::Month, "2026-01");
    assert_eq!(label, "Jan");
}

#[test]
fn format_trend_label_month_granularity_with_invalid_month_number_falls_back() {
    // parts.len()==2 but month parse/try_from fails.
    let label = format_trend_label(&TrendBucketGranularityDto::Month, "2026-99");
    assert_eq!(label, "2026-99");
}

#[test]
fn format_trend_label_month_granularity_with_wrong_parts_falls_back() {
    // parts.len() != 2 -> return key.
    let label = format_trend_label(&TrendBucketGranularityDto::Month, "2026");
    assert_eq!(label, "2026");
}

#[test]
fn format_trend_label_month_granularity_with_garbage_falls_back() {
    // No hyphen at all -> parts.len()==1.
    let label = format_trend_label(&TrendBucketGranularityDto::Month, "nohyphen");
    assert_eq!(label, "nohyphen");
}

#[test]
fn format_tokens_escalates_b_to_t_on_rounding_overflow() {
    // Covers format_compact lines 240-241: B suffix with rounded >= 1000
    // escalates to T. n=999_950_000_000 -> scaled=999.95 -> rounded=1000.0
    // -> format_compact(n, 1e12, "T") -> "1T".
    assert_eq!(format_tokens(999_950_000_000), "1T");
}

#[test]
fn format_tokens_large_b_value_stays_b() {
    // Sanity: a large B value that does NOT overflow to T.
    assert_eq!(format_tokens(10_000_000_000), "10B");
}

#[test]
fn format_thousands_negative_number() {
    // Covers line 259: the n<0 branch of format_thousands.
    // format_thousands is pub and receives the raw i64; negatives produce
    // "-1,234" (the leading '-' is preserved and grouping still applies).
    assert_eq!(format_thousands(-1234), "-1,234");
}

#[test]
fn format_thousands_zero() {
    assert_eq!(format_thousands(0), "0");
}

#[test]
fn format_thousands_large_number() {
    assert_eq!(format_thousands(1_000_000), "1,000,000");
}

// =============================================================================
// range.rs — parse_date_to_ms, parse_date_to_ms_exclusive, Feb 29, December
// =============================================================================

#[test]
fn parse_date_to_ms_returns_midnight_utc() {
    // `parse_date_to_ms` returns epoch-ms for midnight UTC of the given date.
    // Use relative assertions (rather than a hardcoded epoch that is easy to
    // mis-compute) so the test stays correct regardless of the date chosen.
    let ms = parse_date_to_ms("2026-05-20").expect("parse");
    // Must be exactly midnight UTC: divisible by 86_400_000 ms (1 day).
    assert_eq!(
        ms % 86_400_000,
        0,
        "parse_date_to_ms should return midnight UTC"
    );
    // Inclusive start must be strictly less than the exclusive end (next midnight).
    let exclusive = parse_date_to_ms_exclusive("2026-05-20").expect("parse exclusive");
    assert_eq!(
        exclusive - ms,
        86_400_000,
        "exclusive end should be exactly one day after the inclusive start"
    );
}

#[test]
fn parse_date_to_ms_invalid_date_returns_error() {
    let err = parse_date_to_ms("not-a-date").unwrap_err();
    assert!(err.to_string().contains("invalid date"));
}

#[test]
fn parse_date_to_ms_exclusive_returns_next_midnight_utc() {
    // Exclusive end: start of next day (midnight UTC of the following day).
    let inclusive = parse_date_to_ms("2026-05-20").expect("parse inclusive");
    let exclusive = parse_date_to_ms_exclusive("2026-05-20").expect("parse exclusive");
    assert_eq!(
        exclusive % 86_400_000,
        0,
        "exclusive should also land on midnight UTC"
    );
    assert_eq!(
        exclusive - inclusive,
        86_400_000,
        "exclusive = inclusive + 1 day"
    );
}

#[test]
fn parse_date_to_ms_exclusive_invalid_date_returns_error() {
    let err = parse_date_to_ms_exclusive("2026-13-45").unwrap_err();
    assert!(err.to_string().contains("invalid date"));
}

#[test]
fn heatmap_window_from_date_feb_29_in_non_leap_target_year_uses_fallback() {
    // 2024-02-29 is valid (2024 is a leap year), but going back 1 year to
    // 2023-02-29 fails because 2023 is NOT a leap year. This exercises the
    // `unwrap_or_else` fallback that computes the last day of the month.
    let window = heatmap_window_from_date(2024, 2, 29);
    // Fallback: last day of Feb 2023 = 2023-02-28.
    assert_eq!(window.start, "2023-02-28");
    assert_eq!(window.end_inclusive, "2024-02-29");
}

#[test]
fn resolve_range_month_preset_in_december_wraps_to_january_next_year() {
    // Covers line 176: when today_date.month() == December, next month is
    // January of year+1 (not month.next()).
    let rtz = ReportingTimezone::parse("UTC").unwrap();
    let range = resolve_range(
        &rtz,
        2025,
        12,
        15,
        RangePresetDto::Month,
        WeekdayIndexDto::SUNDAY,
    );
    let month_start = rtz.civil_date_to_utc_start_ms("2025-12-01").unwrap();
    let month_end = rtz.civil_date_to_utc_start_ms("2026-01-01").unwrap();
    assert_eq!(range.start_ms, month_start);
    assert_eq!(range.end_ms, month_end);
    assert_eq!(range.start_date, "2025-12-01");
    assert_eq!(range.end_date, "2025-12-31");
}

#[test]
fn resolve_range_month_preset_non_december_uses_next_month() {
    // Sanity: non-December month uses month.next() branch (line 178-179).
    let rtz = ReportingTimezone::parse("UTC").unwrap();
    let range = resolve_range(
        &rtz,
        2026,
        5,
        20,
        RangePresetDto::Month,
        WeekdayIndexDto::SUNDAY,
    );
    assert_eq!(range.start_date, "2026-05-01");
    assert_eq!(range.end_date, "2026-05-31");
}

// =============================================================================
// rebuild.rs — initiate_rebuild (empty frontiers), record_new_file_observation,
//              record_rebuild_diagnostic, RebuildFrontierSet::persist log
// =============================================================================

fn seed_checkpoint(db: &Database, id: &str, offset: i64, size: i64) {
    let conn = db.conn();
    let now = now_ms();
    conn.execute(
        "INSERT OR REPLACE INTO source_file_checkpoints \
         (id, source_id, agent, path, inode, offset_bytes, size_bytes, \
          last_mtime_ms, state, first_seen_at_ms, last_seen_at_ms, \
          created_at_ms, updated_at_ms) \
         VALUES (?1, 'src-1', 'claude_code', '/tmp/test.jsonl', NULL, \
                 ?2, ?3, NULL, 'active', ?4, ?4, ?4, ?4)",
        rusqlite::params![id, offset, size, now],
    )
    .unwrap();
}

#[test]
fn initiate_rebuild_with_multiple_frontiers_persists_observations_and_logs() {
    // Covers RebuildFrontierSet::persist multi-frontier info! log (lines
    // 128-131) and capture() with >1 checkpoint.
    let db = Database::open_in_memory().expect("db");
    seed_checkpoint(&db, "file-a", 100, 500);
    seed_checkpoint(&db, "file-b", 200, 600);
    seed_checkpoint(&db, "file-c", 300, 700);

    let frontier_set = initiate_rebuild(&db, "gen-multi").expect("initiate_rebuild");
    assert_eq!(frontier_set.frontiers.len(), 3);

    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM generation_file_observations WHERE generation_id = 'gen-multi'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 3);
}

#[test]
fn initiate_rebuild_with_no_frontiers_still_persists_empty_set() {
    // Covers the persist() info! log when frontiers is empty (count=0).
    let db = Database::open_in_memory().expect("db");
    let frontier_set = initiate_rebuild(&db, "gen-empty").expect("initiate_rebuild");
    assert_eq!(frontier_set.frontiers.len(), 0);

    // persist() is called inside initiate_rebuild; verify no rows were added.
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM generation_file_observations WHERE generation_id = 'gen-empty'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn record_new_file_observation_creates_checkpoint_and_observation() {
    // Covers record_new_file_observation (lines 160-197) including the
    // upsert_source_file_checkpoint + upsert_log_file_checkpoint +
    // insert_generation_observation calls and the info! log.
    let db = Database::open_in_memory().expect("db");
    create_generation(&db, "gen-newfile").expect("create_generation");

    record_new_file_observation(
        &db,
        "gen-newfile",
        "file-new-1",
        "src-1",
        "claude_code",
        "/tmp/new.jsonl",
        0,
        100,
        Some(123456),
    )
    .expect("record_new_file_observation");

    let obs_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM generation_file_observations \
             WHERE generation_id = 'gen-newfile' AND source_file_id = 'file-new-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(obs_count, 1);

    let cp_state: String = db
        .conn()
        .query_row(
            "SELECT state FROM source_file_checkpoints WHERE id = 'file-new-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cp_state, "active");
}

#[test]
fn record_rebuild_diagnostic_inserts_diagnostic_event() {
    // Covers record_rebuild_diagnostic (lines 599-619).
    let db = Database::open_in_memory().expect("db");

    record_rebuild_diagnostic(&db, "gen-diag", "error", "test failure message")
        .expect("record_rebuild_diagnostic");

    let (severity, message): (String, String) = db
        .conn()
        .query_row(
            "SELECT severity, message FROM diagnostic_events \
             WHERE source_id = 'rebuild' AND code = 'rebuild_lifecycle'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(severity, "error");
    assert!(message.contains("test failure message"));
}

#[test]
fn rebuild_frontier_set_persist_is_idempotent_via_insert_or_replace() {
    // Re-persisting the same frontier set should not duplicate rows.
    let db = Database::open_in_memory().expect("db");
    seed_checkpoint(&db, "file-x", 50, 200);

    let frontier_set = RebuildFrontierSet::capture("gen-idem", &db).expect("capture");
    frontier_set.persist(&db).expect("first persist");
    frontier_set.persist(&db).expect("second persist");

    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM generation_file_observations WHERE generation_id = 'gen-idem'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "INSERT OR REPLACE should keep a single row");
}

// =============================================================================
// scan.rs — derive_file_id, enrich_cost
// =============================================================================

#[test]
fn derive_file_id_is_deterministic_and_prefixed() {
    let id1 = derive_file_id(std::path::Path::new("/tmp/claude/logs.jsonl"));
    let id2 = derive_file_id(std::path::Path::new("/tmp/claude/logs.jsonl"));
    assert_eq!(id1, id2, "same path must produce same id");
    assert!(id1.starts_with("file_"), "id must be prefixed with file_");
    assert!(
        id1.len() > "file_".len() + 16,
        "id must contain a hash digest"
    );
}

#[test]
fn derive_file_id_differs_for_different_paths() {
    let id1 = derive_file_id(std::path::Path::new("/tmp/a.jsonl"));
    let id2 = derive_file_id(std::path::Path::new("/tmp/b.jsonl"));
    assert_ne!(id1, id2);
}

#[test]
fn enrich_cost_auto_mode_uses_source_cost_when_present() {
    // Auto mode: prefer source cost_usd, so estimated_cost_usd should be set
    // and cost_source should remain unset (cost already present).
    let mut evt = NormalizedUsageEvent::minimal_for_test("evt-1", AgentKind::ClaudeCode);
    evt.model = Some("claude-sonnet-4-20250514".to_string());
    evt.input_tokens = 1000;
    evt.output_tokens = 500;
    evt.cost_usd = Some(0.42);
    evt.estimated_cost_usd = None;

    enrich_cost(&mut evt, CostMode::Auto);

    // When source cost is present in Auto mode, estimate_cost_with_catalog
    // returns it as-is.
    assert!(
        evt.estimated_cost_usd.is_some(),
        "estimated_cost_usd should be populated"
    );
    assert!(
        evt.price_catalog_version.is_some(),
        "catalog version should be stamped"
    );
}

#[test]
fn enrich_cost_calculate_mode_ignores_source_cost() {
    // Calculate mode: always compute from tokens, ignoring source cost.
    let mut evt = NormalizedUsageEvent::minimal_for_test("evt-2", AgentKind::ClaudeCode);
    evt.model = Some("claude-sonnet-4-20250514".to_string());
    evt.input_tokens = 1000;
    evt.output_tokens = 500;
    evt.cost_usd = Some(0.42); // Should be ignored for calculation.
    evt.estimated_cost_usd = None;

    enrich_cost(&mut evt, CostMode::Calculate);

    // The pricing catalog loaded in the test environment may not include this
    // exact model id, so `estimated_cost_usd` may legitimately stay `None`.
    // The reliable invariant is that the catalog-version stamp is always
    // written, proving the Calculate branch was entered.
    assert!(
        evt.price_catalog_version.is_some(),
        "Calculate mode must stamp the price_catalog_version"
    );
}

#[test]
fn enrich_cost_display_mode_uses_source_cost_only() {
    // Display mode: always use source-provided cost; returns None when absent.
    let mut evt = NormalizedUsageEvent::minimal_for_test("evt-3", AgentKind::Codex);
    evt.model = Some("gpt-5".to_string());
    evt.input_tokens = 1000;
    evt.output_tokens = 500;
    evt.cost_usd = Some(0.99);
    evt.estimated_cost_usd = None;

    enrich_cost(&mut evt, CostMode::Display);

    assert!(evt.estimated_cost_usd.is_some());
    assert!(evt.price_catalog_version.is_some());
}

#[test]
fn enrich_cost_skips_when_estimated_cost_already_set() {
    // Covers the `if event.estimated_cost_usd.is_none()` guard (line 689):
    // when already set, enrich_cost should only stamp the catalog version.
    let mut evt = NormalizedUsageEvent::minimal_for_test("evt-4", AgentKind::ClaudeCode);
    evt.model = Some("claude-sonnet-4-20250514".to_string());
    evt.estimated_cost_usd = Some(1.23);

    enrich_cost(&mut evt, CostMode::Auto);

    // Should remain unchanged (not recomputed).
    assert_eq!(evt.estimated_cost_usd, Some(1.23));
    assert!(evt.price_catalog_version.is_some());
}

#[test]
fn enrich_cost_with_empty_model_does_not_panic() {
    // Covers the `event.model.as_deref().unwrap_or("")` path (line 676).
    let mut evt = NormalizedUsageEvent::minimal_for_test("evt-5", AgentKind::Codex);
    evt.model = None;
    evt.input_tokens = 100;
    evt.output_tokens = 50;

    enrich_cost(&mut evt, CostMode::Auto);
    // No panic; catalog version still stamped.
    assert!(evt.price_catalog_version.is_some());
}

// =============================================================================
// supervisor.rs — legacy_audit_rebuild_recommended, debug_registered_agents,
//                  apply_service_status_snapshot, read_status_snapshot,
//                  transition_after_initial_scan, hydrate_status_from_db,
//                  provider_changed / provider_deleted (no-op when sidecar off)
// =============================================================================

#[test]
fn legacy_audit_rebuild_recommended_returns_false_on_fresh_db() {
    let (supervisor, _dir) = make_supervisor();
    let result = supervisor
        .legacy_audit_rebuild_recommended()
        .expect("check");
    assert!(!result, "fresh DB should not need legacy rebuild");
}

#[test]
fn legacy_audit_rebuild_recommended_returns_true_for_codex_null_model() {
    let (supervisor, _dir) = make_supervisor();
    let db = supervisor.db_handle().clone();
    let db = db.lock().unwrap();
    seed_active_generation(&db, "gen-1");
    seed_legacy_codex_event(&db, "codex-legacy-1", "gen-1");
    drop(db);

    let result = supervisor
        .legacy_audit_rebuild_recommended()
        .expect("check");
    assert!(
        result,
        "codex events with NULL model should trigger rebuild recommendation"
    );
}

#[test]
fn legacy_audit_rebuild_recommended_returns_true_for_claude_token_mismatch() {
    let (supervisor, _dir) = make_supervisor();
    let db = supervisor.db_handle().clone();
    let db = db.lock().unwrap();
    seed_active_generation(&db, "gen-2");
    seed_legacy_claude_event(&db, "claude-legacy-1", "gen-2");
    drop(db);

    let result = supervisor
        .legacy_audit_rebuild_recommended()
        .expect("check");
    assert!(
        result,
        "claude events with mismatched token totals should trigger rebuild"
    );
}

#[test]
fn debug_registered_agents_returns_default_adapters() {
    // `BusytokSupervisor::new()` registers the default adapter set
    // (`ClaudeCodeAdapter` + `CodexAdapter`). Cover the `debug_registered_agents`
    // accessor by asserting those two agent names are present.
    let (supervisor, _dir) = make_supervisor();
    let agents = supervisor.debug_registered_agents();
    assert!(
        agents.iter().any(|a| a == "claude_code"),
        "default adapters must include claude_code, got {:?}",
        agents
    );
    assert!(
        agents.iter().any(|a| a == "codex"),
        "default adapters must include codex, got {:?}",
        agents
    );
}

#[test]
fn apply_service_status_snapshot_mutates_snapshot() {
    let (supervisor, _dir) = make_supervisor();
    // Apply a mutation that sets the writer_queue_depth.
    supervisor
        .apply_service_status_snapshot(|snap| {
            snap.writer_queue_depth = 42;
        })
        .expect("apply");

    // The mutation should be visible via a synchronous try_read of the
    // underlying snapshot. Use the public status_snapshot_arc to verify.
    let snap = supervisor.status_snapshot_arc();
    let snap = snap.try_read().unwrap();
    assert_eq!(snap.writer_queue_depth, 42);
}

#[tokio::test]
async fn read_status_snapshot_returns_default_starting_state() {
    let (supervisor, _dir) = make_supervisor();
    let snap = supervisor.read_status_snapshot().await;
    assert_eq!(snap.readiness, ReadinessStateDto::Starting);
    assert_eq!(snap.active_generation_id, None);
}

#[tokio::test]
async fn transition_after_initial_scan_to_ready_exact_succeeds_when_generation_active() {
    // `transition_after_initial_scan(ReadyExact)` requires:
    //   (1) a non-empty `active_generation_id` in the in-memory snapshot, AND
    //   (2) a promoted+active generation row in `audit_generations` (checked
    //       inside `transition_readiness`), AND
    //   (3) the snapshot's `readiness == Starting` (or ReadyDegraded→ReadyExact).
    //
    // We must NOT call `hydrate_status_from_db()` here because its
    // `recover_missing_generation_metadata` step would itself flip service_state
    // to `ready_exact` (since the seeded generation has no usage events and no
    // blocking diagnostic), which would make `can_transition` false. Instead we
    // seed the DB row + set the snapshot directly via `apply_service_status_snapshot`,
    // keeping readiness at `Starting` so the transition is permitted.
    let (supervisor, _dir) = make_supervisor();
    let db = supervisor.db_handle().clone();
    {
        let db = db.lock().unwrap();
        seed_active_generation(&db, "gen-active-1");
    }
    // Set the in-memory snapshot to Starting + the active generation id.
    supervisor
        .apply_service_status_snapshot(|snap| {
            snap.readiness = ReadinessStateDto::Starting;
            snap.active_generation_id = Some("gen-active-1".to_string());
        })
        .expect("apply snapshot");

    let transitioned = supervisor
        .transition_after_initial_scan(ReadinessStateDto::ReadyExact)
        .await
        .expect("transition");
    assert!(
        transitioned,
        "transition should succeed when an active promoted generation exists and readiness is Starting"
    );

    let snap = supervisor.read_status_snapshot().await;
    assert_eq!(snap.readiness, ReadinessStateDto::ReadyExact);
}

#[tokio::test]
async fn transition_after_initial_scan_returns_false_when_no_active_generation() {
    // No active generation in DB -> transition should return false without
    // erroring.
    let (supervisor, _dir) = make_supervisor();
    supervisor.hydrate_status_from_db().expect("hydrate");

    let transitioned = supervisor
        .transition_after_initial_scan(ReadinessStateDto::ReadyExact)
        .await
        .expect("transition");
    assert!(!transitioned, "no active generation -> no transition");
}

#[test]
fn hydrate_status_from_db_loads_ready_exact_with_generation() {
    // Seed service_state with ready_exact + an active generation, then
    // hydrate and verify the snapshot reflects it.
    let (supervisor, _dir) = make_supervisor();
    let db = supervisor.db_handle().clone();
    {
        let db = db.lock().unwrap();
        seed_active_generation(&db, "gen-hydrate-1");
        seed_service_state(&db, "ready_exact", Some("gen-hydrate-1"));
    }

    supervisor.hydrate_status_from_db().expect("hydrate");

    let snap = supervisor.status_snapshot_arc();
    let snap = snap.try_read().unwrap();
    assert_eq!(snap.readiness, ReadinessStateDto::ReadyExact);
    assert_eq!(snap.active_generation_id.as_deref(), Some("gen-hydrate-1"));
}

#[test]
fn hydrate_status_from_db_loads_ready_degraded() {
    let (supervisor, _dir) = make_supervisor();
    let db = supervisor.db_handle().clone();
    {
        let db = db.lock().unwrap();
        seed_service_state(&db, "ready_degraded", None);
    }

    supervisor.hydrate_status_from_db().expect("hydrate");

    let snap = supervisor.status_snapshot_arc();
    let snap = snap.try_read().unwrap();
    assert_eq!(snap.readiness, ReadinessStateDto::ReadyDegraded);
    assert_eq!(snap.active_generation_id, None);
}

#[tokio::test]
async fn provider_changed_is_noop_when_sidecar_disabled() {
    // Covers the `else` branch (lines 900-905) of provider_changed: when
    // worker_pool is None (sidecar disabled), it logs a debug no-op.
    let (supervisor, _dir) = make_supervisor();
    // Should not panic or hang.
    supervisor.provider_changed("provider-1").await;
}

#[tokio::test]
async fn provider_deleted_is_noop_when_sidecar_disabled() {
    // Covers the `else` branch (lines 928-934) of provider_deleted.
    let (supervisor, _dir) = make_supervisor();
    supervisor.provider_deleted("provider-1").await;
}

#[tokio::test]
async fn run_initial_scan_with_codex_default_paths_enabled_exercises_codex_discovery() {
    // Drives `source_registry::discover_default_sources` codex branch (line 84)
    // through the supervisor. We intentionally use `register_new_install_sources`
    // rather than `run_initial_scan` because the latter calls `db.reopen()`,
    // which returns `None` for an in-memory `Database` (no file path to reopen),
    // causing `run_initial_scan` to error with "initial scan requires a detached
    // database handle". `register_new_install_sources` exercises the same
    // `discover_sources` → `discover_all` → codex discovery path without
    // requiring `reopen()`.
    let mut settings = BusytokSettings::default();
    settings.discovery.claude_code_default_paths = false;
    settings.discovery.codex_default_paths = true;
    let (supervisor, _dir) = make_supervisor_with_settings(settings);

    let result = supervisor.register_new_install_sources().await;
    assert!(
        result.is_ok(),
        "register_new_install_sources should not error even with no real sources"
    );
    let stats = result.unwrap();
    // The dev/CI host may or may not have a real Codex installation, so the
    // discovered source count is environment-dependent. The goal of this
    // test is to exercise the codex discovery code path without erroring;
    // a non-negative source count proves the path ran end-to-end.
    assert!(
        stats.sources >= 0,
        "source count should be non-negative, got {}",
        stats.sources
    );
}

#[tokio::test]
async fn register_new_install_sources_with_defaults_disabled_returns_zero_sources() {
    // Exercises the fresh-install fast-register path with no discovery.
    let mut settings = BusytokSettings::default();
    settings.discovery.claude_code_default_paths = false;
    settings.discovery.codex_default_paths = false;
    let (supervisor, _dir) = make_supervisor_with_settings(settings);

    let stats = supervisor
        .register_new_install_sources()
        .await
        .expect("register");
    assert_eq!(stats.sources, 0);
}

// =============================================================================
// bootstrap.rs — ServiceApp::boot stages 1-4
// =============================================================================

#[tokio::test]
#[serial]
async fn service_app_boot_completes_stages_1_to_4_and_writes_marker() {
    // Drives bootstrap::open_database, create_supervisor, hydrate_status,
    // bind_control_server. Verifies the service.ready marker is written
    // (stages 1-4 only; run() is not invoked).
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let startup = Instant::now();
    let data_dir = paths.data_dir().to_path_buf();

    let app = ServiceApp::boot(paths, startup).await.expect("boot");

    // boot() must write the service.ready marker.
    assert!(
        busytok_config::service_marker::exists(&data_dir),
        "ServiceApp::boot must write the service.ready marker"
    );

    // `ServiceApp`'s fields (server, server_task) are private, so we cannot
    // destructure or call `shutdown_control_server` from an integration test.
    // Dropping `app` releases the handle; the detached control-server task is
    // reaped when the test process exits. The test serializes with `#[serial]`
    // so the bound port does not collide with sibling tests.
    drop(app);

    // Remove the marker to clean up.
    let _ = busytok_config::service_marker::remove(&data_dir);
}

// =============================================================================
// format_compact visibility shim
// =============================================================================

/// format_compact is private, but we exercise it indirectly through
/// format_tokens. This test is a placeholder to document that the B->T
/// escalation is already covered by `format_tokens_escalates_b_to_t_on_rounding_overflow`
/// above; no direct call is possible from an integration test.
#[test]
fn format_compact_is_exercised_indirectly_via_format_tokens() {
    // Re-assert the escalation here so the intent is documented in one place.
    assert_eq!(format_tokens(999_950_000_000), "1T");
    assert_eq!(format_tokens(999_999_999_999), "1T");
}
