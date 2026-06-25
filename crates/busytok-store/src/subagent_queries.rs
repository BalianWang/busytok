//! SQL query functions for the logical-subagent runtime tables.
//!
//! Each function takes a `&rusqlite::Connection` so it can run inside the
//! caller's transaction. `Database` thin wrappers live in `db.rs`.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::repository::{
    SubagentHarnessBindingRow, SubagentLogicalSubagentRow, SubagentMemoryRow,
    SubagentResourceEventRow, SubagentTaskRow, SubagentUsageRecordRow,
};

// --- logical_subagents -----------------------------------------------------

pub fn upsert_logical_subagent(conn: &Connection, row: &SubagentLogicalSubagentRow) -> Result<()> {
    conn.execute(
        "INSERT INTO subagent_logical_subagents \
             (id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, \
              default_model, status, created_at_ms, updated_at_ms, last_active_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
         ON CONFLICT(id) DO UPDATE SET \
             name=excluded.name, project_id=excluded.project_id, repo_path=excluded.repo_path, \
             repo_hash=excluded.repo_hash, branch=excluded.branch, intent=excluded.intent, \
             default_profile=excluded.default_profile, default_model=excluded.default_model, \
             status=excluded.status, updated_at_ms=excluded.updated_at_ms, \
             last_active_at_ms=excluded.last_active_at_ms",
        params![
            row.id,
            row.name,
            row.project_id,
            row.repo_path,
            row.repo_hash,
            row.branch,
            row.intent,
            row.default_profile,
            row.default_model,
            row.status,
            row.created_at_ms,
            row.updated_at_ms,
            row.last_active_at_ms,
        ],
    )
    .map(|_| ())
    .with_context(|| format!("upsert logical subagent {}", row.id))
}

pub fn get_logical_subagent(
    conn: &Connection,
    id: &str,
) -> Result<Option<SubagentLogicalSubagentRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, \
                default_model, status, created_at_ms, updated_at_ms, last_active_at_ms \
         FROM subagent_logical_subagents WHERE id = ?1",
    )?;
    let row_opt = stmt
        .query_row(params![id], |row| {
            Ok(SubagentLogicalSubagentRow {
                id: row.get(0)?,
                name: row.get(1)?,
                project_id: row.get(2)?,
                repo_path: row.get(3)?,
                repo_hash: row.get(4)?,
                branch: row.get(5)?,
                intent: row.get(6)?,
                default_profile: row.get(7)?,
                default_model: row.get(8)?,
                status: row.get(9)?,
                created_at_ms: row.get(10)?,
                updated_at_ms: row.get(11)?,
                last_active_at_ms: row.get(12)?,
            })
        })
        .ok();
    Ok(row_opt)
}

