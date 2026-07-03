//! Coverage gap tests for `busytok-aggregator`.
//!
//! Targets uncovered source lines reported by `cargo llvm-cov`:
//! - `mutations.rs`: `RollupOptions` constructors (`utc`/`local`/`Default`/
//!   `for_timezone` fallback), `merge_cost` branches, `rebuild_sessions`,
//!   `rebuild_projects`, `rebuild_model_summaries`, `session_rollups_to_rows`,
//!   `project_rollups_to_rows`, `model_rollups_to_rows`.
//! - `daily.rs`: private `merge_cost` branches (covered indirectly through
//!   `build_weekly_usage_value`), `build_weekly_usage_value` itself.
//! - `blocks.rs`: `calculate_burn_rate` Moderate/High status branches and
//!   zero/negative-duration `None` returns; `identify_session_blocks` gap
//!   block + `usage_limit_reset` propagation paths.
//! - `summary.rs`: private `merge_opt_cost` branches (covered indirectly
//!   through `build_scan_mutations` → `build_realtime_summary`).

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

use busytok_aggregator::{
    build_scan_mutations, build_weekly_usage_value, calculate_burn_rate, identify_session_blocks,
    model_rollups_to_rows, project_rollups_to_rows, rebuild_model_summaries, rebuild_projects,
    rebuild_sessions, session_rollups_to_rows, RollupOptions,
};
use busytok_domain::{
    AgentKind, BillingBlock, BurnStatus, ModelSummary, NormalizedUsageEvent, ProjectSummary,
    SessionSummary,
};

/// Build a `NormalizedUsageEvent` with the minimum fields needed for
/// aggregator tests. Mirrors the `make_event` pattern in
/// `recompute_rules.rs` to avoid touching real log sources.
fn make_event(
    id: &str,
    agent: AgentKind,
    session_id: &str,
    tokens: i64,
    model: &str,
    ts_ms: i64,
) -> NormalizedUsageEvent {
    let mut e = NormalizedUsageEvent::minimal_for_test(id, agent);
    e.session_id = session_id.to_string();
    e.total_tokens = tokens;
    e.input_tokens = tokens / 2;
    e.output_tokens = tokens / 2;
    e.model = if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    };
    e.timestamp_ms = ts_ms;
    e.project_hash = Some("proj-hash-123".to_string());
    e.project_path = Some("/home/user/proj".to_string());
    e
}

// ── RollupOptions constructors ─────────────────────────────────────────

#[test]
fn rollup_options_utc_has_utc_timezone() {
    let opts = RollupOptions::utc();
    assert_eq!(opts.timezone.canonical_name(), "UTC");
}

#[test]
fn rollup_options_default_is_local() {
    // `Default` delegates to `local()`. Both must produce a non-empty
    // canonical name and agree on the value.
    let def = RollupOptions::default();
    let loc = RollupOptions::local();
    assert!(!def.timezone.canonical_name().is_empty());
    assert_eq!(
        def.timezone.canonical_name(),
        loc.timezone.canonical_name(),
        "Default and local() must agree"
    );
}

#[test]
fn rollup_options_local_resolves_non_empty() {
    let opts = RollupOptions::local();
    let name = opts.timezone.canonical_name();
    assert!(!name.is_empty(), "local timezone must resolve to a name");
    assert_ne!(name, "local", "local must be resolved, not literal");
}

#[test]
fn rollup_options_for_timezone_falls_back_to_utc_on_invalid_input() {
    // `ReportingTimezone::parse("not-a-zone")` returns Err, so the
    // fallback branch must produce a UTC zone.
    let opts = RollupOptions::for_timezone("not-a-zone");
    assert_eq!(opts.timezone.canonical_name(), "UTC");
}

#[test]
fn rollup_options_for_timezone_accepts_iana() {
    let opts = RollupOptions::for_timezone("Asia/Shanghai");
    assert_eq!(opts.timezone.canonical_name(), "Asia/Shanghai");
    assert!(opts.timezone.is_iana());
}

// ── rebuild_sessions ──────────────────────────────────────────────────

