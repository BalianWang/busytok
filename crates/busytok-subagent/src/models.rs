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

    /// Whether this status is terminal (no further transitions).
    /// `Completed`, `Failed`, and `Cancelled` are terminal; `Queued` and
    /// `Running` are not.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        )
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
    pub bound_provider_id: String,
    pub bound_model_id: String,
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
    /// Spec §3.3: when creating a new subagent, both must be provided
    /// together. Ignored when reusing an existing subagent.
    pub bound_provider_id: Option<String>,
    pub bound_model_id: Option<String>,
    /// Reuse policy for name-based resolution:
    /// - `create`: fail if a subagent with the same name exists; otherwise create new
    /// - `reuse`: fail if no such subagent exists; otherwise reuse existing
    /// - `fail`: fail if a subagent with the same name exists (alias for `create`)
    ///
    /// Default (None): create-or-reuse, but fail if `--bind-*` is given
    ///   and the existing subagent's binding differs from the request.
    pub reuse_policy: Option<String>,
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

/// Why a task was placed in `queued` status instead of starting immediately.
///
/// Surfaced on `DelegateResult` so external orchestrators (CLI, automation
/// scripts) can distinguish "gate paused" from "subagent busy" from
/// "global hot session capacity contention" without guessing from logs.
/// `None` when the task started immediately (`running`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueReason {
    /// The pressure gate is paused (memory pressure or sidecar RSS limit).
    /// The task will start automatically when the gate clears and the
    /// subagent is free.
    PressureGatePaused,
    /// The subagent already has a running task. The queued task will start
    /// after the current one completes (per-subagent FIFO).
    SubagentBusy,
    /// Cross-subagent global hot session capacity contention: all hot
    /// sessions for the bound provider/model were busy when the task
    /// attempted to start, so it was re-queued for dispatcher retry.
    /// Distinguished from `SubagentBusy` (same subagent already running)
    /// so orchestrators can tell per-subagent serialization apart from
    /// global capacity queuing.
    HotSessionLimit,
}

impl QueueReason {
    /// Stable string representation for the IPC/DTO boundary.
    /// Mirrors `#[serde(rename_all = "snake_case")]` — keep in sync.
    pub fn as_str(&self) -> &'static str {
        match self {
            QueueReason::PressureGatePaused => "pressure_gate_paused",
            QueueReason::SubagentBusy => "subagent_busy",
            QueueReason::HotSessionLimit => "hot_session_limit",
        }
    }
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
    /// Whether a new subagent was created (true) or an existing one was
    /// reused (false). Surfaced on the response DTO so callers can verify
    /// the reuse-policy outcome.
    pub created: bool,
    /// Why the task was queued (`None` when the task started immediately).
    /// Present only when `status == Queued`. Lets CLI/automation distinguish
    /// "blocked by pressure gate" from "subagent busy" without reading logs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_reason: Option<QueueReason>,
}

/// Classification of task failures (Task 1 / Phase 3). Used by Task 3's
/// error-handling layer to pick a recovery strategy:
/// - `Auth`        → credential refresh / re-prompt (Phase 4 UI)
/// - `RateLimit`   → exponential backoff with jitter
/// - `Timeout`     → retry once with a longer budget
/// - `Crash`       → sidecar restart (supervisor crash-reconciliation)
/// - `Network`     → retry with circuit-breaker
/// - `Unknown`     → propagate to caller, no auto-recovery
///
/// Serialized as `snake_case` for stable JSON contracts across the IPC
/// boundary (Tauri command results, sidecar JSON-RPC error data).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskErrorKind {
    Auth,
    RateLimit,
    Timeout,
    Crash,
    Network,
    /// Hot session capacity contention — all hot sessions were busy
    /// and retries/re-queues were exhausted. Distinguished from `Unknown`
    /// so callers can surface "capacity limit reached" instead of a
    /// generic failure.
    HotSessionLimit,
    Unknown,
}

