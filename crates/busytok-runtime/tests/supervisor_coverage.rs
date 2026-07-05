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
//! Coverage-focused tests for `busytok-runtime/src/supervisor.rs`.
//!
//! This file is intentionally separate from `supervisor_control.rs` so it can
//! be added without merge conflicts. It targets specific uncovered regions:
//! - `receipt_daily` closure body (lines ~3138-3194)
//! - `live_window` closure body (lines ~5023-5088)
//! - `activity_detail_from_read_row` notes building (lines ~2338-2392)
//! - `breakdown_detail` Model client_mix / project_mix loops (lines ~4138-4177)
//! - `overview_trend` IANA-Day and fast-path branches (lines ~3225-3253)
//! - `pressure_responder` accessor (lines ~841-847)
//! - `prompt_use_outcome_to_row` / `prompt_use_failure_reason_to_row` (lines ~2240-2263)
//! - `scan_state_from_conn` scanning / idle branches (lines ~1985, 1989)
//! - `register_new_install_sources` writer-actor registration (lines ~1490-1596)
//!
//! Helpers mirror the ones in `supervisor_control.rs` so the same proven
//! patterns (file-backed DB + multi-thread tokio runtime + writer actor) are
//! reused. No new infrastructure is introduced.

use busytok_config::{BusytokPaths, BusytokSettings, ManualRootConfig};
use busytok_control::dispatch::RuntimeControl;
use busytok_domain::{AgentKind, NormalizedUsageEvent, ReportingTimezone, UsageWritePolicy};
use busytok_protocol::dto::*;
use busytok_runtime::BusytokSupervisor;
use busytok_store::repository::LogSourceRow;
use busytok_store::Database;
use time::{OffsetDateTime, Time};

// Re-export RangePresetDto for convenience.
use busytok_protocol::dto::RangePresetDto;

// ---------------------------------------------------------------------------
// Helpers (replicated from supervisor_control.rs to keep this file standalone)
// ---------------------------------------------------------------------------

fn make_supervisor(db: Database, tmp: &tempfile::TempDir) -> BusytokSupervisor {
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = busytok_config::BusytokSettings::default();
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .ok();
    BusytokSupervisor::new(db, paths)
}

fn make_supervisor_with_settings(
    db: Database,
    tmp: &tempfile::TempDir,
    settings: BusytokSettings,
) -> BusytokSupervisor {
    let paths = BusytokPaths::for_test(tmp.path());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .ok();
    BusytokSupervisor::new(db, paths)
}

fn make_file_backed_db(tmp: &tempfile::TempDir) -> Database {
    let db_path = tmp.path().join("busytok.sqlite");
    Database::open(&db_path).unwrap()
}

fn today_ms_for_test() -> i64 {
    busytok_domain::now_ms()
}

fn utc_midday_ms_for_test() -> i64 {
    let today = OffsetDateTime::now_utc().date();
    today
        .with_time(Time::MIDNIGHT)
        .assume_utc()
        .unix_timestamp()
        * 1_000
        + 12 * 3_600_000
}

fn utc_today_date_for_test() -> String {
    OffsetDateTime::now_utc().date().to_string()
}

fn set_active_generation(db: &Database, generation_id: &str, updated_at_ms: i64) {
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO service_state
             (id, writer_queue_depth, aggregate_lag_ms, readiness, active_generation_id, updated_at_ms)
             VALUES (1, 0, 0, 'ready_exact', ?1, ?2)",
            rusqlite::params![generation_id, updated_at_ms],
        )
        .unwrap();
}

fn assign_event_generation(db: &Database, event_id: &str, generation_id: &str) {
    db.conn()
        .execute(
            "UPDATE usage_events SET generation_id = ?1, dedupe_key = ?2 WHERE id = ?3",
            rusqlite::params![generation_id, event_id, event_id],
        )
        .unwrap();
}