#[test]
fn rebuild_sessions_groups_by_session_and_tracks_timestamps() {
    let events = vec![
        make_event(
            "a",
            AgentKind::ClaudeCode,
            "sess-1",
            100,
            "claude-sonnet-4",
            1_000,
        ),
        make_event(
            "b",
            AgentKind::ClaudeCode,
            "sess-1",
            200,
            "claude-haiku",
            2_000,
        ),
        make_event(
            "c",
            AgentKind::ClaudeCode,
            "sess-1",
            50,
            "claude-sonnet-4",
            500,
        ),
    ];

    let rows = rebuild_sessions(&events, "UTC");
    assert_eq!(rows.len(), 1, "all events share one session_id");

    let row = &rows[0];
    assert_eq!(row.id, "sess-1");
    assert_eq!(row.agent, "claude_code");
    assert_eq!(row.started_at_ms, 500);
    assert_eq!(row.last_seen_at_ms, 2_000);
    assert_eq!(row.total_tokens, 350);
    assert_eq!(row.event_count, 3);
    assert_eq!(row.is_active, 0, "rebuild marks all sessions inactive");

    // Models list is deduplicated: claude-sonnet-4 + claude-haiku.
    let models: Vec<String> =
        serde_json::from_str(&row.model_list_json).expect("model_list_json is valid JSON");
    assert_eq!(models.len(), 2);
    assert!(models.contains(&"claude-sonnet-4".to_string()));
    assert!(models.contains(&"claude-haiku".to_string()));
}

#[test]
fn rebuild_sessions_skips_empty_session_id() {
    let mut e1 = make_event("a", AgentKind::ClaudeCode, "", 100, "m", 1_000);
    let e2 = make_event("b", AgentKind::ClaudeCode, "sess-2", 50, "m", 2_000);
    let rows = rebuild_sessions(&[e1.clone(), e2], "UTC");

    // Empty session_id is skipped.sess-2 is the only retained row.
    e1.session_id = String::new();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "sess-2");
}

#[test]
fn rebuild_sessions_dedupes_models() {
    let events = vec![
        make_event("a", AgentKind::Codex, "s1", 10, "gpt-5", 1_000),
        make_event("b", AgentKind::Codex, "s1", 10, "gpt-5", 2_000),
        make_event("c", AgentKind::Codex, "s1", 10, "gpt-5", 3_000),
    ];
    let rows = rebuild_sessions(&events, "UTC");
    let models: Vec<String> = serde_json::from_str(&rows[0].model_list_json).expect("valid JSON");
    assert_eq!(models, vec!["gpt-5".to_string()]);
}

#[test]
fn rebuild_sessions_merges_costs_all_branches() {
    // Drive every branch of `merge_cost`:
    //   (None, Some) on first event,
    //   (Some, Some) on second event with cost,
    //   (Some, None) on third event with no cost.
    let mut e1 = make_event("a", AgentKind::ClaudeCode, "s1", 10, "m", 1_000);
    e1.cost_usd = Some(0.10);
    let mut e2 = make_event("b", AgentKind::ClaudeCode, "s1", 10, "m", 2_000);
    e2.cost_usd = Some(0.20);
    let mut e3 = make_event("c", AgentKind::ClaudeCode, "s1", 10, "m", 3_000);
    e3.cost_usd = None;

    let rows = rebuild_sessions(&[e1, e2, e3], "UTC");
    assert_eq!(rows.len(), 1);
    assert!(
        (rows[0].total_cost_usd.unwrap() - 0.30).abs() < 0.0001,
        "got {:?}",
        rows[0].total_cost_usd
    );
}

// ── rebuild_projects ──────────────────────────────────────────────────

#[test]
fn rebuild_projects_groups_by_hash_and_derives_display_name() {
    let events = vec![
        make_event("a", AgentKind::ClaudeCode, "s1", 100, "m", 1_000),
        make_event("b", AgentKind::ClaudeCode, "s2", 50, "m", 2_000),
    ];
    let rows = rebuild_projects(&events, "UTC");
    assert_eq!(rows.len(), 1, "all events share project_hash");

    let row = &rows[0];
    assert_eq!(row.id, "proj-hash-123");
    assert_eq!(row.project_hash, "proj-hash-123");
    assert_eq!(row.agent.as_deref(), Some("claude_code"));
    assert_eq!(row.first_seen_at_ms, 1_000);
    assert_eq!(row.last_seen_at_ms, 2_000);
    assert_eq!(row.total_tokens, 150);
    assert_eq!(row.session_count, 2, "two distinct session ids");

    // Display name is the last path component of `/home/user/proj`.
    assert_eq!(row.display_name.as_deref(), Some("proj"));
}

