use std::sync::Arc;

use async_trait::async_trait;
use tracing::{info, warn};

use crate::error::SubagentError;
use crate::mock_executor::{ExecutorInput, ExecutorOutput, TaskExecutor};
use crate::models::{TaskStatus, TaskUsage};
use crate::sidecar::supervisor::PiSidecarSupervisor;
use crate::sidecar::SidecarError;

pub struct SidecarTaskExecutor {
    supervisor: Arc<PiSidecarSupervisor>,
}

impl SidecarTaskExecutor {
    pub fn new(supervisor: Arc<PiSidecarSupervisor>) -> Self {
        Self { supervisor }
    }
}

#[async_trait]
impl TaskExecutor for SidecarTaskExecutor {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        let handle = self
            .supervisor
            .ensure_started()
            .await
            .map_err(sidecar_to_anyhow)?;
        // Note: `tools`, `prompt_artifact_ref`, and `memory_snapshot` are
        // deferred to Plan 4 (ContextBuilder). Plan 2 sends the minimal set.
        let params = serde_json::json!({
            "logical_subagent_id": input.subagent_id,
            "logical_subagent_name": input.subagent_name,
            "cwd": input.cwd,
            "profile": input.profile,
            "model": input.model,
            "prompt": input.prompt,
            "timeout_ms": input.timeout_seconds.map(|s| s * 1000),
        });
        info!(
            event_code = "subagent.sidecar.turn_auto.start",
            subagent_id = %input.subagent_id,
            profile = %input.profile,
            "sending turn_auto to sidecar"
        );
        let result = handle.turn_auto(params).await.map_err(|e| {
            warn!(event_code = "subagent.sidecar.turn_auto.failed", error = %e);
            sidecar_to_anyhow(e)
        })?;
        let adapter_session_id = result
            .get("adapter_session_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        let session_reused = result
            .get("session_reused")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let status_str = result
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("completed");
        let status = match status_str {
            "completed" => TaskStatus::Completed,
            "failed" => TaskStatus::Failed,
            "timeout" => TaskStatus::Failed,
            _ => TaskStatus::Completed,
        };
        let summary = result
            .pointer("/result/task_summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let usage = result
            .get("usage")
            .map(|u| TaskUsage {
                model: u.get("model").and_then(|v| v.as_str()).map(String::from),
                provider: u.get("provider").and_then(|v| v.as_str()).map(String::from),
                input_tokens: u.get("input_tokens").and_then(|v| v.as_i64()),
                output_tokens: u.get("output_tokens").and_then(|v| v.as_i64()),
                cache_read_tokens: u.get("cache_read_tokens").and_then(|v| v.as_i64()),
                cache_write_tokens: u.get("cache_write_tokens").and_then(|v| v.as_i64()),
                cost_usd: u.get("cost_usd").and_then(|v| v.as_f64()),
            })
            .unwrap_or_default();
        Ok(ExecutorOutput {
            adapter_session_id,
            session_reused,
            status,
            summary,
            usage,
        })
    }
}

/// Convert `SidecarError` → `SubagentError` (preserving application error codes)
/// → `anyhow::Error`. The `delegate()` method downcasts back to `SubagentError`
/// so the control contract (`subagent.profile_not_found`, etc.) is honored.
fn sidecar_to_anyhow(e: SidecarError) -> anyhow::Error {
    anyhow::Error::from(SubagentError::from(e))
}