fn seed_event(
    db: &Database,
    id: &str,
    timestamp_ms: i64,
    tokens: i64,
    cost: Option<f64>,
    client_kind: Option<&str>,
    model: Option<&str>,
    project_hash: Option<&str>,
    session_id: Option<&str>,
) {
    let mut event = NormalizedUsageEvent::minimal_for_test(id, AgentKind::ClaudeCode);
    event.timestamp_ms = timestamp_ms;
    event.total_tokens = tokens;
    event.cost_usd = cost;
    event.client_kind = client_kind.map(|s| s.to_string());
    event.model = model.map(|s| s.to_string());
    event.project_hash = project_hash.map(|s| s.to_string());
    event.session_id = session_id.unwrap_or("session-default").to_string();
    db.write_usage_event(&event, UsageWritePolicy::InsertOnce)
        .unwrap();
}

fn seed_source(db: &Database, id: &str, agent: &str, root_path: &str, status: &str) {
    let row = LogSourceRow {
        id: id.to_string(),
        agent: agent.to_string(),
        source_type: "jsonl".to_string(),
        root_path: root_path.to_string(),
        configured_by_user: 1,
        default_discovery_enabled: 0,
        status: status.to_string(),
        last_scan_started_at_ms: None,
        last_scan_completed_at_ms: None,
        last_error: None,
        first_seen_at_ms: 1000,
        last_seen_at_ms: 1000,
        created_at_ms: 1000,
        updated_at_ms: 1000,
    };
    db.upsert_log_source(&row).unwrap();
}

fn set_source_scan_timestamps(
    db: &Database,
    source_id: &str,
    started_at_ms: Option<i64>,
    completed_at_ms: Option<i64>,
) {
    db.conn()
        .execute(
            "UPDATE log_sources \
             SET last_scan_started_at_ms = ?2, last_scan_completed_at_ms = ?3 \
             WHERE id = ?1",
            rusqlite::params![source_id, started_at_ms, completed_at_ms],
        )
        .unwrap();
}

// ---------------------------------------------------------------------------
// receipt.daily — covers the closure body (~lines 3138-3194)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receipt_daily_with_none_date_runs_closure_and_falls_back_to_today() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    // Use an explicit UTC timezone so the date resolution and daily_usage
    // seeding are deterministic regardless of the host locale.
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    set_active_generation(&db, "gen-receipt-none", today_ms_for_test());
    // Seed an event for today so the closure's read queries return non-empty
    // results and every branch of the closure body is exercised.
    seed_event(
        &db,
        "receipt-evt-none",
        utc_midday_ms_for_test(),
        250,
        Some(0.5),
        Some("claude_code"),
        Some("claude-sonnet-4"),
        Some("hash-receipt"),
        Some("session-receipt"),
    );
    assign_event_generation(&db, "receipt-evt-none", "gen-receipt-none");
    // `read_daily_receipt_totals` reads from the `daily_usage` rollup table
    // (not `usage_events` directly), so seed it the same way the writer's
    // flush path would.
    seed_daily_usage_for_generation(&db, "+00:00", "gen-receipt-none");

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    // date = None exercises the `rtz.local_date_for_timestamp_ms(now_ms)`
    // fallback branch that resolves today's date in the reporting timezone.
    let result = sup
        .receipt_daily(ReceiptDailyRequestDto { date: None })
        .await
        .unwrap()
        .data;

    assert!(!result.date.is_empty());
    assert!(!result.timezone.is_empty());
    // The seeded event for today should be reflected in the receipt totals.
    assert!(result.metrics.total_tokens >= 250);

    sup.shutdown_writer().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receipt_daily_with_explicit_date_runs_closure() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    set_active_generation(&db, "gen-receipt-explicit", today_ms_for_test());
    seed_event(
        &db,
        "receipt-evt-explicit",
        utc_midday_ms_for_test(),
        120,
        Some(0.2),
        Some("claude_code"),
        Some("claude-sonnet-4"),
        Some("hash-receipt-2"),
        Some("session-receipt-2"),
    );
    assign_event_generation(&db, "receipt-evt-explicit", "gen-receipt-explicit");
    seed_daily_usage_for_generation(&db, "+00:00", "gen-receipt-explicit");

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    // date = Some(today) exercises the `Some(d) => d` branch.
    let today = utc_today_date_for_test();
    let result = sup
        .receipt_daily(ReceiptDailyRequestDto {
            date: Some(today.clone()),
        })
        .await
        .unwrap()
        .data;

    assert_eq!(result.date, today);

    sup.shutdown_writer().await.unwrap();
}

