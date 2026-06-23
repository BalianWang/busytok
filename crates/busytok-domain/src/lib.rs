#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
#![allow(clippy::uninlined_format_args)]
#![cfg_attr(test, allow(clippy::inconsistent_digit_grouping))]
//! Busytok domain types and identity rules.
//!
//! This is the core domain crate. ALL other Busytok crates depend on it.
//! The crate remains pure and storage-agnostic — no SQLite, no network,
//! no file I/O. It defines the shared vocabulary that the rest of the
//! system uses.

pub mod agent;
pub mod cache_metrics;
pub mod events;
pub mod identity;
pub mod time;
pub mod timezone;

// Re-export all public types for convenient access.
pub use agent::{
    AgentKind, AgentStatus, BillingBlock, BurnRate, BurnStatus, LogFile, LogFileState, LogSource,
    LogSourceStatus, LogSourceType, ModelSummary, ProjectSummary, RealtimeSummary, SessionSummary,
};
pub use events::{
    CodexTokenSnapshot, NormalizedEvent, NormalizedUsageEvent, OperationalDiagnosticEvent,
    ParseContext, ParseError, ParsedLogEvent, ToolEvent, UsageWritePolicy,
};
pub use identity::{
    derive_project_hash, derive_session_id, hash_short, metadata_event_hash,
    normalize_project_path, IdentityError, MetadataFingerprint,
};
pub use time::now_ms;
pub use timezone::{
    detect_system_iana_timezone, resolve_local_timezone, DayBoundary, ReportingTimezone,
};
