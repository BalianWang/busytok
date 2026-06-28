//! Domain models for the logical-subagent layer.
//!
//! These are the in-memory types the manager works with. Persistence uses the
//! `…Row` structs in `busytok_store::repository`; conversions happen at the
//! manager boundary.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Runtime status of a logical subagent (see spec §3.3 state machine).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentStatus {
    Hot,
    Warm,
    Cold,
    Deleted,
}

impl SubagentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SubagentStatus::Hot => "hot",
            SubagentStatus::Warm => "warm",
            SubagentStatus::Cold => "cold",
            SubagentStatus::Deleted => "deleted",
        }
    }
}

impl FromStr for SubagentStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "hot" => Ok(Self::Hot),
            "warm" => Ok(Self::Warm),
            "cold" => Ok(Self::Cold),
            "deleted" => Ok(Self::Deleted),
            other => Err(format!("invalid subagent status: {other}")),
        }
    }
}

/// Lifecycle status of a single delegated task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Queued => "queued",
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }
}

impl FromStr for TaskStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("invalid task status: {other}")),
        }
    }
}

/// A logical subagent — long-lived identity stored in SQLite.
#[derive(Debug, Clone, Serialize)]
pub struct LogicalSubagent {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub repo_path: String,
    pub repo_hash: String,
    pub branch: Option<String>,
    pub intent: Option<String>,
    pub default_profile: String,
    pub default_model: Option<String>,
    pub status: SubagentStatus,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_active_at_ms: Option<i64>,
}

impl LogicalSubagent {
    /// Names must be 1..=64 chars of `[A-Za-z0-9._-]`, no leading dot.
    pub fn is_valid_name(name: &str) -> bool {
        if name.is_empty() || name.len() > 64 || name.starts_with('.') {
            return false;
        }
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    }
}

/// A single delegated task (summary view).
#[derive(Debug, Clone, Serialize)]
pub struct SubagentTaskSummary {
    pub id: String,
    pub subagent_id: String,
    pub profile: String,
    pub status: TaskStatus,
    pub prompt: Option<String>,
    pub result_summary: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}

/// Request to create-or-continue a subagent and run one task.
#[derive(Debug, Clone, Deserialize)]
pub struct DelegateRequest {
    pub subagent_name: String,
    /// UUID shortcut, bypassing name resolution.
    pub subagent_id: Option<String>,
    pub cwd: String,
    pub profile: String,
    pub intent: Option<String>,
    pub prompt: String,
    /// Spec §4.3: when set, references a stored artifact instead of the inline
    /// `prompt`. Mutually exclusive with `prompt`.
    pub prompt_artifact_ref: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub model_override: Option<String>,
    pub source_harness: Option<String>,
    pub source_session_id: Option<String>,
}

/// Resolution params for single-subagent operations (show/tasks/hibernate/delete).
/// Exactly one of `id` (UUID) or `name` (+ `cwd`) is set.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResolveParams {
    pub name: Option<String>,
    pub id: Option<String>,
    pub cwd: Option<String>,
}

/// Token usage returned by a task (mock in this plan).
#[derive(Debug, Clone, Default, Serialize)]
pub struct TaskUsage {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
}

/// The result of a `delegate` call.
#[derive(Debug, Clone, Serialize)]
pub struct DelegateResult {
    pub task_id: String,
    pub subagent_id: String,
    pub subagent_name: String,
    pub adapter: String,
    pub adapter_session_id: Option<String>,
    pub session_reused: bool,
    pub status: TaskStatus,
    pub profile: String,
    pub model: Option<String>,
    pub summary: Option<String>,
    pub usage: TaskUsage,
}
