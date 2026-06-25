//! Name / id resolution for logical subagents.

use busytok_domain::derive_project_hash;
use busytok_store::{SubagentLogicalSubagentRow, SubagentMemoryRow};

use crate::error::{Result, SubagentError};
use crate::models::LogicalSubagent;

/// Resolved identity for a delegate request.
pub struct Resolved {
    pub subagent: LogicalSubagent,
    pub created: bool,
}

/// Look up or create a subagent by name within the repo scope of `cwd`.
///
/// MVP: `project_id == repo_hash`.
pub fn resolve_by_name(
    db: &busytok_store::Database,
    name: &str,
    cwd: &str,
    default_profile: &str,
    default_model: Option<&str>,
) -> Result<Resolved> {
    if !LogicalSubagent::is_valid_name(name) {
        return Err(SubagentError::InvalidName(name.to_string()));
    }
    // Canonicalize cwd at this single chokepoint so callers (CLI, e2e) agree
    // on repo_hash regardless of whether they pre-canonicalized.
    let canonical_cwd = std::fs::canonicalize(cwd)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| cwd.to_string());
    let repo_hash = derive_project_hash(&canonical_cwd);
    let matches = db
        .subagent_find_by_name_in_repo(&repo_hash, &repo_hash, name)
        .map_err(SubagentError::Store)?;
    match matches.len() {
        0 => Ok(Resolved {
            subagent: create_subagent(
                db,
                name,
                &canonical_cwd,
                &repo_hash,
                default_profile,
                default_model,
            )?,
            created: true,
        }),
        1 => Ok(Resolved {
            subagent: row_to_model(&matches[0]),
            created: false,
        }),
        _ => Err(SubagentError::AmbiguousName(name.to_string())),
    }
}

/// Look up by UUID directly.
pub fn resolve_by_id(db: &busytok_store::Database, id: &str) -> Result<LogicalSubagent> {
    db.subagent_get_logical(id)
        .map_err(SubagentError::Store)?
        .map(|r| row_to_model(&r))
        .ok_or_else(|| SubagentError::NotFound(id.to_string()))
}

/// Look up (WITHOUT creating) a subagent by name within the repo scope of `cwd`.
/// Used by read-only operations (show/tasks/hibernate/delete); delegate uses the
/// create-or-lookup `resolve_by_name`.
pub fn lookup_by_name(
    db: &busytok_store::Database,
    name: &str,
    cwd: &str,
) -> Result<LogicalSubagent> {
    if !LogicalSubagent::is_valid_name(name) {
        return Err(SubagentError::InvalidName(name.to_string()));
    }
    let canonical_cwd = std::fs::canonicalize(cwd)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| cwd.to_string());
    let repo_hash = derive_project_hash(&canonical_cwd);
    let matches = db
        .subagent_find_by_name_in_repo(&repo_hash, &repo_hash, name)
        .map_err(SubagentError::Store)?;
    match matches.len() {
        0 => Err(SubagentError::NotFound(name.to_string())),
        1 => Ok(row_to_model(&matches[0])),
        _ => Err(SubagentError::AmbiguousName(name.to_string())),
    }
}

fn create_subagent(
    db: &busytok_store::Database,
    name: &str,
    cwd: &str,
    repo_hash: &str,
    default_profile: &str,
    default_model: Option<&str>,
) -> Result<LogicalSubagent> {
    let now = busytok_domain::now_ms();
    // Plain UUID v4 (spec §3.2). The adapter_session_id/task_id prefixes are
    // only for those entities; logical subagent ids are bare UUIDs.
    let id = uuid::Uuid::new_v4().to_string();
    let row = SubagentLogicalSubagentRow {
        id: id.clone(),
        name: name.to_string(),
        project_id: repo_hash.to_string(),
        repo_path: cwd.to_string(),
        repo_hash: repo_hash.to_string(),
        branch: None,
        intent: None,
        default_profile: default_profile.to_string(),
        default_model: default_model.map(|s| s.to_string()),
        status: "cold".to_string(),
        created_at_ms: now,
        updated_at_ms: now,
        last_active_at_ms: None,
    };
    db.subagent_upsert_logical(&row)
        .map_err(SubagentError::Store)?;
    // seed an empty memory row so hibernate/restore always finds one
    db.subagent_upsert_memory(&SubagentMemoryRow::for_test(&id))
        .map_err(SubagentError::Store)?;
    Ok(row_to_model(&row))
}

pub fn row_to_model(r: &SubagentLogicalSubagentRow) -> LogicalSubagent {
    LogicalSubagent {
        id: r.id.clone(),
        name: r.name.clone(),
        project_id: r.project_id.clone(),
        repo_path: r.repo_path.clone(),
        repo_hash: r.repo_hash.clone(),
        branch: r.branch.clone(),
        intent: r.intent.clone(),
        default_profile: r.default_profile.clone(),
        default_model: r.default_model.clone(),
        status: r.status.parse().unwrap_or_else(|s| {
            tracing::warn!(
                event_code = "subagent.session.parse_status_failed",
                raw_status = %s,
                "failed to parse subagent status, falling back to Cold"
            );
            crate::models::SubagentStatus::Cold
        }),
        created_at_ms: r.created_at_ms,
        updated_at_ms: r.updated_at_ms,
        last_active_at_ms: r.last_active_at_ms,
    }
}