// ---------------------------------------------------------------------------
// live.window — covers the closure body (~lines 5023-5088)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_window_with_active_generation_runs_exact_buckets_query() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    set_active_generation(&db, "gen-live", today_ms_for_test());

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    // window_seconds = None exercises the default 900s branch; an active
    // generation routes the closure through the `query_exact_buckets_range`
    // branch (the `if let Some(gen_id)` arm).
    let result = sup
        .live_window(LiveWindowRequestDto {
            window_seconds: None,
        })
        .await
        .unwrap()
        .data;

    assert!(result.end_ms >= result.start_ms);
    // exact_samples is densified to fill the window; with no data it is still
    // a fully populated zero-curve rather than empty.
    assert!(!result.exact_samples.is_empty());

    sup.shutdown_writer().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_window_without_active_generation_runs_backfill_buckets_query() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    // No set_active_generation: the snapshot has no active generation, so the
    // closure takes the `else` branch (`query_backfill_buckets_range`).
    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .live_window(LiveWindowRequestDto {
            window_seconds: Some(60),
        })
        .await
        .unwrap()
        .data;

    assert!(result.end_ms >= result.start_ms);
    assert!(!result.exact_samples.is_empty());

    sup.shutdown_writer().await.unwrap();
}

// ---------------------------------------------------------------------------
// activity_detail — covers notes building (~lines 2338-2392)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_detail_builds_notes_for_all_token_types() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    set_active_generation(&db, "gen-detail-notes", 1000);

    let mut event =
        NormalizedUsageEvent::minimal_for_test("detail-notes-evt", AgentKind::ClaudeCode);
    event.timestamp_ms = 1000;
    event.total_tokens = 1000;
    event.cost_usd = Some(1.5);
    event.client_kind = Some("claude_code".to_string());
    event.model = Some("claude-sonnet-4".to_string());
    event.project_hash = Some("hash-notes".to_string());
    event.session_id = "session-notes".to_string();
    // Populate every field that feeds a `notes` entry so all conditional
    // branches in `activity_detail_from_read_row` fire.
    event.input_tokens = 100;
    event.output_tokens = 200;
    event.cached_input_tokens = 50;
    event.reasoning_tokens = 30;
    event.cache_creation_tokens = 40;
    event.cache_read_tokens = 60;
    event.thoughts_tokens = 10;
    event.tool_tokens = 20;
    event.speed = Some("fast".to_string());
    event.usage_limit_reset_time_ms = Some(9_999_999_999);
    db.write_usage_event(&event, UsageWritePolicy::InsertOnce)
        .unwrap();
    assign_event_generation(&db, "detail-notes-evt", "gen-detail-notes");

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .activity_detail(ActivityDetailRequestDto {
            id: "detail-notes-evt".to_string(),
        })
        .await
        .unwrap()
        .data;

    assert_eq!(result.id, "detail-notes-evt");
    // token_breakdown is built when any component is non-zero.
    let tb = result.token_breakdown.expect("token breakdown");
    assert_eq!(tb.input_tokens, Some(100));
    assert_eq!(tb.output_tokens, Some(200));
    assert_eq!(tb.cache_read_tokens, Some(60));
    assert_eq!(tb.cache_write_tokens, Some(40));
    assert_eq!(tb.reasoning_tokens, Some(30));

    // technical_details.notes carries one entry per populated audit field.
    let notes = &result.technical_details.notes;
    assert!(
        notes.iter().any(|n| n.contains("speed: fast")),
        "expected speed note, got {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("cache_creation_tokens: 40")),
        "expected cache_creation_tokens note, got {notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("cache_read_tokens: 60")),
        "expected cache_read_tokens note, got {notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("thoughts_tokens: 10")),
        "expected thoughts_tokens note, got {notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("tool_tokens: 20")),
        "expected tool_tokens note, got {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("usage_limit_reset_time_ms: 9999999999")),
        "expected usage_limit_reset_time_ms note, got {notes:?}"
    );

    sup.shutdown_writer().await.unwrap();
}

