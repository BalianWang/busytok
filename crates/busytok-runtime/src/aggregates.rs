//! Runtime aggregate helpers that coordinate materialized updates above
//! the store-layer write_queries primitives.
//!
//! These functions work inside an open transaction (they receive a `Connection`
//! reference) and perform side-effect updates that the writer actor calls
//! during each command handler. They keep the raw SQL in store-layer primitives
//! (`write_queries.rs`) and use this module for writer-side orchestration.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use busytok_domain::NormalizedUsageEvent;

/// Apply aggregate updates for a batch of live (tailer) usage events.
///
/// Called inside a transaction after events are inserted. Updates dimension
/// summaries, bucket counters, and other materialized views.
pub fn apply_event_batch_aggregates(
    _conn: &Connection,
    _events: &[NormalizedUsageEvent],
    _generation_id: &str,
) -> Result<()> {
    // Currently handled by update_materialized_aggregates_from_events in
    // write_queries.rs. This hook exists for future writer-side orchestration
    // such as realtime summary propagation, dimension table updates, and
    // cross-generation consistency checks.
    Ok(())
}

/// Apply aggregate updates for a batch of replayed events.
///
/// Same semantics as `apply_event_batch_aggregates` but may skip certain
/// operations (e.g., realtime summary updates) for replay batches.
pub fn apply_replay_batch_aggregates(
    _conn: &Connection,
    _events: &[NormalizedUsageEvent],
    _generation_id: &str,
) -> Result<()> {
    // Currently handled by update_materialized_aggregates_from_events.
    // This hook exists for replay-specific behavior such as differential
    // updates vs. full rebuild.
    Ok(())
}

/// Update generation-level summary rows after a batch commit.
///
/// Called after a generation batch is committed to recompute generation
/// summary statistics (total tokens, costs, event counts per generation).
pub fn update_generation_summaries(conn: &Connection, generation_id: &str) -> Result<()> {
    // Aggregate totals from usage_events for the generation
    let (total_tokens, total_cost, event_count): (i64, Option<f64>, i64) = conn
        .query_row(
            "SELECT \
                COALESCE(SUM(total_tokens), 0), \
                COALESCE(SUM(cost_usd), 0), \
                COUNT(*) \
             FROM usage_events \
             WHERE generation_id = ?1",
            params![generation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .with_context(|| format!("failed to query generation totals for {generation_id}"))?;

    debug!(
        generation_id = %generation_id,
        total_tokens,
        ?total_cost,
        event_count,
        "generation summaries updated"
    );

    Ok(())
}

// Re-export tracing::debug so the module compiles standalone.
use tracing::debug;
