//! In-process mock task executor (Step 1 only).
//!
//! Produces a deterministic canned result so the management layer, store,
//! control protocol, and CLI can be validated without the Pi sidecar.
//! Plan 2 swaps in the real sidecar-backed `TaskExecutor`.

use crate::models::{TaskStatus, TaskUsage};

pub struct MockTaskOutput {
    pub status: TaskStatus,
    pub summary: String,
    pub usage: TaskUsage,
}

/// Run a mock task. The summary echoes the prompt so tests can assert on it.
pub(crate) fn run_mock(prompt: &str, model: Option<&str>) -> MockTaskOutput {
    let summary = format!("[mock] no sidecar wired yet; prompt was: {prompt}");
    MockTaskOutput {
        status: TaskStatus::Completed,
        summary: summary.clone(),
        usage: TaskUsage {
            model: model.map(|s| s.to_string()),
            provider: Some("mock".to_string()),
            input_tokens: Some(prompt.len() as i64),
            output_tokens: Some(summary.len() as i64),
            ..Default::default()
        },
    }
}
