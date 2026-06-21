use busytok_aggregator::{build_scan_mutations, RollupOptions};
use busytok_domain::{AgentKind, NormalizedUsageEvent};

#[test]
fn builds_daily_model_project_session_and_summary_mutations_for_scan_batch() {
    let mut event = NormalizedUsageEvent::minimal_for_test("claude:req-a", AgentKind::ClaudeCode);
    event.timestamp_ms = 1_768_435_200_000;
    event.project_hash = Some("hash-a".into());
    event.model = Some("claude-sonnet-4-20250514".into());
    event.session_id = "session-a".into();
    event.total_tokens = 150;

    let mutations = build_scan_mutations(&[event], RollupOptions::for_timezone("+08:00"), "gen-test").unwrap();
    assert!(mutations
        .daily_usage
        .iter()
        .any(|row| row.date == "2026-01-15" && row.total_tokens == 150));
    assert!(mutations
        .session_rollups
        .iter()
        .any(|row| row.id == "session-a"));
    assert!(mutations
        .project_rollups
        .iter()
        .any(|row| row.project_hash.as_deref() == Some("hash-a")));
    assert!(mutations
        .model_rollups
        .iter()
        .any(|row| row.model.as_deref() == Some("claude-sonnet-4-20250514")));
    assert!(mutations
        .realtime_summary
        .contains_key("today_total_tokens"));
}