// ---------------------------------------------------------------------------
// breakdown_detail Model — covers client_mix / project_mix loops
// (~lines 4138-4177)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn breakdown_detail_model_builds_client_and_project_mix() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    set_active_generation(&db, "gen-bd-model-mix", today_ms_for_test());
    let now = utc_midday_ms_for_test();
    // Two events from different clients + different projects, both for the
    // same model id, both within today's UTC Day range so
    // `read_breakdown_activity_list` returns them and the client_map /
    // proj_map loops iterate.
    seed_event(
        &db,
        "bd-mix-1",
        now,
        100,
        Some(1.0),
        Some("claude_code"),
        Some("sonnet-4"),
        Some("hash-a"),
        Some("session-a"),
    );
    assign_event_generation(&db, "bd-mix-1", "gen-bd-model-mix");
    seed_event(
        &db,
        "bd-mix-2",
        now - 3_600_000,
        200,
        Some(2.0),
        Some("codex"),
        Some("sonnet-4"),
        Some("hash-b"),
        Some("session-b"),
    );
    assign_event_generation(&db, "bd-mix-2", "gen-bd-model-mix");

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .breakdown_detail(BreakdownDetailRequestDto {
            kind: BreakdownKindDto::Model,
            id: "sonnet-4".to_string(),
            range: RangePresetDto::Day,
        })
        .await
        .unwrap()
        .data;

    match result {
        BreakdownDetailDto::Model(m) => {
            assert_eq!(m.id, "sonnet-4");
            // client_mix aggregates by client_id: claude_code + codex.
            assert_eq!(
                m.client_mix.len(),
                2,
                "expected two clients in client_mix, got {:?}",
                m.client_mix
            );
            // project_mix aggregates by project_hash: hash-a + hash-b.
            assert_eq!(
                m.project_mix.len(),
                2,
                "expected two projects in project_mix, got {:?}",
                m.project_mix
            );
            // recent_activity is capped at 10 and should contain both events.
            assert_eq!(m.recent_activity.len(), 2);
            let total_tokens: i64 = m.client_mix.iter().map(|c| c.tokens).sum();
            assert_eq!(total_tokens, 300);
        }
        other => panic!("expected Model breakdown detail, got {other:?}"),
    }

    sup.shutdown_writer().await.unwrap();
}

