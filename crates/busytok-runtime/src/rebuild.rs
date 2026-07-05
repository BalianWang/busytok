//! Rebuild orchestration: frontier capture, replay queue drain, and
//! promotion barrier.
//!
//! This module provides the high-level rebuild lifecycle:
//!
//! 1. **Frontier capture**: at rebuild start, records each source file's current
//!    (offset, size, mtime) as a `generation_file_observation` row. These
//!    frontiers define the boundary between scanned data and live tail data.
//!
//! 2. **Replay queue**: while the scan runs, tail deltas above the frontier are
//!    accumulated in `tail_replay_queue`. After the scan, these are drained
//!    and applied to the new generation.
//!
//! 3. **Promotion barrier**: a short-lived (<5 s) gate that stops new promotion-
//!    sensitive writes, drains pending replay rows, runs bounded consistency
//!    checks, updates checkpoints, and atomically flips the active generation
//!    pointer.
//!
//! 4. **Drift guardrail**: if the consistency check detects aggregate drift
//!    (e.g. file truncated or replaced during scan), promotion is refused,
//!    a diagnostic is persisted, and the service enters `ReadyDegraded`.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use tracing::{info, warn};

use busytok_domain::now_ms;
use busytok_protocol::dto::ReadinessStateDto;
use busytok_store::write_queries;
use busytok_store::Database;

use crate::status::ServiceStatusSnapshot;

/// Maximum wall-clock time allowed for the promotion barrier.
pub const BARRIER_TIMEOUT_MS: u64 = 5_000;

/// Maximum number of replay rows to drain per batch during the barrier.
pub const REPLAY_DRAIN_BATCH_LIMIT: i64 = 10_000;

/// Maximum number of rows to check during bounded consistency validation.
pub const CONSISTENCY_CHECK_LIMIT: i64 = 500;

// ── Rebuild frontier ───────────────────────────────────────────────────────

/// Captures the state of a single source file at the start of a rebuild.
#[derive(Debug, Clone)]
pub struct RebuildFrontier {
    pub source_file_id: String,
    pub source_id: String,
    pub agent: String,
    pub path: String,
    pub offset_bytes: i64,
    pub size_bytes: i64,
    pub last_mtime_ms: Option<i64>,
}

/// Captures frontiers for all source files needed by a rebuild.
#[derive(Debug, Clone)]
pub struct RebuildFrontierSet {
    pub generation_id: String,
    pub frontiers: Vec<RebuildFrontier>,
    pub captured_at_ms: i64,
}