pub fn list_active_by_repo(
    conn: &Connection,
    repo_hash: &str,
) -> Result<Vec<SubagentLogicalSubagentRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, \
                default_model, status, created_at_ms, updated_at_ms, last_active_at_ms \
         FROM subagent_logical_subagents \
         WHERE repo_hash = ?1 AND status != 'deleted' \
         ORDER BY last_active_at_ms DESC NULLS LAST",
    )?;
    let rows = stmt
        .query_map(params![repo_hash], |row| {
            Ok(SubagentLogicalSubagentRow {
                id: row.get(0)?,
                name: row.get(1)?,
                project_id: row.get(2)?,
                repo_path: row.get(3)?,
                repo_hash: row.get(4)?,
                branch: row.get(5)?,
                intent: row.get(6)?,
                default_profile: row.get(7)?,
                default_model: row.get(8)?,
                status: row.get(9)?,
                created_at_ms: row.get(10)?,
                updated_at_ms: row.get(11)?,
                last_active_at_ms: row.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn find_by_name_in_repo(
    conn: &Connection,
    project_id: &str,
    repo_hash: &str,
    name: &str,
) -> Result<Vec<SubagentLogicalSubagentRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, \
                default_model, status, created_at_ms, updated_at_ms, last_active_at_ms \
         FROM subagent_logical_subagents \
         WHERE project_id = ?1 AND repo_hash = ?2 AND name = ?3 AND status != 'deleted'",
    )?;
    let rows = stmt
        .query_map(params![project_id, repo_hash, name], |row| {
            Ok(SubagentLogicalSubagentRow {
                id: row.get(0)?,
                name: row.get(1)?,
                project_id: row.get(2)?,
                repo_path: row.get(3)?,
                repo_hash: row.get(4)?,
                branch: row.get(5)?,
                intent: row.get(6)?,
                default_profile: row.get(7)?,
                default_model: row.get(8)?,
                status: row.get(9)?,
                created_at_ms: row.get(10)?,
                updated_at_ms: row.get(11)?,
                last_active_at_ms: row.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// List subagents, optionally filtered by status and/or project.
/// `include_deleted = false` excludes soft-deleted rows.
pub fn list_filtered(
    conn: &Connection,
    status: Option<&str>,
    project: Option<&str>,
    include_deleted: bool,
) -> Result<Vec<SubagentLogicalSubagentRow>> {
    let mut sql = String::from(
        "SELECT id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, \
                default_model, status, created_at_ms, updated_at_ms, last_active_at_ms \
         FROM subagent_logical_subagents WHERE 1=1",
    );
    if !include_deleted {
        sql.push_str(" AND status != 'deleted'");
    }
    if status.is_some() {
        sql.push_str(" AND status = :status");
    }
    if project.is_some() {
        sql.push_str(" AND project_id = :project");
    }
    sql.push_str(" ORDER BY last_active_at_ms DESC NULLS LAST");

    let mut stmt = conn.prepare(&sql)?;
    let mut params_vec: Vec<(&str, &dyn rusqlite::ToSql)> = Vec::new();
    let status_val: String;
    if let Some(s) = status {
        status_val = s.to_string();
        params_vec.push((":status", &status_val));
    }
    let project_val: String;
    if let Some(p) = project {
        project_val = p.to_string();
        params_vec.push((":project", &project_val));
    }
    let rows = stmt
        .query_map(params_vec.as_slice(), |row| {
            Ok(SubagentLogicalSubagentRow {
                id: row.get(0)?,
                name: row.get(1)?,
                project_id: row.get(2)?,
                repo_path: row.get(3)?,
                repo_hash: row.get(4)?,
                branch: row.get(5)?,
                intent: row.get(6)?,
                default_profile: row.get(7)?,
                default_model: row.get(8)?,
                status: row.get(9)?,
                created_at_ms: row.get(10)?,
                updated_at_ms: row.get(11)?,
                last_active_at_ms: row.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Hard-delete a logical subagent and all its dependents, in FK-safe order,
/// wrapped in a single transaction so a mid-cascade failure leaves no orphans.
///
/// Per spec §3.5 there is **no `ON DELETE CASCADE`** on the subagent tables —
/// audit data must never be silently removed. Hard delete is explicit, at the
/// application (store) layer: delete children in dependency order, then the row.
pub fn hard_delete_logical_subagent(conn: &Connection, id: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    // usage_records reference both tasks and the logical row → delete first.
    tx.execute(
        "DELETE FROM subagent_usage_records WHERE subagent_id = ?1",
        params![id],
    )
    .with_context(|| format!("delete usage records for subagent {id}"))?;
    tx.execute(
        "DELETE FROM subagent_tasks WHERE subagent_id = ?1",
        params![id],
    )
    .with_context(|| format!("delete tasks for subagent {id}"))?;
    tx.execute(
        "DELETE FROM subagent_harness_bindings WHERE subagent_id = ?1",
        params![id],
    )
    .with_context(|| format!("delete bindings for subagent {id}"))?;
    tx.execute(
        "DELETE FROM subagent_memory WHERE subagent_id = ?1",
        params![id],
    )
    .with_context(|| format!("delete memory for subagent {id}"))?;
    // resource_events.target_id is a free-text column (no FK); subagent-scoped
    // events carry the subagent id there. Per spec §3.5 hard delete removes events.
    tx.execute(
        "DELETE FROM subagent_resource_events WHERE target_id = ?1",
        params![id],
    )
    .with_context(|| format!("delete resource events for subagent {id}"))?;
    tx.execute(
        "DELETE FROM subagent_logical_subagents WHERE id = ?1",
        params![id],
    )
    .with_context(|| format!("hard-delete logical subagent {id}"))?;
    tx.commit()
        .with_context(|| format!("commit hard-delete for subagent {id}"))
}

// --- memory ----------------------------------------------------------------

pub fn upsert_memory(conn: &Connection, row: &SubagentMemoryRow) -> Result<()> {
    conn.execute(
        "INSERT INTO subagent_memory \
             (id, subagent_id, hot_summary, long_summary, key_files_json, decisions_json, \
              attempts_json, open_questions_json, artifact_refs_json, last_compacted_at_ms, \
              last_compacted_task_id, updated_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
         ON CONFLICT(subagent_id) DO UPDATE SET \
             hot_summary=excluded.hot_summary, long_summary=excluded.long_summary, \
             key_files_json=excluded.key_files_json, decisions_json=excluded.decisions_json, \
             attempts_json=excluded.attempts_json, open_questions_json=excluded.open_questions_json, \
             artifact_refs_json=excluded.artifact_refs_json, \
             last_compacted_at_ms=excluded.last_compacted_at_ms, \
             last_compacted_task_id=excluded.last_compacted_task_id, \
             updated_at_ms=excluded.updated_at_ms",
        params![
            row.id, row.subagent_id, row.hot_summary, row.long_summary, row.key_files_json,
            row.decisions_json, row.attempts_json, row.open_questions_json,
            row.artifact_refs_json, row.last_compacted_at_ms, row.last_compacted_task_id,
            row.updated_at_ms,
        ],
    )
    .map(|_| ())
    .with_context(|| format!("upsert memory for subagent {}", row.subagent_id))
}

pub fn get_memory(conn: &Connection, subagent_id: &str) -> Result<Option<SubagentMemoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, hot_summary, long_summary, key_files_json, decisions_json, \
                attempts_json, open_questions_json, artifact_refs_json, last_compacted_at_ms, \
                last_compacted_task_id, updated_at_ms \
         FROM subagent_memory WHERE subagent_id = ?1",
    )?;
    let row_opt = stmt
        .query_row(params![subagent_id], |row| {
            Ok(SubagentMemoryRow {
                id: row.get(0)?,
                subagent_id: row.get(1)?,
                hot_summary: row.get(2)?,
                long_summary: row.get(3)?,
                key_files_json: row.get(4)?,
                decisions_json: row.get(5)?,
                attempts_json: row.get(6)?,
                open_questions_json: row.get(7)?,
                artifact_refs_json: row.get(8)?,
                last_compacted_at_ms: row.get(9)?,
                last_compacted_task_id: row.get(10)?,
                updated_at_ms: row.get(11)?,
            })
        })
        .ok();
    Ok(row_opt)
}

// --- tasks -----------------------------------------------------------------

pub fn insert_task(conn: &Connection, row: &SubagentTaskRow) -> Result<()> {
    conn.execute(
        "INSERT INTO subagent_tasks \
             (id, subagent_id, source_harness, source_session_id, intent, profile, prompt, \
              prompt_artifact_ref, output_schema_name, output_schema_version, status, \
              result_summary, result_json, error, created_at_ms, started_at_ms, completed_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            row.id,
            row.subagent_id,
            row.source_harness,
            row.source_session_id,
            row.intent,
            row.profile,
            row.prompt,
            row.prompt_artifact_ref,
            row.output_schema_name,
            row.output_schema_version,
            row.status,
            row.result_summary,
            row.result_json,
            row.error,
            row.created_at_ms,
            row.started_at_ms,
            row.completed_at_ms,
        ],
    )
    .map(|_| ())
    .with_context(|| format!("insert task {}", row.id))
}

pub fn get_task(conn: &Connection, id: &str) -> Result<Option<SubagentTaskRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, source_harness, source_session_id, intent, profile, prompt, \
                prompt_artifact_ref, output_schema_name, output_schema_version, status, \
                result_summary, result_json, error, created_at_ms, started_at_ms, completed_at_ms \
         FROM subagent_tasks WHERE id = ?1",
    )?;
    let row_opt = stmt
        .query_row(params![id], |row| {
            Ok(SubagentTaskRow {
                id: row.get(0)?,
                subagent_id: row.get(1)?,
                source_harness: row.get(2)?,
                source_session_id: row.get(3)?,
                intent: row.get(4)?,
                profile: row.get(5)?,
                prompt: row.get(6)?,
                prompt_artifact_ref: row.get(7)?,
                output_schema_name: row.get(8)?,
                output_schema_version: row.get(9)?,
                status: row.get(10)?,
                result_summary: row.get(11)?,
                result_json: row.get(12)?,
                error: row.get(13)?,
                created_at_ms: row.get(14)?,
                started_at_ms: row.get(15)?,
                completed_at_ms: row.get(16)?,
            })
        })
        .ok();
    Ok(row_opt)
}

pub fn list_tasks(
    conn: &Connection,
    subagent_id: &str,
    limit: i64,
) -> Result<Vec<SubagentTaskRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, source_harness, source_session_id, intent, profile, prompt, \
                prompt_artifact_ref, output_schema_name, output_schema_version, status, \
                result_summary, result_json, error, created_at_ms, started_at_ms, completed_at_ms \
         FROM subagent_tasks WHERE subagent_id = ?1 ORDER BY created_at_ms DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![subagent_id, limit], |row| {
            Ok(SubagentTaskRow {
                id: row.get(0)?,
                subagent_id: row.get(1)?,
                source_harness: row.get(2)?,
                source_session_id: row.get(3)?,
                intent: row.get(4)?,
                profile: row.get(5)?,
                prompt: row.get(6)?,
                prompt_artifact_ref: row.get(7)?,
                output_schema_name: row.get(8)?,
                output_schema_version: row.get(9)?,
                status: row.get(10)?,
                result_summary: row.get(11)?,
                result_json: row.get(12)?,
                error: row.get(13)?,
                created_at_ms: row.get(14)?,
                started_at_ms: row.get(15)?,
                completed_at_ms: row.get(16)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn set_task_status(
    conn: &Connection,
    id: &str,
    status: &str,
    result_summary: Option<String>,
    error: Option<String>,
) -> Result<()> {
    let now = busytok_domain::now_ms();
    let completed_at: Option<i64> =
        (status == "completed" || status == "failed" || status == "cancelled").then_some(now);
    conn.execute(
        "UPDATE subagent_tasks SET status = ?2, result_summary = COALESCE(?3, result_summary), \
            error = COALESCE(?4, error), completed_at_ms = COALESCE(?5, completed_at_ms) \
         WHERE id = ?1",
        params![id, status, result_summary, error, completed_at],
    )
    .map(|_| ())
    .with_context(|| format!("set task {} status {}", id, status))
}

// --- harness bindings ------------------------------------------------------

pub fn upsert_binding(conn: &Connection, row: &SubagentHarnessBindingRow) -> Result<()> {
    conn.execute(
        "INSERT INTO subagent_harness_bindings \
             (id, subagent_id, harness, adapter_session_id, adapter_process_id, is_hot, status, \
              created_at_ms, last_used_at_ms, closed_at_ms, detail_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
         ON CONFLICT(id) DO UPDATE SET \
             adapter_session_id=excluded.adapter_session_id, \
             adapter_process_id=excluded.adapter_process_id, is_hot=excluded.is_hot, \
             status=excluded.status, last_used_at_ms=excluded.last_used_at_ms, \
             closed_at_ms=excluded.closed_at_ms, detail_json=excluded.detail_json",
        params![
            row.id,
            row.subagent_id,
            row.harness,
            row.adapter_session_id,
            row.adapter_process_id,
            row.is_hot,
            row.status,
            row.created_at_ms,
            row.last_used_at_ms,
            row.closed_at_ms,
            row.detail_json,
        ],
    )
    .map(|_| ())
    .with_context(|| format!("upsert binding {}", row.id))
}

pub fn hot_binding(
    conn: &Connection,
    subagent_id: &str,
    harness: &str,
) -> Result<Option<SubagentHarnessBindingRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, harness, adapter_session_id, adapter_process_id, is_hot, status, \
                created_at_ms, last_used_at_ms, closed_at_ms, detail_json \
         FROM subagent_harness_bindings WHERE subagent_id = ?1 AND harness = ?2 AND is_hot = 1",
    )?;
    let row_opt = stmt
        .query_row(params![subagent_id, harness], |row| {
            Ok(SubagentHarnessBindingRow {
                id: row.get(0)?,
                subagent_id: row.get(1)?,
                harness: row.get(2)?,
                adapter_session_id: row.get(3)?,
                adapter_process_id: row.get(4)?,
                is_hot: row.get(5)?,
                status: row.get(6)?,
                created_at_ms: row.get(7)?,
                last_used_at_ms: row.get(8)?,
                closed_at_ms: row.get(9)?,
                detail_json: row.get(10)?,
            })
        })
        .ok();
    Ok(row_opt)
}

// --- usage + resource events ----------------------------------------------

pub fn insert_usage_record(conn: &Connection, row: &SubagentUsageRecordRow) -> Result<()> {
    conn.execute(
        "INSERT INTO subagent_usage_records \
             (id, task_id, subagent_id, source_usage_event_id, harness, provider, model, \
              input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, \
              total_cost_usd, duration_ms, created_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            row.id,
            row.task_id,
            row.subagent_id,
            row.source_usage_event_id,
            row.harness,
            row.provider,
            row.model,
            row.input_tokens,
            row.output_tokens,
            row.cache_read_tokens,
            row.cache_write_tokens,
            row.total_cost_usd,
            row.duration_ms,
            row.created_at_ms,
        ],
    )
    .map(|_| ())
    .with_context(|| format!("insert usage record {}", row.id))
}

pub fn insert_resource_event(conn: &Connection, row: &SubagentResourceEventRow) -> Result<()> {
    conn.execute(
        "INSERT INTO subagent_resource_events \
             (id, event_type, target_id, rss_mb, cpu_percent, detail_json, created_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.id,
            row.event_type,
            row.target_id,
            row.rss_mb,
            row.cpu_percent,
            row.detail_json,
            row.created_at_ms,
        ],
    )
    .map(|_| ())
    .with_context(|| format!("insert resource event {}", row.event_type))
}
