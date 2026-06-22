//! Mutation planner: computes database-ready mutations from a batch of usage events.
//!
//! The planner returns daily/session/project/model/realtime summary mutations
//! without performing checkpoint writes. Store/runtime owns the transaction;
//! aggregator owns deterministic aggregate math.

use std::collections::HashMap;

use anyhow::Result;
use busytok_domain::now_ms;
use busytok_domain::{
    AgentKind, ModelSummary, NormalizedUsageEvent, ProjectSummary, ReportingTimezone,
    SessionSummary,
};
use busytok_store::DailyUsageRow;
use tracing::warn;

use crate::daily::date_from_timestamp_ms;
use crate::summary::build_realtime_summary;
use busytok_store::{ModelSummaryRow, ProjectRow, SessionRow};

/// Configuration for rollup computation, primarily the timezone.
#[derive(Debug, Clone)]
pub struct RollupOptions {
    /// The reporting timezone for all date boundary calculations.
    pub timezone: ReportingTimezone,
}

impl RollupOptions {
    /// Create options for the given timezone identifier.
    ///
    /// Accepts IANA names, fixed offsets, "UTC", and "local".
    /// Falls back to UTC on parse failure.
    pub fn for_timezone(timezone: &str) -> Self {
        let rtz = ReportingTimezone::parse(timezone).unwrap_or_else(|e| {
            tracing::warn!(
                "failed to parse timezone '{}' in RollupOptions::for_timezone: {e}, falling back to UTC",
                timezone
            );
            ReportingTimezone::utc()
        });
        Self { timezone: rtz }
    }

    /// Create options defaulting to UTC.
    pub fn utc() -> Self {
        Self {
            timezone: ReportingTimezone::utc(),
        }
    }

    /// Create options using the system's detected local timezone.
    pub fn local() -> Self {
        Self {
            timezone: ReportingTimezone::parse("local").unwrap_or_else(|e| {
                tracing::warn!(
                    "failed to resolve local timezone in RollupOptions::local: {e}, falling back to UTC"
                );
                ReportingTimezone::utc()
            }),
        }
    }
}

impl Default for RollupOptions {
    fn default() -> Self {
        Self::local()
    }
}

/// Database-ready mutations computed from a batch of usage events.
///
/// Each field is a vector of rows suitable for direct insertion into the
/// corresponding store table. The caller (runtime/store) is responsible for
/// transactional persistence.
#[derive(Debug, Clone)]
pub struct ScanAggregateMutations {
    /// Daily usage aggregate rows keyed by (date, timezone, agent, project_hash, model).
    pub daily_usage: Vec<DailyUsageRow>,
    /// Session rollup summaries.
    pub session_rollups: Vec<SessionSummary>,
    /// Project rollup summaries.
    pub project_rollups: Vec<ProjectSummary>,
    /// Model rollup summaries.
    pub model_rollups: Vec<ModelSummary>,
    /// Realtime summary key-value pairs for the dashboard.
    pub realtime_summary: HashMap<String, serde_json::Value>,
}