impl RebuildFrontierSet {
    /// Capture frontiers from the database's current `source_file_checkpoints`.
    ///
    /// Reads every checkpoint row and snapshots its (offset, size, mtime)
    /// into a frontier set labelled with the given generation ID.
    pub fn capture(generation_id: &str, db: &Database) -> Result<Self> {
        let conn = db.conn();
        let captured_at_ms = now_ms();

        let mut stmt = conn.prepare(
            "SELECT id, source_id, agent, path, offset_bytes, size_bytes, last_mtime_ms \
             FROM source_file_checkpoints \
             WHERE state = 'active'",
        )?;

        let frontiers: Vec<RebuildFrontier> = stmt
            .query_map([], |row| {
                Ok(RebuildFrontier {
                    source_file_id: row.get(0)?,
                    source_id: row.get(1)?,
                    agent: row.get(2)?,
                    path: row.get(3)?,
                    offset_bytes: row.get(4)?,
                    size_bytes: row.get(5)?,
                    last_mtime_ms: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        info!(
            generation_id = %generation_id,
            frontier_count = frontiers.len(),
            "captured rebuild frontiers"
        );

        Ok(Self {
            generation_id: generation_id.to_string(),
            frontiers,
            captured_at_ms,
        })
    }

    /// Persist each frontier as a `generation_file_observation` row.
    ///
    /// Uses `INSERT OR REPLACE` so re-detection of the same file during
    /// a rebuild is idempotent.
    pub fn persist(&self, db: &Database) -> Result<()> {
        let conn = db.conn();
        for frontier in &self.frontiers {
            write_queries::insert_generation_observation(
                conn,
                &self.generation_id,
                &frontier.source_file_id,
                frontier.offset_bytes,
                frontier.size_bytes,
                frontier.last_mtime_ms,
                Some("ok"),
                None,
            )?;
        }

        info!(
            generation_id = %self.generation_id,
            count = self.frontiers.len(),
            "persisted rebuild frontiers"
        );
        Ok(())
    }
}

// ── Generation lifecycle ───────────────────────────────────────────────────

/// Create a new audit generation row in `state = 'building'`.
///
/// Returns the newly allocated generation ID.
pub fn create_generation(db: &Database, gen_id: &str) -> Result<()> {
    let conn = db.conn();
    let now = now_ms();

    conn.execute(
        "INSERT OR REPLACE INTO audit_generations \
         (generation_id, state, started_at_ms, is_active, created_at_ms, updated_at_ms) \
         VALUES (?1, 'building', ?2, 0, ?2, ?2)",
        rusqlite::params![gen_id, now],
    )
    .context("failed to create audit generation")?;

    info!(generation_id = %gen_id, "created audit generation (building)");
    Ok(())
}

/// Create a generation_file_observation row for a file discovered during
/// the rebuild (new files not in the original frontier set).
pub fn record_new_file_observation(
    db: &Database,
    gen_id: &str,
    file_id: &str,
    source_id: &str,
    agent: &str,
    path: &str,
    offset: i64,
    size: i64,
    mtime: Option<i64>,
) -> Result<()> {
    let conn = db.conn();
    // Ensure a source_file_checkpoints row exists for this new file.
    write_queries::upsert_source_file_checkpoint(
        conn, file_id, source_id, agent, path, None, offset, size, mtime, "active", None,
    )?;
    write_queries::upsert_log_file_checkpoint(
        conn, file_id, source_id, agent, path, None, offset, size, mtime, "active", None,
    )?;
    // Record the frontier observation at discovery time.
    write_queries::insert_generation_observation(
        conn,
        gen_id,
        file_id,
        offset,
        size,
        mtime,
        Some("new_file"),
        None,
    )?;

    info!(
        generation_id = %gen_id,
        file_id = %file_id,
        "recorded new-file observation for rebuild"
    );
    Ok(())
}

// ── Promotion barrier ──────────────────────────────────────────────────────

/// Result of a promotion barrier execution.
#[derive(Debug)]
pub struct PromotionResult {
    /// Whether the promotion was successful.
    pub promoted: bool,
    /// The generation that was promoted (or would have been).
    pub generation_id: String,
    /// Wall-clock duration of the barrier in milliseconds.
    pub barrier_duration_ms: u64,
    /// Number of replay rows drained and applied.
    pub replay_rows_applied: i64,
    /// Reason for non-promotion, if applicable.
    pub degradation_reason: Option<String>,
}

/// Execute the promotion barrier for a rebuild.
///
/// This is the core atomic gate between the old generation and the new one.
/// It:
///
/// 1. Drains the tail replay queue, applying all pending rows to the target
///    generation (bounded: `REPLAY_DRAIN_BATCH_LIMIT` per batch, loops until
///    drained).
/// 2. Runs a bounded consistency check (not a full table scan — validates
///    a sample of generation_file_observations against current checkpoints).
/// 3. If consistency passes: atomically promotes the generation (deactivates
///    all others, sets `is_active = 1` on the target, state = 'promoted').
/// 4. If consistency fails: persists a diagnostic, leaves the last promoted
///    generation active, sets the service to `ReadyDegraded`.
///
/// The entire barrier is bounded to `BARRIER_TIMEOUT_MS` (5 seconds). If the
/// timeout is exceeded, the barrier is aborted and a diagnostic is emitted.
pub fn execute_promotion_barrier(
    db: &Arc<Mutex<Database>>,
    status: &Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    to_generation_id: &str,
) -> Result<PromotionResult> {
    let start = Instant::now();
    let mut replay_applied = 0i64;

    // ── Phase 1: Drain replay queue ────────────────────────────────────

    {
        let db = db.lock().unwrap();
        let conn = db.conn();

        // Drain replay rows in bounded batches.
        loop {
            let elapsed = start.elapsed().as_millis() as u64;
            if elapsed >= BARRIER_TIMEOUT_MS {
                warn!(
                    barrier_elapsed_ms = elapsed,
                    "promotion barrier timeout exceeded during replay drain"
                );
                return Ok(PromotionResult {
                    promoted: false,
                    generation_id: to_generation_id.to_string(),
                    barrier_duration_ms: elapsed,
                    replay_rows_applied: replay_applied,
                    degradation_reason: Some("barrier timeout during replay drain".to_string()),
                });
            }

            let applied = write_queries::apply_replay_rows_to_target_generation(
                conn,
                to_generation_id,
                None,
                REPLAY_DRAIN_BATCH_LIMIT,
            )?;

            if applied == 0 {
                break; // queue drained
            }
            replay_applied += applied;

            info!(
                batch_applied = applied,
                total_applied = replay_applied,
                "replay queue batch drained"
            );
        }
    }

    let after_replay = start.elapsed().as_millis() as u64;
    if after_replay >= BARRIER_TIMEOUT_MS {
        warn!(
            barrier_elapsed_ms = after_replay,
            "promotion barrier timeout exceeded after replay drain"
        );
        return Ok(PromotionResult {
            promoted: false,
            generation_id: to_generation_id.to_string(),
            barrier_duration_ms: after_replay,
            replay_rows_applied: replay_applied,
            degradation_reason: Some("barrier timeout after replay drain".to_string()),
        });
    }

    // ── Phase 2: Bounded consistency check ─────────────────────────────

    let drift_detected = {
        let db = db.lock().unwrap();
        let conn = db.conn();

        // Check: for the target generation, do generation_file_observations
        // agree with source_file_checkpoints on offset/size/mtime?
        //
        // We check a sample of up to CONSISTENCY_CHECK_LIMIT rows.
        // Drift is detected if any observation shows an offset that is
        // GREATER than the current checkpoint offset (file shrunk since
        // observation) or if mtimes disagree for non-zero-size files.
        let mut stmt = conn.prepare(
            "SELECT gfo.source_file_id, gfo.offset_bytes, gfo.size_bytes, \
                    gfo.last_mtime_ms, \
                    sfc.offset_bytes AS current_offset, \
                    sfc.size_bytes AS current_size, \
                    sfc.last_mtime_ms AS current_mtime \
             FROM generation_file_observations gfo \
             LEFT JOIN source_file_checkpoints sfc \
               ON sfc.id = gfo.source_file_id \
             WHERE gfo.generation_id = ?1 \
             LIMIT ?2",
        )?;

        let mut drift = false;

        let rows = stmt.query_map(
            rusqlite::params![to_generation_id, CONSISTENCY_CHECK_LIMIT],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                ))
            },
        )?;

        for row in rows {
            let (
                file_id,
                obs_offset,
                obs_size,
                obs_mtime,
                current_offset,
                current_size,
                current_mtime,
            ) = row?;

            // Drift condition 1: observation offset exceeds current checkpoint
            // offset — the file was truncated or replaced after the rebuild
            // scanned it.
            if current_offset < obs_offset && obs_size > 0 {
                warn!(
                    file_id = %file_id,
                    obs_offset,
                    current_offset,
                    "drift detected: file offset regressed"
                );
                drift = true;
                break;
            }

            // Drift condition 2: mtime differs while sizes are distinct
            // (file replaced with different version during scan).
            if let (Some(obs_mt), Some(cur_mt)) = (obs_mtime, current_mtime) {
                if obs_mt != cur_mt && obs_size != current_size && obs_size > 0 {
                    warn!(
                        file_id = %file_id,
                        obs_mtime = obs_mt,
                        current_mtime = cur_mt,
                        obs_size,
                        current_size,
                        "drift detected: file size and mtime mismatch"
                    );
                    drift = true;
                    break;
                }
            }
        }

        drift
    };

    if drift_detected {
        let total_elapsed = start.elapsed().as_millis() as u64;

        // Persist diagnostic.
        {
            let db = db.lock().unwrap();
            let conn = db.conn();
            let now = now_ms();
            let diag_id = format!("drift-{}-{}", to_generation_id, now);
            conn.execute(
                "INSERT OR REPLACE INTO diagnostic_events \
                 (id, agent, source_id, source_file_id, source_path, source_line, \
                  severity, code, message, details_json, happened_at_ms, created_at_ms) \
                 VALUES (?1, NULL, 'rebuild', NULL, NULL, NULL, \
                         'error', 'generation_drift', \
                         ?2, NULL, ?3, ?3)",
                rusqlite::params![
                    diag_id,
                    format!(
                        "generation {} has drift vs current checkpoints; promotion refused",
                        to_generation_id
                    ),
                    now,
                ],
            )
            .context("failed to persist drift diagnostic")?;
        }

        // Update status to ready_degraded (in-memory + DB).
        {
            let now = now_ms();
            let db = db.lock().unwrap();
            let conn = db.conn();
            conn.execute(
                "INSERT INTO service_state \
                 (id, writer_queue_depth, aggregate_lag_ms, readiness, \
                  active_generation_id, updated_at_ms) \
                 VALUES (1, 0, 0, 'ready_degraded', \
                         COALESCE((SELECT active_generation_id FROM service_state WHERE id = 1), ''), \
                         ?1) \
                 ON CONFLICT(id) DO UPDATE SET \
                   readiness = 'ready_degraded', \
                   updated_at_ms = ?1",
                rusqlite::params![now],
            )
            .context("failed to persist ready_degraded to service_state")?;
        }
        {
            let mut snap = status
                .try_write()
                .map_err(|_| anyhow::anyhow!("status lock contention"))?;
            let current_gen = snap.active_generation_id.clone();
            snap.apply_durable_transition(ReadinessStateDto::ReadyDegraded, current_gen);
        }

        warn!(
            generation = %to_generation_id,
            barrier_duration_ms = total_elapsed,
            "promotion refused due to consistency drift; entering ReadyDegraded"
        );

        return Ok(PromotionResult {
            promoted: false,
            generation_id: to_generation_id.to_string(),
            barrier_duration_ms: total_elapsed,
            replay_rows_applied: replay_applied,
            degradation_reason: Some("consistency drift detected".to_string()),
        });
    }

    // ── Phase 3: Atomic promotion ──────────────────────────────────────

    {
        let db = db.lock().unwrap();
        let conn = db.conn();
        let tx = conn.unchecked_transaction()?;
        let now = now_ms();
        // Deactivate all existing generations.
        tx.execute(
            "UPDATE audit_generations SET is_active = 0, updated_at_ms = ?1",
            rusqlite::params![now],
        )
        .context("failed to deactivate existing generations")?;

        // Promote the target generation.
        tx.execute(
            "INSERT INTO audit_generations \
             (generation_id, state, started_at_ms, promoted_at_ms, is_active, \
              created_at_ms, updated_at_ms) \
             VALUES (?1, 'promoted', ?2, ?3, 1, ?2, ?3) \
             ON CONFLICT(generation_id) DO UPDATE SET \
               state = 'promoted', \
               promoted_at_ms = ?3, \
               is_active = 1, \
               updated_at_ms = ?3",
            rusqlite::params![to_generation_id, now, now],
        )
        .context("failed to promote generation")?;

        write_queries::rebuild_source_summaries_tx(&tx, to_generation_id)
            .context("failed to rebuild source summaries during promotion")?;

        // Update service_state.
        tx.execute(
            "INSERT OR REPLACE INTO service_state \
             (id, writer_queue_depth, aggregate_lag_ms, readiness, \
              active_generation_id, last_exact_rebuild_at_ms, updated_at_ms) \
             VALUES (1, 0, 0, 'ready_exact', ?1, ?2, ?2)",
            rusqlite::params![to_generation_id, now],
        )
        .context("failed to update service_state after promotion")?;

        tx.commit()
            .context("failed to commit promotion transaction")?;

        info!(
            generation = %to_generation_id,
            replay_rows_applied = replay_applied,
            "generation promoted successfully"
        );
    }

    // Update in-memory status snapshot.
    {
        let mut snap = status
            .try_write()
            .map_err(|_| anyhow::anyhow!("status lock contention"))?;
        let new_gen = Some(to_generation_id.to_string());
        snap.apply_durable_transition(ReadinessStateDto::ReadyExact, new_gen);
    }

    let total_elapsed = start.elapsed().as_millis() as u64;

    if total_elapsed > BARRIER_TIMEOUT_MS {
        warn!(
            barrier_duration_ms = total_elapsed,
            timeout_ms = BARRIER_TIMEOUT_MS,
            "promotion barrier exceeded timeout budget"
        );
    }

    Ok(PromotionResult {
        promoted: true,
        generation_id: to_generation_id.to_string(),
        barrier_duration_ms: total_elapsed,
        replay_rows_applied: replay_applied,
        degradation_reason: None,
    })
}

// ── High-level rebuild orchestration ───────────────────────────────────────

/// Run a complete rebuild cycle.
///
/// This is the top-level entry point for a rebuild. It:
///
/// 1. Creates a new audit generation (state = 'building').
/// 2. Captures rebuild frontiers from current source file checkpoints.
/// 3. The caller is responsible for running the actual scan (via
///    `scan_once_via_writer`) with the target generation. During the scan,
///    live tail deltas accumulate in the `tail_replay_queue`.
/// 4. After scan completion, calls `execute_promotion_barrier` to drain
///    replay and atomically promote.
///
/// Returns the generation ID and frontier set for the caller to use during
/// the scan phase.
pub fn initiate_rebuild(db: &Database, gen_id: &str) -> Result<RebuildFrontierSet> {
    // Step 1: Create the generation row.
    create_generation(db, gen_id)?;

    // Step 2: Capture frontiers.
    let frontier_set = RebuildFrontierSet::capture(gen_id, db)?;

    // Step 3: Persist frontiers as generation_file_observations.
    frontier_set.persist(db)?;

    info!(
        generation_id = %gen_id,
        "rebuild initiated"
    );

    Ok(frontier_set)
}

/// Record a source file that was discovered during the rebuild as a
/// new-file observation.
///
/// New files discovered while the rebuild is running need their own
/// frontier recorded at discovery time so the tail replay can correctly
/// delineate scanned vs. live data.
pub fn record_new_file_during_rebuild(
    db: &Database,
    gen_id: &str,
    frontier: &RebuildFrontier,
) -> Result<()> {
    record_new_file_observation(
        db,
        gen_id,
        &frontier.source_file_id,
        &frontier.source_id,
        &frontier.agent,
        &frontier.path,
        frontier.offset_bytes,
        frontier.size_bytes,
        frontier.last_mtime_ms,
    )
}

// ── Diagnostics ────────────────────────────────────────────────────────────

/// Record a rebuild diagnostic event (error, warning, or info).
pub fn record_rebuild_diagnostic(
    db: &Database,
    gen_id: &str,
    severity: &str,
    message: &str,
) -> Result<()> {
    let conn = db.conn();
    let now = now_ms();
    let diag_id = format!("rebuild-{}-{}-{}", gen_id, severity, now);

    conn.execute(
        "INSERT OR REPLACE INTO diagnostic_events \
         (id, agent, source_id, source_file_id, source_path, source_line, \
          severity, code, message, details_json, happened_at_ms, created_at_ms) \
         VALUES (?1, NULL, 'rebuild', NULL, NULL, NULL, \
                 ?2, 'rebuild_lifecycle', ?3, NULL, ?4, ?4)",
        rusqlite::params![diag_id, severity, message, now],
    )
    .context("failed to record rebuild diagnostic")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use busytok_store::Database;

    fn seeded_db() -> Database {
        Database::open_in_memory().expect("open db")
    }

    fn seed_checkpoint(db: &Database, id: &str, offset: i64, size: i64) {
        let conn = db.conn();
        let now = now_ms();
        conn.execute(
            "INSERT OR REPLACE INTO source_file_checkpoints \
             (id, source_id, agent, path, inode, offset_bytes, size_bytes, \
              last_mtime_ms, state, first_seen_at_ms, last_seen_at_ms, \
              created_at_ms, updated_at_ms) \
             VALUES (?1, 'src-1', 'claude_code', '/tmp/test.jsonl', NULL, \
                     ?2, ?3, NULL, 'active', ?4, ?4, ?4, ?4)",
            rusqlite::params![id, offset, size, now],
        )
        .unwrap();
    }

    #[test]
    fn capture_frontiers_reads_all_active_checkpoints() {
        let db = seeded_db();
        seed_checkpoint(&db, "file-a", 100, 500);
        seed_checkpoint(&db, "file-b", 200, 600);

        let frontiers = RebuildFrontierSet::capture("gen-test", &db).unwrap();
        assert_eq!(frontiers.frontiers.len(), 2);
        assert_eq!(frontiers.generation_id, "gen-test");

        let a = frontiers
            .frontiers
            .iter()
            .find(|f| f.source_file_id == "file-a")
            .unwrap();
        assert_eq!(a.offset_bytes, 100);
        assert_eq!(a.size_bytes, 500);

        let b = frontiers
            .frontiers
            .iter()
            .find(|f| f.source_file_id == "file-b")
            .unwrap();
        assert_eq!(b.offset_bytes, 200);
        assert_eq!(b.size_bytes, 600);
    }

    #[test]
    fn persist_frontiers_creates_observations() {
        let db = seeded_db();
        seed_checkpoint(&db, "file-a", 100, 500);

        let frontiers = RebuildFrontierSet::capture("gen-test", &db).unwrap();
        frontiers.persist(&db).unwrap();

        let count: i64 = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM generation_file_observations WHERE generation_id = 'gen-test'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn create_generation_inserts_building_row() {
        let db = seeded_db();
        create_generation(&db, "gen-1").unwrap();

        let state: String = db
            .conn()
            .query_row(
                "SELECT state FROM audit_generations WHERE generation_id = 'gen-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "building");
    }

    #[test]
    fn promotion_barrier_promotes_generation() {
        let db = seeded_db();
        let db = Arc::new(Mutex::new(db));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));

        // Seed a generation.
        {
            let d = db.lock().unwrap();
            create_generation(&d, "gen-promote").unwrap();
            seed_checkpoint(&d, "file-a", 100, 500);
            write_queries::insert_generation_observation(
                d.conn(),
                "gen-promote",
                "file-a",
                100,
                500,
                None,
                Some("ok"),
                None,
            )
            .unwrap();
        }

        let result = execute_promotion_barrier(&db, &status, "gen-promote").unwrap();

        assert!(result.promoted, "promotion should succeed");
        assert!(result.barrier_duration_ms < BARRIER_TIMEOUT_MS);

        // Verify generation is now active.
        let d = db.lock().unwrap();
        let is_active: i32 = d
            .conn()
            .query_row(
                "SELECT is_active FROM audit_generations WHERE generation_id = 'gen-promote'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(is_active, 1);
    }

    #[test]
    fn drift_detected_when_observation_offset_exceeds_checkpoint() {
        let db = seeded_db();
        let db = Arc::new(Mutex::new(db));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));

        // Seed: observation shows offset 5000, but checkpoint regressed to 2000
        // (file truncated after observation was recorded).
        {
            let d = db.lock().unwrap();
            create_generation(&d, "gen-drift").unwrap();
            seed_checkpoint(&d, "file-a", 2000, 5000); // current is 2000
            write_queries::insert_generation_observation(
                d.conn(),
                "gen-drift",
                "file-a",
                5000, // observation was at 5000
                5000,
                None,
                Some("ok"),
                None,
            )
            .unwrap();
        }

        let result = execute_promotion_barrier(&db, &status, "gen-drift").unwrap();

        assert!(!result.promoted, "promotion should be refused on drift");
        assert!(result.degradation_reason.is_some());

        let snap = status.try_read().unwrap();
        assert!(matches!(snap.readiness, ReadinessStateDto::ReadyDegraded));
    }