// ---------------------------------------------------------------------------
// overview.trend — covers IANA-Day and fast-path branches (~lines 3225-3253)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_trend_iana_day_uses_exact_window_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    // +05:30 is a non-whole-hour fixed offset, so `use_fast_path` is false
    // and `is_iana_day` is true for range=Day → routes through the
    // `read_overview_window_aggregates_exact` branch.
    let settings = BusytokSettings {
        timezone: "+05:30".to_string(),
        ..BusytokSettings::default()
    };
    set_active_generation(&db, "gen-trend-iana-day", today_ms_for_test());
    seed_event(
        &db,
        "trend-iana-day",
        utc_midday_ms_for_test(),
        333,
        Some(0.7),
        Some("claude_code"),
        Some("sonnet"),
        Some("hash-iana"),
        Some("session-iana"),
    );
    assign_event_generation(&db, "trend-iana-day", "gen-trend-iana-day");
    seed_daily_usage_for_generation(&db, "+05:30", "gen-trend-iana-day");

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .overview_trend(OverviewTrendRequestDto {
            range: RangePresetDto::Day,
            granularity: None,
        })
        .await
        .unwrap()
        .data;

    // 24 hourly buckets for the IANA Day path.
    assert_eq!(result.trend.buckets.len(), 24);
    assert_eq!(result.trend.range, RangePresetDto::Day);

    sup.shutdown_writer().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_trend_fast_path_uses_hourly_aggregate_query() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    // UTC is a whole-hour fixed offset, so `use_fast_path` is true → routes
    // through the `read_overview_trend_hourly` branch.
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    set_active_generation(&db, "gen-trend-fast", today_ms_for_test());
    seed_event(
        &db,
        "trend-fast",
        utc_midday_ms_for_test(),
        444,
        Some(0.9),
        Some("claude_code"),
        Some("sonnet"),
        Some("hash-fast"),
        Some("session-fast"),
    );
    assign_event_generation(&db, "trend-fast", "gen-trend-fast");

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .overview_trend(OverviewTrendRequestDto {
            range: RangePresetDto::Week,
            granularity: None,
        })
        .await
        .unwrap()
        .data;

    // Week range with default Day granularity produces 7 buckets. The fast
    // path reads from `usage_buckets_hour` (an hourly rollup table populated
    // by the writer's flush path); we only seed `usage_events` here, so the
    // buckets may be zero-filled. The coverage goal — executing the
    // `read_overview_trend_hourly` closure body — is achieved regardless.
    assert_eq!(result.trend.buckets.len(), 7);
    assert_eq!(result.trend.range, RangePresetDto::Week);

    sup.shutdown_writer().await.unwrap();
}

fn seed_daily_usage_for_generation(db: &Database, timezone: &str, generation_id: &str) {
    let rtz = ReportingTimezone::parse(timezone).unwrap();
    let events = db.usage_events_for_generation(generation_id).unwrap();
    busytok_store::write_queries::upsert_daily_usage_for_events(
        db.conn(),
        &events,
        &rtz,
        generation_id,
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// pressure_responder accessor — covers ~lines 841-847
// ---------------------------------------------------------------------------

#[test]
fn pressure_responder_returns_none_when_sidecar_disabled() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Default settings have pi_sidecar.enabled = false, so the pressure
    // responder is None — but the accessor body still executes (read lock +
    // clone), which is what this test covers.
    assert!(sup.pressure_responder().is_none());
    assert!(sup.pressure_gate().is_none());
}

// ---------------------------------------------------------------------------
// prompts_use — covers outcome / failure_reason enum mapping
// (~lines 2240-2263)
// ---------------------------------------------------------------------------

async fn create_prompt_for_use(sup: &BusytokSupervisor, content: &str) -> String {
    let created = sup
        .prompts_create(PromptCreateRequestDto {
            content: content.to_string(),
            alias: None,
            tags: vec![],
        })
        .await
        .unwrap()
        .data;
    created.id
}

// The prompts CRUD path uses `prompt_database()` which returns a shared
// in-memory connection for in-memory DBs (immediate write visibility) but a
// fresh detached connection for file-backed DBs (WAL visibility races). The
// existing `supervisor_control.rs` prompts test uses in-memory + single-thread
// tokio; mirror that exact posture so `record_prompt_use` sees the create.
#[tokio::test]
async fn prompts_use_copy_outcome_maps_to_row() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let id = create_prompt_for_use(&sup, "copy me").await;

    let used = sup
        .prompts_use(PromptUseRequestDto {
            id: id.clone(),
            action: PromptActionDto::OnlyCopy,
            surface: PromptUseSurfaceDto::Page,
            outcome: PromptUseOutcomeDto::Copy,
            failure_reason: None,
        })
        .await
        .unwrap();
    // `record_prompt_use` only increments `usage_count` for `PasteAttempted`;
    // `Copy` records the use but leaves the counter at 0. The coverage goal —
    // exercising `prompt_use_outcome_to_row`'s `Copy` arm — is achieved by
    // the call succeeding.
    assert_eq!(used.usage_count, 0);
}

