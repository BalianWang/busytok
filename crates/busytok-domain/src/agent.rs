use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Which AI agent produced the log source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    ClaudeCode,
    Codex,
}

impl AgentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude_code",
            AgentKind::Codex => "codex",
        }
    }
}

impl std::fmt::Display for AgentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for AgentKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "claude_code" | "claude" | "Claude Code" => Ok(AgentKind::ClaudeCode),
            "codex" | "Codex" => Ok(AgentKind::Codex),
            other => Err(format!("unknown agent kind: {other}")),
        }
    }
}

/// The storage format of a log source directory or file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogSourceType {
    Jsonl,
    SQLite,
    Directory,
}

/// Operational status of a log source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogSourceStatus {
    #[default]
    Active,
    Paused,
    Error,
    Unknown,
}

/// A source defines where a category of logs comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSource {
    pub id: String,
    pub agent: AgentKind,
    pub root_path: PathBuf,
    pub source_type: LogSourceType,
    pub status: LogSourceStatus,
}

impl LogSource {
    pub fn for_test(id: &str, agent: AgentKind) -> Self {
        Self {
            id: id.to_string(),
            agent,
            root_path: PathBuf::from("/tmp/test-source"),
            source_type: LogSourceType::Jsonl,
            status: LogSourceStatus::Active,
        }
    }
}

/// State of a tracked physical log file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFileState {
    #[default]
    Active,
    Missing,
    Rotated,
    Truncated,
    Error,
    Completed,
}

/// A file represents a tracked physical file with checkpoint state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogFile {
    pub id: String,
    pub source_id: String,
    pub path: PathBuf,
    pub inode: Option<String>,
    pub size_bytes: u64,
    pub offset_bytes: u64,
    pub last_mtime_ms: Option<i64>,
    pub state: LogFileState,
}

impl LogFile {
    pub fn for_test(id: &str, source_id: &str) -> Self {
        Self {
            id: id.to_string(),
            source_id: source_id.to_string(),
            path: PathBuf::from("/tmp/test-file.jsonl"),
            inode: None,
            size_bytes: 0,
            offset_bytes: 0,
            last_mtime_ms: None,
            state: LogFileState::Active,
        }
    }
}

/// Operational health status for an agent source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatus {
    pub agent: AgentKind,
    pub discovered: bool,
    pub watching: bool,
    pub last_successful_update_ms: Option<i64>,
    pub file_count: u64,
    pub parsed_file_count: u64,
    pub scan_state: String,
}

impl AgentStatus {
    pub fn for_test(agent: AgentKind) -> Self {
        Self {
            agent,
            discovered: true,
            watching: false,
            last_successful_update_ms: None,
            file_count: 0,
            parsed_file_count: 0,
            scan_state: "idle".to_string(),
        }
    }
}

/// Session rollup summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub agent: Option<AgentKind>,
    pub project_hash: Option<String>,
    pub started_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub model_list_json: Option<String>,
    pub total_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub event_count: i64,
}

impl SessionSummary {
    pub fn for_test(id: &str) -> Self {
        Self {
            id: id.to_string(),
            agent: None,
            project_hash: None,
            started_at_ms: 0,
            last_seen_at_ms: 0,
            model_list_json: None,
            total_tokens: 0,
            total_cost_usd: None,
            event_count: 0,
        }
    }
}

/// A billing block — typically 5 hours, matching Claude's billing windows.
/// Groups usage events into time-based windows with gap detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingBlock {
    /// ISO string of block start time (floored to hour)
    pub id: String,
    /// Block start time (floored to the hour)
    pub start_time_ms: i64,
    /// Block end time (start + duration_hours)
    pub end_time_ms: i64,
    /// Last activity time in the block
    pub actual_end_time_ms: Option<i64>,
    /// Whether the block is still active (last event within session_duration of now)
    pub is_active: bool,
    /// True if this is a gap block (no activity period between real blocks)
    pub is_gap: bool,
    /// Token counts for this block
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    /// Total cost for the block
    pub cost_usd: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
    /// Models used in this block
    pub models: Vec<String>,
    /// Event count
    pub event_count: i64,
    /// Usage limit reset time (from API error messages)
    pub usage_limit_reset_time_ms: Option<i64>,
    /// Agent kind
    pub agent: Option<String>,
}

impl BillingBlock {
    pub fn entries_count(&self) -> i64 {
        self.event_count
    }
}

/// Burn rate status level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BurnStatus {
    Normal,
    Moderate,
    High,
}

/// Burn rate information for a billing block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BurnRate {
    pub tokens_per_minute: f64,
    /// Cost per hour. `None` when cost data is unavailable (vs. `Some(0.0)`
    /// which means the block had zero cost).
    pub cost_per_hour: Option<f64>,
    pub status: BurnStatus,
}

/// Project rollup summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub project_hash: Option<String>,
    pub project_path: Option<String>,
    pub total_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub event_count: i64,
    pub session_count: i64,
}

impl ProjectSummary {
    pub fn for_test(hash: &str) -> Self {
        Self {
            project_hash: Some(hash.to_string()),
            project_path: None,
            total_tokens: 0,
            total_cost_usd: None,
            event_count: 0,
            session_count: 0,
        }
    }
}

/// Model rollup summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSummary {
    pub model: Option<String>,
    pub total_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub event_count: i64,
}

impl ModelSummary {
    pub fn for_test(model: &str) -> Self {
        Self {
            model: Some(model.to_string()),
            total_tokens: 0,
            total_cost_usd: None,
            event_count: 0,
        }
    }
}

/// Precomputed summary for the home dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealtimeSummary {
    pub total_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub total_events: i64,
    pub total_sessions: i64,
    pub total_projects: i64,
}

impl RealtimeSummary {
    pub fn for_test() -> Self {
        Self {
            total_tokens: 0,
            total_cost_usd: None,
            total_events: 0,
            total_sessions: 0,
            total_projects: 0,
        }
    }
}
