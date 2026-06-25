//! Errors for the logical-subagent management layer.
//!
//! Each variant maps to a control-protocol error code so the dispatcher can
//! surface a stable, machine-readable failure to clients.

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
        }
    }
}

pub type Result<T> = std::result::Result<T, SubagentError>;