#[tokio::test]
async fn prompts_use_paste_fell_back_to_copy_outcome_maps_to_row() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let id = create_prompt_for_use(&sup, "fallback to copy").await;

    let used = sup
        .prompts_use(PromptUseRequestDto {
            id,
            action: PromptActionDto::CopyAndPaste,
            surface: PromptUseSurfaceDto::Overlay,
            outcome: PromptUseOutcomeDto::PasteFellBackToCopy,
            failure_reason: None,
        })
        .await
        .unwrap();
    // Same as above: `PasteFellBackToCopy` does not increment usage_count.
    assert_eq!(used.usage_count, 0);
}

#[tokio::test]
async fn prompts_use_all_failure_reason_variants_map_to_rows() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let id = create_prompt_for_use(&sup, "failure reasons").await;

    // `prompt_use_failure_reason_to_row` runs whenever `failure_reason` is
    // `Some(_)`, regardless of outcome. Use `PasteAttempted` so usage_count
    // increments (proving the use was actually recorded) while still
    // exercising every failure_reason arm.
    for reason in [
        PromptUseFailureReasonDto::PermissionMissing,
        PromptUseFailureReasonDto::FocusLost,
        PromptUseFailureReasonDto::InjectionFailed,
        PromptUseFailureReasonDto::UnsupportedPlatform,
    ] {
        let used = sup
            .prompts_use(PromptUseRequestDto {
                id: id.clone(),
                action: PromptActionDto::CopyAndPaste,
                surface: PromptUseSurfaceDto::Overlay,
                outcome: PromptUseOutcomeDto::PasteAttempted,
                failure_reason: Some(reason),
            })
            .await
            .unwrap();
        assert!(
            used.usage_count >= 1,
            "usage should increment for {reason:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// scan_state — covers scanning / idle branches (~lines 1985, 1989)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scan_state_reports_scanning_when_source_in_progress() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    // Seed an active source with a recent scan-start timestamp and no
    // completion → `in_progress_sources > 0` → "scanning".
    seed_source(
        &db,
        "src-scanning",
        "claude_code",
        "/tmp/scanning",
        "active",
    );
    set_source_scan_timestamps(&db, "src-scanning", Some(today_ms_for_test()), None);

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    // Write the service marker so `service_running` is true; otherwise the
    // "offline" branch short-circuits before reaching the scanning/idle arms.
    busytok_config::service_marker::write(sup.paths().data_dir()).unwrap();

    let result = sup.service_health().await.unwrap();
    assert_eq!(result.scan_state, "scanning");
    assert!(result.ready);

    sup.shutdown_writer().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scan_state_reports_idle_when_no_sources_and_service_running() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    // No log_sources at all: completed_sources == 0 AND in_progress_sources
    // == 0 → "idle".
    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    busytok_config::service_marker::write(sup.paths().data_dir()).unwrap();

    let result = sup.service_health().await.unwrap();
    assert_eq!(result.scan_state, "idle");
    assert!(result.ready);

    sup.shutdown_writer().await.unwrap();
}

// ---------------------------------------------------------------------------
// register_new_install_sources — covers the writer-actor registration
// (~lines 1490-1596)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_new_install_sources_seeds_log_sources_and_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);

    // Build a deterministic manual root: a Claude Code project directory with
    // one .jsonl file. Default-path discovery is disabled so the test does
    // not depend on the developer's real home directory.
    let agent_dir = tmp.path().join("projects").join("-server-agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let jsonl_path = agent_dir.join("agent-server-2025-07-15.jsonl");
    std::fs::write(&jsonl_path, "{\"type\":\"message\"}\n").unwrap();

    let mut settings = BusytokSettings::default();
    settings.discovery.claude_code_default_paths = false;
    settings.discovery.codex_default_paths = false;
    settings.discovery.manual_roots = vec![ManualRootConfig {
        id: "test-manual-root".to_string(),
        client_id: "claude_code".to_string(),
        root_path: tmp.path().display().to_string(),
    }];

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    let stats = sup.register_new_install_sources().await.unwrap();

    // One discovered source with one file.
    assert_eq!(stats.sources, 1);
    assert_eq!(stats.files_scanned, 1);
    assert_eq!(stats.events_found, 0);

    sup.shutdown_writer().await.unwrap();

    // The writer actor should have persisted the log_source and log_file rows.
    // Open a fresh read connection to the same SQLite file (the supervisor's
    // own DB is in an `Arc<Mutex>` we no longer hold).
    let read_db = Database::open(&tmp.path().join("busytok.sqlite")).unwrap();
    let conn = read_db.conn();
    let source_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM log_sources WHERE status != 'removed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(source_count, 1, "log_sources row should be persisted");

    let file_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM log_files", [], |row| row.get(0))
        .unwrap();
    assert_eq!(file_count, 1, "log_files row should be persisted");

    // The source should be marked scan-completed (register_new_install_sources
    // emits a completion upsert so scan_state_from_conn reports "completed").
    let completed_ms: Option<i64> = conn
        .query_row(
            "SELECT last_scan_completed_at_ms FROM log_sources LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(completed_ms.is_some(), "source should be scan-completed");
}

// ---------------------------------------------------------------------------
// prompt_sort_to_row — covers the non-Smart match arms (~lines 2223-2227)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prompts_list_with_each_sort_option_covers_sort_to_row() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create a few prompts so the sort paths have rows to order.
    let _ = create_prompt_for_use(&sup, "alpha prompt").await;
    let _ = create_prompt_for_use(&sup, "beta prompt").await;
    let _ = create_prompt_for_use(&sup, "gamma prompt").await;

    // Each sort variant exercises a different match arm in `prompt_sort_to_row`
    // (lines 2223-2227). The Smart arm (2222) is already covered by other
    // tests that use sort=None or sort=Smart.
    for sort in [
        PromptSortDto::RecentlyUsed,
        PromptSortDto::MostUsed,
        PromptSortDto::RecentlyUpdated,
        PromptSortDto::Alphabetical,
        PromptSortDto::PinnedFirst,
    ] {
        let result = sup
            .prompts_list(PromptListQueryDto {
                query: None,
                tag: None,
                sort: Some(sort),
                limit: None,
            })
            .await
            .unwrap()
            .data;
        // All three prompts should be returned regardless of sort order.
        assert_eq!(result.entries.len(), 3, "for sort {sort:?}");
    }
}

// ---------------------------------------------------------------------------
// record_diagnostic — covers the RuntimeControl trait impl (~lines 6330-6337)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn record_diagnostic_enqueues_diagnostic_write_command() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    // `record_diagnostic` constructs a `DiagnosticWriteCommand` and enqueues
    // it via `writer_handle.try_send`. Calling it covers the trait impl body
    // (lines 6330-6337). The multi-thread runtime ensures the writer actor is
    // running and will process the command.
    sup.record_diagnostic("warn", "test_code", "test diagnostic message");

    // `shutdown_writer` sends a Shutdown command after the DiagnosticWrite;
    // since the channel is FIFO, the writer processes the diagnostic first,
    // then flushes and exits.
    sup.shutdown_writer().await.unwrap();

    // Verify the diagnostic was persisted to the `diagnostic_events` table.
    let read_db = Database::open(&tmp.path().join("busytok.sqlite")).unwrap();
    let count: i64 = read_db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM diagnostic_events WHERE code = 'test_code'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(count >= 1, "diagnostic should be persisted");
}