#[test]
fn rebuild_projects_skips_empty_hash() {
    let mut e_with_hash = make_event("a", AgentKind::ClaudeCode, "s1", 100, "m", 1_000);
    let mut e_no_hash = make_event("b", AgentKind::ClaudeCode, "s2", 50, "m", 2_000);
    e_no_hash.project_hash = None;
    e_with_hash.project_hash = Some("real-hash".to_string());

    let rows = rebuild_projects(&[e_with_hash, e_no_hash], "UTC");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].project_hash, "real-hash");
}

#[test]
fn rebuild_projects_handles_path_without_slash() {
    // `display_name` derivation: `rsplit('/').next()` on a bare name returns
    // the name itself. Verify that branch.
    let mut e = make_event("a", AgentKind::ClaudeCode, "s1", 100, "m", 1_000);
    e.project_hash = Some("hash-bare".to_string());
    e.project_path = Some("barename".to_string());
    let rows = rebuild_projects(&[e], "UTC");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].display_name.as_deref(), Some("barename"));
}

#[test]
fn rebuild_projects_handles_none_project_path() {
    let mut e = make_event("a", AgentKind::ClaudeCode, "s1", 100, "m", 1_000);
    e.project_hash = Some("hash-no-path".to_string());
    e.project_path = None;
    let rows = rebuild_projects(&[e], "UTC");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].display_name.is_none());
    assert!(rows[0].project_path.is_none());
}

#[test]
fn rebuild_projects_merges_costs_all_branches() {
    let mut e1 = make_event("a", AgentKind::ClaudeCode, "s1", 10, "m", 1_000);
    e1.cost_usd = Some(0.5);
    let mut e2 = make_event("b", AgentKind::ClaudeCode, "s1", 10, "m", 2_000);
    e2.cost_usd = None;
    let mut e3 = make_event("c", AgentKind::ClaudeCode, "s2", 10, "m", 3_000);
    e3.cost_usd = Some(1.5);

    let rows = rebuild_projects(&[e1, e2, e3], "UTC");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].total_cost_usd, Some(2.0));
}

// ── rebuild_model_summaries ───────────────────────────────────────────

#[test]
fn rebuild_model_summaries_groups_by_model() {
    let events = vec![
        make_event(
            "a",
            AgentKind::ClaudeCode,
            "s1",
            100,
            "claude-sonnet",
            1_000,
        ),
        make_event("b", AgentKind::ClaudeCode, "s1", 50, "claude-sonnet", 2_000),
        make_event("c", AgentKind::ClaudeCode, "s1", 30, "claude-haiku", 3_000),
    ];
    let rows = rebuild_model_summaries(&events);
    assert_eq!(rows.len(), 2);

    let by_model: std::collections::HashMap<String, (i64, Option<f64>, i64)> = rows
        .into_iter()
        .map(|r| (r.model, (r.total_tokens, r.total_cost_usd, r.event_count)))
        .collect();
    let sonnet = by_model.get("claude-sonnet").expect("sonnet row present");
    assert_eq!(sonnet.0, 150);
    assert_eq!(sonnet.2, 2);
    let haiku = by_model.get("claude-haiku").expect("haiku row present");
    assert_eq!(haiku.0, 30);
    assert_eq!(haiku.2, 1);
}

#[test]
fn rebuild_model_summaries_skips_empty_model() {
    let mut e_with_model = make_event("a", AgentKind::ClaudeCode, "s1", 100, "real", 1_000);
    let mut e_no_model = make_event("b", AgentKind::ClaudeCode, "s2", 50, "", 2_000);
    e_no_model.model = None;
    e_with_model.model = Some("real".to_string());

    let rows = rebuild_model_summaries(&[e_with_model, e_no_model]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].model, "real");
}

// ── *_to_rows conversion functions ────────────────────────────────────

