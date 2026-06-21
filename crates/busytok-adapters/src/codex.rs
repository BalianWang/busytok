//! OpenAI Codex JSONL log adapter.
//!
//! Parses the JSONL format written by OpenAI Codex into cumulative token
//! snapshots. The adapter emits `ParsedLogEvent::CodexTokenSnapshot` for
//! each line; the runtime converts cumulative snapshots into usage deltas.
//!
//! Key Codex semantics:
//! - `total_token_usage` is cumulative (growing across snapshots)
//! - `last_token_usage` is the delta for the most recent turn
//! - When that explicit delta is present, runtime should prefer it over
//!   re-deriving a delta from the cumulative totals
//! - Reasoning tokens are informational, must NOT be added to output_tokens
//! - This adapter never reads prompt/response content, tool arguments, or API keys

use std::path::Path;

use busytok_domain::{
    AgentKind, CodexTokenSnapshot, ParseContext, ParseError, ParsedLogEvent, UsageWritePolicy,
};
use serde::Deserialize;
use serde_json::Value;

use crate::adapter::AgentLogAdapter;

/// Adapter for parsing OpenAI Codex JSONL usage logs.
#[derive(Debug, Default, Clone)]
pub struct CodexAdapter;

/// Top-level structure of a Codex JSONL line.
///
/// Codex logs contain cumulative token snapshots. Each line has:
/// - `session_id`: the session identifier
/// - `timestamp`: ISO 8601 timestamp
/// - `model`: the model used (optional, may come from turn_context)
/// - `total_token_usage`: cumulative token counts across all turns
/// - `last_token_usage`: delta for the most recent turn (optional)
/// - `costUSD`: pre-calculated cost (optional)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CodexLine {
    session_id: Option<String>,
    timestamp: String,
    model: Option<String>,
    last_token_usage: Option<CodexTokenUsage>,
    total_token_usage: Option<CodexTokenUsage>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct CodexEventEnvelope {
    timestamp: String,
    #[serde(rename = "type")]
    event_type: String,
    payload: Value,
}

/// Token usage structure within a Codex JSONL line.
///
/// Codex reports:
/// - `input_tokens`: total input tokens
/// - `cached_input_tokens`: cached portion of input (prompt caching)
/// - `output_tokens`: normal output tokens (includes reasoning cost)
/// - `reasoning_output_tokens`: informational breakdown for reasoning
///   (NOT added to output_tokens for billing)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CodexTokenUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default, alias = "cache_read_input_tokens")]
    cached_input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    reasoning_output_tokens: i64,
    #[serde(default)]
    total_tokens: i64,
}

impl AgentLogAdapter for CodexAdapter {
    fn agent(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn can_parse_path(&self, path: &Path) -> bool {
        // Codex logs live in ~/.codex/sessions/**/*.jsonl, and recent desktop
        // builds name the files `rollout-*.jsonl` rather than including "codex"
        // in the basename.
        let path_str = path.to_string_lossy().to_lowercase();
        path.extension().is_some_and(|ext| ext == "jsonl")
            && (path_str.contains("/.codex/sessions/")
                || path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().to_lowercase().contains("codex")))
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

        if let Ok(parsed) = serde_json::from_str::<CodexLine>(trimmed) {
            if parsed.total_token_usage.is_some() || parsed.last_token_usage.is_some() {
                return self.snapshot_from_legacy_line(ctx, trimmed, parsed);
            }
        }

        let parsed: CodexEventEnvelope =
            serde_json::from_str(trimmed).map_err(|e| ParseError::MalformedJson {
                reason: format!("Codex JSON parse error: {e}"),
            })?;

        if parsed.event_type != "event_msg" {
            return Ok(vec![]);
        }

        let payload_type = parsed
            .payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if payload_type != "token_count" {
            return Ok(vec![]);
        }

        let Some(info) = parsed.payload.get("info").and_then(Value::as_object) else {
            return Ok(vec![]);
        };
        let Some(total_usage_value) = info
            .get("total_token_usage")
            .cloned()
            .or_else(|| info.get("last_token_usage").cloned())
        else {
            return Ok(vec![]);
        };
        let total_usage: CodexTokenUsage =
            serde_json::from_value(total_usage_value).map_err(|e| ParseError::MalformedJson {
                reason: format!("Codex token_count usage parse error: {e}"),
            })?;
        let last_usage: Option<CodexTokenUsage> = info
            .get("last_token_usage")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| ParseError::MalformedJson {
                reason: format!("Codex token_count delta parse error: {e}"),
            })?;
        let model = info
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        self.snapshot_from_usage(
            ctx,
            trimmed,
            &parsed.timestamp,
            infer_session_id(ctx),
            model,
            last_usage,
            Some(total_usage),
            None,
        )
    }

    fn write_policy(&self) -> UsageWritePolicy {
        UsageWritePolicy::InsertOnce
    }

    fn clone_boxed(&self) -> Box<dyn AgentLogAdapter + Send + Sync> {
        Box::new(self.clone())
    }
}

