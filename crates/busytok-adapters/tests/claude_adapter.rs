use busytok_adapters::{AgentLogAdapter, ClaudeCodeAdapter};
use busytok_domain::{ParseContext, ParsedLogEvent};

fn unwrap_usage(events: Vec<ParsedLogEvent>) -> busytok_domain::NormalizedUsageEvent {
    match events.into_iter().next().unwrap() {
        ParsedLogEvent::Normalized(ne) => ne.into_usage().unwrap(),
        ParsedLogEvent::CodexTokenSnapshot(_) => {
            panic!("expected Normalized, got CodexTokenSnapshot")
        }
    }
}

#[test]
fn parses_basic_claude_usage() {
    let adapter = ClaudeCodeAdapter;
    let line = include_str!("../../../fixtures/claude-code/basic.jsonl")
        .lines()
        .next()
        .unwrap();
    let ctx = ParseContext::for_test("source-1", "/tmp/session.jsonl", 1, 0, line.len() as u64);
    let events = adapter.parse_line(&ctx, line).unwrap();
    let usage = unwrap_usage(events);
    assert_eq!(usage.session_id, "session-a");
    assert_eq!(usage.source_request_id.as_deref(), Some("req-a"));
    assert_eq!(usage.source_offset_start, 0);
    assert_eq!(usage.source_offset_end, line.len() as u64);
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 50);
    assert_eq!(usage.cost_usd, Some(0.001));
    assert_eq!(usage.estimated_cost_usd, None);
    assert_eq!(usage.cost_source.as_deref(), Some("source"));
}

#[test]
fn derives_project_from_cwd() {
    let adapter = ClaudeCodeAdapter;
    let line = include_str!("../../../fixtures/claude-code/basic.jsonl")
        .lines()
        .next()
        .unwrap();
    let ctx = ParseContext::for_test("source-1", "/tmp/session.jsonl", 1, 0, line.len() as u64);
    let events = adapter.parse_line(&ctx, line).unwrap();
    let usage = unwrap_usage(events);
    assert!(
        usage.project_path.is_some(),
        "project_path should be derived from cwd"
    );
    assert!(
        usage.project_hash.is_some(),
        "project_hash should be derived from cwd"
    );
    // derive_project_hash returns a hex-encoded SHA-256 hash (64 chars).
    assert_eq!(
        usage.project_hash.unwrap().len(),
        64,
        "project_hash should be a SHA-256 hex hash"
    );
}

#[test]
fn parses_cache_tokens() {
    let adapter = ClaudeCodeAdapter;
    let line = include_str!("../../../fixtures/claude-code/cache.jsonl")
        .lines()
        .next()
        .unwrap();
    let ctx = ParseContext::for_test("source-1", "/tmp/session.jsonl", 1, 0, line.len() as u64);
    let events = adapter.parse_line(&ctx, line).unwrap();
    let usage = unwrap_usage(events);
    assert_eq!(usage.cache_creation_tokens, 20);
    assert_eq!(usage.cache_read_tokens, 80);
    assert_eq!(usage.cached_input_tokens, 80);
}

#[test]
fn malformed_json_becomes_parse_error_not_panic() {
    let adapter = ClaudeCodeAdapter;
    let ctx = ParseContext::for_test("source-1", "/tmp/session.jsonl", 1, 0, 1);
    let err = adapter.parse_line(&ctx, "{").unwrap_err();
    assert!(err.to_string().contains("malformed"));
}

#[test]
fn duplicate_request_ids_with_different_message_ids_stay_distinct() {
    let adapter = ClaudeCodeAdapter;
    let ids: Vec<String> = include_str!("../../../fixtures/claude-code/duplicate-request.jsonl")
        .lines()
        .enumerate()
        .map(|(idx, line)| {
            let ctx = ParseContext::for_test(
                "source-1",
                "/tmp/session.jsonl",
                idx as u64 + 1,
                0,
                line.len() as u64,
            );
            unwrap_usage(adapter.parse_line(&ctx, line).unwrap()).id
        })
        .collect();
    assert_eq!(ids, vec!["claude:msg-c:req-dup", "claude:msg-c2:req-dup"]);
}

#[test]
fn deepseek_input_tokens_repaired_when_cache_exceeds_raw_input() {
    let adapter = ClaudeCodeAdapter;
    let line = include_str!("../../../fixtures/claude-code/deepseek-cache.jsonl")
        .lines()
        .next()
        .unwrap();
    let ctx = ParseContext::for_test("source-1", "/tmp/session.jsonl", 1, 0, line.len() as u64);
    let events = adapter.parse_line(&ctx, line).unwrap();
    let usage = unwrap_usage(events);

    // DeepSeek returns raw input as non-cached only (1,407,288) and
    // cache_read as 111,080,192. The adapter must detect the invariant
    // violation and restore input_tokens to the true total.
    assert_eq!(usage.input_tokens, 1407288 + 111080192);
    assert_eq!(usage.cached_input_tokens, 111080192);
    assert_eq!(usage.cache_read_tokens, 111080192);
    assert_eq!(usage.cache_creation_tokens, 0);
    // total_tokens = input + output + cache_read + cache_creation
    let expected_total = (1407288 + 111080192) + 50000 + 111080192 + 0;
    assert_eq!(usage.total_tokens, expected_total);
}

#[test]
fn anthropic_compliant_unchanged_by_deepseek_fix() {
    let adapter = ClaudeCodeAdapter;
    // Regular Anthropic-format where cache ≤ input — should be untouched
    let line = include_str!("../../../fixtures/claude-code/cache.jsonl")
        .lines()
        .next()
        .unwrap();
    let ctx = ParseContext::for_test("source-1", "/tmp/session.jsonl", 1, 0, line.len() as u64);
    let events = adapter.parse_line(&ctx, line).unwrap();
    let usage = unwrap_usage(events);
    // input_tokens=200, cache_creation=20, cache_read=80
    // cache 20+80=100 ≤ 200, so no repair triggered
    assert_eq!(usage.input_tokens, 200);
    assert_eq!(usage.cache_creation_tokens, 20);
    assert_eq!(usage.cached_input_tokens, 80);
    assert_eq!(usage.cache_read_tokens, 80);
}

#[test]
fn claude_event_has_non_empty_raw_event_hash() {
    let adapter = ClaudeCodeAdapter;
    let line = include_str!("../../../fixtures/claude-code/basic.jsonl")
        .lines()
        .next()
        .unwrap();
    let ctx = ParseContext::for_test("source-1", "/tmp/session.jsonl", 1, 0, line.len() as u64);
    let events = adapter.parse_line(&ctx, line).unwrap();
    let usage = unwrap_usage(events);
    assert!(
        !usage.raw_event_hash.is_empty(),
        "raw_event_hash must be present"
    );
    assert_eq!(
        usage.raw_event_hash.len(),
        64,
        "raw_event_hash must be a SHA-256 hex hash"
    );
}