#[test]
fn session_rollups_to_rows_preserves_fields() {
    let summary = SessionSummary {
        id: "sess-xyz".to_string(),
        agent: Some(AgentKind::Codex),
        project_hash: Some("hash-1".to_string()),
        started_at_ms: 1_000,
        last_seen_at_ms: 2_000,
        model_list_json: Some(r#"["m1","m2"]"#.to_string()),
        total_tokens: 500,
        total_cost_usd: Some(0.25),
        event_count: 4,
    };

    let rows = session_rollups_to_rows(&[summary]);
    assert_eq!(rows.len(), 1);

    let r = &rows[0];
    assert_eq!(r.id, "sess-xyz");
    assert_eq!(r.agent, "codex");
    assert_eq!(r.project_hash.as_deref(), Some("hash-1"));
    assert_eq!(r.started_at_ms, 1_000);
    assert_eq!(r.last_seen_at_ms, 2_000);
    assert_eq!(r.model_list_json, r#"["m1","m2"]"#);
    assert_eq!(r.total_tokens, 500);
    assert_eq!(r.total_cost_usd, Some(0.25));
    assert_eq!(r.event_count, 4);
    assert_eq!(r.is_active, 0);
}

#[test]
fn session_rollups_to_rows_defaults_missing_model_list() {
    let summary = SessionSummary {
        id: "sess".to_string(),
        agent: None,
        project_hash: None,
        started_at_ms: 0,
        last_seen_at_ms: 0,
        model_list_json: None,
        total_tokens: 0,
        total_cost_usd: None,
        event_count: 0,
    };

    let rows = session_rollups_to_rows(&[summary]);
    assert_eq!(rows.len(), 1);
    // When `model_list_json` is None, conversion falls back to `"[]"`.
    assert_eq!(rows[0].model_list_json, "[]");
    assert_eq!(rows[0].agent, "");
    assert!(rows[0].project_hash.is_none());
}

#[test]
fn project_rollups_to_rows_preserves_fields() {
    let summary = ProjectSummary {
        project_hash: Some("hash-1".to_string()),
        project_path: Some("/home/user/proj".to_string()),
        total_tokens: 1_000,
        total_cost_usd: Some(0.5),
        event_count: 10,
        session_count: 3,
    };

    let rows = project_rollups_to_rows(&[summary]);
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.id, "hash-1");
    assert_eq!(r.project_hash, "hash-1");
    assert_eq!(r.project_path.as_deref(), Some("/home/user/proj"));
    assert_eq!(r.display_name.as_deref(), Some("proj"));
    assert_eq!(r.total_tokens, 1_000);
    assert_eq!(r.total_cost_usd, Some(0.5));
    assert_eq!(r.session_count, 3);
}

#[test]
fn project_rollups_to_rows_handles_missing_hash() {
    let summary = ProjectSummary {
        project_hash: None,
        project_path: None,
        total_tokens: 0,
        total_cost_usd: None,
        event_count: 0,
        session_count: 0,
    };
    let rows = project_rollups_to_rows(&[summary]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "");
    assert_eq!(rows[0].project_hash, "");
    assert!(rows[0].project_path.is_none());
    assert!(rows[0].display_name.is_none());
}

#[test]
fn model_rollups_to_rows_preserves_fields() {
    let summary = ModelSummary {
        model: Some("gpt-5".to_string()),
        total_tokens: 7_000,
        total_cost_usd: Some(1.25),
        event_count: 9,
    };
    let rows = model_rollups_to_rows(&[summary]);
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.model, "gpt-5");
    assert_eq!(r.total_tokens, 7_000);
    assert_eq!(r.total_cost_usd, Some(1.25));
    assert_eq!(r.event_count, 9);
}

#[test]
fn model_rollups_to_rows_handles_none_model() {
    let summary = ModelSummary {
        model: None,
        total_tokens: 0,
        total_cost_usd: None,
        event_count: 0,
    };
    let rows = model_rollups_to_rows(&[summary]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].model, "");
    assert_eq!(rows[0].total_tokens, 0);
    assert!(rows[0].total_cost_usd.is_none());
}

// ── build_weekly_usage_value (daily.rs) ───────────────────────────────