impl CodexAdapter {
    fn snapshot_from_legacy_line(
        &self,
        ctx: &ParseContext,
        raw_line: &str,
        parsed: CodexLine,
    ) -> Result<Vec<ParsedLogEvent>, ParseError> {
        self.snapshot_from_usage(
            ctx,
            raw_line,
            &parsed.timestamp,
            parsed.session_id.unwrap_or_else(|| infer_session_id(ctx)),
            parsed.model,
            parsed.last_token_usage,
            parsed.total_token_usage,
            parsed.cost_usd,
        )
    }

    fn snapshot_from_usage(
        &self,
        ctx: &ParseContext,
        raw_line: &str,
        timestamp: &str,
        session_id: String,
        model: Option<String>,
        last_token_usage: Option<CodexTokenUsage>,
        total_token_usage: Option<CodexTokenUsage>,
        cost_usd: Option<f64>,
    ) -> Result<Vec<ParsedLogEvent>, ParseError> {
        let usage = total_token_usage
            .clone()
            .or(last_token_usage.clone())
            .ok_or_else(|| ParseError::MissingRequiredField {
                field: "total_token_usage or last_token_usage".to_string(),
            })?;
        let delta_usage = last_token_usage;

        let timestamp_ms = parse_iso8601_to_ms(timestamp).unwrap_or_else(busytok_domain::now_ms);
        let total_tokens = if usage.total_tokens > 0 {
            usage.total_tokens
        } else {
            usage.input_tokens + usage.output_tokens + usage.reasoning_output_tokens
        };

        let snapshot = CodexTokenSnapshot {
            source_file_id: ctx.source_file_id.clone(),
            source_path: ctx.source_path.clone(),
            source_line: ctx.source_line,
            source_offset_start: ctx.source_offset_start,
            source_offset_end: ctx.source_offset_end,
            session_id,
            turn_id: None,          // Codex does not emit turn_id in the snapshot line
            token_event_ordinal: 0, // assigned by runtime from persisted state
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_tokens: usage.reasoning_output_tokens,
            total_tokens,
            delta_input_tokens: delta_usage.as_ref().map(|u| u.input_tokens),
            delta_cached_input_tokens: delta_usage.as_ref().map(|u| u.cached_input_tokens),
            delta_output_tokens: delta_usage.as_ref().map(|u| u.output_tokens),
            delta_reasoning_tokens: delta_usage.as_ref().map(|u| u.reasoning_output_tokens),
            delta_total_tokens: delta_usage.as_ref().map(|u| {
                if u.total_tokens > 0 {
                    u.total_tokens
                } else {
                    u.input_tokens + u.output_tokens + u.reasoning_output_tokens
                }
            }),
            model,
            model_provider: Some("openai".to_string()),
            cost_usd,
            raw_usage_json: raw_line.to_string(),
            timestamp_ms,
        };

        Ok(vec![ParsedLogEvent::CodexTokenSnapshot(snapshot)])
    }
}

