//! Generation and readiness state management.
//!
//! Owns the active generation ID and readiness transition logic.
//! SQL queries and commands live in `busytok_store::generation_queries`
//! and `busytok_store::generation_commands`.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use busytok_protocol::dto::ReadinessStateDto;
use busytok_store::Database;

use crate::status::ServiceStatusSnapshot;

pub(crate) struct GenerationManager {
    db: Arc<Mutex<Database>>,
    status: Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    active_generation_id: Mutex<Option<String>>,
}

/// Report returned by [`GenerationManager::hydrate_from_db`].
/// `repaired` is true when generation metadata was missing and was recovered
/// from existing usage_events.
pub(crate) struct GenerationHydrationReport {
    pub active_generation_id: Option<String>,
    pub repaired: bool,
    pub readiness: Option<ReadinessStateDto>,
    pub latest_event_seq: Option<i64>,
}

impl GenerationManager {
    pub fn new(
        db: Arc<Mutex<Database>>,
        status: Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    ) -> Self {
        Self {
            db,
            status,
            active_generation_id: Mutex::new(None),
        }
    }

    // ── Public API ────────────────────────────────────────────────────

    /// Get current active generation ID (in-memory cache).
    pub fn active_generation_id(&self) -> Option<String> {
        self.active_generation_id.lock().unwrap().clone()
    }

    /// Record that a generation has been created or promoted and is now
    /// the active generation. Called by scan operations after they
    /// successfully create or promote a generation.
    ///
    /// Updates both the in-memory cache and the status snapshot so that
    /// `transition_after_initial_scan()` (which reads from the snapshot)
    /// sees the correct active generation on the next call.
    ///
    /// This is a narrow semantic method — there is no public unset
    /// because clearing the active generation is a managed side effect
    /// of other GenerationManager operations.
    pub fn activate_generation(&self, id: String) -> Result<()> {
        // Acquire snapshot write first — if it fails, don't touch the cache
        // so both state stores remain consistent.
        let mut snap = self.status.try_write().map_err(|e| {
            tracing::warn!(
                event_code = "generation.activate.snapshot_lock_contention",
                error = %e,
                "failed to acquire status snapshot write lock"
            );
            anyhow::anyhow!("status snapshot lock contention: {e}")
        })?;
        *self.active_generation_id.lock().unwrap() = Some(id.clone());
        snap.active_generation_id = Some(id);
        Ok(())
    }

    /// Activate a generation and atomically apply ReadyExact readiness.
    ///
    /// Used by the initial-scan promotion path where both the active
    /// generation ID and the readiness transition must be applied in a
    /// single snapshot write. Callers that only need to record the active
    /// generation (without a readiness change) should use
    /// [`activate_generation`] instead.
    pub(crate) fn activate_and_apply_ready_exact(&self, gen_id: String) -> Result<()> {
        let mut snap = self.status.try_write().map_err(|e| {
            tracing::warn!(
                event_code = "generation.activate_ready_exact.snapshot_lock_contention",
                error = %e,
                "failed to acquire status snapshot write lock"
            );
            anyhow::anyhow!("status snapshot lock contention: {e}")
        })?;
        *self.active_generation_id.lock().unwrap() = Some(gen_id.clone());
        snap.apply_durable_transition(ReadinessStateDto::ReadyExact, Some(gen_id));
        Ok(())
    }

    /// Ensure an active generation exists for existing usage events.
    /// If none found via service_state, returns None.
    pub fn ensure_active_generation_for_existing_events(&self) -> Result<Option<String>> {
        let active = {
            let db = self.db.lock().unwrap();
            busytok_store::generation_queries::read_active_generation(db.conn())?
        };

        if let Some(ref gen_id) = active {
            *self.active_generation_id.lock().unwrap() = Some(gen_id.clone());
        }

        Ok(active)
    }

    /// Check whether the database has active degradation diagnostics
    /// that block recovery to ReadyExact.
    pub fn has_active_degradation_blocker(&self) -> Result<bool> {
        let db = self.db.lock().unwrap();
        busytok_store::generation_queries::has_blocking_degradation_diagnostic(db.conn())
    }