#[test]
fn build_weekly_usage_value_groups_events_by_week() {
    // Two events on different days but the same Sunday-start week:
    // 2026-01-12 (Sunday) and 2026-01-15 (Wednesday), both UTC.
    // ts_ms = 1736640000 * 1000 = 2025-01-12 00:00 UTC. Wait — use 2026 dates.
    // 2026-01-12 00:00:00 UTC = 1768224000 sec.
    let sunday_ts = 1_768_224_000_000i64;
    let wed_ts = 1_768_483_200_000i64; // 2026-01-15 00:00:00 UTC (3 days later)

    let events = vec![
        make_event("a", AgentKind::ClaudeCode, "s1", 100, "m1", sunday_ts),
        make_event("b", AgentKind::ClaudeCode, "s1", 50, "m1", wed_ts),
    ];

    let val = build_weekly_usage_value(&events, RollupOptions::utc()).expect("ok");
    let arr = val.as_array().expect("array");
    assert_eq!(arr.len(), 1, "events share week + agent + model");

    let row = &arr[0];
    assert_eq!(row["week"], "2026-01-11");
    assert_eq!(row["agent"], "claude_code");
    assert_eq!(row["model"], "m1");
    assert_eq!(row["total_tokens"], 150);
    assert_eq!(row["event_count"], 2);
    assert_eq!(row["timezone"], "UTC");
}

#[test]
fn build_weekly_usage_value_empty_events_returns_empty_array() {
    let val = build_weekly_usage_value(&[], RollupOptions::utc()).expect("ok");
    assert!(val.as_array().unwrap().is_empty());
}

#[test]
fn build_weekly_usage_value_merges_costs_all_branches() {
    // Cover all four `merge_cost` branches in daily.rs:
    // (Some, Some), (Some, None), (None, Some), (None, None).
    let ts = 1_768_224_000_000i64; // 2026-01-12 00:00 UTC

    let mut e1 = make_event("a", AgentKind::ClaudeCode, "s1", 10, "m", ts);
    e1.cost_usd = Some(0.10);
    e1.estimated_cost_usd = Some(0.05);
    let mut e2 = make_event("b", AgentKind::ClaudeCode, "s1", 10, "m", ts);
    e2.cost_usd = Some(0.20);
    e2.estimated_cost_usd = None;
    let mut e3 = make_event("c", AgentKind::ClaudeCode, "s1", 10, "m", ts);
    e3.cost_usd = None;
    e3.estimated_cost_usd = Some(0.30);
    let mut e4 = make_event("d", AgentKind::ClaudeCode, "s1", 10, "m", ts);
    e4.cost_usd = None;
    e4.estimated_cost_usd = None;

    let val = build_weekly_usage_value(&[e1, e2, e3, e4], RollupOptions::utc()).expect("ok");
    let arr = val.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let row = &arr[0];
    // cost_usd: 0.10 + 0.20 = 0.30 (Some+Some, Some+None for e3, None for e4 -> still 0.30)
    assert!(
        (row["cost_usd"].as_f64().unwrap() - 0.30).abs() < 0.0001,
        "got {}",
        row["cost_usd"]
    );
    // estimated_cost_usd: 0.05 + 0.30 = 0.35 (None initial + Some, Some+None, None+None)
    assert!(
        (row["estimated_cost_usd"].as_f64().unwrap() - 0.35).abs() < 0.0001,
        "got {}",
        row["estimated_cost_usd"]
    );
}

// ── calculate_burn_rate (blocks.rs) ───────────────────────────────────

fn make_burn_block(
    input: i64,
    output: i64,
    duration_ms: i64,
    cost_usd: Option<f64>,
) -> BillingBlock {
    // duration_ms measures (actual_end - start). Pick start=0, actual_end=duration_ms.
    BillingBlock {
        id: "test".to_string(),
        start_time_ms: 0,
        end_time_ms: duration_ms + 3_600_000,
        actual_end_time_ms: Some(duration_ms),
        is_active: false,
        is_gap: false,
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        cost_usd,
        estimated_cost_usd: None,
        models: vec![],
        event_count: 1,
        usage_limit_reset_time_ms: None,
        agent: Some("claude_code".to_string()),
    }
}

#[test]
fn calculate_burn_rate_normal_status() {
    // input+output = 1000 tokens over 10 minutes -> 100 tpm (under 2000 = Normal).
    let block = make_burn_block(500, 500, 600_000, Some(0.01));
    let rate = calculate_burn_rate(&block).expect("Some BurnRate");
    assert_eq!(rate.status, BurnStatus::Normal);
    assert!((rate.tokens_per_minute - 100.0).abs() < 0.01);
    assert!((rate.cost_per_hour.unwrap() - 0.06).abs() < 0.001);
}

