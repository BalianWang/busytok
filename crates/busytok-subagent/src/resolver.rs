//! Name / id resolution for logical subagents.

use busytok_domain::derive_project_hash;
use busytok_store::{SubagentLogicalSubagentRow, SubagentMemoryRow};

use crate::error::{Result, SubagentError};
use crate::models::LogicalSubagent;

/// Resolved identity for a delegate request.
#[derive(Debug)]
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
    bound_provider_id: &str,
    bound_model_id: &str,
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
    // Filter out soft-deleted tombstones at the Rust level so delegate never
    // revives a tombstoned subagent. The partial unique index
    // `idx_subagent_unique_active_name` guarantees at most one non-deleted row
    // per (project_id, repo_hash, name), so after filtering `matches` has at
    // most one row — `AmbiguousName` is unreachable in practice but kept as a
    // defensive guard. (Mirrors the filter in `lookup_by_name_impl`.)
    let active: Vec<_> = matches
        .into_iter()
        .filter(|r| r.status != "deleted")
        .collect();
    match active.len() {
        0 => {
            // Creation path (spec §3.3 strict): both bound fields MUST be
            // provided and validated against the provider/model catalog.
            // Empty strings are a programming error — the manager's
            // `delegate()` passes empty strings only when the caller supplied
            // `(None, None)`, which is valid for the reuse path but rejected
            // here for creation. There is no "create without binding" path.
            if bound_provider_id.is_empty() || bound_model_id.is_empty() {
                return Err(SubagentError::Validation(
                    "bound_provider_id and bound_model_id are both required to create a subagent"
                        .into(),
                ));
            }
            validate_bound_provider_model(db, bound_provider_id, bound_model_id)?;
            Ok(Resolved {
                subagent: create_subagent(
                    db,
                    name,
                    &canonical_cwd,
                    &repo_hash,
                    default_profile,
                    bound_provider_id,
                    bound_model_id,
                )?,
                created: true,
            })
        }
        1 => Ok(Resolved {
            subagent: row_to_model(&active[0]),
            created: false,
        }),
        _ => Err(SubagentError::AmbiguousName(name.to_string())),
    }
}

/// Spec §3.3 creation-time validation: the bound provider must exist + be
/// enabled, and the bound model must exist + be enabled under that provider.
/// Called BEFORE inserting the logical subagent row so a failed validation
/// never writes a half-bound row to the DB.
fn validate_bound_provider_model(
    db: &busytok_store::Database,
    provider_id: &str,
    model_id: &str,
) -> Result<()> {
    let provider = db
        .get_provider_with_secret(provider_id)
        .map_err(SubagentError::Store)?
        .ok_or_else(|| SubagentError::Validation(format!("provider not found: {provider_id}")))?;
    if !provider.enabled {
        return Err(SubagentError::Validation(format!(
            "provider disabled: {provider_id}"
        )));
    }
    let model = db
        .get_model_by_provider_and_model_id(provider_id, model_id)
        .map_err(SubagentError::Store)?
        .ok_or_else(|| {
            SubagentError::Validation(format!("model not found in provider: {model_id}"))
        })?;
    if !model.enabled {
        return Err(SubagentError::Validation(format!(
            "model disabled: {model_id}"
        )));
    }
    Ok(())
}

/// Look up by UUID directly. Tombstoned (`status='deleted'`) subagents are
/// rejected as `NotFound` so ordinary read/write paths cannot revive them.
/// Callers that must see tombstones (e.g. hard delete) use
/// [`resolve_by_id_include_deleted`].
pub fn resolve_by_id(db: &busytok_store::Database, id: &str) -> Result<LogicalSubagent> {
    let sub = resolve_by_id_include_deleted(db, id)?;
    if sub.status == crate::models::SubagentStatus::Deleted {
        return Err(SubagentError::NotFound(id.to_string()));
    }
    Ok(sub)
}

/// Look up by UUID, including tombstoned rows. Used by `delete` (which needs
/// to operate on already-deleted rows for hard delete) and internal tooling.
pub fn resolve_by_id_include_deleted(
    db: &busytok_store::Database,
    id: &str,
) -> Result<LogicalSubagent> {
    db.subagent_get_logical(id)
        .map_err(SubagentError::Store)?
        .map(|r| row_to_model(&r))
        .ok_or_else(|| SubagentError::NotFound(id.to_string()))
}

/// Look up (WITHOUT creating) a subagent by name within the repo scope of `cwd`.
/// Used by read-only operations (show/tasks/hibernate/delete); delegate uses the
/// create-or-lookup `resolve_by_name`. Tombstoned rows are rejected.
pub fn lookup_by_name(
    db: &busytok_store::Database,
    name: &str,
    cwd: &str,
) -> Result<LogicalSubagent> {
    lookup_by_name_impl(db, name, cwd, false)
}

/// Look up by name including tombstoned rows. Used by `delete(hard=true)` so a
/// soft-deleted subagent can still be hard-deleted by name + cwd.
pub fn lookup_by_name_include_deleted(
    db: &busytok_store::Database,
    name: &str,
    cwd: &str,
) -> Result<LogicalSubagent> {
    lookup_by_name_impl(db, name, cwd, true)
}

fn lookup_by_name_impl(
    db: &busytok_store::Database,
    name: &str,
    cwd: &str,
    include_deleted: bool,
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
    let candidates: Vec<_> = if include_deleted {
        matches.iter().collect()
    } else {
        matches.iter().filter(|r| r.status != "deleted").collect()
    };
    match candidates.len() {
        0 => Err(SubagentError::NotFound(name.to_string())),
        1 => Ok(row_to_model(candidates[0])),
        _ => Err(SubagentError::AmbiguousName(name.to_string())),
    }
}

fn create_subagent(
    db: &busytok_store::Database,
    name: &str,
    cwd: &str,
    repo_hash: &str,
    default_profile: &str,
    bound_provider_id: &str,
    bound_model_id: &str,
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
        bound_provider_id: bound_provider_id.to_string(),
        bound_model_id: bound_model_id.to_string(),
        status: "cold".to_string(),
        created_at_ms: now,
        updated_at_ms: now,
        last_active_at_ms: None,
    };
    db.subagent_upsert_logical(&row)
        .map_err(SubagentError::Store)?;
    // seed an empty memory row so hibernate/restore always finds one
    db.subagent_upsert_memory(&SubagentMemoryRow::new_empty(&id))
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
        bound_provider_id: r.bound_provider_id.clone(),
        bound_model_id: r.bound_model_id.clone(),
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