    /// Transition readiness after initial scan completes.
    pub async fn transition_after_initial_scan(
        &self,
        target_readiness: ReadinessStateDto,
    ) -> Result<bool> {
        let current = {
            let snap = self
                .status
                .try_read()
                .map_err(|e| anyhow::anyhow!("status snapshot lock contention: {e}"))?;
            snap.clone()
        };

        let active_generation_id = current.active_generation_id.clone().unwrap_or_default();
        if matches!(target_readiness, ReadinessStateDto::ReadyExact)
            && active_generation_id.trim().is_empty()
        {
            return Ok(false);
        }

        let can_transition = current.readiness == ReadinessStateDto::Starting
            || (current.readiness == ReadinessStateDto::ReadyDegraded
                && target_readiness == ReadinessStateDto::ReadyExact);
        if !can_transition {
            return Ok(false);
        }

        let readiness_value = match target_readiness {
            ReadinessStateDto::Starting => "starting",
            ReadinessStateDto::Rebuilding => "rebuilding",
            ReadinessStateDto::ReadyDegraded => "ready_degraded",
            ReadinessStateDto::ReadyExact => "ready_exact",
        };

        let status_db = self.db.clone();
        let target_str = readiness_value.to_string();
        let gen_id = active_generation_id.clone();
        let transitioned = tokio::task::spawn_blocking(move || {
            let db = status_db.lock().unwrap();
            busytok_store::generation_commands::transition_readiness(
                db.conn(),
                &target_str,
                &gen_id,
            )
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

        if transitioned {
            let mut snap = self.status.write().await;
            let can = snap.readiness == ReadinessStateDto::Starting
                || (snap.readiness == ReadinessStateDto::ReadyDegraded
                    && target_readiness == ReadinessStateDto::ReadyExact);
            if can {
                let gen_id = snap.active_generation_id.clone();
                snap.apply_durable_transition(target_readiness, gen_id);
                Ok(true)
            } else {
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }

    /// Mark ready_exact if the given generation is promoted and valid.
    pub async fn mark_ready_exact_if_generation_valid(&self, gen_id: &str) -> Result<bool> {
        let gen_id = gen_id.to_string();
        let gen_id_for_db = gen_id.clone();
        let db = self.db.clone();
        let promoted = tokio::task::spawn_blocking(move || {
            let db = db.lock().unwrap();
            if !busytok_store::generation_queries::generation_is_promoted_active(
                db.conn(),
                &gen_id_for_db,
            )? {
                return Ok::<_, anyhow::Error>(false);
            }
            busytok_store::generation_commands::persist_ready_exact_for_generation(
                db.conn(),
                &gen_id_for_db,
            )?;
            Ok(true)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

        if promoted {
            *self.active_generation_id.lock().unwrap() = Some(gen_id.clone());
            let mut snap = self.status.write().await;
            snap.apply_durable_transition(ReadinessStateDto::ReadyExact, Some(gen_id));
        }

        Ok(promoted)
    }

    /// Hydrate generation/readiness state from persisted service_state.
    pub fn hydrate_from_db(&self, timezone: &str) -> Result<GenerationHydrationReport> {
        let db = self.db.lock().unwrap();
        let recovered =
            busytok_store::generation_commands::recover_missing_generation_metadata(&db, timezone)
                .context("failed to recover generation metadata during status hydration")?;
        let store_row = busytok_store::read_queries::read_service_state(db.conn())
            .context("failed to read service_state for status hydration")?;

        let readiness: ReadinessStateDto = match store_row.readiness.as_deref() {
            Some("ready_exact") => ReadinessStateDto::ReadyExact,
            Some("ready_degraded") => ReadinessStateDto::ReadyDegraded,
            Some("rebuilding") => ReadinessStateDto::Rebuilding,
            Some("starting") | None => ReadinessStateDto::Starting,
            Some(other) => {
                tracing::warn!(
                    event_code = "generation.hydrate.unexpected_readiness",
                    readiness_value = other,
                    "unexpected readiness value in service_state, \
                     falling back to Starting"
                );
                ReadinessStateDto::Starting
            }
        };

        let runtime_row = crate::status::ServiceStateRow {
            latest_event_seq: store_row.latest_event_seq,
            readiness: readiness.clone(),
            active_generation_id: store_row.active_generation_id.clone(),
            writer_queue_depth: store_row.writer_queue_depth,
            aggregate_lag_ms: store_row.aggregate_lag_ms,
        };

        {
            let mut snap = self
                .status
                .try_write()
                .map_err(|e| anyhow::anyhow!("status snapshot lock contention: {e}"))?;
            snap.hydrate_from_service_state_row(&runtime_row);
            if let Some(ref result) = recovered {
                let recovered_readiness = to_readiness_state_from_store(&result.readiness);
                snap.apply_durable_transition(
                    recovered_readiness,
                    Some(result.generation_id.clone()),
                );
                if result.repaired {
                    snap.invalidate_chip_data();
                }
            }
        }

        let active_gen = {
            let snap = self
                .status
                .try_read()
                .map_err(|e| anyhow::anyhow!("status snapshot lock contention: {e}"))?;
            snap.active_generation_id.clone()
        };
        *self.active_generation_id.lock().unwrap() = active_gen.clone();

        let final_readiness = recovered
            .as_ref()
            .map(|r| to_readiness_state_from_store(&r.readiness))
            .unwrap_or(readiness);

        tracing::info!(
            event_code = "generation.hydrate.complete",
            readiness = ?final_readiness,
            generation_id = ?active_gen,
            "generation/readiness state hydrated from persisted service_state"
        );

        Ok(GenerationHydrationReport {
            active_generation_id: active_gen.clone(),
            repaired: recovered.as_ref().map(|r| r.repaired).unwrap_or(false),
            readiness: Some(final_readiness),
            latest_event_seq: runtime_row.latest_event_seq,
        })
    }
}

/// Map store-layer readiness enum to protocol DTO.
fn to_readiness_state_from_store(
    store: &busytok_store::generation_commands::StoreReadiness,
) -> ReadinessStateDto {
    match store {
        busytok_store::generation_commands::StoreReadiness::ReadyExact => {
            ReadinessStateDto::ReadyExact
        }
        busytok_store::generation_commands::StoreReadiness::ReadyDegraded => {
            ReadinessStateDto::ReadyDegraded
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_manager_starts_with_no_active_generation() {
        let db = Database::open_in_memory().expect("db");
        let db = Arc::new(Mutex::new(db));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let manager = GenerationManager::new(db, status);
        assert!(manager.active_generation_id().is_none());
    }

    #[test]
    fn activate_generation_updates_cache_and_snapshot() {
        let db = Database::open_in_memory().expect("db");
        let db = Arc::new(Mutex::new(db));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));

        let manager = GenerationManager::new(db, Arc::clone(&status));
        manager
            .activate_generation("gen-test".to_string())
            .expect("activate");
        assert_eq!(manager.active_generation_id(), Some("gen-test".to_string()));
        // Snapshot must also reflect the active generation.
        let snap = status.try_read().unwrap();
        assert_eq!(snap.active_generation_id, Some("gen-test".to_string()));
    }

    #[test]
    fn hydrate_from_db_on_fresh_db_returns_empty_report() {
        let db = Database::open_in_memory().expect("db");
        let db = Arc::new(Mutex::new(db));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let manager = GenerationManager::new(db, status);
        let report = manager.hydrate_from_db("UTC").expect("hydrate");
        assert!(report.active_generation_id.is_none());
        assert!(!report.repaired);
    }

    /// transition_after_initial_scan returns false when no active
    /// generation exists — it cannot transition to ReadyExact.
    #[tokio::test]
    async fn transition_to_ready_exact_without_active_gen_returns_false() {
        let db = Database::open_in_memory().expect("db");
        let db = Arc::new(Mutex::new(db));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let manager = GenerationManager::new(db, status);
        let result = manager
            .transition_after_initial_scan(ReadinessStateDto::ReadyExact)
            .await
            .expect("transition should not error");
        assert!(!result, "no active gen → ReadyExact must return false");
    }

    /// mark_ready_exact_if_generation_valid returns false when the
    /// generation_id doesn't exist in audit_generations.
    #[tokio::test]
    async fn mark_ready_exact_nonexistent_gen_returns_false() {
        let db = Database::open_in_memory().expect("db");
        let db = Arc::new(Mutex::new(db));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));

        let manager = GenerationManager::new(db, status);
        let result = manager
            .mark_ready_exact_if_generation_valid("nonexistent-gen")
            .await
            .expect("call should not error");
        assert!(
            !result,
            "nonexistent generation should not be marked ready_exact"
        );
    }

    /// mark_ready_exact_if_generation_valid returns true when the
    /// generation exists in audit_generations as promoted + active.
    #[tokio::test]
    async fn mark_ready_exact_promoted_active_gen_returns_true() {
        let db = Database::open_in_memory().expect("db");
        // Seed a promoted active generation.
        let now = busytok_domain::now_ms();
        db.conn()
            .execute(
                "INSERT INTO audit_generations \
             (generation_id, state, started_at_ms, promoted_at_ms, is_active, \
              created_at_ms, updated_at_ms) \
             VALUES ('gen-promoted', 'promoted', ?1, ?1, 1, ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("seed");

        let db = Arc::new(Mutex::new(db));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));

        let manager = GenerationManager::new(db, status);
        let result = manager
            .mark_ready_exact_if_generation_valid("gen-promoted")
            .await
            .expect("call should not error");
        assert!(
            result,
            "promoted active generation should be marked ready_exact"
        );
    }

    /// hydrate_from_db on an empty DB results in snapshot readiness = Starting.
    #[tokio::test]
    async fn hydrate_from_db_on_empty_db_sets_snapshot_to_starting() {
        let db = Database::open_in_memory().expect("db");
        let db = Arc::new(Mutex::new(db));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let manager = GenerationManager::new(db, status);
        let report = manager.hydrate_from_db("UTC").expect("hydrate");
        assert!(!report.repaired, "empty DB should not trigger repair");

        let snap = manager.status.try_read().unwrap();
        assert_eq!(snap.readiness, ReadinessStateDto::Starting);
    }

    /// When an active generation exists but blocking diagnostics prevent
    /// recovery to ReadyExact, the report and snapshot must both reflect
    /// ReadyDegraded — not the raw store_row readiness.
    #[test]
    fn hydrate_from_db_blocking_diagnostic_yields_degraded_report_and_snapshot() {
        let db = Database::open_in_memory().expect("db");
        let now = busytok_domain::now_ms();

        // Seed a promoted active generation.
        db.conn()
            .execute(
                "INSERT INTO audit_generations \
                 (generation_id, state, started_at_ms, promoted_at_ms, is_active, \
                  created_at_ms, updated_at_ms) \
                 VALUES ('gen-1', 'promoted', ?1, ?1, 1, ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("seed generation");

        // Seed service_state as starting (the raw DB value).
        db.conn()
            .execute(
                "INSERT INTO service_state \
                 (id, readiness, active_generation_id, writer_queue_depth, \
                  aggregate_lag_ms, updated_at_ms) \
                 VALUES (1, 'starting', 'gen-1', 0, 0, ?1)",
                rusqlite::params![now],
            )
            .expect("seed state");

        // Insert a blocking degradation diagnostic.
        db.conn()
            .execute(
                "INSERT INTO diagnostic_events \
                 (id, severity, code, message, happened_at_ms, created_at_ms) \
                 VALUES ('d1', 'error', 'generation_drift', 'test', ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("seed diagnostic");

        let db = Arc::new(Mutex::new(db));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let manager = GenerationManager::new(db, status);
        let report = manager.hydrate_from_db("UTC").expect("hydrate");

        // Report readiness must be ReadyDegraded, not Starting (the raw store value).
        assert_eq!(
            report.readiness,
            Some(ReadinessStateDto::ReadyDegraded),
            "blocking diagnostic → report readiness must be ReadyDegraded"
        );

        // Snapshot must also be ReadyDegraded.
        let snap = manager.status.try_read().unwrap();
        assert_eq!(
            snap.readiness,
            ReadinessStateDto::ReadyDegraded,
            "blocking diagnostic → snapshot readiness must be ReadyDegraded"
        );
    }
}