#[test]
fn calculate_burn_rate_moderate_status() {
    // 2500 non-cache tokens over 1 min -> 2500 tpm (in [2000, 5000) = Moderate).
    let block = make_burn_block(1250, 1250, 60_000, None);
    let rate = calculate_burn_rate(&block).expect("Some");
    assert_eq!(rate.status, BurnStatus::Moderate);
}

#[test]
fn calculate_burn_rate_high_status() {
    // 6000 non-cache tokens over 1 min -> 6000 tpm (>= 5000 = High).
    let block = make_burn_block(3000, 3000, 60_000, None);
    let rate = calculate_burn_rate(&block).expect("Some");
    assert_eq!(rate.status, BurnStatus::High);
}

#[test]
fn calculate_burn_rate_returns_none_for_zero_duration() {
    let block = make_burn_block(100, 100, 0, None);
    assert!(calculate_burn_rate(&block).is_none());
}

#[test]
fn calculate_burn_rate_returns_none_for_negative_duration() {
    // Negative duration = actual_end < start. Should return None.
    let mut block = make_burn_block(100, 100, 600_000, None);
    block.start_time_ms = 1_000_000;
    block.actual_end_time_ms = Some(500_000); // < start
    assert!(calculate_burn_rate(&block).is_none());
}

#[test]
fn calculate_burn_rate_with_none_cost_returns_none_cost_per_hour() {
    let block = make_burn_block(500, 500, 600_000, None);
    let rate = calculate_burn_rate(&block).expect("Some");
    assert!(rate.cost_per_hour.is_none());
}

#[test]
fn calculate_burn_rate_returns_none_for_gap_block() {
    let mut block = make_burn_block(500, 500, 600_000, None);
    block.is_gap = true;
    assert!(calculate_burn_rate(&block).is_none());
}

#[test]
fn calculate_burn_rate_returns_none_when_actual_end_is_none() {
    let mut block = make_burn_block(500, 500, 600_000, None);
    block.actual_end_time_ms = None;
    assert!(calculate_burn_rate(&block).is_none());
}

// ── identify_session_blocks (blocks.rs) ──────────────────────────────

#[test]
fn identify_session_blocks_propagates_usage_limit_reset() {
    let mut e1 = make_event("a", AgentKind::ClaudeCode, "s1", 100, "m1", 1_000);
    e1.usage_limit_reset_time_ms = Some(9_999_999);
    let mut e2 = make_event("b", AgentKind::ClaudeCode, "s1", 50, "m2", 2_000);
    e2.usage_limit_reset_time_ms = Some(8_888_888);

    let blocks = identify_session_blocks(&[e1, e2], 5);
    assert_eq!(blocks.len(), 1);
    // The last non-None value wins.
    assert_eq!(
        blocks[0].usage_limit_reset_time_ms,
        Some(8_888_888),
        "last non-None usage_limit_reset wins"
    );
    // Models are deduplicated.
    assert_eq!(blocks[0].models.len(), 2);
}

#[test]
fn identify_session_blocks_creates_gap_block_for_long_gap() {
    // Two events more than 5 hours apart should yield: real block, gap block, real block.
    let gap_ms = 6 * 60 * 60 * 1000;
    let events = vec![
        make_event("a", AgentKind::ClaudeCode, "s1", 100, "m1", 1_000),
        make_event("b", AgentKind::ClaudeCode, "s2", 50, "m2", 1_000 + gap_ms),
    ];
    let blocks = identify_session_blocks(&events, 5);
    assert_eq!(blocks.len(), 3, "block + gap + block");
    assert!(!blocks[0].is_gap);
    assert!(blocks[1].is_gap, "middle block should be the gap");
    assert_eq!(blocks[1].event_count, 0);
    assert!(blocks[1].cost_usd.is_none());
    assert!(!blocks[2].is_gap);
}

