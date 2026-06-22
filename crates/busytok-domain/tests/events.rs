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
use busytok_domain::{AgentKind, NormalizedEvent, NormalizedUsageEvent};
use busytok_domain::{
    AgentStatus, LogFile, LogFileState, LogSource, LogSourceStatus, ModelSummary, ProjectSummary,
    RealtimeSummary, SessionSummary,
};

#[test]
fn usage_event_defaults_are_zero_and_private() {
    let event = NormalizedUsageEvent::minimal_for_test("id-1", AgentKind::ClaudeCode);
    assert_eq!(event.input_tokens, 0);
    assert_eq!(event.output_tokens, 0);
    let serialized = serde_json::to_value(&event).unwrap();
    assert!(serialized.get("prompt").is_none());
    assert!(serialized.get("response").is_none());
    assert!(serialized.get("raw_payload").is_none());
}

#[test]
fn normalized_event_can_hold_tool_and_diagnostic_events() {
    assert!(matches!(
        NormalizedEvent::diagnostic_for_test("d1"),
        NormalizedEvent::OperationalDiagnostic(_)
    ));
    assert!(matches!(
        NormalizedEvent::tool_for_test("t1"),
        NormalizedEvent::Tool(_)
    ));
}

#[test]
fn shared_source_file_and_summary_types_live_in_domain() {
    let source = LogSource::for_test("source-1", AgentKind::ClaudeCode);
    assert_eq!(source.status, LogSourceStatus::Active);
    let file = LogFile::for_test("file-1", "source-1");
    assert_eq!(file.state, LogFileState::Active);
    let status = AgentStatus::for_test(AgentKind::ClaudeCode);
    assert_eq!(status.agent, AgentKind::ClaudeCode);
    assert_eq!(SessionSummary::for_test("session-a").id, "session-a");
    assert_eq!(
        ProjectSummary::for_test("hash-a").project_hash.as_deref(),
        Some("hash-a")
    );
    assert_eq!(
        ModelSummary::for_test("claude-sonnet-4-20250514")
            .model
            .as_deref(),
        Some("claude-sonnet-4-20250514")
    );
    assert!(serde_json::to_value(RealtimeSummary::for_test())
        .unwrap()
        .is_object());
}
