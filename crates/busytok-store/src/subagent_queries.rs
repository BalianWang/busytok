//! SQL query functions for the logical-subagent runtime tables.
//!
//! Each function takes a `&rusqlite::Connection` so it can run inside the
//! caller's transaction. `Database` thin wrappers live in `db.rs`.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

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
    // NOTE: deliberately NOT filtering `status != 'deleted'` at the SQL level.
    // Callers that need to exclude tombstones (e.g. `resolve_by_name`,
    // `lookup_by_name`) apply a Rust-level filter on the returned rows so the
    // `include_deleted` flag in `lookup_by_name_impl` actually takes effect.
    // Filtering here would make `lookup_by_name_include_deleted` unable to
    // reach soft-deleted rows, breaking hard-delete-by-name.
    let mut stmt = conn.prepare(
        "SELECT id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, \
                default_model, status, created_at_ms, updated_at_ms, last_active_at_ms \
         FROM subagent_logical_subagents \
         WHERE project_id = ?1 AND repo_hash = ?2 AND name = ?3",
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
              result_summary, result_json, error, created_at_ms, started_at_ms, completed_at_ms, \
              timeout_seconds, model_override) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
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
            row.timeout_seconds,
            row.model_override,
        ],
    )
    .map(|_| ())
    .with_context(|| format!("insert task {}", row.id))
}

pub fn get_task(conn: &Connection, id: &str) -> Result<Option<SubagentTaskRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, source_harness, source_session_id, intent, profile, prompt, \
                prompt_artifact_ref, output_schema_name, output_schema_version, status, \
                result_summary, result_json, error, created_at_ms, started_at_ms, completed_at_ms, \
                timeout_seconds, model_override \
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
                timeout_seconds: row.get(17)?,
                model_override: row.get(18)?,
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
                result_summary, result_json, error, created_at_ms, started_at_ms, completed_at_ms, \
                timeout_seconds, model_override \
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
                timeout_seconds: row.get(17)?,
                model_override: row.get(18)?,
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

/// Count tasks for a subagent with `created_at_ms > since_ms`.
/// Used by `MemoryUpdater` for compaction trigger (a) — the authoritative
/// count of tasks since last compaction, NOT capped by `recent_tasks_limit`.
pub fn count_tasks_since(conn: &Connection, subagent_id: &str, since_ms: i64) -> Result<u32> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM subagent_tasks \
             WHERE subagent_id = ?1 AND created_at_ms > ?2",
            params![subagent_id, since_ms],
            |row| row.get(0),
        )
        .with_context(|| format!("count tasks since {} for {}", since_ms, subagent_id))?;
    Ok(count as u32)
}