#[test]
fn identify_session_blocks_with_multiple_models_dedups() {
    let mut e1 = make_event("a", AgentKind::ClaudeCode, "s1", 10, "m1", 1_000);
    let mut e2 = make_event("b", AgentKind::ClaudeCode, "s1", 20, "m1", 2_000);
    let mut e3 = make_event("c", AgentKind::ClaudeCode, "s1", 30, "m2", 3_000);
    e1.model = Some("m1".to_string());
    e2.model = Some("m1".to_string());
    e3.model = Some("m2".to_string());
    let _ = (&mut e1, &mut e2, &mut e3); // suppress unused_mut warnings

    let blocks = identify_session_blocks(&[e1, e2, e3], 5);
    assert_eq!(blocks.len(), 1);
    let models = &blocks[0].models;
    assert_eq!(models.len(), 2, "dedup model list");
    assert!(models.contains(&"m1".to_string()));
    assert!(models.contains(&"m2".to_string()));
}

#[test]
fn identify_session_blocks_returns_empty_for_empty_input() {
    let blocks = identify_session_blocks(&[], 5);
    assert!(blocks.is_empty());
}

// ── build_scan_mutations (summary.rs merge_opt_cost branches) ─────────

#[test]
fn build_scan_mutations_empty_events_returns_empty_mutations() {
    let mutations = build_scan_mutations(&[], RollupOptions::utc(), "gen-empty").expect("ok");
    assert!(mutations.daily_usage.is_empty());
    assert!(mutations.session_rollups.is_empty());
    assert!(mutations.project_rollups.is_empty());
    assert!(mutations.model_rollups.is_empty());
    // Realtime summary always contains keys (today_total_tokens, etc.).
    assert!(mutations
        .realtime_summary
        .contains_key("today_total_tokens"));
    assert!(mutations.realtime_summary.contains_key("top_projects"));
    assert!(mutations.realtime_summary.contains_key("top_models"));
}

#[test]
fn build_scan_mutations_merges_costs_across_events() {
    // Drive `merge_opt_cost` branches in summary.rs through events
    // with mixed Some/None cost_usd/estimated_cost_usd combinations.
    let ts = 1_768_224_000_000i64; // 2026-01-12 00:00 UTC (today-ish)

    let mut e1 = make_event("a", AgentKind::ClaudeCode, "s1", 100, "m1", ts);
    e1.project_hash = Some("proj-1".to_string());
    e1.cost_usd = Some(0.10);
    e1.estimated_cost_usd = Some(0.05);

    let mut e2 = make_event("b", AgentKind::ClaudeCode, "s1", 50, "m1", ts + 1_000);
    e2.project_hash = Some("proj-1".to_string());
    e2.cost_usd = Some(0.20);
    e2.estimated_cost_usd = None;

    let mut e3 = make_event("c", AgentKind::ClaudeCode, "s2", 25, "m2", ts + 2_000);
    e3.project_hash = Some("proj-1".to_string());
    e3.cost_usd = None;
    e3.estimated_cost_usd = Some(0.30);

    let mutations =
        build_scan_mutations(&[e1, e2, e3], RollupOptions::utc(), "gen-test").expect("ok");

    // top_projects aggregates by project_hash.
    let top_projects = mutations
        .realtime_summary
        .get("top_projects")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(top_projects.len(), 1);
    let proj = &top_projects[0];
    assert_eq!(proj["project_hash"], "proj-1");
    // top_projects cost = sum of `cost_usd.or(estimated_cost_usd)` for each event:
    //   e1: 0.10 (cost_usd wins), e2: 0.20 (cost_usd), e3: 0.30 (estimated, no cost_usd)
    //   = 0.60.
    assert!(
        (proj["total_cost_usd"].as_f64().unwrap() - 0.60).abs() < 0.0001,
        "got {}",
        proj["total_cost_usd"]
    );

    // top_models aggregates by model.
    let top_models = mutations
        .realtime_summary
        .get("top_models")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(top_models.len(), 2);

    // model rollups also exercise merge_cost in mutations.rs.
    assert_eq!(mutations.model_rollups.len(), 2);
    let m1_row = mutations
        .model_rollups
        .iter()
        .find(|r| r.model.as_deref() == Some("m1"))
        .expect("m1 row");
    // m1 events: e1 (cost_usd=0.10) + e2 (cost_usd=0.20) = 0.30.
    assert!(
        (m1_row.total_cost_usd.unwrap() - 0.30).abs() < 0.0001,
        "got {:?}",
        m1_row.total_cost_usd
    );
}
