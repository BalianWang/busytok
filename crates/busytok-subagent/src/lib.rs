//! Busytok logical-subagent runtime.
//!
//! Owns long-lived subagent identity, memory, and task history. In this plan
//! (Step 1) task execution is a mock; the Pi sidecar executor lands in Plan 2.

pub mod context;
pub mod error;
pub mod manager;
pub mod memory;
pub mod mock_executor;
pub mod models;
pub mod pressure;
pub mod resolver;
pub mod resource;
pub mod sidecar;
pub mod util;

pub use error::{Result, SubagentError};
pub use manager::{CancelOutcome, LifecycleRegistry, SubagentManager, TaskCompletionHook};
pub use models::{DelegateRequest, DelegateResult, QueueReason, TaskErrorKind, TaskStatus};
pub use pressure::{PressureAction, PressureGate, PressureResponder};
