//! Task executor abstraction. Plan 1 had a mock; Plan 2 adds a sidecar-backed
//! executor. The trait lets `SubagentManager` stay executor-agnostic.

use crate::context::CompactContext;
use crate::context::MemorySnapshot;
use crate::memory::MemoryUpdate;
use crate::models::{TaskStatus, TaskUsage};

/// Input to a task executor — everything needed to run one turn.
#[derive(Clone)]
pub struct ExecutorInput {
    pub subagent_id: String,
    pub subagent_name: String,
    pub cwd: String,
    pub profile: String,
    pub model: Option<String>,
    pub prompt: String,
    /// Spec §4.3: when set, the sidecar resolves this artifact path instead of
    /// the inline `prompt`. Mutually exclusive with `prompt`.
    pub prompt_artifact_ref: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub tools: Vec<String>,
    pub memory: MemorySnapshot,
    pub context: CompactContext,
    pub write_access: bool,
}

/// Output from a task executor — mapped into `DelegateResult` by the manager.
pub struct ExecutorOutput {
    pub adapter_session_id: Option<String>,
    pub session_reused: bool,
    pub status: TaskStatus,
    pub summary: String,
    pub usage: TaskUsage,
    pub memory_update: MemoryUpdate,
}

/// Executor trait — `SubagentManager` calls this to run a task.
/// Plan 1: `MockTaskExecutor`. Plan 2: `SidecarTaskExecutor`.
#[async_trait::async_trait]
pub trait TaskExecutor: Send + Sync {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput>;
}

/// Deterministic in-process mock executor. Used by Plan 1 tests and by Plan 2
/// when `pi_sidecar.enabled = false`.
pub struct MockTaskExecutor;

#[async_trait::async_trait]
impl TaskExecutor for MockTaskExecutor {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        let summary = format!("[mock] no sidecar wired yet; prompt was: {}", input.prompt);
        Ok(ExecutorOutput {
            adapter_session_id: None,
            session_reused: false,
            status: TaskStatus::Completed,
            summary: summary.clone(),
            usage: TaskUsage {
                model: input.model.clone(),
                provider: Some("mock".to_string()),
                input_tokens: Some(input.prompt.len() as i64),
                output_tokens: Some(summary.len() as i64),
                ..Default::default()
            },
            memory_update: MemoryUpdate::default(),
        })
    }
}

/// Executor that always fails. Injected when `pi_sidecar.enabled = true`
/// but the sidecar config could not be resolved at supervisor construction
/// time. This ensures delegate calls fail loudly instead of silently
/// succeeding via `MockTaskExecutor` — which would mask a deployment
/// misconfiguration as "functional". The error is wrapped as
/// `SubagentError::SidecarSpawn` (via `anyhow::Error::from`) so
/// `SubagentManager::delegate` can downcast it and preserve the semantic
/// error code `subagent.sidecar_spawn_failed` through the RPC contract.
pub struct FailingTaskExecutor {
    pub reason: String,
}

#[async_trait::async_trait]
impl TaskExecutor for FailingTaskExecutor {
    async fn execute(&self, _input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        Err(anyhow::Error::from(
            crate::error::SubagentError::SidecarSpawn(format!(
                "sidecar was enabled but failed to initialize: {}",
                self.reason
            )),
        ))
    }
}