/// Compute database-ready mutations from a batch of usage events.
///
/// This is a pure function: given the same events and options, it always
/// produces the same mutations. It does not touch the database.
pub fn build_scan_mutations(
    events: &[NormalizedUsageEvent],
    options: RollupOptions,
    generation_id: &str,
) -> Result<ScanAggregateMutations> {
    let rtz = &options.timezone;

    // ── Daily usage ───────────────────────────────────────────────────
    // Group by (date, agent, project_hash, model) and sum token fields.
    let mut daily_map: HashMap<(String, String, String, String), DailyAccumulator> = HashMap::new();

    for event in events {
        let date = date_from_timestamp_ms(event.timestamp_ms, rtz)?;
        let agent = event.agent.as_str().to_string();
        let project_hash = event.project_hash.clone().unwrap_or_default();
        let model = event.model.clone().unwrap_or_default();
        let key = (date, agent, project_hash, model);

        let acc = daily_map.entry(key).or_default();
        acc.input_tokens += event.input_tokens;
        acc.output_tokens += event.output_tokens;
        acc.total_tokens += event.total_tokens;
        acc.cached_input_tokens += event.cached_input_tokens;
        acc.cache_creation_tokens += event.cache_creation_tokens;
        acc.cache_read_tokens += event.cache_read_tokens;
        acc.reasoning_tokens += event.reasoning_tokens;
        acc.thoughts_tokens += event.thoughts_tokens;
        acc.tool_tokens += event.tool_tokens;
        acc.cost_usd = merge_cost(acc.cost_usd, event.cost_usd);
        acc.estimated_cost_usd = merge_cost(acc.estimated_cost_usd, event.estimated_cost_usd);
        acc.event_count += 1;
    }

    let daily_usage: Vec<DailyUsageRow> = daily_map
        .into_iter()
        .map(|((date, agent, project_hash, model), acc)| DailyUsageRow {
            date,
            timezone: rtz.canonical_name().to_string(),
            agent,
            project_hash,
            model,
            input_tokens: acc.input_tokens,
            output_tokens: acc.output_tokens,
            total_tokens: acc.total_tokens,
            cached_input_tokens: acc.cached_input_tokens,
            cache_creation_tokens: acc.cache_creation_tokens,
            cache_read_tokens: acc.cache_read_tokens,
            reasoning_tokens: acc.reasoning_tokens,
            thoughts_tokens: acc.thoughts_tokens,
            tool_tokens: acc.tool_tokens,
            cost_usd: acc.cost_usd,
            estimated_cost_usd: acc.estimated_cost_usd,
            event_count: acc.event_count,
            generation_id: generation_id.to_string(),
        })
        .collect();

    // ── Session rollups ───────────────────────────────────────────────
    let mut session_map: HashMap<String, SessionAccumulator> = HashMap::new();

    for event in events {
        if event.session_id.is_empty() {
            continue;
        }
        let key = event.session_id.clone();
        let acc = session_map
            .entry(key)
            .or_insert_with(|| SessionAccumulator {
                id: event.session_id.clone(),
                agent: Some(event.agent),
                project_hash: event.project_hash.clone(),
                started_at_ms: event.timestamp_ms,
                last_seen_at_ms: event.timestamp_ms,
                models: Vec::new(),
                total_tokens: 0,
                total_cost_usd: None,
                event_count: 0,
            });

        if event.timestamp_ms < acc.started_at_ms {
            acc.started_at_ms = event.timestamp_ms;
        }
        if event.timestamp_ms > acc.last_seen_at_ms {
            acc.last_seen_at_ms = event.timestamp_ms;
        }
        if let Some(ref model) = event.model {
            if !model.is_empty() && !acc.models.contains(model) {
                acc.models.push(model.clone());
            }
        }
        acc.total_tokens += event.total_tokens;
        acc.total_cost_usd = merge_cost(acc.total_cost_usd, event.cost_usd);
        acc.event_count += 1;
    }

    let session_rollups: Vec<SessionSummary> = session_map
        .into_values()
        .map(|acc| {
            let model_list_json = serde_json::to_string(&acc.models).unwrap_or_else(|e| {
                warn!(
                    "failed to serialize model list for session {}: {e}",
                    &acc.id
                );
                "[]".to_string()
            });
            SessionSummary {
                id: acc.id,
                agent: acc.agent,
                project_hash: acc.project_hash,
                started_at_ms: acc.started_at_ms,
                last_seen_at_ms: acc.last_seen_at_ms,
                model_list_json: Some(model_list_json),
                total_tokens: acc.total_tokens,
                total_cost_usd: acc.total_cost_usd,
                event_count: acc.event_count,
            }
        })
        .collect();

    // ── Project rollups ───────────────────────────────────────────────
    let mut project_map: HashMap<String, ProjectAccumulator> = HashMap::new();

    for event in events {
        let hash = event.project_hash.clone().unwrap_or_default();
        if hash.is_empty() {
            continue;
        }
        let acc = project_map
            .entry(hash.clone())
            .or_insert_with(|| ProjectAccumulator {
                project_hash: Some(hash),
                project_path: event.project_path.clone(),
                agent: Some(event.agent),
                first_seen_at_ms: event.timestamp_ms,
                last_seen_at_ms: event.timestamp_ms,
                total_tokens: 0,
                total_cost_usd: None,
                event_count: 0,
                sessions: Vec::new(),
            });

        // Keep the first non-empty project path
        if acc.project_path.is_none() && event.project_path.is_some() {
            acc.project_path = event.project_path.clone();
        }
        if event.timestamp_ms < acc.first_seen_at_ms {
            acc.first_seen_at_ms = event.timestamp_ms;
        }
        if event.timestamp_ms > acc.last_seen_at_ms {
            acc.last_seen_at_ms = event.timestamp_ms;
        }
        acc.total_tokens += event.total_tokens;
        acc.total_cost_usd = merge_cost(acc.total_cost_usd, event.cost_usd);
        acc.event_count += 1;
        if !event.session_id.is_empty() && !acc.sessions.contains(&event.session_id) {
            acc.sessions.push(event.session_id.clone());
        }
    }

    let project_rollups: Vec<ProjectSummary> = project_map
        .into_values()
        .map(|acc| ProjectSummary {
            project_hash: acc.project_hash,
            project_path: acc.project_path,
            total_tokens: acc.total_tokens,
            total_cost_usd: acc.total_cost_usd,
            event_count: acc.event_count,
            session_count: acc.sessions.len() as i64,
        })
        .collect();

    // ── Model rollups ─────────────────────────────────────────────────
    let mut model_map: HashMap<String, ModelAccumulator> = HashMap::new();

    for event in events {
        let model = event.model.clone().unwrap_or_default();
        if model.is_empty() {
            continue;
        }
        let acc = model_map
            .entry(model.clone())
            .or_insert_with(|| ModelAccumulator {
                model: Some(model),
                total_tokens: 0,
                total_cost_usd: None,
                event_count: 0,
            });

        acc.total_tokens += event.total_tokens;
        acc.total_cost_usd = merge_cost(acc.total_cost_usd, event.cost_usd);
        acc.event_count += 1;
    }

    let model_rollups: Vec<ModelSummary> = model_map
        .into_values()
        .map(|acc| ModelSummary {
            model: acc.model,
            total_tokens: acc.total_tokens,
            total_cost_usd: acc.total_cost_usd,
            event_count: acc.event_count,
        })
        .collect();

    // ── Realtime summary ──────────────────────────────────────────────
    let realtime_summary = build_realtime_summary(events, rtz)?;

    Ok(ScanAggregateMutations {
        daily_usage,
        session_rollups,
        project_rollups,
        model_rollups,
        realtime_summary,
    })
}

