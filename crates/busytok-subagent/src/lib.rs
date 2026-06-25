//! Busytok logical-subagent runtime.
//!
//! Owns long-lived subagent identity, memory, and task history. In this plan
//! (Step 1) task execution is a mock; the Pi sidecar executor lands in Plan 2.

pub mod error;
pub mod models;

pub use error::{Result, SubagentError};