/// Whether the given subagent has a task currently in `'running'` status.
/// Used by `delegate()` to enforce per-subagent serialization (spec §6.4
/// line 737): if a running task exists, the new task is inserted as
/// `'queued'` instead of `'running'`.
pub fn has_running_task(conn: &Connection, subagent_id: &str) -> rusqlite::Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM subagent_tasks WHERE subagent_id = ? AND status = 'running'",
        rusqlite::params![subagent_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Count subagent tasks by status. Returns (queued, running).
pub fn task_counts_by_status(conn: &Connection) -> Result<(u32, u32)> {
    let queued: i64 = conn.query_row(
        "SELECT COUNT(*) FROM subagent_tasks WHERE status = 'queued'",
        [],
        |row| row.get(0),
    )?;
    let running: i64 = conn.query_row(
        "SELECT COUNT(*) FROM subagent_tasks WHERE status = 'running'",
        [],
        |row| row.get(0),
    )?;
    Ok((queued as u32, running as u32))
}

/// Atomically pick the oldest "queued" task and flip it to "running".
/// Enforces per-subagent FIFO (spec §6.4 line 737): only picks from
/// subagents that have NO running task. This ensures same-subagent tasks
/// are serialized.
///
/// **Atomicity (Round 3 Finding 1 fix):** pick + flip happen inside a
/// single `BEGIN IMMEDIATE` transaction with a CAS guard
/// (`WHERE id = ? AND status = 'queued'`). If two dispatchers race on
/// the same task, only one UPDATE affects 1 row; the other gets 0 rows
/// and returns `None`. The RAII `Transaction` auto-rolls-back on drop.
pub fn pick_oldest_queued_task(conn: &Connection) -> rusqlite::Result<Option<SubagentTaskRow>> {
    // `Transaction::new_unchecked` (vs `conn.transaction_with_behavior`)
    // avoids requiring `&mut Connection` — `Database::conn()` returns `&Connection`.
    // Mirrors the pattern in `run_subagent_doctor` (supervisor.rs).
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    // 1. Pick candidate id (still 'queued', per-subagent FIFO).
    let id_opt: Option<String> = tx
        .query_row(
            "SELECT id FROM subagent_tasks \
             WHERE status = 'queued' \
               AND subagent_id NOT IN ( \
                   SELECT subagent_id FROM subagent_tasks WHERE status = 'running' \
               ) \
             ORDER BY created_at_ms ASC \
             LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(id) = id_opt else {
        tx.commit()?;
        return Ok(None);
    };
    // 2. CAS flip: only updates if still 'queued'. rows_affected == 1 means we won.
    let now = busytok_domain::now_ms();
    let rows = tx.execute(
        "UPDATE subagent_tasks SET status = 'running', started_at_ms = ?1 \
         WHERE id = ?2 AND status = 'queued'",
        rusqlite::params![now, id],
    )?;
    if rows == 0 {
        // Lost the race — another dispatcher flipped it first.
        tx.commit()?;
        return Ok(None);
    }
    // 3. Fetch the full row (status is now 'running', started_at_ms = now).
    let task = tx
        .query_row(
            "SELECT id, subagent_id, source_harness, source_session_id, intent, profile, prompt, \
                    prompt_artifact_ref, output_schema_name, output_schema_version, status, \
                    result_summary, result_json, error, created_at_ms, started_at_ms, \
                    completed_at_ms, timeout_seconds, model_override \
             FROM subagent_tasks WHERE id = ?1",
            rusqlite::params![id],
            |r| {
                Ok(SubagentTaskRow {
                    id: r.get(0)?,
                    subagent_id: r.get(1)?,
                    source_harness: r.get(2)?,
                    source_session_id: r.get(3)?,
                    intent: r.get(4)?,
                    profile: r.get(5)?,
                    prompt: r.get(6)?,
                    prompt_artifact_ref: r.get(7)?,
                    output_schema_name: r.get(8)?,
                    output_schema_version: r.get(9)?,
                    status: r.get(10)?,
                    result_summary: r.get(11)?,
                    result_json: r.get(12)?,
                    error: r.get(13)?,
                    created_at_ms: r.get(14)?,
                    started_at_ms: r.get(15)?,
                    completed_at_ms: r.get(16)?,
                    timeout_seconds: r.get(17)?,
                    model_override: r.get(18)?,
                })
            },
        )
        .optional()?;
    tx.commit()?;
    Ok(task)
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

/// Upsert a hot binding, keyed on the partial unique index
/// `idx_subagent_binding_one_hot (subagent_id, harness) WHERE is_hot = 1`.
/// A re-delegate to the same subagent+harness updates the existing row
/// instead of creating a duplicate.
pub fn upsert_hot_binding(conn: &Connection, row: &SubagentHarnessBindingRow) -> Result<()> {
    conn.execute(
        "INSERT INTO subagent_harness_bindings \
             (id, subagent_id, harness, adapter_session_id, adapter_process_id, is_hot, status, \
              created_at_ms, last_used_at_ms, closed_at_ms, detail_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
         ON CONFLICT(subagent_id, harness) WHERE is_hot = 1 DO UPDATE SET \
             adapter_session_id = excluded.adapter_session_id, \
             adapter_process_id = excluded.adapter_process_id, \
             status = excluded.status, \
             last_used_at_ms = excluded.last_used_at_ms, \
             detail_json = excluded.detail_json",
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
    .with_context(|| format!("upsert hot binding {} {}", row.subagent_id, row.harness))
}

/// Atomically: (1) upsert the hot binding, (2) set the logical subagent
/// status to `hot`. Both writes commit in a single transaction so the spec
/// §3.3 invariant ("status='hot' iff is_hot=1 binding exists") holds at every
/// observable point. Call this ONLY when a real adapter_session_id exists.
pub fn commit_hot_binding_and_status(
    conn: &Connection,
    binding: &SubagentHarnessBindingRow,
    subagent_id: &str,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    upsert_hot_binding(&tx, binding)?;
    let now = busytok_domain::now_ms();
    tx.execute(
        "UPDATE subagent_logical_subagents SET status = 'hot', updated_at_ms = ?1, \
            last_active_at_ms = COALESCE(last_active_at_ms, ?1) \
         WHERE id = ?2",
        params![now, subagent_id],
    )
    .with_context(|| format!("set logical status hot for {subagent_id}"))?;
    tx.commit()
        .context("commit hot binding + status transaction")?;
    Ok(())
}

/// Atomically: (1) upsert the (now-closed) binding, (2) roll the logical
/// subagent status to `new_status` (`warm` or `cold`). Both writes commit in a
/// single transaction so the spec §3.3 invariant ("status='hot' iff is_hot=1
/// binding exists") holds at every observable point — without this, hibernate
/// would briefly leave `status='hot'` with no `is_hot=1` binding between the
/// binding flip and the status flip.
///
/// Mirrors `commit_hot_binding_and_status` but for the hibernate (cool-down)
/// direction. The caller must pre-populate `binding` with `is_hot=0`,
/// `status='closed'`, and `closed_at_ms=Some(now)` before calling.
///
/// `status='deleted'` tombstones are excluded from the logical status update
/// (Plan 1 deletion semantics) — a hibernate on an already-soft-deleted
/// subagent must not revive it. The binding is still upserted (bindings are
/// not tombstone-protected, only logical status is — same rule as
/// `release_hot_bindings_for_shutdown`).
pub fn commit_hibernate_binding_and_status(
    conn: &Connection,
    binding: &SubagentHarnessBindingRow,
    subagent_id: &str,
    new_status: &str,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    upsert_binding(&tx, binding)?;
    let now = busytok_domain::now_ms();
    tx.execute(
        "UPDATE subagent_logical_subagents SET status = ?1, updated_at_ms = ?2, \
            last_active_at_ms = COALESCE(last_active_at_ms, ?2) \
         WHERE id = ?3 AND status != 'deleted'",
        params![new_status, now, subagent_id],
    )
    .with_context(|| format!("set logical status {new_status} for {subagent_id}"))?;
    tx.commit()
        .context("commit hibernate binding + status transaction")?;
    Ok(())
}

/// Atomically commit an eviction: (1) optionally write the `hot_summary`
/// returned by `session.prepare_hibernate`, (2) flip the binding to closed
/// (`is_hot=0, status='closed'`), (3) set the logical subagent status to
/// `warm` if `subagent_memory.hot_summary IS NOT NULL` after the write, else
/// `cold`.
///
/// Spec §3.3 invariant: `status='warm'` iff recoverable memory exists
/// (`subagent_memory.hot_summary IS NOT NULL`); `status='cold'` when no
/// memory. This honors the invariant even when `prepare_hibernate` returned
/// a null `memory_delta` — the subagent keeps `warm` if a prior session wrote
/// a `hot_summary`, and falls to `cold` only when no memory exists at all.
///
/// `hot_summary`: `Some(s)` writes `s` to the memory row; `None` skips the
/// write (the delta was null/absent) but the status is still computed from
/// the final memory state.
///
/// The caller must pre-populate `binding` with `is_hot=0`, `status='closed'`,
/// and `closed_at_ms=Some(now)` before calling. Unlike
/// `commit_hibernate_binding_and_status` (which takes a hardcoded
/// `new_status`), this computes the logical status from memory state so the
/// §3.3 `warm`/`cold` rule cannot be violated by a caller passing the wrong
/// string.
pub fn commit_eviction(
    conn: &Connection,
    binding: &SubagentHarnessBindingRow,
    subagent_id: &str,
    hot_summary: Option<&str>,
) -> Result<String> {
    let tx = conn.unchecked_transaction()?;
    if let Some(summary) = hot_summary {
        write_hot_summary(&tx, subagent_id, summary)?;
    }
    upsert_binding(&tx, binding)?;
    // Compute final logical status from the memory row AFTER the optional
    // write: 'warm' iff hot_summary IS NOT NULL, else 'cold'. `.optional()`
    // maps "no memory row" to None → false (cold); real DB errors propagate.
    let has_memory: bool = tx
        .query_row(
            "SELECT hot_summary IS NOT NULL FROM subagent_memory WHERE subagent_id = ?1",
            params![subagent_id],
            |row| row.get(0),
        )
        .optional()
        .with_context(|| format!("query memory state for {subagent_id}"))?
        .unwrap_or(false);
    let new_status = if has_memory { "warm" } else { "cold" };
    let now = busytok_domain::now_ms();
    tx.execute(
        "UPDATE subagent_logical_subagents SET status = ?1, updated_at_ms = ?2, \
            last_active_at_ms = COALESCE(last_active_at_ms, ?2) \
         WHERE id = ?3 AND status != 'deleted'",
        params![new_status, now, subagent_id],
    )
    .with_context(|| format!("set logical status {new_status} for {subagent_id}"))?;
    tx.commit().context("commit eviction transaction")?;
    Ok(new_status.to_string())
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

/// Find the least-recently-used hot binding for a harness (spec §8.3 step 1).
/// Returns None if no hot bindings exist.
pub fn find_lru_hot_binding(
    conn: &Connection,
    harness: &str,
) -> Result<Option<SubagentHarnessBindingRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, harness, adapter_session_id, adapter_process_id, \
                is_hot, status, created_at_ms, last_used_at_ms, closed_at_ms, detail_json \
         FROM subagent_harness_bindings \
         WHERE harness = ?1 AND is_hot = 1 \
         ORDER BY last_used_at_ms ASC \
         LIMIT 1",
    )?;
    let row = stmt.query_row(params![harness], |row| {
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
    });
    match row {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Find a hot binding by adapter_session_id and harness.
/// Used by the eviction flow to locate the binding for a specific session.
pub fn find_hot_binding_by_session(
    conn: &Connection,
    adapter_session_id: &str,
    harness: &str,
) -> Result<Option<SubagentHarnessBindingRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, harness, adapter_session_id, adapter_process_id, \
                is_hot, status, created_at_ms, last_used_at_ms, closed_at_ms, detail_json \
         FROM subagent_harness_bindings \
         WHERE adapter_session_id = ?1 AND harness = ?2 AND is_hot = 1",
    )?;
    let row_opt = stmt
        .query_row(params![adapter_session_id, harness], |row| {
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

/// Write just the `hot_summary` field of a subagent's memory row.
/// Used by the eviction flow to persist the memory delta returned by
/// `session.prepare_hibernate`. Lives in the store layer so the executor
/// can call it directly without going through SubagentManager.
pub fn write_hot_summary(conn: &Connection, subagent_id: &str, hot_summary: &str) -> Result<()> {
    // UPSERT memory row with just hot_summary (other fields unchanged).
    // Pattern: get-or-create the memory row, update hot_summary, upsert.
    let existing: Option<SubagentMemoryRow> = conn
        .query_row(
            "SELECT id, subagent_id, hot_summary, long_summary, key_files_json, \
                    decisions_json, attempts_json, open_questions_json, artifact_refs_json, \
                    last_compacted_at_ms, last_compacted_task_id, updated_at_ms \
             FROM subagent_memory WHERE subagent_id = ?1",
            params![subagent_id],
            |row| {
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
            },
        )
        .ok();
    let now = busytok_domain::now_ms();
    match existing {
        Some(mut mem) => {
            mem.hot_summary = Some(hot_summary.to_string());
            mem.updated_at_ms = now;
            conn.execute(
                "UPDATE subagent_memory SET hot_summary = ?1, updated_at_ms = ?2 WHERE subagent_id = ?3",
                params![mem.hot_summary, mem.updated_at_ms, subagent_id],
            )?;
        }
        None => {
            conn.execute(
                "INSERT INTO subagent_memory (id, subagent_id, hot_summary, long_summary, \
                 key_files_json, decisions_json, attempts_json, open_questions_json, \
                 artifact_refs_json, last_compacted_at_ms, last_compacted_task_id, updated_at_ms) \
                 VALUES (?1, ?2, ?3, NULL, '[]', '[]', '[]', '[]', '[]', NULL, NULL, ?4)",
                params![format!("mem_{subagent_id}"), subagent_id, hot_summary, now,],
            )?;
        }
    }
    Ok(())
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

/// List resource events, optionally filtered by `target_id`, newest first.
pub fn list_resource_events(
    conn: &Connection,
    target_id: Option<&str>,
    limit: i64,
) -> Result<Vec<SubagentResourceEventRow>> {
    let mut sql = String::from(
        "SELECT id, event_type, target_id, rss_mb, cpu_percent, detail_json, created_at_ms \
         FROM subagent_resource_events WHERE 1=1",
    );
    if target_id.is_some() {
        sql.push_str(" AND target_id = :target_id");
    }
    sql.push_str(" ORDER BY created_at_ms DESC LIMIT :limit");

    let mut stmt = conn.prepare(&sql)?;
    let target_val: String;
    let mut params_vec: Vec<(&str, &dyn rusqlite::ToSql)> = vec![(":limit", &limit)];
    if let Some(t) = target_id {
        target_val = t.to_string();
        params_vec.push((":target_id", &target_val));
    }
    let rows = stmt
        .query_map(params_vec.as_slice(), |row| {
            Ok(SubagentResourceEventRow {
                id: row.get(0)?,
                event_type: row.get(1)?,
                target_id: row.get(2)?,
                rss_mb: row.get(3)?,
                cpu_percent: row.get(4)?,
                detail_json: row.get(5)?,
                created_at_ms: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Converge DB state after a sidecar crash, per spec §3.3 + §5.4.
/// Runs in a single transaction so readers never observe a half-converged
/// state. Returns counts for observability logging.
///
/// **Binding-anchored (spec §3.3: binding is authoritative for "is a worker
/// process running")**: the affected `subagent_id` set is collected FIRST from
/// the hot bindings of the crashed harness, then all subsequent updates are
/// scoped to that set. This avoids two bugs present in a profile-prefix
/// approach:
///   (a) `default_profile LIKE 'pi%'` is imprecise (profiles are free-form
///       strings; future profiles like `pi-search-v2` would match `pi` even
///       if they belonged to a different harness adapter).
///   (b) Updating logical status for "all subagents with no hot binding"
///       would also rewrite `deleted` tombstones and unrelated cold/warm
///       subagents, destroying Plan 1's deletion semantics.
///
/// **Task filter: `subagent_id IN affected` ONLY.** Do NOT filter by
/// `subagent_tasks.source_harness` — that column means the task's *origin*
/// (`claude-code | codex | cli`, spec line 193), not the sidecar adapter that
/// executed it. Filtering `source_harness='pi'` would miss every real Pi
/// sidecar task (their origin is the harness that invoked delegate, e.g.
/// `claude-code`), leaving running tasks orphaned after a crash. The affected
/// `subagent_id` set (from hot bindings) already encodes "had a session on
/// the crashed sidecar", which is the correct scope.
///
/// Steps:
/// 1. Collect affected `subagent_id` set from `subagent_harness_bindings
///    WHERE is_hot=1 AND harness=?`.
/// 2. Mark in-flight tasks (`status='running'` AND `subagent_id IN affected`)
///    → `failed`/`SIDECAR_CRASHED`.
/// 3. Release hot bindings for this harness → `is_hot=0, status='crashed'`.
/// 4. Roll back logical status for the affected set ONLY, excluding
///    `status='deleted'` tombstones: `warm` if memory exists, else `cold`.
pub fn reconcile_sidecar_crash(
    conn: &Connection,
    harness: &str,
) -> Result<CrashReconciliationCounts> {
    let now = busytok_domain::now_ms();
    let tx = conn.unchecked_transaction()?;

    // 1. Collect affected subagent_id set from hot bindings.
    //    This is the authoritative "who was affected" — not profile prefix,
    //    not source_harness (which is origin, not executor).
    let affected_ids: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT subagent_id FROM subagent_harness_bindings \
             WHERE is_hot = 1 AND harness = ?1",
        )?;
        let rows = stmt.query_map(params![harness], |row| row.get::<_, String>(0))?;
        let mut v = Vec::new();
        for r in rows {
            v.push(r?);
        }
        v
    };
    if affected_ids.is_empty() {
        // No hot bindings for this harness — nothing to reconcile.
        // Commit the empty tx for consistency.
        tx.commit().context("commit empty crash reconciliation")?;
        return Ok(CrashReconciliationCounts::default());
    }

    // 2. Mark in-flight tasks as failed. Scope by subagent_id IN affected ONLY.
    //    NOT source_harness — that column is task origin (claude-code|codex|cli,
    //    spec line 193), not the executing sidecar adapter. The affected set
    //    from hot bindings already encodes "had a session on this sidecar".
    let placeholders = affected_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 2))
        .collect::<Vec<_>>()
        .join(",");
    let sql_tasks = format!(
        "UPDATE subagent_tasks SET status = 'failed', error = 'SIDECAR_CRASHED', \
            completed_at_ms = ?1 \
         WHERE status = 'running' AND subagent_id IN ({placeholders})",
    );
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(now)];
    for id in &affected_ids {
        params_vec.push(Box::new(id.clone()));
    }
    let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
    let tasks_failed = tx
        .execute(&sql_tasks, params_refs.as_slice())
        .with_context(|| format!("reconcile tasks for harness {harness}"))?;

    // 3. Release hot bindings: is_hot=0, status='crashed'.
    let bindings_released = tx
        .execute(
            "UPDATE subagent_harness_bindings SET is_hot = 0, status = 'crashed', \
                closed_at_ms = ?1 \
             WHERE is_hot = 1 AND harness = ?2",
            params![now, harness],
        )
        .with_context(|| format!("reconcile bindings for harness {harness}"))?;

    // 4. Roll back logical status for the affected set ONLY.
    //    Exclude deleted tombstones (Plan 1 deletion semantics).
    //    Roll back to warm if memory.hot_summary exists, else cold.
    let sql_status = format!(
        "UPDATE subagent_logical_subagents SET status = CASE \
            WHEN EXISTS (SELECT 1 FROM subagent_memory \
                         WHERE subagent_memory.subagent_id = subagent_logical_subagents.id \
                         AND subagent_memory.hot_summary IS NOT NULL) THEN 'warm' \
            ELSE 'cold' END, \
            updated_at_ms = ?1 \
         WHERE status != 'deleted' AND id IN ({placeholders})",
    );
    let status_rolled_back = tx
        .execute(&sql_status, params_refs.as_slice())
        .context("reconcile logical status after crash")?;

    tx.commit()
        .context("commit crash reconciliation transaction")?;
    Ok(CrashReconciliationCounts {
        tasks_failed,
        bindings_released,
        status_rolled_back,
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CrashReconciliationCounts {
    pub tasks_failed: usize,
    pub bindings_released: usize,
    pub status_rolled_back: usize,
}

/// Converge DB state after a **graceful** sidecar shutdown, per spec §3.3.
///
/// Mirrors `reconcile_sidecar_crash` but with two key differences reflecting
/// that this is a controlled shutdown (the sidecar was asked to
/// `prepare_hibernate` + `adapter.shutdown` first), not a crash:
///
/// 1. Hot bindings are released to `status='closed'` (NOT `'crashed'`).
/// 2. In-flight tasks are NOT touched. The sidecar was given a chance to
///    finish or roll back its own work; a graceful shutdown must not
///    unilaterally rewrite task status. (Crash reconciliation marks running
///    tasks `failed`/`SIDECAR_CRASHED` because a crash gives no such chance.)
///
/// Otherwise the same binding-anchored pattern as `reconcile_sidecar_crash`:
///
/// 1. Collect affected `subagent_id` set from `subagent_harness_bindings
///    WHERE is_hot=1 AND harness=?` FIRST.
/// 2. Release hot bindings for this harness → `is_hot=0, status='closed'`.
/// 3. Roll back logical status for the affected set ONLY, excluding
///    `status='deleted'` tombstones (Plan 1 deletion semantics):
///    `warm` if memory exists, else `cold`.
///
/// Spec §3.3 invariant: after this returns, `status='hot'` iff a hot binding
/// exists — so a dead sidecar process never leaves a `status='hot'` row.
pub fn release_hot_bindings_for_shutdown(
    conn: &Connection,
    harness: &str,
) -> Result<ShutdownReconciliationCounts> {
    let now = busytok_domain::now_ms();
    let tx = conn.unchecked_transaction()?;

    // 1. Collect affected subagent_id set from hot bindings (binding-anchored,
    //    same as reconcile_sidecar_crash). This is the authoritative
    //    "who was affected" scope for all subsequent updates.
    let affected_ids: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT subagent_id FROM subagent_harness_bindings \
             WHERE is_hot = 1 AND harness = ?1",
        )?;
        let rows = stmt.query_map(params![harness], |row| row.get::<_, String>(0))?;
        let mut v = Vec::new();
        for r in rows {
            v.push(r?);
        }
        v
    };
    if affected_ids.is_empty() {
        // No hot bindings for this harness — nothing to reconcile.
        // Commit the empty tx for consistency.
        tx.commit()
            .context("commit empty shutdown reconciliation")?;
        return Ok(ShutdownReconciliationCounts::default());
    }

    // 2. Release hot bindings: is_hot=0, status='closed'.
    //    NOT 'crashed' — this is graceful shutdown.
    let bindings_released = tx
        .execute(
            "UPDATE subagent_harness_bindings SET is_hot = 0, status = 'closed', \
                closed_at_ms = ?1 \
             WHERE is_hot = 1 AND harness = ?2",
            params![now, harness],
        )
        .with_context(|| format!("release hot bindings for shutdown harness {harness}"))?;

    // 3. Roll back logical status for the affected set ONLY.
    //    Exclude deleted tombstones (Plan 1 deletion semantics).
    //    Roll back to warm if memory.hot_summary exists, else cold.
    let placeholders = affected_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 2))
        .collect::<Vec<_>>()
        .join(",");
    let sql_status = format!(
        "UPDATE subagent_logical_subagents SET status = CASE \
            WHEN EXISTS (SELECT 1 FROM subagent_memory \
                         WHERE subagent_memory.subagent_id = subagent_logical_subagents.id \
                         AND subagent_memory.hot_summary IS NOT NULL) THEN 'warm' \
            ELSE 'cold' END, \
            updated_at_ms = ?1 \
         WHERE status != 'deleted' AND id IN ({placeholders})",
    );
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(now)];
    for id in &affected_ids {
        params_vec.push(Box::new(id.clone()));
    }
    let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
    let status_rolled_back = tx
        .execute(&sql_status, params_refs.as_slice())
        .context("reconcile logical status after shutdown")?;

    tx.commit()
        .context("commit shutdown reconciliation transaction")?;
    Ok(ShutdownReconciliationCounts {
        bindings_released,
        status_rolled_back,
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ShutdownReconciliationCounts {
    pub bindings_released: usize,
    pub status_rolled_back: usize,
}

// --- Phase 2: aggregate task queries (no subagent_id filter) --------------
//
// These three functions feed the Subagent Monitoring Page (spec §4 Phase 2):
//   * `list_recent_tasks_all`   — `tasks_recent` (fixed limit 20, all subagents)
//   * `count_tasks_by_subagent` — `subagents[].task_count`
//   * `last_task_by_subagent`   — `subagents[].last_task_{created_at,status}`

/// Most recent tasks across ALL subagents, ordered by `created_at_ms` desc
/// with `id` desc as a deterministic tie-break (spec §4 Phase 2: `tasks_recent`
/// fixed limit 20, no subagent_id filter).
pub fn list_recent_tasks_all(conn: &Connection, limit: i64) -> Result<Vec<SubagentTaskRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, source_harness, source_session_id, intent, profile, prompt, \
                prompt_artifact_ref, output_schema_name, output_schema_version, status, \
                result_summary, result_json, error, created_at_ms, started_at_ms, completed_at_ms, \
                timeout_seconds, model_override \
         FROM subagent_tasks \
         ORDER BY created_at_ms DESC, id DESC \
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit], |row| {
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
                timeout_seconds: row.get(17)?,
                model_override: row.get(18)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// `(subagent_id, task_count)` for every subagent that has at least one task.
/// Spec §4 Phase 2: `subagents[].task_count`.
pub fn count_tasks_by_subagent(conn: &Connection) -> Result<Vec<(String, u32)>> {
    let mut stmt = conn.prepare(
        "SELECT subagent_id, COUNT(*) AS cnt \
         FROM subagent_tasks \
         GROUP BY subagent_id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// `(subagent_id, created_at_ms, status)` for the most recent task of each
/// subagent that has at least one task. Spec §4 Phase 2:
/// `subagents[].last_task_{created_at,status}`. Uses `ROW_NUMBER()` with
/// `id DESC` tie-break for deterministic output when multiple tasks share
/// the same `created_at_ms`.
pub fn last_task_by_subagent(conn: &Connection) -> Result<Vec<(String, i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT subagent_id, created_at_ms, status \
         FROM ( \
             SELECT subagent_id, created_at_ms, status, \
                    ROW_NUMBER() OVER ( \
                        PARTITION BY subagent_id \
                        ORDER BY created_at_ms DESC, id DESC \
                    ) AS rn \
             FROM subagent_tasks \
         ) ranked \
         WHERE rn = 1",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod phase2_tests {
    use super::*;
    use crate::db::Database;
    use crate::repository::SubagentTaskRow;

    fn seed_subagent(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO subagent_logical_subagents \
                 (id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, \
                  default_model, status, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, 'proj', '/repo', 'hash', NULL, NULL, 'pi/review-cheap', NULL, \
                     'warm', 1000, 1000)",
            rusqlite::params![id, id],
        )
        .unwrap();
    }

    fn seed_task(conn: &Connection, id: &str, subagent_id: &str, status: &str, created_at_ms: i64) {
        let mut row = SubagentTaskRow::for_test(id, subagent_id, "pi/review-cheap", "prompt");
        row.status = status.to_string();
        row.created_at_ms = created_at_ms;
        insert_task(conn, &row).unwrap();
    }

    #[test]
    fn list_recent_tasks_all_returns_across_all_subagents() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_subagent(conn, "sub-a");
        seed_subagent(conn, "sub-b");
        seed_task(conn, "t1", "sub-a", "completed", 1000);
        seed_task(conn, "t2", "sub-b", "failed", 2000);
        seed_task(conn, "t3", "sub-a", "completed", 3000);

        let tasks = list_recent_tasks_all(conn, 20).unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].id, "t3"); // desc order
        assert_eq!(tasks[1].id, "t2");
        assert_eq!(tasks[2].id, "t1");
    }

    #[test]
    fn list_recent_tasks_all_respects_limit() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_subagent(conn, "sub-a");
        for i in 0..10 {
            seed_task(conn, &format!("t{i}"), "sub-a", "completed", 1000 + i);
        }
        let tasks = list_recent_tasks_all(conn, 3).unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].id, "t9");
    }

    #[test]
    fn list_recent_tasks_all_empty_when_no_tasks() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        let tasks = list_recent_tasks_all(conn, 20).unwrap();
        assert!(tasks.is_empty());
    }

    #[test]
    fn count_tasks_by_subagent_groups_correctly() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_subagent(conn, "sub-a");
        seed_subagent(conn, "sub-b");
        seed_task(conn, "t1", "sub-a", "completed", 1000);
        seed_task(conn, "t2", "sub-a", "failed", 2000);
        seed_task(conn, "t3", "sub-b", "completed", 3000);

        let counts = count_tasks_by_subagent(conn).unwrap();
        let mut map: std::collections::HashMap<String, u32> = counts.into_iter().collect();
        assert_eq!(map.remove("sub-a").unwrap(), 2);
        assert_eq!(map.remove("sub-b").unwrap(), 1);
    }

    #[test]
    fn count_tasks_by_subagent_empty_when_no_tasks() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        let counts = count_tasks_by_subagent(conn).unwrap();
        assert!(counts.is_empty());
    }

    #[test]
    fn last_task_by_subagent_returns_latest() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_subagent(conn, "sub-a");
        seed_task(conn, "t1", "sub-a", "completed", 1000);
        seed_task(conn, "t2", "sub-a", "failed", 2000);

        let lasts = last_task_by_subagent(conn).unwrap();
        assert_eq!(lasts.len(), 1);
        let (sub_id, created_at, status) = &lasts[0];
        assert_eq!(sub_id, "sub-a");
        assert_eq!(*created_at, 2000);
        assert_eq!(status, "failed");
    }

    #[test]
    fn last_task_by_subagent_empty_when_no_tasks() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_subagent(conn, "sub-a");
        let lasts = last_task_by_subagent(conn).unwrap();
        assert!(lasts.is_empty());
    }

    /// Tie-break determinism: when two tasks share the same `created_at_ms`,
    /// `list_recent_tasks_all` must order by `id DESC` so pagination is stable.
    #[test]
    fn list_recent_tasks_all_tiebreak_deterministic() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_subagent(conn, "sub-a");
        seed_task(conn, "t1", "sub-a", "completed", 1000);
        seed_task(conn, "t2", "sub-a", "completed", 1000);

        let tasks = list_recent_tasks_all(conn, 20).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "t2", "higher id wins tie-break");
        assert_eq!(tasks[1].id, "t1");
    }

    /// Tie-break determinism: when two tasks for the same subagent share the
    /// same `created_at_ms`, `last_task_by_subagent` must return exactly one
    /// row (the higher `id`) — not duplicates.
    #[test]
    fn last_task_by_subagent_tiebreak_deterministic() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_subagent(conn, "sub-a");
        seed_task(conn, "t1", "sub-a", "completed", 1000);
        seed_task(conn, "t2", "sub-a", "failed", 1000);

        let lasts = last_task_by_subagent(conn).unwrap();
        assert_eq!(lasts.len(), 1, "exactly one row despite tied created_at_ms");
        let (sub_id, created_at, status) = &lasts[0];
        assert_eq!(sub_id, "sub-a");
        assert_eq!(*created_at, 1000);
        assert_eq!(status, "failed", "higher id wins tie-break");
    }
}
