//! Generation and readiness write commands.
//!
//! These functions persist readiness state and recover missing generation
//! metadata. They operate on [`rusqlite::Connection`] or [`Database`].

use anyhow::{Context, Result};
use rusqlite::Connection;
use tracing::info;

use crate::generation_queries;
use crate::write_queries;
use crate::Database;
use busytok_domain::now_ms;
use busytok_domain::ReportingTimezone;

/// Store-layer readiness enum — does not depend on protocol DTOs.
pub enum StoreReadiness {
    ReadyExact,
    ReadyDegraded,
}

pub struct PersistReadinessResult {
    pub generation_id: String,
    pub readiness: StoreReadiness,
    pub updated_at_ms: i64,
}

pub struct GenerationRecoveryResult {
    pub generation_id: String,
    pub repaired: bool,
    pub readiness: StoreReadiness,
}

/// Persist ready_exact state for the given generation.
pub fn persist_ready_exact_for_generation(
    conn: &Connection,
    gen_id: &str,
) -> Result<PersistReadinessResult> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO service_state \
         (id, writer_queue_depth, aggregate_lag_ms, readiness, \
          active_generation_id, last_exact_rebuild_at_ms, updated_at_ms) \
         VALUES (1, 0, 0, 'ready_exact', ?1, ?2, ?2) \
         ON CONFLICT(id) DO UPDATE SET \
           readiness = 'ready_exact', \
           active_generation_id = ?1, \
           last_exact_rebuild_at_ms = ?2, \
           updated_at_ms = ?2",
        rusqlite::params![gen_id, now],
    )?;
    Ok(PersistReadinessResult {
        generation_id: gen_id.to_string(),
        readiness: StoreReadiness::ReadyExact,
        updated_at_ms: now,
    })
}

/// Persist ready_degraded state for the given generation.
pub fn persist_ready_degraded_for_generation(
    conn: &Connection,
    gen_id: &str,
) -> Result<PersistReadinessResult> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO service_state \
         (id, writer_queue_depth, aggregate_lag_ms, readiness, \
          active_generation_id, last_exact_rebuild_at_ms, updated_at_ms) \
         VALUES (1, 0, 0, 'ready_degraded', ?1, NULL, ?2) \
         ON CONFLICT(id) DO UPDATE SET \
           readiness = 'ready_degraded', \
           active_generation_id = ?1, \
           last_exact_rebuild_at_ms = NULL, \
           updated_at_ms = ?2",
        rusqlite::params![gen_id, now],
    )?;
    Ok(PersistReadinessResult {
        generation_id: gen_id.to_string(),
        readiness: StoreReadiness::ReadyDegraded,
        updated_at_ms: now,
    })
}

/// Transition the readiness state in service_state.
///
/// Encapsulates the complex CAS (compare-and-swap) SQL that was inline in
/// [`GenerationManager::transition_after_initial_scan`].
pub fn transition_readiness(
    conn: &Connection,
    target: &str,
    active_generation_id: &str,
) -> Result<bool> {
    let now_ms = now_ms();

    if target == "ready_exact" {
        let promoted_generation_exists: bool = conn.query_row(
            "SELECT EXISTS ( \
                SELECT 1 FROM audit_generations \
                WHERE generation_id = ?1 AND state = 'promoted' AND is_active = 1 \
             )",
            [active_generation_id],
            |row| row.get(0),
        )?;
        if !promoted_generation_exists {
            return Ok(false);
        }
    }

    conn.execute(
        "INSERT INTO service_state \
         (id, writer_queue_depth, aggregate_lag_ms, readiness, active_generation_id, updated_at_ms) \
         VALUES (1, 0, 0, ?2, ?3, ?1) \
         ON CONFLICT(id) DO UPDATE SET \
           readiness = CASE \
               WHEN service_state.readiness IN ('starting', 'ready_degraded') \
                AND (?2 != 'ready_exact' OR EXISTS ( \
                    SELECT 1 FROM audit_generations \
                    WHERE generation_id = ?3 AND state = 'promoted' AND is_active = 1 \
                )) THEN ?2 \
               ELSE service_state.readiness \
           END, \
           active_generation_id = CASE \
               WHEN service_state.readiness IN ('starting', 'ready_degraded') \
                AND (?2 != 'ready_exact' OR EXISTS ( \
                    SELECT 1 FROM audit_generations \
                    WHERE generation_id = ?3 AND state = 'promoted' AND is_active = 1 \
                )) THEN ?3 \
               ELSE service_state.active_generation_id \
           END, \
           updated_at_ms = CASE \
               WHEN service_state.readiness IN ('starting', 'ready_degraded') \
                AND (?2 != 'ready_exact' OR EXISTS ( \
                    SELECT 1 FROM audit_generations \
                    WHERE generation_id = ?3 AND state = 'promoted' AND is_active = 1 \
                )) THEN ?1 \
               ELSE service_state.updated_at_ms \
           END",
        rusqlite::params![now_ms, target, active_generation_id],
    )?;

    let persisted: (Option<String>, Option<String>) = conn.query_row(
        "SELECT readiness, active_generation_id FROM service_state WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(matches!(
        persisted,
        (Some(readiness), Some(gen_id))
            if readiness == target && gen_id == active_generation_id
    ))
}

