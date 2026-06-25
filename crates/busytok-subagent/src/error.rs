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

    #[error("sidecar rpc error: {0}")]
    SidecarRpc(String),

    #[error("sidecar io error: {0}")]
    SidecarIo(String),

    #[error("sidecar timeout: {0}")]
    SidecarTimeout(String),

    #[error("sidecar crashed: {0}")]
    SidecarCrashed(String),
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
            SubagentError::SidecarRpc(_) => "subagent.sidecar_rpc_error",
            SubagentError::SidecarIo(_) => "subagent.sidecar_io_error",
            SubagentError::SidecarTimeout(_) => "subagent.sidecar_timeout",
            SubagentError::SidecarCrashed(_) => "subagent.sidecar_crashed",
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
            SidecarError::Rpc(msg) => SubagentError::SidecarRpc(msg),
            SidecarError::Timeout(msg) => SubagentError::SidecarTimeout(msg),
            SidecarError::Crashed(msg) => SubagentError::SidecarCrashed(msg),
            SidecarError::Io(msg) => SubagentError::SidecarIo(msg),
            SidecarError::Application(code, msg) => {
                use crate::sidecar::protocol::*;
                match code {
                    SESSION_NOT_FOUND => SubagentError::NotFound(msg),
                    PROFILE_NOT_FOUND => SubagentError::ProfileNotFound(msg),
                    TASK_TIMEOUT => SubagentError::TaskTimeout,
                    // Other application codes (HOT_SESSION_LIMIT_REACHED,
                    // SIDECAR_UNHEALTHY, TOOL_NOT_ALLOWED, INVALID_OUTPUT_SCHEMA,
                    // PROTOCOL_MISMATCH) surface as generic sidecar RPC errors.
                    _ => SubagentError::SidecarRpc(format!("[{code}] {msg}")),
                }
            }
        }
    }
}

pub type Result<T> = std::result::Result<T, SubagentError>;