    /// `record_new_file_observation` writes the source_file_checkpoint,
    /// log_file_checkpoint, and generation_file_observation rows for a newly
    /// discovered file. Covers the new-file recording path.
    #[test]
    fn record_new_file_observation_writes_all_three_rows() {
        let db = seeded_db();
        create_generation(&db, "gen-newfile").unwrap();

        record_new_file_observation(
            &db,
            "gen-newfile",
            "file-new-1",
            "src-1",
            "claude_code",
            "/tmp/new.jsonl",
            0,
            100,
            None,
        )
        .unwrap();

        // source_file_checkpoints row created.
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM source_file_checkpoints WHERE id = 'file-new-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // log_files row created (upsert_log_file_checkpoint writes to log_files table).
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM log_files WHERE id = 'file-new-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // generation_file_observations row created with new_file scan_status.
        let scan_status: String = db
            .conn()
            .query_row(
                "SELECT scan_status FROM generation_file_observations \
                 WHERE generation_id = 'gen-newfile' AND source_file_id = 'file-new-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(scan_status, "new_file");
    }

    /// `record_new_file_observation` is idempotent for the checkpoint rows
    /// (upsert) — calling twice with the same file_id doesn't fail.
    #[test]
    fn record_new_file_observation_is_idempotent_for_checkpoints() {
        let db = seeded_db();
        create_generation(&db, "gen-idem").unwrap();

        // First call writes everything.
        record_new_file_observation(
            &db,
            "gen-idem",
            "file-x",
            "src-1",
            "claude_code",
            "/tmp/x.jsonl",
            0,
            100,
            None,
        )
        .unwrap();

        // Second call upserts (no duplicate-key failure).
        record_new_file_observation(
            &db,
            "gen-idem",
            "file-x",
            "src-1",
            "claude_code",
            "/tmp/x.jsonl",
            200,
            500,
            None,
        )
        .unwrap();

        // Checkpoint row count is still 1 (upsert, not insert).
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM source_file_checkpoints WHERE id = 'file-x'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    // ── Drift condition 2: mtime + size mismatch ───────────────────────────

    /// Helper that seeds a checkpoint with an explicit mtime (the default
    /// `seed_checkpoint` sets mtime to NULL, which cannot trigger condition 2).
    fn seed_checkpoint_with_mtime(
        db: &Database,
        id: &str,
        offset: i64,
        size: i64,
        mtime: Option<i64>,
    ) {
        let conn = db.conn();
        let now = now_ms();
        conn.execute(
            "INSERT OR REPLACE INTO source_file_checkpoints \
             (id, source_id, agent, path, inode, offset_bytes, size_bytes, \
              last_mtime_ms, state, first_seen_at_ms, last_seen_at_ms, \
              created_at_ms, updated_at_ms) \
             VALUES (?1, 'src-1', 'claude_code', '/tmp/test.jsonl', NULL, \
                     ?2, ?3, ?4, 'active', ?5, ?5, ?5, ?5)",
            rusqlite::params![id, offset, size, mtime, now],
        )
        .unwrap();
    }

    /// Drift condition 2: observation mtime differs from checkpoint mtime AND
    /// sizes differ (file replaced with a different version during scan).
    /// The offset must NOT regress so condition 1 doesn't fire first.
    #[test]
    fn drift_detected_when_mtime_and_size_mismatch() {
        let db = seeded_db();
        let db = Arc::new(Mutex::new(db));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));

        {
            let d = db.lock().unwrap();
            create_generation(&d, "gen-drift2").unwrap();
            // Checkpoint: offset=100, size=600, mtime=2000
            seed_checkpoint_with_mtime(&d, "file-d2", 100, 600, Some(2000));
            // Observation: offset=100 (same — no regression), size=500, mtime=1000
            write_queries::insert_generation_observation(
                d.conn(),
                "gen-drift2",
                "file-d2",
                100,
                500,        // different size
                Some(1000), // different mtime
                Some("ok"),
                None,
            )
            .unwrap();
        }

        let result = execute_promotion_barrier(&db, &status, "gen-drift2").unwrap();

        assert!(
            !result.promoted,
            "promotion should be refused on mtime+size drift"
        );
        assert!(result.degradation_reason.is_some());

        let snap = status.try_read().unwrap();
        assert!(matches!(snap.readiness, ReadinessStateDto::ReadyDegraded));
    }

