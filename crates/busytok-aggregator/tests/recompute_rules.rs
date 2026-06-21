//! Recompute correctness tests for the aggregator.
//!
//! Covers:
//! - Replace semantics: passing the final (replacement) event produces correct totals
//! - Timezone-aware date boundaries: same UTC timestamp yields different dates per offset
//! - Restart-safe idempotent replay: same events produce same daily aggregates

use busytok_aggregator::{build_scan_mutations, RollupOptions};
use busytok_domain::{AgentKind, NormalizedUsageEvent};

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
    e
}

#[test]
fn replacement_produces_correct_daily_totals() {
    // Simulate an event inserted at 100 tokens, then replaced at 300 tokens
    // (delayed-token model). build_scan_mutations is a pure function:
    // passing only the final event yields the correct final total.
    let first = make_event(
        "evt-1",
        AgentKind::Codex,
        "sess-1",
        100,
        "gpt-5",
        1_700_000_000_000,
    );
    let replacement = make_event(
        "evt-1",
        AgentKind::Codex,
        "sess-1",
        300,
        "gpt-5",
        1_700_000_000_000,
    );

    let opts = RollupOptions::for_timezone("+08:00");

    // First insert
    let m1 = build_scan_mutations(&[first.clone()], opts.clone(), "gen-test").unwrap();
    let day_total_1: i64 = m1.daily_usage.iter().map(|d| d.total_tokens).sum();
    assert_eq!(day_total_1, 100);

    // Replacement — passing the final event
    let m2 = build_scan_mutations(&[replacement], opts, "gen-test").unwrap();
    let day_total_2: i64 = m2.daily_usage.iter().map(|d| d.total_tokens).sum();
    assert_eq!(day_total_2, 300);
}

#[test]
fn date_rollups_respect_timezone_boundaries() {
    use busytok_aggregator::daily::date_from_timestamp_ms;

    // 2026-05-18 03:00 UTC = 2026-05-18 11:00 +08:00
    let ts = 1_779_073_200_000;
    let date_plus8 = date_from_timestamp_ms(ts, &busytok_domain::ReportingTimezone::parse("+08:00").unwrap()).unwrap();
    assert_eq!(date_plus8, "2026-05-18");

    // 2026-05-18 03:00 UTC = 2026-05-17 20:00 -07:00 (previous day)
    let date_minus7 = date_from_timestamp_ms(ts, &busytok_domain::ReportingTimezone::parse("-07:00").unwrap()).unwrap();
    assert_eq!(date_minus7, "2026-05-17");

    // Same timestamp, UTC
    let date_utc = date_from_timestamp_ms(ts, &busytok_domain::ReportingTimezone::parse("UTC").unwrap()).unwrap();
    assert_eq!(date_utc, "2026-05-18");
}

#[test]
fn restart_safe_idempotent_replay() {
    // Processing the same events twice must produce identical daily aggregates.
    let events = vec![
        make_event(
            "evt-a",
            AgentKind::ClaudeCode,
            "sess-1",
            500,
            "claude-sonnet",
            1_700_000_000_000,
        ),
        make_event(
            "evt-b",
            AgentKind::ClaudeCode,
            "sess-1",
            300,
            "claude-sonnet",
            1_700_000_000_100,
        ),
    ];

    let opts = RollupOptions::for_timezone("UTC");
    let m1 = build_scan_mutations(&events, opts.clone(), "gen-test").unwrap();
    let m2 = build_scan_mutations(&events, opts, "gen-test").unwrap();

    // Both runs must produce the same number of daily rows and the same totals.
    assert_eq!(m1.daily_usage.len(), m2.daily_usage.len());
    let total1: i64 = m1.daily_usage.iter().map(|d| d.total_tokens).sum();
    let total2: i64 = m2.daily_usage.iter().map(|d| d.total_tokens).sum();
    assert_eq!(total1, total2);
    assert_eq!(total1, 800);
}
