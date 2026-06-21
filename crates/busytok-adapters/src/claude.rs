//! Claude Code JSONL log adapter.
//!
//! Parses the JSONL format written by Claude Code into normalized usage events.
//! This adapter never reads prompt/response content, tool arguments, or API keys.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use busytok_domain::{
    derive_session_id, metadata_event_hash, now_ms, AgentKind, MetadataFingerprint,
    NormalizedEvent, NormalizedUsageEvent, ParseContext, ParseError, ParsedLogEvent,
    UsageWritePolicy,
};

use crate::adapter::AgentLogAdapter;

/// Adapter for parsing Claude Code JSONL usage logs.
#[derive(Debug, Default, Clone)]
pub struct ClaudeCodeAdapter;

/// Top-level structure of a Claude Code JSONL line.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeLine {
    cwd: Option<String>,
    session_id: Option<String>,
    timestamp: Option<String>,
    version: Option<String>,
    message: Option<ClaudeMessage>,
    request_id: Option<String>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
    #[serde(rename = "isApiErrorMessage")]
    is_api_error_message: Option<bool>,
}

/// The `message` object within a Claude Code line.
#[derive(Debug, Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ClaudeUsage>,
    /// Content array, only accessed for system-level error metadata
    /// (usage limit reset timestamp extraction). Never read for prompt/response content.
    content: Option<Vec<serde_json::Value>>,
}

/// The `message.usage` object within a Claude Code line.
#[derive(Debug, Deserialize)]
struct ClaudeUsage {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    speed: Option<String>,
}

impl AgentLogAdapter for ClaudeCodeAdapter {
    fn agent(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }

    fn can_parse_path(&self, path: &Path) -> bool {
        // Claude Code logs live in ~/.claude/projects/*/sessions/*/JSONL
        // or are named *.jsonl. Exclude paths that belong to other agents.
        let name_lower = path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        (path.extension().is_some_and(|ext| ext == "jsonl") || name_lower.ends_with("jsonl"))
            && !name_lower.contains("codex")
    }