    /// Drift condition 2 does NOT fire when mtime differs but sizes are equal
    /// (the `obs_size != current_size` guard prevents false positives).
    #[test]
    fn no_drift_when_mtime_differs_but_size_same() {
        let db = seeded_db();
        let db = Arc::new(Mutex::new(db));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));

        {
            let d = db.lock().unwrap();
            create_generation(&d, "gen-nodrift").unwrap();
            seed_checkpoint_with_mtime(&d, "file-nd", 100, 500, Some(2000));
            write_queries::insert_generation_observation(
                d.conn(),
                "gen-nodrift",
                "file-nd",
                100,
                500,        // same size
                Some(1000), // different mtime — but size equal, so no drift
                Some("ok"),
                None,
            )
            .unwrap();
        }

        let result = execute_promotion_barrier(&db, &status, "gen-nodrift").unwrap();
        assert!(
            result.promoted,
            "promotion should succeed: same size means no drift"
        );
    }

    // ── initiate_rebuild ───────────────────────────────────────────────────

    #[test]
    fn initiate_rebuild_creates_generation_and_persists_frontiers() {
        let db = seeded_db();
        seed_checkpoint(&db, "file-init", 100, 500);

        let frontier_set = initiate_rebuild(&db, "gen-init").unwrap();

        assert_eq!(frontier_set.generation_id, "gen-init");
        assert_eq!(frontier_set.frontiers.len(), 1);
        assert_eq!(frontier_set.frontiers[0].source_file_id, "file-init");

        // Generation row created in 'building' state.
        let state: String = db
            .conn()
            .query_row(
                "SELECT state FROM audit_generations WHERE generation_id = 'gen-init'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "building");

        // Frontier persisted as a generation_file_observation row.
        let obs_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM generation_file_observations WHERE generation_id = 'gen-init'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(obs_count, 1);
    }

    // ── record_new_file_during_rebuild ─────────────────────────────────────

    #[test]
    fn record_new_file_during_rebuild_writes_observation() {
        let db = seeded_db();
        create_generation(&db, "gen-rndr").unwrap();

        let frontier = RebuildFrontier {
            source_file_id: "file-rndr".to_string(),
            source_id: "src-rndr".to_string(),
            agent: "claude_code".to_string(),
            path: "/tmp/rndr.jsonl".to_string(),
            offset_bytes: 0,
            size_bytes: 200,
            last_mtime_ms: None,
        };

        record_new_file_during_rebuild(&db, "gen-rndr", &frontier).unwrap();

        // source_file_checkpoints row created.
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM source_file_checkpoints WHERE id = 'file-rndr'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // generation_file_observations row with new_file scan_status.
        let scan_status: String = db
            .conn()
            .query_row(
                "SELECT scan_status FROM generation_file_observations \
                 WHERE generation_id = 'gen-rndr' AND source_file_id = 'file-rndr'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(scan_status, "new_file");
    }

    // ── record_rebuild_diagnostic ──────────────────────────────────────────

    #[test]
    fn record_rebuild_diagnostic_inserts_diagnostic_event() {
        let db = seeded_db();
        create_generation(&db, "gen-diag").unwrap();

        record_rebuild_diagnostic(&db, "gen-diag", "warning", "slow scan detected").unwrap();

        let (severity, message): (String, String) = db
            .conn()
            .query_row(
                "SELECT severity, message FROM diagnostic_events \
                 WHERE id LIKE 'rebuild-gen-diag-warning-%' \
                 ORDER BY created_at_ms DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(severity, "warning");
        assert_eq!(message, "slow scan detected");
    }
}