fn infer_session_id(ctx: &ParseContext) -> String {
    Path::new(&ctx.source_path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| ctx.source_file_id.clone())
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

    #[test]
    fn parse_iso8601_basic() {
        let ms = parse_iso8601_to_ms("2026-05-15T09:00:00Z").unwrap();
        assert!(ms > 0);
    }

    #[test]
    fn codex_parses_snapshot_line() {
        let adapter = CodexAdapter;
        let ctx = ParseContext::for_test("test-file-id", "/test/codex.jsonl", 1, 0, 100);
        let line = r#"{"session_id":"codex-session-2","timestamp":"2026-05-15T09:00:00Z","model":"gpt-5.1-codex","last_token_usage":{"input_tokens":300,"output_tokens":150,"reasoning_output_tokens":50},"total_token_usage":{"input_tokens":800,"output_tokens":350,"reasoning_output_tokens":50},"costUSD":0.02}"#;

        let result = adapter.parse_line(&ctx, line).unwrap();
        assert_eq!(result.len(), 1);
        match &result[0] {
            ParsedLogEvent::CodexTokenSnapshot(snap) => {
                assert_eq!(snap.session_id, "codex-session-2");
                assert_eq!(snap.input_tokens, 800);
                assert_eq!(snap.output_tokens, 350);
                assert_eq!(snap.reasoning_tokens, 50);
                assert_eq!(snap.model.as_deref(), Some("gpt-5.1-codex"));
                assert_eq!(snap.cost_usd, Some(0.02));
            }
            _ => panic!("expected CodexTokenSnapshot"),
        }
    }

    #[test]
    fn codex_parses_token_count_event_stream_line() {
        let adapter = CodexAdapter;
        let ctx = ParseContext::for_test(
            "test-file-id",
            "/Users/test/.codex/sessions/2026/05/20/rollout-abc123.jsonl",
            12,
            0,
            120,
        );
        let line = r#"{"timestamp":"2026-05-20T07:16:22.790Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":14495,"cached_input_tokens":3456,"output_tokens":160,"reasoning_output_tokens":0,"total_tokens":14655},"last_token_usage":{"input_tokens":14495,"cached_input_tokens":3456,"output_tokens":160,"reasoning_output_tokens":0,"total_tokens":14655},"model_context_window":258400}}}"#;

        let result = adapter.parse_line(&ctx, line).unwrap();
        assert_eq!(result.len(), 1);
        match &result[0] {
            ParsedLogEvent::CodexTokenSnapshot(snap) => {
                assert_eq!(snap.session_id, "rollout-abc123");
                assert_eq!(snap.input_tokens, 14495);
                assert_eq!(snap.cached_input_tokens, 3456);
                assert_eq!(snap.output_tokens, 160);
                assert_eq!(snap.total_tokens, 14655);
                assert_eq!(snap.model, None);
            }
            _ => panic!("expected CodexTokenSnapshot"),
        }
    }

    #[test]
    fn codex_ignores_token_count_without_usage_info() {
        let adapter = CodexAdapter;
        let ctx = ParseContext::for_test("test-file-id", "/test/rollout.jsonl", 1, 0, 100);
        let line = r#"{"timestamp":"2026-05-20T07:16:22.790Z","type":"event_msg","payload":{"type":"token_count","info":null}}"#;

        let result = adapter.parse_line(&ctx, line).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn codex_parses_cache_read_alias() {
        let adapter = CodexAdapter;
        let ctx = ParseContext::for_test("test-file-id", "/test/rollout.jsonl", 1, 0, 100);
        let line = r#"{"timestamp":"2026-05-20T07:16:22.790Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cache_read_input_tokens":80,"output_tokens":5,"reasoning_output_tokens":0,"total_tokens":105}}}}"#;

        let result = adapter.parse_line(&ctx, line).unwrap();
        match &result[0] {
            ParsedLogEvent::CodexTokenSnapshot(snap) => {
                assert_eq!(snap.cached_input_tokens, 80);
            }
            _ => panic!("expected CodexTokenSnapshot"),
        }
    }

    #[test]
    fn codex_preserves_zero_component_last_usage_heartbeats_as_snapshots() {
        let adapter = CodexAdapter;
        let ctx = ParseContext::for_test("test-file-id", "/test/rollout.jsonl", 1, 0, 100);
        let line = r#"{"timestamp":"2026-05-20T07:16:22.790Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":900,"output_tokens":10,"reasoning_output_tokens":5,"total_tokens":1015},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":7738}}}}"#;

        let result = adapter.parse_line(&ctx, line).unwrap();
        match &result[0] {
            ParsedLogEvent::CodexTokenSnapshot(snap) => {
                assert_eq!(snap.delta_input_tokens, Some(0));
                assert_eq!(snap.delta_cached_input_tokens, Some(0));
                assert_eq!(snap.delta_output_tokens, Some(0));
                assert_eq!(snap.delta_reasoning_tokens, Some(0));
                assert_eq!(snap.delta_total_tokens, Some(7738));
            }
            other => panic!("expected CodexTokenSnapshot, got {other:?}"),
        }
    }

    #[test]
    fn codex_total_fallback_includes_reasoning_tokens() {
        let adapter = CodexAdapter;
        let ctx = ParseContext::for_test("test-file-id", "/test/rollout.jsonl", 1, 0, 100);
        let line = r#"{"timestamp":"2026-05-20T07:16:22.790Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":7}}}}"#;

        let result = adapter.parse_line(&ctx, line).unwrap();
        match &result[0] {
            ParsedLogEvent::CodexTokenSnapshot(snap) => {
                assert_eq!(snap.total_tokens, 112);
            }
            other => panic!("expected CodexTokenSnapshot, got {other:?}"),
        }
    }

    #[test]
    fn codex_zero_component_heartbeat_with_zero_total_is_preserved() {
        let adapter = CodexAdapter;
        let ctx = ParseContext::for_test("test-file-id", "/test/rollout.jsonl", 1, 0, 100);
        let line = r#"{"timestamp":"2026-05-20T07:16:22.790Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":7,"total_tokens":112},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":0}}}}"#;

        let result = adapter.parse_line(&ctx, line).unwrap();
        match &result[0] {
            ParsedLogEvent::CodexTokenSnapshot(snap) => {
                assert_eq!(snap.delta_input_tokens, Some(0));
                assert_eq!(snap.delta_cached_input_tokens, Some(0));
                assert_eq!(snap.delta_output_tokens, Some(0));
                assert_eq!(snap.delta_reasoning_tokens, Some(0));
                assert_eq!(snap.delta_total_tokens, Some(0));
            }
            other => panic!("expected CodexTokenSnapshot, got {other:?}"),
        }
    }

    #[test]
    fn codex_ignores_non_token_count_event_lines() {
        let adapter = CodexAdapter;
        let ctx = ParseContext::for_test("test-file-id", "/test/rollout.jsonl", 1, 0, 100);
        let line = r#"{"timestamp":"2026-05-20T07:16:15.610Z","type":"session_meta","payload":{"id":"019e443e-03a6-73a0-8974-8996f43e4a6c"}}"#;

        let result = adapter.parse_line(&ctx, line).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn codex_empty_line_returns_empty() {
        let adapter = CodexAdapter;
        let ctx = ParseContext::for_test("test-file-id", "/test/codex.jsonl", 1, 0, 100);
        let result = adapter.parse_line(&ctx, "  ").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn codex_can_parse_path() {
        let adapter = CodexAdapter;
        assert!(adapter.can_parse_path(Path::new("/logs/codex-sessions.jsonl")));
        assert!(adapter.can_parse_path(Path::new(
            "/Users/test/.codex/sessions/2026/05/20/rollout-abc123.jsonl"
        )));
        assert!(!adapter.can_parse_path(Path::new("/logs/claude-sessions.jsonl")));
    }

    #[test]
    fn codex_agent_kind() {
        let adapter = CodexAdapter;
        assert_eq!(adapter.agent(), AgentKind::Codex);
    }

    #[test]
    fn codex_write_policy_is_insert_once() {
        let adapter = CodexAdapter;
        assert_eq!(adapter.write_policy(), UsageWritePolicy::InsertOnce);
    }

    #[test]
    fn codex_parses_model_from_token_count_info_model() {
        let adapter = CodexAdapter;
        let ctx = ParseContext::for_test("test-id", "/test/file.jsonl", 1, 0, 100);
        let line = r#"{"timestamp":"2026-05-20T07:16:22.790Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5.4","total_token_usage":{"input_tokens":100,"output_tokens":50,"total_tokens":150}}}}"#;

        let result = adapter.parse_line(&ctx, line).unwrap();
        assert_eq!(result.len(), 1);
        match &result[0] {
            ParsedLogEvent::CodexTokenSnapshot(snap) => {
                assert_eq!(snap.model.as_deref(), Some("gpt-5.4"));
            }
            _ => panic!("expected CodexTokenSnapshot"),
        }
    }

    #[test]
    fn codex_ignores_empty_model_from_info() {
        let adapter = CodexAdapter;
        let ctx = ParseContext::for_test("test-id", "/test/file.jsonl", 1, 0, 100);
        let line = r#"{"timestamp":"2026-05-20T07:16:22.790Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"  ","total_token_usage":{"input_tokens":100,"output_tokens":50,"total_tokens":150}}}}"#;

        let result = adapter.parse_line(&ctx, line).unwrap();
        match &result[0] {
            ParsedLogEvent::CodexTokenSnapshot(snap) => {
                assert_eq!(snap.model, None);
            }
            _ => panic!("expected CodexTokenSnapshot"),
        }
    }
}
