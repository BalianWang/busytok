//! Read-only generation and readiness queries.
//!
//! These functions operate on a raw [`rusqlite::Connection`] and are the
//! canonical store layer for generation state reads.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

/// Read the current active generation_id from service_state + audit_generations.
/// Returns None if no valid promoted generation exists.
pub fn read_active_generation(conn: &Connection) -> Result<Option<String>> {
    let service_gen = conn
        .query_row(
            "SELECT active_generation_id FROM service_state WHERE id = 1",
            [],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .context("failed to read active_generation_id from service_state")?
        .flatten()
        .filter(|id: &String| !id.trim().is_empty());

    if let Some(ref gen_id) = service_gen {
        if generation_is_promoted_active(conn, gen_id)? {
            return Ok(Some(gen_id.clone()));
        }
    }

    conn.query_row(
        "SELECT generation_id FROM audit_generations \
         WHERE state = 'promoted' AND is_active = 1 \
         ORDER BY COALESCE(promoted_at_ms, updated_at_ms) DESC \
         LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .context("failed to read promoted generation from audit_generations")
}

/// Check if a generation is promoted and active.
pub fn generation_is_promoted_active(conn: &Connection, gen_id: &str) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audit_generations \
             WHERE generation_id = ?1 AND state = 'promoted' AND is_active = 1",
            rusqlite::params![gen_id],
            |row| row.get(0),
        )
        .context("failed to check promoted generation")?;
    Ok(count > 0)
}

/// Check if a generation has usage events.
pub(crate) fn generation_has_usage_events(conn: &Connection, gen_id: &str) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM usage_events WHERE generation_id = ?1",
            rusqlite::params![gen_id],
            |row| row.get(0),
        )
        .context("failed to check generation usage events")?;
    Ok(count > 0)
}

/// Check if a generation has materialized read-plane rows.
pub(crate) fn generation_has_materialized_read_rows(
    conn: &Connection,
    gen_id: &str,
) -> Result<bool> {
    for table in [
        "usage_buckets_2s",
        "usage_buckets_hour",
        "usage_buckets_day",
        "usage_by_project_day",
        "usage_by_model_day",
        "usage_by_session_day",
        "usage_by_client_day",
        "source_health_summary",
    ] {
        let sql = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE generation_id = ?1)");
        let exists: i64 = conn
            .query_row(&sql, rusqlite::params![gen_id], |row| row.get(0))
            .with_context(|| format!("failed to check materialized rows in {table}"))?;
        if exists != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Check if service_state is ready_exact for a given generation.
pub fn service_state_is_ready_exact_for_generation(
    conn: &Connection,
    gen_id: &str,
) -> Result<bool> {
    let row: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT readiness, active_generation_id FROM service_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .context("failed to read readiness from service_state")?;
    Ok(matches!(
        row,
        Some((Some(readiness), Some(active)))
            if readiness == "ready_exact" && active == gen_id
    ))
}

/// Check for blocking degradation diagnostics.
pub fn has_blocking_degradation_diagnostic(conn: &Connection) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM diagnostic_events \
             WHERE severity = 'error' \
               AND code IN ('generation_drift', 'promotion_failed')",
            [],
            |row| row.get(0),
        )
        .context("failed to check blocking degradation diagnostics")?;
    Ok(count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;

    #[test]
    fn generation_has_usage_events_returns_false_on_empty_db() {
        let db = Database::open_in_memory().expect("db");
        let result = generation_has_usage_events(db.conn(), "nonexistent").expect("check");
        assert!(!result);
    }

    #[test]
    fn generation_has_usage_events_returns_true_when_events_exist() {
        let db = Database::open_in_memory().expect("db");
        db.conn().execute(
            "INSERT INTO usage_events \
             (id, agent, source_file_id, source_path, source_line, source_offset_start, \
              source_offset_end, session_id, raw_event_hash, timestamp_ms, created_at_ms, \
              updated_at_ms, generation_id) \
             VALUES ('ev1', 'claude_code', 'f1', '/p.jsonl', 1, 0, 10, 's1', 'h1', 1000, 1000, 1000, 'gen-1')",
            [],
        ).expect("seed");
        let result = generation_has_usage_events(db.conn(), "gen-1").expect("check");
        assert!(result);
    }

    #[test]
    fn generation_has_materialized_read_rows_returns_false_on_empty_db() {
        let db = Database::open_in_memory().expect("db");
        let result =
            generation_has_materialized_read_rows(db.conn(), "nonexistent").expect("check");
        assert!(!result);
    }
}