/// Accumulator for daily usage aggregation.
#[derive(Default)]
struct DailyAccumulator {
    input_tokens: i64,
    output_tokens: i64,
    total_tokens: i64,
    cached_input_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
    reasoning_tokens: i64,
    thoughts_tokens: i64,
    tool_tokens: i64,
    cost_usd: Option<f64>,
    estimated_cost_usd: Option<f64>,
    event_count: i64,
}

/// Accumulator for session rollup aggregation.
struct SessionAccumulator {
    id: String,
    agent: Option<AgentKind>,
    project_hash: Option<String>,
    started_at_ms: i64,
    last_seen_at_ms: i64,
    models: Vec<String>,
    total_tokens: i64,
    total_cost_usd: Option<f64>,
    event_count: i64,
}

/// Accumulator for project rollup aggregation.
struct ProjectAccumulator {
    project_hash: Option<String>,
    project_path: Option<String>,
    agent: Option<AgentKind>,
    first_seen_at_ms: i64,
    last_seen_at_ms: i64,
    total_tokens: i64,
    total_cost_usd: Option<f64>,
    event_count: i64,
    sessions: Vec<String>,
}

/// Accumulator for model rollup aggregation.
struct ModelAccumulator {
    model: Option<String>,
    total_tokens: i64,
    total_cost_usd: Option<f64>,
    event_count: i64,
}

