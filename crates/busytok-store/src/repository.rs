use busytok_domain::{
    CodexTokenSnapshot, NormalizedUsageEvent, OperationalDiagnosticEvent, ToolEvent,
    UsageWritePolicy,
};
use serde::Serialize;

/// Rows produced by the rollup builder closure inside ingest_store_batch.
///
/// The closure receives the list of truly-inserted event IDs and must
/// return the corresponding aggregate rows. This keeps rollup computation
/// inside the same transaction as raw event ingest.
#[derive(Debug, Clone, Default)]
pub struct RollupRows {
    pub daily_usage_rows: Vec<DailyUsageRow>,
    pub model_usage_rows: Vec<ModelUsageRow>,
    pub session_rows: Vec<SessionRow>,
    pub project_rows: Vec<ProjectRow>,
    pub model_summary_rows: Vec<ModelSummaryRow>,
}

/// A Codex token snapshot row matching the `codex_token_snapshots` table schema.
///
/// This stores cumulative token snapshots from Codex logs. The runtime
/// uses these to compute deltas between consecutive snapshots.
#[derive(Debug, Clone)]
pub struct CodexTokenSnapshotRow {
    pub id: String,
    pub source_file_id: String,
    pub source_line: i64,
    pub source_offset_start: i64,
    pub source_offset_end: i64,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub token_event_ordinal: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
    pub model: Option<String>,
    pub raw_usage_json: String,
    pub emitted_event_id: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl CodexTokenSnapshotRow {
    /// Create a snapshot row from a domain CodexTokenSnapshot.
    pub fn from_domain(
        snapshot: &CodexTokenSnapshot,
        ordinal: i64,
        emitted_event_id: Option<String>,
    ) -> Self {
        let now_ms = busytok_domain::now_ms();
        let id = format!(
            "codex-snap:{}:{}:{}:{}",
            snapshot.source_file_id,
            snapshot.session_id,
            snapshot.turn_id.as_deref().unwrap_or("none"),
            ordinal
        );
        Self {
            id,
            source_file_id: snapshot.source_file_id.clone(),
            source_line: snapshot.source_line as i64,
            source_offset_start: snapshot.source_offset_start as i64,
            source_offset_end: snapshot.source_offset_end as i64,
            session_id: snapshot.session_id.clone(),
            turn_id: snapshot.turn_id.clone(),
            token_event_ordinal: ordinal,
            input_tokens: snapshot.input_tokens,
            cached_input_tokens: snapshot.cached_input_tokens,
            output_tokens: snapshot.output_tokens,
            reasoning_tokens: snapshot.reasoning_tokens,
            total_tokens: snapshot.total_tokens,
            model: snapshot.model.clone(),
            raw_usage_json: snapshot.raw_usage_json.clone(),
            emitted_event_id,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }
}

/// A storage-row container for atomic batch writes.
///
/// Runtime builds it from adapter output plus aggregator mutations.
/// Store validates and atomically writes the rows. If any write fails,
/// the entire transaction rolls back and the checkpoint must not advance.
#[derive(Debug, Clone, Default)]
pub struct StoreWriteBatch {
    pub source_id: String,
    pub source_file_id: Option<String>,
    pub source_file_agent: String,
    pub source_file_path: String,
    pub source_file_inode: Option<String>,
    pub checkpoint_offset: Option<u64>,
    pub usage_events: Vec<(NormalizedUsageEvent, UsageWritePolicy)>,
    pub tool_events: Vec<ToolEvent>,
    pub diagnostic_events: Vec<OperationalDiagnosticEvent>,
    pub codex_snapshots: Vec<CodexTokenSnapshotRow>,
    pub daily_usage_rows: Vec<DailyUsageRow>,
    pub model_usage_rows: Vec<ModelUsageRow>,
    pub realtime_summary_rows: Vec<RealtimeSummaryRow>,
    pub session_rows: Vec<SessionRow>,
    pub project_rows: Vec<ProjectRow>,
    pub model_summary_rows: Vec<ModelSummaryRow>,
}

impl StoreWriteBatch {
    /// Create a batch for the given source and file.
    pub fn for_test(source_id: &str, source_file_id: &str) -> Self {
        Self {
            source_id: source_id.to_string(),
            source_file_id: Some(source_file_id.to_string()),
            source_file_agent: "claude_code".to_string(),
            source_file_path: format!("/tmp/{source_file_id}.jsonl"),
            ..Default::default()
        }
    }

    /// Add a usage event with its write policy.
    pub fn usage_event(mut self, event: NormalizedUsageEvent, policy: UsageWritePolicy) -> Self {
        self.usage_events.push((event, policy));
        self
    }

    /// Add a diagnostic event.
    pub fn diagnostic(mut self, event: OperationalDiagnosticEvent) -> Self {
        self.diagnostic_events.push(event);
        self
    }

    /// Add a tool event.
    pub fn tool_event(mut self, event: ToolEvent) -> Self {
        self.tool_events.push(event);
        self
    }

    /// Add a daily usage row.
    pub fn daily_usage_row(mut self, row: DailyUsageRow) -> Self {
        self.daily_usage_rows.push(row);
        self
    }

    /// Add a model usage row.
    pub fn model_usage_row(mut self, row: ModelUsageRow) -> Self {
        self.model_usage_rows.push(row);
        self
    }

    /// Add a realtime summary row.
    pub fn realtime_summary_row(mut self, row: RealtimeSummaryRow) -> Self {
        self.realtime_summary_rows.push(row);
        self
    }

    /// Add a session row.
    pub fn session_row(mut self, row: SessionRow) -> Self {
        self.session_rows.push(row);
        self
    }

    /// Add a project row.
    pub fn project_row(mut self, row: ProjectRow) -> Self {
        self.project_rows.push(row);
        self
    }

    /// Add a model summary row.
    pub fn model_summary_row(mut self, row: ModelSummaryRow) -> Self {
        self.model_summary_rows.push(row);
        self
    }

    /// Set the checkpoint offset to advance after successful write.
    pub fn checkpoint_offset(mut self, offset: u64) -> Self {
        self.checkpoint_offset = Some(offset);
        self
    }
}

/// A daily usage aggregate row.
#[derive(Debug, Clone, Serialize)]
pub struct DailyUsageRow {
    pub date: String,
    pub timezone: String,
    pub agent: String,
    pub project_hash: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub reasoning_tokens: i64,
    pub thoughts_tokens: i64,
    pub tool_tokens: i64,
    pub cost_usd: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
    pub event_count: i64,
    pub generation_id: String,
}

impl DailyUsageRow {
    /// Create a minimal daily usage row for test use.
    pub fn for_test(date: &str, timezone: &str, generation_id: &str, total_tokens: i64) -> Self {
        Self {
            date: date.to_string(),
            timezone: timezone.to_string(),
            agent: "claude_code".to_string(),
            project_hash: String::new(),
            model: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            total_tokens,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            reasoning_tokens: 0,
            thoughts_tokens: 0,
            tool_tokens: 0,
            cost_usd: None,
            estimated_cost_usd: None,
            event_count: 1,
            generation_id: generation_id.to_string(),
        }
    }
}

/// A model usage aggregate row.
#[derive(Debug, Clone)]
pub struct ModelUsageRow {
    pub model: String,
    pub agent: String,
    pub timezone: String,
    pub date: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cached_input_tokens: i64,
    pub reasoning_tokens: i64,
    pub cost_usd: Option<f64>,
    pub event_count: i64,
}

impl ModelUsageRow {
    /// Create a minimal model usage row for test use.
    pub fn for_test(model: &str, total_tokens: i64) -> Self {
        Self {
            model: model.to_string(),
            agent: "claude_code".to_string(),
            timezone: "UTC".to_string(),
            date: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            total_tokens,
            cached_input_tokens: 0,
            reasoning_tokens: 0,
            cost_usd: None,
            event_count: 1,
        }
    }
}

/// A realtime summary key-value row.
#[derive(Debug, Clone)]
pub struct RealtimeSummaryRow {
    pub key: String,
    pub value_json: String,
}

impl RealtimeSummaryRow {
    /// Create a minimal realtime summary row for test use.
    pub fn for_test(key: &str, value_json: &str) -> Self {
        Self {
            key: key.to_string(),
            value_json: value_json.to_string(),
        }
    }
}

/// A session rollup row matching the `sessions` table schema.
#[derive(Debug, Clone, Serialize)]
pub struct SessionRow {
    pub id: String,
    pub agent: String,
    pub project_hash: Option<String>,
    pub started_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub model_list_json: String,
    pub total_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub event_count: i64,
    pub is_active: i32,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl SessionRow {
    /// Create a minimal session row for test use.
    pub fn for_test(id: &str) -> Self {
        Self {
            id: id.to_string(),
            agent: "claude_code".to_string(),
            project_hash: None,
            started_at_ms: 0,
            last_seen_at_ms: 0,
            model_list_json: "[]".to_string(),
            total_tokens: 0,
            total_cost_usd: None,
            event_count: 0,
            is_active: 0,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }
}

/// A project rollup row matching the `projects` table schema.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectRow {
    pub id: String,
    pub project_hash: String,
    pub project_path: Option<String>,
    pub agent: Option<String>,
    pub display_name: Option<String>,
    pub first_seen_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub total_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub session_count: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl ProjectRow {
    /// Create a minimal project row for test use.
    pub fn for_test(hash: &str) -> Self {
        Self {
            id: hash.to_string(),
            project_hash: hash.to_string(),
            project_path: None,
            agent: None,
            display_name: None,
            first_seen_at_ms: 0,
            last_seen_at_ms: 0,
            total_tokens: 0,
            total_cost_usd: None,
            session_count: 0,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }
}

/// A model summary row matching the `model_summary` table schema.
#[derive(Debug, Clone, Serialize)]
pub struct ModelSummaryRow {
    pub model: String,
    pub total_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub event_count: i64,
}

impl ModelSummaryRow {
    /// Create a minimal model summary row for test use.
    pub fn for_test(model: &str) -> Self {
        Self {
            model: model.to_string(),
            total_tokens: 0,
            total_cost_usd: None,
            event_count: 0,
        }
    }
}

/// A log file row from the database.
#[derive(Debug, Clone)]
pub struct LogFileRow {
    pub id: String,
    pub source_id: String,
    pub agent: String,
    pub path: String,
    pub inode: Option<String>,
    pub size_bytes: i64,
    pub offset_bytes: i64,
    pub last_mtime_ms: Option<i64>,
    pub first_seen_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub state: String,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// A log source row from the database.
#[derive(Debug, Clone)]
pub struct LogSourceRow {
    pub id: String,
    pub agent: String,
    pub source_type: String,
    pub root_path: String,
    pub configured_by_user: i32,
    pub default_discovery_enabled: i32,
    pub status: String,
    pub last_scan_started_at_ms: Option<i64>,
    pub last_scan_completed_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub first_seen_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// A diagnostic event row for API responses.
#[derive(Debug, Clone)]
pub struct DiagnosticEventRow {
    pub id: String,
    pub severity: String,
    pub code: String,
    pub message: String,
    pub happened_at_ms: i64,
}

/// Health information about the SQLite store.
#[derive(Debug, Clone)]
pub struct StoreHealthInfo {
    pub healthy: bool,
    pub integrity_message: String,
    pub migration_version: i32,
    pub db_size_bytes: i64,
    pub usage_event_count: i64,
    pub last_log_checkpoint_ms: Option<i64>,
}

// ---------------------------------------------------------------------------
// Logical subagent runtime rows (migration 0003)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SubagentLogicalSubagentRow {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub repo_path: String,
    pub repo_hash: String,
    pub branch: Option<String>,
    pub intent: Option<String>,
    pub default_profile: String,
    pub default_model: Option<String>,
    pub status: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_active_at_ms: Option<i64>,
}

impl SubagentLogicalSubagentRow {
    /// Minimal row for tests. Timestamps seeded from `now_ms()`; status `cold`.
    pub fn for_test(id: &str, name: &str) -> Self {
        let now = busytok_domain::now_ms();
        Self {
            id: id.to_string(),
            name: name.to_string(),
            project_id: "repo-hash-test".to_string(),
            repo_path: "/tmp/repo".to_string(),
            repo_hash: "repo-hash-test".to_string(),
            branch: None,
            intent: None,
            default_profile: "pi/search-cheap".to_string(),
            default_model: None,
            status: "cold".to_string(),
            created_at_ms: now,
            updated_at_ms: now,
            last_active_at_ms: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubagentMemoryRow {
    pub id: String,
    pub subagent_id: String,
    pub hot_summary: Option<String>,
    pub long_summary: Option<String>,
    pub key_files_json: Option<String>,
    pub decisions_json: Option<String>,
    pub attempts_json: Option<String>,
    pub open_questions_json: Option<String>,
    pub artifact_refs_json: Option<String>,
    pub last_compacted_at_ms: Option<i64>,
    pub last_compacted_task_id: Option<String>,
    pub updated_at_ms: i64,
}

impl SubagentMemoryRow {
    /// Create an empty memory row for a subagent (used on creation and as a
    /// fallback when no memory exists yet).
    pub fn new_empty(subagent_id: &str) -> Self {
        Self {
            id: format!("mem-{subagent_id}"),
            subagent_id: subagent_id.to_string(),
            hot_summary: None,
            long_summary: None,
            key_files_json: None,
            decisions_json: None,
            attempts_json: None,
            open_questions_json: None,
            artifact_refs_json: None,
            last_compacted_at_ms: None,
            last_compacted_task_id: None,
            updated_at_ms: busytok_domain::now_ms(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubagentTaskRow {
    pub id: String,
    pub subagent_id: String,
    pub source_harness: Option<String>,
    pub source_session_id: Option<String>,
    pub intent: Option<String>,
    pub profile: String,
    pub prompt: Option<String>,
    pub prompt_artifact_ref: Option<String>,
    pub output_schema_name: Option<String>,
    pub output_schema_version: i64,
    pub status: String,
    pub result_summary: Option<String>,
    pub result_json: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
    /// Task 7 Round 3 Finding 1 fix: persisted execution param so the
    /// dispatcher reads it from the row (single source of truth). `None`
    /// when not specified by the caller (use profile/adapter default).
    pub timeout_seconds: Option<i64>,
    /// Task 7 Round 3 Finding 1 fix: persisted model override so the
    /// dispatcher reads it from the row. `None` → fall back to
    /// `subagent.default_model` then `profile_model`.
    pub model_override: Option<String>,
}

impl SubagentTaskRow {
    pub fn for_test(id: &str, subagent_id: &str, profile: &str, prompt: &str) -> Self {
        Self {
            id: id.to_string(),
            subagent_id: subagent_id.to_string(),
            source_harness: None,
            source_session_id: None,
            intent: None,
            profile: profile.to_string(),
            prompt: Some(prompt.to_string()),
            prompt_artifact_ref: None,
            output_schema_name: None,
            output_schema_version: 1,
            status: "queued".to_string(),
            result_summary: None,
            result_json: None,
            error: None,
            created_at_ms: busytok_domain::now_ms(),
            started_at_ms: None,
            completed_at_ms: None,
            timeout_seconds: None,
            model_override: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubagentHarnessBindingRow {
    pub id: String,
    pub subagent_id: String,
    pub harness: String,
    pub adapter_session_id: Option<String>,
    pub adapter_process_id: Option<String>,
    pub is_hot: i32,
    pub status: String,
    pub created_at_ms: i64,
    pub last_used_at_ms: Option<i64>,
    pub closed_at_ms: Option<i64>,
    pub detail_json: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SubagentUsageRecordRow {
    pub id: String,
    pub task_id: String,
    pub subagent_id: String,
    pub source_usage_event_id: Option<String>,
    pub harness: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub total_cost_usd: Option<f64>,
    pub duration_ms: Option<i64>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct SubagentResourceEventRow {
    pub id: String,
    pub event_type: String,
    pub target_id: Option<String>,
    pub rss_mb: Option<f64>,
    pub cpu_percent: Option<f64>,
    pub detail_json: Option<String>,
    pub created_at_ms: i64,
}