    fn parse_line(
        &self,
        ctx: &ParseContext,
        line: &str,
    ) -> Result<Vec<ParsedLogEvent>, ParseError> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(vec![]);
        }

        let parsed: ClaudeLine =
            serde_json::from_str(trimmed).map_err(|e| ParseError::MalformedJson {
                reason: format!("malformed JSON: {e}"),
            })?;

        // Only process lines that have message.usage (actual API usage lines).
        let message = parsed.message.as_ref();
        let usage = message.and_then(|m| m.usage.as_ref());

        let (usage_val, message_ref) = match (usage, message) {
            (Some(u), Some(m)) => (u, m),
            _ => return Ok(vec![]), // Non-usage lines are silently ignored.
        };

        // Claude Code writes model="<synthetic>" for API error placeholders
        // with zero usage. Skip these — they are not real token consumption.
        if message_ref.model.as_deref() == Some("<synthetic>") {
            return Ok(vec![]);
        }

        let raw_input = usage_val.input_tokens.unwrap_or(0);
        let output_tokens = usage_val.output_tokens.unwrap_or(0);
        let cache_read_tokens = usage_val.cache_read_input_tokens.unwrap_or(0);
        let cache_creation_tokens = usage_val.cache_creation_input_tokens.unwrap_or(0);
        let cached_input_tokens = cache_read_tokens;

        // DeepSeek's Anthropic-format API returns input_tokens as
        // non-cached-only, violating the Anthropic invariant
        // cache_read + cache_creation ≤ input_tokens. When cache
        // exceeds raw_input, add cache_read to restore the total-input
        // semantic that downstream pricing expects.
        let input_tokens = if cache_read_tokens + cache_creation_tokens > raw_input {
            raw_input + cache_read_tokens
        } else {
            raw_input
        };

        let session_id = derive_session_id(parsed.session_id.as_deref(), &ctx.source_file_id);

        // Timestamp: parse ISO 8601 or use current time.
        let timestamp_ms = parsed
            .timestamp
            .as_deref()
            .and_then(parse_iso8601_to_ms)
            .unwrap_or_else(now_ms);

        // Match ccusage semantics: cached prompt tokens are part of total token
        // consumption and must be counted in historical rollups.
        let total_tokens = input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens;

        // Compute raw_event_hash from metadata-only fingerprint.
        let fingerprint = MetadataFingerprint::new("claude_code", &session_id)
            .request_id(parsed.request_id.as_deref().unwrap_or(""))
            .message_id(message_ref.id.as_deref().unwrap_or(""))
            .tokens(input_tokens, output_tokens)
            .total_tokens(total_tokens);
        let raw_event_hash = metadata_event_hash(&fingerprint);

        // Match observed ccusage behavior: repeated updates for the same
        // message collapse onto the latest usage even when request_id is absent.
        let event_id = if let Some(mid) = message_ref.id.as_deref() {
            if let Some(rid) = parsed.request_id.as_deref() {
                format!("claude:{mid}:{rid}")
            } else {
                format!("claude:{mid}")
            }
        } else if let Some(rid) = parsed.request_id.as_deref() {
            format!("claude:req:{rid}")
        } else {
            format!(
                "claude:{}:{}:{raw_event_hash}",
                ctx.source_file_id, ctx.source_offset_start
            )
        };

        // Cost handling:
        // - costUSD present -> cost_usd = Some(costUSD), cost_source = "source"
        // - costUSD absent  -> cost_usd = None, cost_source = "unknown"
        let (cost_usd, cost_source) = match parsed.cost_usd {
            Some(cost) => (Some(cost), Some("source".to_string())),
            None => (None, Some("unknown".to_string())),
        };

        // Extract API usage limit reset time from error messages.
        let usage_limit_reset = if parsed.is_api_error_message.unwrap_or(false) {
            extract_usage_limit_reset(&parsed)
        } else {
            None
        };

        let now = now_ms();

        let event = NormalizedUsageEvent {
            id: event_id,
            agent: AgentKind::ClaudeCode,
            source_file_id: ctx.source_file_id.clone(),
            source_path: ctx.source_path.clone(),
            source_line: ctx.source_line,
            source_offset_start: ctx.source_offset_start,
            source_offset_end: ctx.source_offset_end,
            session_id,
            turn_id: None,
            source_request_id: parsed.request_id,
            message_id: message_ref.id.clone(),
            timestamp_ms,
            project_path: parsed
                .cwd
                .as_deref()
                .and_then(|p| busytok_domain::normalize_project_path(p).ok()),
            project_hash: parsed
                .cwd
                .as_deref()
                .and_then(|p| busytok_domain::normalize_project_path(p).ok())
                .map(|norm| busytok_domain::derive_project_hash(&norm)),
            cwd: parsed.cwd,
            model: message_ref.model.clone(),
            model_provider: Some("anthropic".to_string()),
            agent_version: parsed.version,
            client_kind: Some("claude_code".to_string()),
            speed: usage_val.speed.clone(),
            input_tokens,
            output_tokens,
            total_tokens,
            cached_input_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            reasoning_tokens: 0, // MVP: not available in Claude Code logs
            thoughts_tokens: 0,  // MVP: not available in Claude Code logs
            tool_tokens: 0,      // MVP: not available in Claude Code logs
            cost_usd,
            estimated_cost_usd: None, // Until runtime pricing enrichment
            cost_currency: Some("USD".to_string()),
            cost_source,
            price_catalog_version: None,
            is_error: false,
            error_type: None,
            usage_limit_reset_time_ms: usage_limit_reset,
            raw_event_hash,
            created_at_ms: now,
            updated_at_ms: now,
        };

        Ok(vec![ParsedLogEvent::Normalized(NormalizedEvent::Usage(
            Box::new(event),
        ))])
    }

    fn write_policy(&self) -> UsageWritePolicy {
        UsageWritePolicy::Replace
    }

    fn clone_boxed(&self) -> Box<dyn AgentLogAdapter + Send + Sync> {
        Box::new(self.clone())
    }
}

