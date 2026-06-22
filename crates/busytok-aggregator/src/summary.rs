//! Realtime summary builder: computes dashboard summary data from usage events.
//!
//! Produces a key-value map with:
//! - `today_total_tokens`: total token count for today (in configured timezone)
//! - `today_total_cost_usd`: total cost for today (in configured timezone)
//! - `top_projects`: top N projects by token usage (with cost)
//! - `top_models`: top N models by token usage (with cost)
//! - `active_agents`: list of active agent names
//! - `active_sessions`: count of recently active sessions
//! - `last_events`: last N events by timestamp (with cost)
//! - `agent_status`: per-agent status info

use std::collections::HashMap;

use anyhow::Result;
use busytok_domain::{NormalizedUsageEvent, ReportingTimezone};

use crate::daily::date_from_timestamp_ms;

/// Build a realtime summary map from a batch of usage events.
pub fn build_realtime_summary(
    events: &[NormalizedUsageEvent],
    rtz: &ReportingTimezone,
) -> Result<HashMap<String, serde_json::Value>> {
    let mut summary = HashMap::new();

    // today_total_tokens: sum total_tokens for events that fall on today's date.
    let now_ms = busytok_domain::now_ms();
    let today_date = date_from_timestamp_ms(now_ms, rtz).unwrap_or_default();

    // PRE-COMPUTE: Map each event to its local date string to avoid
    // repeated timezone conversions in filter loops.
    let event_dates: Vec<String> = events
        .iter()
        .map(|e| date_from_timestamp_ms(e.timestamp_ms, rtz).unwrap_or_default())
        .collect();

    let today_total_tokens: i64 = events
        .iter()
        .zip(event_dates.iter())
        .filter(|(_, date)| *date == &today_date)
        .map(|(e, _)| e.total_tokens)
        .sum();

    summary.insert(
        "today_total_tokens".to_string(),
        serde_json::Value::Number(today_total_tokens.into()),
    );

    // today_total_cost_usd: sum cost for events that fall on today's date.
    let today_total_cost_usd: f64 = events
        .iter()
        .zip(event_dates.iter())
        .filter(|(_, date)| *date == &today_date)
        .filter_map(|(e, _)| e.cost_usd.or(e.estimated_cost_usd))
        .sum();
    summary.insert(
        "today_total_cost_usd".to_string(),
        if today_total_cost_usd > 0.0 {
            serde_json::json!(today_total_cost_usd)
        } else {
            serde_json::Value::Null
        },
    );

    // active_agents: list of unique agent names.
    let active_agents: Vec<String> = events
        .iter()
        .map(|e| e.agent.as_str().to_string())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    summary.insert(
        "active_agents".to_string(),
        serde_json::Value::Array(
            active_agents
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );

    // top_projects: top 10 projects by total tokens (with cost).
    let mut project_data: HashMap<String, (i64, Option<f64>)> = HashMap::new();
    for event in events {
        if let Some(ref hash) = event.project_hash {
            if !hash.is_empty() {
                let entry = project_data.entry(hash.clone()).or_default();
                entry.0 += event.total_tokens;
                entry.1 = merge_opt_cost(entry.1, event.cost_usd.or(event.estimated_cost_usd));
            }
        }
    }
    let mut top_projects: Vec<(String, i64, Option<f64>)> = project_data
        .into_iter()
        .map(|(hash, (tokens, cost))| (hash, tokens, cost))
        .collect();
    top_projects.sort_by(|a, b| b.1.cmp(&a.1));
    top_projects.truncate(10);
    let top_projects_json: Vec<serde_json::Value> = top_projects
        .into_iter()
        .map(|(hash, tokens, cost)| {
            serde_json::json!({
                "project_hash": hash,
                "total_tokens": tokens,
                "total_cost_usd": cost,
            })
        })
        .collect();
    summary.insert(
        "top_projects".to_string(),
        serde_json::Value::Array(top_projects_json),
    );

    // top_models: top 10 models by total tokens (with cost).
    let mut model_data: HashMap<String, (i64, Option<f64>)> = HashMap::new();
    for event in events {
        if let Some(ref model) = event.model {
            if !model.is_empty() {
                let entry = model_data.entry(model.clone()).or_default();
                entry.0 += event.total_tokens;
                entry.1 = merge_opt_cost(entry.1, event.cost_usd.or(event.estimated_cost_usd));
            }
        }
    }
    let mut top_models: Vec<(String, i64, Option<f64>)> = model_data
        .into_iter()
        .map(|(model, (tokens, cost))| (model, tokens, cost))
        .collect();
    top_models.sort_by(|a, b| b.1.cmp(&a.1));
    top_models.truncate(10);
    let top_models_json: Vec<serde_json::Value> = top_models
        .into_iter()
        .map(|(model, tokens, cost)| {
            serde_json::json!({
                "model": model,
                "total_tokens": tokens,
                "total_cost_usd": cost,
            })
        })
        .collect();
    summary.insert(
        "top_models".to_string(),
        serde_json::Value::Array(top_models_json),
    );

    // active_sessions: unique session IDs.
    let active_sessions: std::collections::HashSet<&str> = events
        .iter()
        .map(|e| e.session_id.as_str())
        .filter(|s| !s.is_empty())
        .collect();
    summary.insert(
        "active_sessions".to_string(),
        serde_json::Value::Number((active_sessions.len() as i64).into()),
    );

    // last_events: up to 20 most recent events by timestamp (with cost).
    let mut sorted_events: Vec<&NormalizedUsageEvent> = events.iter().collect();
    sorted_events.sort_by(|a, b| b.timestamp_ms.cmp(&a.timestamp_ms));
    sorted_events.truncate(20);
    let last_events_json: Vec<serde_json::Value> = sorted_events
        .iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id,
                "agent": e.agent.as_str(),
                "timestamp_ms": e.timestamp_ms,
                "total_tokens": e.total_tokens,
                "model": e.model,
                "cost_usd": e.cost_usd.or(e.estimated_cost_usd),
            })
        })
        .collect();
    summary.insert(
        "last_events".to_string(),
        serde_json::Value::Array(last_events_json),
    );

    // agent_status: per-agent aggregation.
    let mut agent_data: HashMap<&str, AgentAggregate> = HashMap::new();
    for event in events {
        let entry = agent_data.entry(event.agent.as_str()).or_default();
        entry.event_count += 1;
        entry.total_tokens += event.total_tokens;
        if event.timestamp_ms > entry.last_seen_ms {
            entry.last_seen_ms = event.timestamp_ms;
        }
    }
    let agent_status_json: Vec<serde_json::Value> = agent_data
        .into_iter()
        .map(|(agent, agg)| {
            serde_json::json!({
                "agent": agent,
                "event_count": agg.event_count,
                "total_tokens": agg.total_tokens,
                "last_seen_ms": agg.last_seen_ms,
            })
        })
        .collect();
    summary.insert(
        "agent_status".to_string(),
        serde_json::Value::Array(agent_status_json),
    );

    Ok(summary)
}

#[derive(Default)]
struct AgentAggregate {
    event_count: i64,
    total_tokens: i64,
    last_seen_ms: i64,
}

/// Merge two optional costs by summation.
fn merge_opt_cost(existing: Option<f64>, incoming: Option<f64>) -> Option<f64> {
    match (existing, incoming) {
        (Some(a), Some(b)) => Some(a + b),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}