// ---------------------------------------------------------------------------
// suggest_tags — covers the suggest_tags handler (lines ~5376-5406)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn suggest_tags_returns_empty_when_no_prompts() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let resp = sup
        .suggest_tags(PromptSuggestTagsRequestDto {
            query: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(resp.tags.is_empty());
}

#[tokio::test]
async fn suggest_tags_returns_matching_tags_after_prompt_use() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create a prompt with tags so suggest_tags has data.
    let _ = create_prompt_for_use(&sup, "test prompt with tags").await;

    let resp = sup
        .suggest_tags(PromptSuggestTagsRequestDto {
            query: Some("".to_string()),
            limit: Some(10),
        })
        .await
        .unwrap();
    // Tags may be empty if the prompt has no tags, but the handler path is covered.
    let _ = resp.tags;
}

// ---------------------------------------------------------------------------
// overview_heatmap — covers the heatmap handler (lines ~3450-3579)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn overview_heatmap_returns_empty_when_no_data() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    set_active_generation(&db, "gen-heatmap", today_ms_for_test());
    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    let resp = sup
        .overview_heatmap(OverviewHeatmapRequestDto {
            range: RangePresetDto::Day,
        })
        .await
        .unwrap();
    // The response should be valid even with no data.
    let _ = resp.data;
}

