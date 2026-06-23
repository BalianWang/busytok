use serde::{Deserialize, Serialize};

use crate::cache_metrics::ProviderPayloadShape;
use crate::AgentKind;

/// Core fact table for user-visible metrics.
///
/// All user-visible metrics originate here. This struct never contains
/// prompt, response, tool args, API keys, account IDs, or content fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedUsageEvent {
    pub id: String,
    pub agent: AgentKind,
    pub source_file_id: String,
    pub source_path: String,
    pub source_line: u64,
    pub source_offset_start: u64,
    pub source_offset_end: u64,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub source_request_id: Option<String>,
    pub message_id: Option<String>,
    pub timestamp_ms: i64,
    pub project_path: Option<String>,
    pub project_hash: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub agent_version: Option<String>,
    pub client_kind: Option<String>,
    pub speed: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub reasoning_tokens: i64,
    pub thoughts_tokens: i64,
    pub tool_tokens: i64,
    /// Discriminator recording how the raw provider payload reported tokens.
    /// Provider differences live here; downstream consumes unified fields.
    pub provider_payload_shape: ProviderPayloadShape,
    /// Unified: total prompt input INCLUDING the cacheable portion.
    pub prompt_input_total_tokens: i64,
    /// Unified: prompt input NOT served from cache.
    pub prompt_input_non_cached_tokens: i64,
    pub cost_usd: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
    pub cost_currency: Option<String>,
    pub cost_source: Option<String>,
    pub price_catalog_version: Option<String>,
    pub is_error: bool,
    pub error_type: Option<String>,
    pub usage_limit_reset_time_ms: Option<i64>,
    pub raw_event_hash: String,
    /// True when the source line carried `isSidechain: true` (e.g. Claude Code
    /// `/btw` subagent logs that replay a parent message). Sidechain replays
    /// share a parent's `message_id` but carry a different `request_id`; they
    /// must collapse onto the parent during dedup so its usage is not counted
    /// twice. See [`crate::NormalizedUsageEvent::dedupe_key`].
    pub is_sidechain: bool,
    /// Identity used for cross-event dedup within a generation. For Claude
    /// Code events this is `claude:msg:{message_id}` so a parent and its
    /// sidechain replay collide on the `usage_events(generation_id,
    /// dedupe_key)` unique index. When `None`, the store falls back to the
    /// event `id` (per-row identity, no cross-event collapse).
    pub dedupe_key: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl NormalizedUsageEvent {
    /// Create a minimal event for test use with zero token defaults.
    pub fn minimal_for_test(id: &str, agent: AgentKind) -> Self {
        let now_ms = crate::now_ms();
        Self {
            id: id.to_string(),
            agent,
            source_file_id: String::new(),
            source_path: String::new(),
            source_line: 0,
            source_offset_start: 0,
            source_offset_end: 0,
            session_id: String::new(),
            turn_id: None,
            source_request_id: None,
            message_id: None,
            timestamp_ms: now_ms,
            project_path: None,
            project_hash: None,
            cwd: None,
            model: None,
            model_provider: None,
            agent_version: None,
            client_kind: None,
            speed: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            reasoning_tokens: 0,
            thoughts_tokens: 0,
            tool_tokens: 0,
            provider_payload_shape: ProviderPayloadShape::Codex,
            prompt_input_total_tokens: 0,
            prompt_input_non_cached_tokens: 0,
            cost_usd: None,
            estimated_cost_usd: None,
            cost_currency: None,
            cost_source: None,
            price_catalog_version: None,
            is_error: false,
            error_type: None,
            usage_limit_reset_time_ms: None,
            raw_event_hash: String::new(),
            is_sidechain: false,
            dedupe_key: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }
}

/// Tool call metadata. Does not store arguments or results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEvent {
    pub id: String,
    pub agent: AgentKind,
    pub source_file_id: String,
    pub source_path: String,
    pub source_line: u64,
    pub source_offset_start: u64,
    pub source_offset_end: u64,
    pub session_id: String,
    pub message_id: Option<String>,
    pub tool_name: String,
    pub status: Option<String>,
    pub timestamp_ms: Option<i64>,
    pub project_hash: Option<String>,
    pub created_at_ms: i64,
}

/// Operational diagnostic event for parser/runtime observations.
///
/// These matter for source health, diagnostics, or counters but do not
/// belong in user-facing usage history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationalDiagnosticEvent {
    pub id: String,
    pub agent: Option<AgentKind>,
    pub source_id: Option<String>,
    pub source_file_id: Option<String>,
    pub source_path: Option<String>,
    pub source_line: Option<i64>,
    pub category: String,
    pub severity: String,
    pub message: String,
    pub detail_json: Option<String>,
    pub happened_at_ms: i64,
    pub created_at_ms: i64,
}

/// Parsed output from adapters before runtime processing.
///
/// Adapters emit either a ready-to-store normalized event or an internal
/// Codex snapshot that needs runtime delta conversion before persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParsedLogEvent {
    /// A ready-to-store normalized event (usage, tool, diagnostic).
    Normalized(NormalizedEvent),
    /// An internal Codex cumulative token snapshot awaiting delta conversion.
    CodexTokenSnapshot(CodexTokenSnapshot),
}