/// Recover audit_generations metadata from existing usage_events when the
/// generation row is missing. Returns None if recovery is not needed.
pub fn recover_missing_generation_metadata(
    db: &Database,
    timezone: &str,
) -> Result<Option<GenerationRecoveryResult>> {
    let conn = db.conn();

    if let Some(active) = generation_queries::read_active_generation(conn)? {
        if generation_queries::generation_has_usage_events(conn, &active)?
            && !generation_queries::generation_has_materialized_read_rows(conn, &active)?
        {
            persist_ready_degraded_for_generation(conn, &active)?;
            return Ok(Some(GenerationRecoveryResult {
                generation_id: active,
                repaired: false,
                readiness: StoreReadiness::ReadyDegraded,
            }));
        }

        let repaired =
            !generation_queries::service_state_is_ready_exact_for_generation(conn, &active)?;
        if repaired {
            if generation_queries::has_blocking_degradation_diagnostic(conn)? {
                return Ok(Some(GenerationRecoveryResult {
                    generation_id: active,
                    repaired: false,
                    readiness: StoreReadiness::ReadyDegraded,
                }));
            }
            persist_ready_exact_for_generation(conn, &active)?;
        }
        return Ok(Some(GenerationRecoveryResult {
            generation_id: active,
            repaired,
            readiness: StoreReadiness::ReadyExact,
        }));
    }

    let generation_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM audit_generations", [], |row| {
            row.get(0)
        })?;
    if generation_count > 0 {
        return Ok(None);
    }

    let event_generation_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM usage_events \
         WHERE generation_id IS NOT NULL AND TRIM(generation_id) != ''",
        [],
        |row| row.get(0),
    )?;
    if event_generation_count > 0 {
        return Ok(None);
    }

    let event_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM usage_events", [], |row| row.get(0))?;
    if event_count == 0 {
        return Ok(None);
    }

    let generation_id = format!("gen-bootstrap-{}", now_ms());
    let now = now_ms();

    {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE usage_events \
             SET generation_id = ?1, \
                 dedupe_key = COALESCE(NULLIF(TRIM(dedupe_key), ''), id), \
                 updated_at_ms = ?2",
            rusqlite::params![generation_id, now],
        )
        .context("failed to backfill usage event generation metadata")?;
        tx.execute(
            "UPDATE audit_generations SET is_active = 0, updated_at_ms = ?1",
            rusqlite::params![now],
        )?;
        tx.execute(
            "INSERT INTO audit_generations \
             (generation_id, state, started_at_ms, promoted_at_ms, is_active, \
              created_at_ms, updated_at_ms) \
             VALUES (?1, 'promoted', ?2, ?2, 1, ?2, ?2) \
             ON CONFLICT(generation_id) DO UPDATE SET \
               state = 'promoted', \
               promoted_at_ms = ?2, \
               is_active = 1, \
               updated_at_ms = ?2",
            rusqlite::params![generation_id, now],
        )
        .context("failed to promote recovered audit generation")?;
        tx.commit()
            .context("failed to commit generation metadata recovery")?;
    }

    let all_events = db
        .all_usage_events()
        .context("failed to read usage events for generation recovery")?;

    {
        let tx = conn.unchecked_transaction()?;
        for table in [
            "usage_buckets_2s",
            "usage_buckets_hour",
            "usage_buckets_day",
            "usage_by_project_day",
            "usage_by_model_day",
            "usage_by_session_day",
            "usage_by_client_day",
        ] {
            tx.execute(&format!("DELETE FROM {table}"), [])?;
        }
        write_queries::update_materialized_aggregates_from_events(
            &*tx,
            &all_events,
            &generation_id,
        )
        .context("failed to rebuild recovered generation aggregates")?;

        // Rebuild daily_usage for the recovered generation using the
        // configured reporting timezone so IANA overview reads are correct.
        tx.execute(
            "DELETE FROM daily_usage WHERE generation_id = ?1",
            rusqlite::params![generation_id],
        )
        .context("failed to clear daily_usage for recovered generation")?;
        let rtz = ReportingTimezone::parse(timezone)
            .context("failed to parse timezone for daily_usage recovery rebuild")?;
        write_queries::upsert_daily_usage_for_events(&*tx, &all_events, &rtz, &generation_id)
            .context("failed to rebuild daily_usage for recovered generation")?;

        persist_ready_exact_for_generation(&tx, &generation_id)?;
        tx.commit()
            .context("failed to commit recovered aggregate rebuild")?;
    }

    info!(
        event_code = "generation.recovery.complete",
        generation_id = %generation_id,
        event_count,
        "recovered missing audit generation metadata from existing usage events"
    );

    Ok(Some(GenerationRecoveryResult {
        generation_id,
        repaired: true,
        readiness: StoreReadiness::ReadyExact,
    }))
}