impl TaskErrorKind {
    /// Stable snake_case string for DB persistence (Task 5). Matches the
    /// `#[serde(rename_all = "snake_case")]` serialization so the value
    /// stored in `subagent_tasks.error_kind` is the same string that
    /// serde_json would emit.
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskErrorKind::Auth => "auth",
            TaskErrorKind::RateLimit => "rate_limit",
            TaskErrorKind::Timeout => "timeout",
            TaskErrorKind::Crash => "crash",
            TaskErrorKind::Network => "network",
            TaskErrorKind::HotSessionLimit => "hot_session_limit",
            TaskErrorKind::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `TaskErrorKind` serializes to `snake_case` so the JSON contract is
    /// stable across IPC (Tauri commands, sidecar JSON-RPC error data).
    #[test]
    fn task_error_kind_serializes_snake_case() {
        let cases = [
            (TaskErrorKind::Auth, "\"auth\""),
            (TaskErrorKind::RateLimit, "\"rate_limit\""),
            (TaskErrorKind::Timeout, "\"timeout\""),
            (TaskErrorKind::Crash, "\"crash\""),
            (TaskErrorKind::Network, "\"network\""),
            (TaskErrorKind::Unknown, "\"unknown\""),
        ];
        for (kind, expected) in cases {
            let s = serde_json::to_string(&kind).unwrap();
            assert_eq!(s, expected, "serialize mismatch for {kind:?}");
            // Round-trip back to the same variant.
            let back: TaskErrorKind = serde_json::from_str(&s).unwrap();
            assert_eq!(back, kind, "round-trip mismatch for {kind:?}");
        }
    }

    /// `QueueReason` serializes to stable `snake_case` strings for the IPC
    /// contract (CLI output, Tauri command results). Mirrors the
    /// `task_error_kind_serializes_snake_case` pattern. `QueueReason` is
    /// `Serialize`-only (output field), so no round-trip test.
    #[test]
    fn queue_reason_serializes_snake_case() {
        let cases = [
            (QueueReason::PressureGatePaused, "\"pressure_gate_paused\""),
            (QueueReason::SubagentBusy, "\"subagent_busy\""),
            (QueueReason::HotSessionLimit, "\"hot_session_limit\""),
        ];
        for (reason, expected) in cases {
            let s = serde_json::to_string(&reason).unwrap();
            assert_eq!(s, expected, "serialize mismatch for {reason:?}");
        }
    }

    /// `as_str()` must return the same string serde emits (without quotes).
    /// The DTO boundary in `supervisor.rs` uses `as_str()` — NOT serde — to
    /// convert `QueueReason` to a string for `SubagentDelegateResponseDto`.
    /// If `as_str()` and serde diverge, the JSON output would carry a
    /// different string than the DB-persisted value. This cross-check catches
    /// that drift for both variants. Mirrors `task_error_kind_as_str_matches_serde`.
    #[test]
    fn queue_reason_as_str_matches_serde() {
        let cases = [
            (QueueReason::PressureGatePaused, "pressure_gate_paused"),
            (QueueReason::SubagentBusy, "subagent_busy"),
            (QueueReason::HotSessionLimit, "hot_session_limit"),
        ];
        for (reason, expected) in cases {
            assert_eq!(reason.as_str(), expected, "as_str mismatch for {reason:?}");
            // Cross-check: as_str() must equal the serde serialization
            // (without the surrounding quotes).
            let serde = serde_json::to_string(&reason).unwrap();
            let stripped = serde.trim_matches('"');
            assert_eq!(reason.as_str(), stripped, "as_str != serde for {reason:?}");
        }
    }

    /// All variants are constructible and comparable — Task 3's error
    /// classifier will `match` on this enum, so variant equality matters.
    #[test]
    fn task_error_kind_variants_distinct() {
        let all = [
            TaskErrorKind::Auth,
            TaskErrorKind::RateLimit,
            TaskErrorKind::Timeout,
            TaskErrorKind::Crash,
            TaskErrorKind::Network,
            TaskErrorKind::HotSessionLimit,
            TaskErrorKind::Unknown,
        ];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b, "variants {a:?} and {b:?} must be distinct");
                }
            }
        }
    }

    /// `as_str()` returns the stable snake_case string used for DB persistence
    /// (Task 5). It must match the `#[serde(rename_all = "snake_case")]`
    /// serialization so the value stored in `subagent_tasks.error_kind` is the
    /// same string serde_json would emit.
    #[test]
    fn task_error_kind_as_str_matches_serde() {
        let cases = [
            (TaskErrorKind::Auth, "auth"),
            (TaskErrorKind::RateLimit, "rate_limit"),
            (TaskErrorKind::Timeout, "timeout"),
            (TaskErrorKind::Crash, "crash"),
            (TaskErrorKind::Network, "network"),
            (TaskErrorKind::HotSessionLimit, "hot_session_limit"),
            (TaskErrorKind::Unknown, "unknown"),
        ];
        for (kind, expected) in cases {
            assert_eq!(kind.as_str(), expected, "as_str mismatch for {kind:?}");
            // Cross-check: as_str() must equal the serde serialization
            // (without the surrounding quotes).
            let serde = serde_json::to_string(&kind).unwrap();
            let stripped = serde.trim_matches('"');
            assert_eq!(kind.as_str(), stripped, "as_str != serde for {kind:?}");
        }
    }
}
