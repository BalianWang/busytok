//! Errors for the logical-subagent management layer.
//!
//! Each variant maps to a control-protocol error code so the dispatcher can
//! surface a stable, machine-readable failure to clients.

use crate::sidecar::SidecarError;
use thiserror::Error;

/// A logical-subagent management error.
///
/// `code()` returns the stable string emitted in `ControlResponse` payloads.
#[derive(Debug, Error)]
pub enum SubagentError {
    #[error("logical subagent not found: {0}")]
    NotFound(String),

    #[error("ambiguous subagent name: {0}")]
    AmbiguousName(String),

    #[error("invalid subagent name: {0}")]
    InvalidName(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("profile not found: {0}")]
    ProfileNotFound(String),

    #[error("subagent feature is disabled")]
    Disabled,

    #[error("database error")]
    Store(#[from] anyhow::Error),

    // --- sidecar variants (Plan 2) ---
    #[error("task timed out")]
    TaskTimeout,

    #[error("sidecar spawn failed: {0}")]
    SidecarSpawn(String),

    #[error("sidecar rpc error: {message}")]
    SidecarRpc {
        message: String,
        /// Numeric JSON-RPC error code, preserved from
        /// `SidecarError::Application(code, ...)` when available. `None` for
        /// raw `SidecarError::Rpc(msg)` (no structured code). Used by
        /// `subagent_error_to_task_error_kind` to classify the failure
        /// without re-parsing the message string.
        code: Option<i32>,
    },

    #[error("sidecar io error: {0}")]
    SidecarIo(String),

    #[error("sidecar timeout: {0}")]
    SidecarTimeout(String),

    #[error("sidecar crashed: {0}")]
    SidecarCrashed(String),

    #[error("hot session limit reached, candidate: {candidate}")]
    HotSessionLimit {
        /// The adapter_session_id of the LRU eviction candidate. Empty
        /// when all hot sessions are in-use (busy) and no candidate is
        /// evictable — the executor surfaces this variant without
        /// attempting eviction (Bug 1 fix).
        candidate: String,
    },

    /// Hot session state divergence: the sidecar's view of sessions
    /// diverges from the DB/pool state (e.g. the sidecar named a candidate
    /// that no supervisor owns, or the DB has no hot binding for the named
    /// adapter_session_id). This is NOT a capacity condition — the pool
    /// may or may not be full. Retrying with backoff will not help; the
    /// caller should surface this for diagnosis (sidecar restart or DB
    /// reconciliation may be needed). Distinguished from `HotSessionLimit`
    /// (a genuine capacity condition) and `Validation` (input/config
    /// errors) so callers can handle state-divergence without conflating
    /// it with retryable capacity pressure.
    #[error("hot session state divergence: {0}")]
    HotSessionStateDivergence(String),

    #[error("{0}")]
    Validation(String),
}

impl SubagentError {
    /// Stable machine-readable code used by the control dispatcher.
    pub fn code(&self) -> &'static str {
        match self {
            SubagentError::NotFound(_) => "subagent.not_found",
            SubagentError::AmbiguousName(_) => "subagent.ambiguous_name",
            SubagentError::InvalidName(_) => "subagent.invalid_name",
            SubagentError::InvalidArgument(_) => "subagent.invalid_argument",
            SubagentError::ProfileNotFound(_) => "subagent.profile_not_found",
            SubagentError::Disabled => "subagent.disabled",
            SubagentError::Store(_) => "subagent.store_error",
            SubagentError::TaskTimeout => "subagent.task_timeout",
            SubagentError::SidecarSpawn(_) => "subagent.sidecar_spawn_failed",
            SubagentError::SidecarRpc { .. } => "subagent.sidecar_rpc_error",
            SubagentError::SidecarIo(_) => "subagent.sidecar_io_error",
            SubagentError::SidecarTimeout(_) => "subagent.sidecar_timeout",
            SubagentError::SidecarCrashed(_) => "subagent.sidecar_crashed",
            SubagentError::HotSessionLimit { .. } => "subagent.hot_session_limit",
            SubagentError::HotSessionStateDivergence(_) => "subagent.hot_session_state_divergence",
            SubagentError::Validation(_) => "subagent.validation_error",
        }
    }
}

/// Map a `SidecarError` to the semantically-equivalent `SubagentError`.
/// Application error codes (spec §4.2) are translated to domain variants so
/// the control contract (`subagent.profile_not_found`, `subagent.not_found`,
/// `subagent.task_timeout`) is honored even when the failure originates in the
/// sidecar subprocess.
impl From<SidecarError> for SubagentError {
    fn from(e: SidecarError) -> Self {
        match e {
            SidecarError::Spawn(msg) => SubagentError::SidecarSpawn(msg),
            SidecarError::Rpc(msg) => SubagentError::SidecarRpc {
                message: msg,
                code: None,
            },
            SidecarError::Timeout(msg) => SubagentError::SidecarTimeout(msg),
            SidecarError::Crashed(msg) => SubagentError::SidecarCrashed(msg),
            SidecarError::Io(msg) => SubagentError::SidecarIo(msg),
            SidecarError::Application(code, msg, data) => {
                use crate::sidecar::protocol::*;
                match code {
                    SESSION_NOT_FOUND => SubagentError::NotFound(msg),
                    PROFILE_NOT_FOUND => SubagentError::ProfileNotFound(msg),
                    TASK_TIMEOUT => SubagentError::TaskTimeout,
                    HOT_SESSION_LIMIT_REACHED => {
                        // Extract candidate from the RPC error's data field.
                        // The sidecar is the hot-pool authority (spec §4.4) —
                        // its data.candidate names the LRU session to evict.
                        let candidate = data
                            .as_ref()
                            .and_then(|d| d.get("candidate"))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        SubagentError::HotSessionLimit { candidate }
                    }
                    // Other application codes (SIDECAR_UNHEALTHY,
                    // TOOL_NOT_ALLOWED, INVALID_OUTPUT_SCHEMA, PROTOCOL_MISMATCH)
                    // surface as generic sidecar RPC errors. The numeric code
                    // is preserved in `code` so downstream classifiers
                    // (`subagent_error_to_task_error_kind`) can match on the
                    // structured value rather than re-parsing the message.
                    _ => SubagentError::SidecarRpc {
                        message: format!("[{code}] {msg}"),
                        code: Some(code),
                    },
                }
            }
        }
    }
}

pub type Result<T> = std::result::Result<T, SubagentError>;
