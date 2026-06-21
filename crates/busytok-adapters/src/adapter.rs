//! Agent log adapter trait.
//!
//! Each supported agent (Claude Code, Codex) implements
//! [`AgentLogAdapter`] to parse its JSONL log format into normalized events.

use std::path::Path;

use busytok_domain::{AgentKind, ParseContext, ParseError, ParsedLogEvent, UsageWritePolicy};

/// Trait for parsing agent-specific log lines into normalized events.
///
/// Implementations must not read prompt/response content, tool arguments,
/// or API keys from the log lines.
pub trait AgentLogAdapter {
    /// Which agent this adapter handles.
    fn agent(&self) -> AgentKind;

    /// Whether this adapter can parse the file at the given path.
    fn can_parse_path(&self, path: &Path) -> bool;

    /// Parse a single log line into zero or more normalized events.
    fn parse_line(
        &self,
        context: &ParseContext,
        line: &str,
    ) -> Result<Vec<ParsedLogEvent>, ParseError>;

    /// The write policy for usage events from this adapter.
    fn write_policy(&self) -> UsageWritePolicy;

    /// Clone this adapter into a boxed trait object.
    fn clone_boxed(&self) -> Box<dyn AgentLogAdapter + Send + Sync>;
}
