//! Task executor abstraction. Plan 1 had a mock; Plan 2 adds a sidecar-backed
//! executor. The trait lets `SubagentManager` stay executor-agnostic.

use crate::context::CompactContext;
use crate::context::MemorySnapshot;
use crate::memory::MemoryUpdate;
use crate::models::{TaskErrorKind, TaskStatus, TaskUsage};

/// Input to a task executor — everything needed to run one turn.
#[derive(Clone)]
pub struct ExecutorInput {
    /// Stable task identity threaded through the sidecar turn so a delayed
    /// cancel RPC cannot abort a replacement turn for the same subagent.
    pub task_id: String,
    pub subagent_id: String,
    pub subagent_name: String,
    pub cwd: String,
    pub profile: String,
    pub model: String,
    pub prompt: String,
    /// Spec §4.3: when set, the sidecar resolves this artifact path instead of
    /// the inline `prompt`. Mutually exclusive with `prompt`.
    pub prompt_artifact_ref: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub tools: Vec<String>,
    pub memory: MemorySnapshot,
    pub context: CompactContext,
    pub write_access: bool,
    /// Spec §3.3 + Task 5: bound provider id (NOT NULL — subagent is bound
    /// at create time). Threading it through the execution path lets the
    /// WorkerPool route to the correct per-provider supervisor.
    pub provider_id: String,
    /// Task 5: provider kind (OpenAiCompatible / AnthropicCompatible) so
    /// the sidecar knows which protocol to speak.
    pub provider_kind: busytok_domain::ProviderKind,
    /// Task 5: provider base URL (e.g. `https://api.openai.com/v1`).
    pub provider_base_url: String,
    /// 瞬态执行态数据：不写回 task row，不进日志明文，不进 DTO/response/diagnostic。
    /// Threading the API key end-to-end so the sidecar can register it in
    /// `AuthStorage` (Task 7) — never logged in plaintext.
    pub provider_api_key: String,
    // Model metadata — threaded to the sidecar so `registerProvider` can
    // build a complete model definition (spec §5.2).
    pub model_reasoning: bool,
    pub model_context_window: i64,
    pub model_max_tokens: i64,
    pub model_display_name: Option<String>,
}

/// Output from a task executor — mapped into `DelegateResult` by the manager.
pub struct ExecutorOutput {
    pub adapter_session_id: Option<String>,
    pub session_reused: bool,
    pub status: TaskStatus,
    pub summary: String,
    pub usage: TaskUsage,
    pub memory_update: MemoryUpdate,
    /// Phase 3: classified error kind for failed/timeout tasks. `None` on
    /// success or when the executor couldn't classify the failure (treated
    /// as `Unknown` by Task 3's error handler).
    pub error_kind: Option<TaskErrorKind>,
}

/// Executor trait — `SubagentManager` calls this to run a task.
/// Plan 1: `MockTaskExecutor`. Plan 2: `SidecarTaskExecutor`.
#[async_trait::async_trait]
pub trait TaskExecutor: Send + Sync {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput>;

    /// Cancel an in-flight `execute()` call for the given subagent. This is
    /// the execution-protocol counterpart to `SubagentManager::cancel_task`:
    /// the manager flips the DB status to `cancelled` and sends a local cancel
    /// signal (dropping the executor future), while this method actually
    /// aborts the underlying model call — stopping token generation at the
    /// provider.
    ///
    /// **Best-effort:** the caller MUST NOT assume the cancel succeeded.
    /// If the sidecar is unreachable, the session is not found, or the turn
    /// has already completed, this method returns `Ok(())` without error.
    /// The DB status is already `cancelled` regardless of this call's outcome.
    ///
    /// Default impl: no-op (mock executors don't have a real model call to
    /// cancel). `SidecarTaskExecutor` overrides this to send a
    /// `session.cancel` RPC to the sidecar process.
    async fn cancel(&self, _subagent_id: &str, _provider_id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    /// Cancel a specific task turn. Executors that do not need task-scoped
    /// cancellation inherit the legacy subagent-wide behavior.
    async fn cancel_for_task(
        &self,
        subagent_id: &str,
        provider_id: &str,
        _task_id: &str,
    ) -> anyhow::Result<()> {
        self.cancel(subagent_id, provider_id).await
    }

    /// Activate a session — move it from `pending` to `active` (LRU-eligible)
    /// in the sidecar's hot pool. Called by the manager AFTER the DB hot
    /// binding is committed. An activation error causes the manager to roll
    /// the binding back to warm/cold; the completed task result is preserved.
    ///
    /// Default impl: no-op (mock executors don't have a pending state).
    /// `SidecarTaskExecutor` overrides this to send a `session.activate` RPC.
    async fn activate_session(
        &self,
        _adapter_session_id: &str,
        _provider_id: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Close a sidecar session after a DB lifecycle transition. This is used
    /// both to remove an uncommitted pending session after a persistence
    /// failure and to release an active session after hibernate. The caller
    /// serializes it with same-subagent execution so a successful rebind
    /// cannot be closed accidentally.
    ///
    /// Default impl: no-op (mock executors don't have a sidecar pool).
    /// `SidecarTaskExecutor` overrides this to send a `session.close` RPC.
    async fn close_session(
        &self,
        _adapter_session_id: &str,
        _provider_id: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }
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
                model: Some(input.model.clone()),
                provider: Some("mock".to_string()),
                input_tokens: Some(input.prompt.len() as i64),
                output_tokens: Some(summary.len() as i64),
                ..Default::default()
            },
            memory_update: MemoryUpdate::default(),
            error_kind: None,
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