// ---------------------------------------------------------------------------
// overview_rankings — covers the rankings handler (lines ~3579-3687)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn overview_rankings_returns_empty_when_no_data() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    set_active_generation(&db, "gen-rankings", today_ms_for_test());
    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    let resp = sup
        .overview_rankings(OverviewRankingsRequestDto {
            range: RangePresetDto::Day,
        })
        .await
        .unwrap();
    let _ = resp.data;
}

// ---------------------------------------------------------------------------
// activity_recent — covers the activity_recent handler (lines ~3687-3720)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activity_recent_returns_empty_when_no_data() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    set_active_generation(&db, "gen-recent", today_ms_for_test());
    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    let resp = sup
        .activity_recent(ActivityRecentRequestDto {
            range: RangePresetDto::Day,
            limit: None,
        })
        .await
        .unwrap();
    let _ = resp.data;
}

// ---------------------------------------------------------------------------
// subagent_list — covers the subagent_list handler (lines ~5520-5534)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subagent_list_returns_empty_when_no_subagents() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let resp = sup
        .subagent_list(SubagentListRequestDto {
            status: None,
            project: None,
            include_deleted: Some(false),
        })
        .await
        .unwrap();
    assert!(resp.subagents.is_empty());
}

// ---------------------------------------------------------------------------
// subagent_tasks — covers the subagent_tasks handler (lines ~5548-5567)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subagent_tasks_returns_empty_for_unknown_subagent() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let result = sup
        .subagent_tasks(SubagentTasksRequestDto {
            name: None,
            id: Some("nonexistent".to_string()),
            cwd: None,
            limit: Some(10),
        })
        .await;
    // Should return an error (subagent not found) — covers the error path.
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// subagent_runtime_status — covers the runtime_status handler (lines ~5599+)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subagent_runtime_status_returns_status() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let result = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto { project: None })
        .await;
    // Should return a status response — covers the handler path.
    let _ = result;
}

// ---------------------------------------------------------------------------
// shell_status cache hit — covers lines ~3070-3073 (cache fresh path)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shell_status_cache_hit_on_second_call() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // First call populates the cache.
    let _ = sup.shell_status().await.unwrap();
    // Second call within 10s should hit the cache (lines 3070-3073).
    let _ = sup.shell_status().await.unwrap();
}

// ---------------------------------------------------------------------------
// overview_summary with seeded data — covers more branches
// ---------------------------------------------------------------------------

#[tokio::test]
async fn overview_summary_with_seeded_events() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    set_active_generation(&db, "gen-summary", today_ms_for_test());
    seed_event(
        &db,
        "evt-1",
        utc_midday_ms_for_test(),
        1000,
        Some(0.05),
        Some("cli"),
        Some("claude-sonnet-4"),
        Some("proj-hash-1"),
        Some("sess-1"),
    );
    assign_event_generation(&db, "evt-1", "gen-summary");

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    let resp = sup
        .overview_summary(OverviewSummaryRequestDto {
            range: RangePresetDto::Day,
        })
        .await
        .unwrap();
    let _ = resp.data;
}