/// Merge two optional costs by summation. If both are None, returns None.
fn merge_cost(existing: Option<f64>, incoming: Option<f64>) -> Option<f64> {
    match (existing, incoming) {
        (Some(a), Some(b)) => Some(a + b),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

// ── Full-rebuild functions ──────────────────────────────────────────

/// Rebuild session rollup rows from raw usage events.
///
/// Groups events by session_id and produces `SessionRow` structs suitable
/// for the `replace_sessions` store method. After a full rebuild, all
/// sessions are marked inactive (`is_active = 0`); the tailer updates
/// `is_active` in the realtime path.
pub fn rebuild_sessions(events: &[NormalizedUsageEvent], _timezone: &str) -> Vec<SessionRow> {
    let mut session_map: HashMap<String, SessionAccumulator> = HashMap::new();

    for event in events {
        if event.session_id.is_empty() {
            continue;
        }
        let key = event.session_id.clone();
        let acc = session_map
            .entry(key)
            .or_insert_with(|| SessionAccumulator {
                id: event.session_id.clone(),
                agent: Some(event.agent),
                project_hash: event.project_hash.clone(),
                started_at_ms: event.timestamp_ms,
                last_seen_at_ms: event.timestamp_ms,
                models: Vec::new(),
                total_tokens: 0,
                total_cost_usd: None,
                event_count: 0,
            });

        if event.timestamp_ms < acc.started_at_ms {
            acc.started_at_ms = event.timestamp_ms;
        }
        if event.timestamp_ms > acc.last_seen_at_ms {
            acc.last_seen_at_ms = event.timestamp_ms;
        }
        if let Some(ref model) = event.model {
            if !model.is_empty() && !acc.models.contains(model) {
                acc.models.push(model.clone());
            }
        }
        acc.total_tokens += event.total_tokens;
        acc.total_cost_usd = merge_cost(acc.total_cost_usd, event.cost_usd);
        acc.event_count += 1;
    }

    let now = now_ms();
    session_map
        .into_values()
        .map(|acc| {
            let model_list_json = serde_json::to_string(&acc.models).unwrap_or_else(|e| {
                warn!(
                    "failed to serialize model list for session {}: {e}",
                    &acc.id
                );
                "[]".to_string()
            });
            SessionRow {
                id: acc.id,
                agent: acc
                    .agent
                    .map(|a| a.as_str().to_string())
                    .unwrap_or_default(),
                project_hash: acc.project_hash,
                started_at_ms: acc.started_at_ms,
                last_seen_at_ms: acc.last_seen_at_ms,
                model_list_json,
                total_tokens: acc.total_tokens,
                total_cost_usd: acc.total_cost_usd,
                event_count: acc.event_count,
                is_active: 0, // inactive after full rebuild
                created_at_ms: now,
                updated_at_ms: now,
            }
        })
        .collect()
}

/// Rebuild project rollup rows from raw usage events.
///
/// Groups events by project_hash and produces `ProjectRow` structs suitable
/// for the `replace_projects` store method. The `id` field is set to the
/// `project_hash` value for simplicity.
pub fn rebuild_projects(events: &[NormalizedUsageEvent], _timezone: &str) -> Vec<ProjectRow> {
    let mut project_map: HashMap<String, ProjectAccumulator> = HashMap::new();

    for event in events {
        let hash = event.project_hash.clone().unwrap_or_default();
        if hash.is_empty() {
            continue;
        }
        let acc = project_map
            .entry(hash.clone())
            .or_insert_with(|| ProjectAccumulator {
                project_hash: Some(hash.clone()),
                project_path: event.project_path.clone(),
                agent: Some(event.agent),
                first_seen_at_ms: event.timestamp_ms,
                last_seen_at_ms: event.timestamp_ms,
                total_tokens: 0,
                total_cost_usd: None,
                event_count: 0,
                sessions: Vec::new(),
            });

        // Keep the first non-empty project path
        if acc.project_path.is_none() && event.project_path.is_some() {
            acc.project_path = event.project_path.clone();
        }
        if event.timestamp_ms < acc.first_seen_at_ms {
            acc.first_seen_at_ms = event.timestamp_ms;
        }
        if event.timestamp_ms > acc.last_seen_at_ms {
            acc.last_seen_at_ms = event.timestamp_ms;
        }
        acc.total_tokens += event.total_tokens;
        acc.total_cost_usd = merge_cost(acc.total_cost_usd, event.cost_usd);
        acc.event_count += 1;
        if !event.session_id.is_empty() && !acc.sessions.contains(&event.session_id) {
            acc.sessions.push(event.session_id.clone());
        }
    }

    let now = now_ms();
    project_map
        .into_values()
        .map(|acc| {
            let hash = acc.project_hash.unwrap_or_default();
            let path = acc.project_path.clone();
            // Derive display_name from project_path: use the last path component.
            let display_name = path.as_ref().and_then(|p| {
                let trimmed = p.trim_end_matches('/');
                trimmed.rsplit('/').next().map(|s| s.to_string())
            });
            ProjectRow {
                id: hash.clone(),
                project_hash: hash,
                project_path: acc.project_path,
                agent: acc.agent.map(|a| a.as_str().to_string()),
                display_name,
                first_seen_at_ms: acc.first_seen_at_ms,
                last_seen_at_ms: acc.last_seen_at_ms,
                total_tokens: acc.total_tokens,
                total_cost_usd: acc.total_cost_usd,
                session_count: acc.sessions.len() as i64,
                created_at_ms: now,
                updated_at_ms: now,
            }
        })
        .collect()
}

/// Rebuild model summary rows from raw usage events.
///
/// Groups events by model name and produces `ModelSummaryRow` structs suitable
/// for the `replace_model_summaries` store method. This is a simple cross-date
/// summary (no timezone or date dimension).
pub fn rebuild_model_summaries(events: &[NormalizedUsageEvent]) -> Vec<ModelSummaryRow> {
    let mut model_map: HashMap<String, ModelAccumulator> = HashMap::new();

    for event in events {
        let model = event.model.clone().unwrap_or_default();
        if model.is_empty() {
            continue;
        }
        let acc = model_map
            .entry(model.clone())
            .or_insert_with(|| ModelAccumulator {
                model: Some(model),
                total_tokens: 0,
                total_cost_usd: None,
                event_count: 0,
            });

        acc.total_tokens += event.total_tokens;
        acc.total_cost_usd = merge_cost(acc.total_cost_usd, event.cost_usd);
        acc.event_count += 1;
    }

    model_map
        .into_values()
        .map(|acc| ModelSummaryRow {
            model: acc.model.unwrap_or_default(),
            total_tokens: acc.total_tokens,
            total_cost_usd: acc.total_cost_usd,
            event_count: acc.event_count,
        })
        .collect()
}

// ── Conversion functions ──────────────────────────────────────────

/// Convert session rollup summaries to session rows for store batch writes.
pub fn session_rollups_to_rows(rollups: &[SessionSummary]) -> Vec<SessionRow> {
    let now = now_ms();
    rollups
        .iter()
        .map(|s| SessionRow {
            id: s.id.clone(),
            agent: s.agent.map(|a| a.as_str().to_string()).unwrap_or_default(),
            project_hash: s.project_hash.clone(),
            started_at_ms: s.started_at_ms,
            last_seen_at_ms: s.last_seen_at_ms,
            model_list_json: s
                .model_list_json
                .clone()
                .unwrap_or_else(|| "[]".to_string()),
            total_tokens: s.total_tokens,
            total_cost_usd: s.total_cost_usd,
            event_count: s.event_count,
            is_active: 0,
            created_at_ms: now,
            updated_at_ms: now,
        })
        .collect()
}

/// Convert project rollup summaries to project rows for store batch writes.
pub fn project_rollups_to_rows(rollups: &[ProjectSummary]) -> Vec<ProjectRow> {
    let now = now_ms();
    rollups
        .iter()
        .map(|p| {
            let hash = p.project_hash.clone().unwrap_or_default();
            let path = p.project_path.clone();
            let display_name = path.as_ref().and_then(|p_str| {
                let trimmed = p_str.trim_end_matches('/');
                trimmed.rsplit('/').next().map(|s| s.to_string())
            });
            ProjectRow {
                id: hash.clone(),
                project_hash: hash,
                project_path: p.project_path.clone(),
                agent: None,
                display_name,
                first_seen_at_ms: 0,
                last_seen_at_ms: 0,
                total_tokens: p.total_tokens,
                total_cost_usd: p.total_cost_usd,
                session_count: p.session_count,
                created_at_ms: now,
                updated_at_ms: now,
            }
        })
        .collect()
}

/// Convert model rollup summaries to model summary rows for store batch writes.
pub fn model_rollups_to_rows(rollups: &[ModelSummary]) -> Vec<ModelSummaryRow> {
    rollups
        .iter()
        .map(|m| ModelSummaryRow {
            model: m.model.clone().unwrap_or_default(),
            total_tokens: m.total_tokens,
            total_cost_usd: m.total_cost_usd,
            event_count: m.event_count,
        })
        .collect()
}