/// Internal Codex cumulative token snapshot parsed from JSONL.
///
/// This is NOT a user-facing event. The runtime converts it into a
/// NormalizedUsageEvent delta before persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexTokenSnapshot {
    pub source_file_id: String,
    pub source_path: String,
    pub source_line: u64,
    pub source_offset_start: u64,
    pub source_offset_end: u64,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub token_event_ordinal: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
    pub delta_input_tokens: Option<i64>,
    pub delta_cached_input_tokens: Option<i64>,
    pub delta_output_tokens: Option<i64>,
    pub delta_reasoning_tokens: Option<i64>,
    pub delta_total_tokens: Option<i64>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub cost_usd: Option<f64>,
    pub raw_usage_json: String,
    pub timestamp_ms: i64,
}

/// Unified event union emitted by adapters.
///
/// Adapters must not invent extra top-level event kinds during MVP.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NormalizedEvent {
    Usage(Box<NormalizedUsageEvent>),
    Tool(ToolEvent),
    OperationalDiagnostic(OperationalDiagnosticEvent),
}

impl NormalizedEvent {
    pub fn diagnostic_for_test(id: &str) -> Self {
        let now_ms = crate::now_ms();
        NormalizedEvent::OperationalDiagnostic(OperationalDiagnosticEvent {
            id: id.to_string(),
            agent: None,
            source_id: None,
            source_file_id: None,
            source_path: None,
            source_line: None,
            category: "test".to_string(),
            severity: "info".to_string(),
            message: "test diagnostic".to_string(),
            detail_json: None,
            happened_at_ms: now_ms,
            created_at_ms: now_ms,
        })
    }

    pub fn tool_for_test(id: &str) -> Self {
        let now_ms = crate::now_ms();
        NormalizedEvent::Tool(ToolEvent {
            id: id.to_string(),
            agent: AgentKind::ClaudeCode,
            source_file_id: String::new(),
            source_path: String::new(),
            source_line: 0,
            source_offset_start: 0,
            source_offset_end: 0,
            session_id: String::new(),
            message_id: None,
            tool_name: "test_tool".to_string(),
            status: None,
            timestamp_ms: Some(now_ms),
            project_hash: None,
            created_at_ms: now_ms,
        })
    }
}

impl OperationalDiagnosticEvent {
    /// Create a minimal diagnostic event for test use.
    pub fn for_test(id: &str) -> Self {
        let now_ms = crate::now_ms();
        Self {
            id: id.to_string(),
            agent: None,
            source_id: None,
            source_file_id: None,
            source_path: None,
            source_line: None,
            category: "test".to_string(),
            severity: "info".to_string(),
            message: "test diagnostic".to_string(),
            detail_json: None,
            happened_at_ms: now_ms,
            created_at_ms: now_ms,
        }
    }
}

/// Parse errors emitted by adapters during log processing.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ParseError {
    #[error("malformed JSON: {reason}")]
    MalformedJson { reason: String },
    #[error("missing required field: {field}")]
    MissingRequiredField { field: String },
    #[error("unsupported schema: {details}")]
    UnsupportedSchema { details: String },
    #[error("content excluded: {reason}")]
    ContentExcluded { reason: String },
}

/// Context supplied by the tailer/runtime for adapter parsing.
///
/// This is the only source of file identity, offsets, and replay ordering.
/// Adapters should not recompute file IDs or infer replay order from timestamps.
#[derive(Debug, Clone)]
pub struct ParseContext {
    pub source_file_id: String,
    pub source_path: String,
    pub inode: Option<String>,
    pub source_line: u64,
    pub source_offset_start: u64,
    pub source_offset_end: u64,
    pub replay_sequence: u64,
}

/// Policy for how a usage event should be persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageWritePolicy {
    /// Ignore duplicate inserts (idempotent). Used by Codex.
    InsertOnce,
    /// Replace existing row on re-insert. Used by Claude Code.
    Replace,
}

impl ParseContext {
    /// Create a parse context for test use.
    pub fn for_test(
        source_file_id: &str,
        source_path: &str,
        source_line: u64,
        source_offset_start: u64,
        source_offset_end: u64,
    ) -> Self {
        Self {
            source_file_id: source_file_id.to_string(),
            source_path: source_path.to_string(),
            inode: None,
            source_line,
            source_offset_start,
            source_offset_end,
            replay_sequence: source_line,
        }
    }
}

impl NormalizedEvent {
    /// Extract the inner NormalizedUsageEvent, if this is a Usage variant.
    pub fn into_usage(self) -> Option<NormalizedUsageEvent> {
        match self {
            NormalizedEvent::Usage(u) => Some(*u),
            _ => None,
        }
    }
}

#[cfg(test)]
mod unified_field_tests {
    use super::*;
    use crate::cache_metrics::ProviderPayloadShape;

    #[test]
    fn minimal_event_has_default_unified_fields() {
        let e = NormalizedUsageEvent::minimal_for_test("t", crate::AgentKind::ClaudeCode);
        assert_eq!(e.provider_payload_shape, ProviderPayloadShape::Codex);
        assert_eq!(e.prompt_input_total_tokens, 0);
        assert_eq!(e.prompt_input_non_cached_tokens, 0);
    }
}