/// Extract the usage limit reset timestamp from a Claude Code API error message.
///
/// Looks for the specific pattern `Claude AI usage limit reached|{unix_timestamp}`
/// in the message content and returns the timestamp in milliseconds.
/// This is administrative metadata, not user content.
fn extract_usage_limit_reset(parsed: &ClaudeLine) -> Option<i64> {
    let content = parsed.message.as_ref()?.content.as_ref()?;
    for c in content {
        if let Some(text) = c.get("text").and_then(|v| v.as_str()) {
            if text.contains("Claude AI usage limit reached") {
                // Extract timestamp after | character
                if let Some(ts_str) = text.split('|').nth(1) {
                    if let Ok(ts) = ts_str.trim().parse::<i64>() {
                        if ts > 0 {
                            let reset_ms = ts * 1000; // Convert seconds to ms
                            debug!("extracted usage limit reset timestamp: {} ms", reset_ms);
                            return Some(reset_ms);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Context window usage info extracted from a transcript file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextWindowInfo {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub context_limit: i64,
}

/// Read a Claude Code transcript JSONL file and extract context window info
/// from the most recent assistant message with usage data.
///
/// Mirrors ccusage's `calculateContextTokens`. Only reads system-level
/// usage metadata from the transcript (input_tokens, output_tokens),
/// never prompt/response content.
pub fn calculate_context_from_transcript(transcript_path: &Path) -> Option<ContextWindowInfo> {
    use std::io::{BufRead, BufReader};
    let file = match std::fs::File::open(transcript_path) {
        Ok(f) => {
            debug!("opened transcript file: {}", transcript_path.display());
            f
        }
        Err(e) => {
            warn!(
                "transcript file not found: {}: {}",
                transcript_path.display(),
                e
            );
            return None;
        }
    };
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();

    // Iterate from last line to first to find the most recent assistant usage.
    for line in lines.iter().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                debug!(
                    path = %transcript_path.display(),
                    line_bytes = trimmed.len(),
                    "skipping unparseable transcript line"
                );
                continue;
            }
        };
        if parsed.get("type").and_then(|v| v.as_str()) == Some("assistant") {
            if let Some(usage) = parsed.get("message").and_then(|m| m.get("usage")) {
                let input_tokens = usage
                    .get("input_tokens")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let cache_creation = usage
                    .get("cache_creation_input_tokens")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let cache_read = usage
                    .get("cache_read_input_tokens")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let total_input = input_tokens + cache_creation + cache_read;
                return Some(ContextWindowInfo {
                    input_tokens: total_input,
                    output_tokens: usage
                        .get("output_tokens")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                    context_limit: 200_000, // default fallback
                });
            }
        }
    }
    None
}

/// Parse an ISO 8601 timestamp string to epoch milliseconds.
fn parse_iso8601_to_ms(ts: &str) -> Option<i64> {
    let ts = ts.trim();

    // Try OffsetDateTime (with timezone info like Z or +05:30).
    if let Ok(dt) = time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339)
    {
        return Some(dt.unix_timestamp() * 1000 + dt.millisecond() as i64);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_iso8601_basic() {
        let ms = parse_iso8601_to_ms("2026-05-15T08:00:00Z").unwrap();
        assert!(ms > 0);
    }

    #[test]
    fn parse_iso8601_with_milliseconds() {
        let ms = parse_iso8601_to_ms("2026-05-15T08:00:00.123Z").unwrap();
        let base = parse_iso8601_to_ms("2026-05-15T08:00:00Z").unwrap();
        assert_eq!(ms, base + 123);
    }

    #[test]
    fn parse_iso8601_with_offset() {
        let ms_utc = parse_iso8601_to_ms("2026-05-15T08:00:00Z").unwrap();
        let ms_offset = parse_iso8601_to_ms("2026-05-15T13:30:00+05:30").unwrap();
        assert_eq!(ms_utc, ms_offset);
    }

    #[test]
    fn can_parse_jsonl_extension() {
        let adapter = ClaudeCodeAdapter;
        assert!(adapter.can_parse_path(Path::new("/tmp/test.jsonl")));
        assert!(!adapter.can_parse_path(Path::new("/tmp/test.txt")));
        assert!(!adapter.can_parse_path(Path::new("/tmp/codex-session.jsonl")));
    }

    #[test]
    fn parse_line_total_tokens_include_cache_tokens() {
        let adapter = ClaudeCodeAdapter;
        let ctx = ParseContext::for_test("claude-file", "/tmp/claude.jsonl", 1, 0, 100);
        let line = r#"{"sessionId":"sess-1","timestamp":"2026-05-15T08:00:00Z","message":{"id":"msg-1","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":25,"cache_read_input_tokens":10}}}"#;

        let result = adapter.parse_line(&ctx, line).unwrap();
        match &result[0] {
            ParsedLogEvent::Normalized(NormalizedEvent::Usage(event)) => {
                assert_eq!(event.input_tokens, 100);
                assert_eq!(event.output_tokens, 50);
                assert_eq!(event.cache_creation_tokens, 25);
                assert_eq!(event.cache_read_tokens, 10);
                assert_eq!(event.total_tokens, 185);
            }
            other => panic!("expected usage event, got {other:?}"),
        }
    }

    #[test]
    fn parse_line_without_request_id_reuses_message_identity() {
        let adapter = ClaudeCodeAdapter;
        let line = r#"{"sessionId":"sess-1","timestamp":"2026-05-15T08:00:00Z","message":{"id":"msg-1","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":50}}}"#;

        let first_ctx = ParseContext::for_test("claude-file", "/tmp/claude.jsonl", 1, 0, 100);
        let second_ctx = ParseContext::for_test("claude-file", "/tmp/claude.jsonl", 2, 101, 200);

        let first = adapter.parse_line(&first_ctx, line).unwrap();
        let second = adapter.parse_line(&second_ctx, line).unwrap();

        let first_id = match &first[0] {
            ParsedLogEvent::Normalized(NormalizedEvent::Usage(event)) => event.id.clone(),
            other => panic!("expected usage event, got {other:?}"),
        };
        let second_id = match &second[0] {
            ParsedLogEvent::Normalized(NormalizedEvent::Usage(event)) => event.id.clone(),
            other => panic!("expected usage event, got {other:?}"),
        };

        assert_eq!(
            first_id, second_id,
            "message-id-only entries should collapse so the latest update replaces the earlier one"
        );
    }

    #[test]
    fn parse_line_same_message_id_different_request_ids_stay_distinct() {
        let adapter = ClaudeCodeAdapter;
        let first_ctx = ParseContext::for_test("claude-file", "/tmp/claude.jsonl", 1, 0, 100);
        let second_ctx = ParseContext::for_test("claude-file", "/tmp/claude.jsonl", 2, 101, 200);
        let first = r#"{"requestId":"req-1","sessionId":"sess-1","timestamp":"2026-05-15T08:00:00Z","message":{"id":"msg-1","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":50}}}"#;
        let second = r#"{"requestId":"req-2","sessionId":"sess-1","timestamp":"2026-05-15T08:00:01Z","message":{"id":"msg-1","model":"claude-sonnet-4-20250514","usage":{"input_tokens":120,"output_tokens":60}}}"#;

        let first_id = match &adapter.parse_line(&first_ctx, first).unwrap()[0] {
            ParsedLogEvent::Normalized(NormalizedEvent::Usage(event)) => event.id.clone(),
            other => panic!("expected usage event, got {other:?}"),
        };
        let second_id = match &adapter.parse_line(&second_ctx, second).unwrap()[0] {
            ParsedLogEvent::Normalized(NormalizedEvent::Usage(event)) => event.id.clone(),
            other => panic!("expected usage event, got {other:?}"),
        };

        assert_ne!(
            first_id, second_id,
            "different request ids must not collapse into one Claude usage identity"
        );
    }

    #[test]
    fn parse_line_skips_synthetic_model() {
        let adapter = ClaudeCodeAdapter;
        let ctx = ParseContext::for_test("claude-file", "/tmp/claude.jsonl", 1, 0, 100);
        let line = r#"{"sessionId":"sess-1","timestamp":"2026-05-15T08:00:00Z","isApiErrorMessage":true,"message":{"id":"msg-syn","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#;

        let result = adapter.parse_line(&ctx, line).unwrap();
        assert!(
            result.is_empty(),
            "synthetic error placeholders should be skipped"
        );
    }

    // -- extract_usage_limit_reset tests --

    fn make_claude_line(content_text: Option<&str>) -> ClaudeLine {
        let message = content_text.map(|text| {
            let content: Vec<serde_json::Value> = vec![serde_json::json!({"text": text})];
            ClaudeMessage {
                id: None,
                model: None,
                usage: None,
                content: Some(content),
            }
        });
        ClaudeLine {
            cwd: None,
            session_id: None,
            timestamp: None,
            version: None,
            message,
            request_id: None,
            cost_usd: None,
            is_api_error_message: Some(true),
        }
    }

    #[test]
    fn extract_usage_limit_reset_valid_timestamp() {
        // Valid error message with a unix timestamp.
        let line = make_claude_line(Some("Claude AI usage limit reached|1777777777"));
        let result = extract_usage_limit_reset(&line);
        assert_eq!(result, Some(1777777777 * 1000));
    }

    #[test]
    fn extract_usage_limit_reset_missing_timestamp() {
        // Error message without the | separator.
        let line = make_claude_line(Some("Claude AI usage limit reached"));
        assert!(extract_usage_limit_reset(&line).is_none());
    }

    #[test]
    fn extract_usage_limit_reset_non_error_message_content() {
        // A message without the "Claude AI usage limit reached" pattern.
        let message = ClaudeMessage {
            id: None,
            model: None,
            usage: None,
            content: Some(vec![serde_json::json!({
                "text": "Some other error message"
            })]),
        };
        let line = ClaudeLine {
            cwd: None,
            session_id: None,
            timestamp: None,
            version: None,
            message: Some(message),
            request_id: None,
            cost_usd: None,
            is_api_error_message: Some(true),
        };
        assert!(extract_usage_limit_reset(&line).is_none());
    }

    #[test]
    fn extract_usage_limit_reset_no_message() {
        // A line with no message at all.
        let line = ClaudeLine {
            cwd: None,
            session_id: None,
            timestamp: None,
            version: None,
            message: None,
            request_id: None,
            cost_usd: None,
            is_api_error_message: Some(true),
        };
        assert!(extract_usage_limit_reset(&line).is_none());
    }

    #[test]
    fn extract_usage_limit_reset_malformed_timestamp() {
        // | followed by non-numeric text.
        let line = make_claude_line(Some("Claude AI usage limit reached|not-a-number"));
        assert!(extract_usage_limit_reset(&line).is_none());
    }

    #[test]
    fn extract_usage_limit_reset_negative_timestamp() {
        // | followed by a negative number (invalid).
        let line = make_claude_line(Some("Claude AI usage limit reached|-1"));
        assert!(extract_usage_limit_reset(&line).is_none());
    }

    // -- calculate_context_from_transcript tests --

    #[test]
    fn calculate_context_from_transcript_finds_latest_assistant() {
        let dir = std::env::temp_dir();
        let mut path = dir.join("test_transcript_finds_latest.jsonl");
        path.set_extension("jsonl");
        let content = r#"{"type":"user","message":{"usage":null}}
{"type":"assistant","message":{"usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}}}
{"type":"assistant","message":{"usage":{"input_tokens":200,"output_tokens":80,"cache_creation_input_tokens":20,"cache_read_input_tokens":15}}}
"#;
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(content.as_bytes()).unwrap();
        }
        let result = calculate_context_from_transcript(&path);
        std::fs::remove_file(&path).ok();
        assert!(result.is_some());
        let info = result.unwrap();
        // Latest assistant: cached_input = 200 + 20 + 15 = 235, output = 80
        assert_eq!(info.input_tokens, 235);
        assert_eq!(info.output_tokens, 80);
        assert_eq!(info.context_limit, 200_000);
    }

    #[test]
    fn calculate_context_from_transcript_no_assistant_returns_none() {
        let dir = std::env::temp_dir();
        let mut path = dir.join("test_transcript_no_assistant.jsonl");
        path.set_extension("jsonl");
        let content = r#"{"type":"user","message":{"usage":null}}
{"type":"user","message":{"usage":{"input_tokens":100,"output_tokens":50}}}
"#;
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(content.as_bytes()).unwrap();
        }
        let result = calculate_context_from_transcript(&path);
        std::fs::remove_file(&path).ok();
        assert!(result.is_none());
    }

    #[test]
    fn calculate_context_from_transcript_missing_file_returns_none() {
        let path = Path::new("/tmp/nonexistent_transcript_test_file.jsonl");
        let result = calculate_context_from_transcript(path);
        assert!(result.is_none());
    }

    #[test]
    fn calculate_context_from_transcript_uses_last_assistant() {
        // When multiple assistant messages exist, the LAST one is used.
        let dir = std::env::temp_dir();
        let mut path = dir.join("test_transcript_last_assistant.jsonl");
        path.set_extension("jsonl");
        let content = r#"{"type":"assistant","message":{"usage":{"input_tokens":50,"output_tokens":25,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}
{"type":"user","message":{"usage":null}}
{"type":"assistant","message":{"usage":{"input_tokens":500,"output_tokens":300,"cache_creation_input_tokens":100,"cache_read_input_tokens":50}}}
"#;
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(content.as_bytes()).unwrap();
        }
        let result = calculate_context_from_transcript(&path);
        std::fs::remove_file(&path).ok();
        assert!(result.is_some());
        let info = result.unwrap();
        // Last assistant: input = 500 + 100 + 50 = 650, output = 300
        assert_eq!(info.input_tokens, 650);
        assert_eq!(info.output_tokens, 300);
    }

    #[test]
    fn calculate_context_from_transcript_skips_bad_last_line() {
        // When the last line is malformed JSON (common during live writing),
        // the function should still find valid data from earlier lines.
        let dir = std::env::temp_dir();
        let mut path = dir.join("test_transcript_bad_last_line.jsonl");
        path.set_extension("jsonl");
        let content = r#"{"type":"assistant","message":{"usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}}}
{"type":"user","message":{"usage":null}}
{"type":"assistant","message":{"usage":{"input_tokens":200,"output_tokens":80,"cache_creation_input_tokens":20,"cache_read_input_tokens":15}}}
{"type":"assistant","message":{"usage":{"input_tokens":300,"output_tokens":120,"cach
"#;
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(content.as_bytes()).unwrap();
        }
        let result = calculate_context_from_transcript(&path);
        std::fs::remove_file(&path).ok();
        assert!(result.is_some());
        let info = result.unwrap();
        // Should use the valid second-to-last assistant line: 200 + 20 + 15 = 235, output = 80
        assert_eq!(info.input_tokens, 235);
        assert_eq!(info.output_tokens, 80);
    }
}
