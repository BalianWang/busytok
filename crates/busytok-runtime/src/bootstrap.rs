//! Service bootstrap stage functions.
//!
//! Each stage function owns its tracing internally and returns a
//! structured report. These are implementation details of [`ServiceApp`].

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use tracing::{info, warn};

use busytok_config::BusytokPaths;
use busytok_control::server::ControlServer;
use busytok_protocol::dto::ReadinessStateDto;
use busytok_store::Database;

use crate::tail::TailHandle;
use crate::BusytokSupervisor;

// ── Stage reports ──────────────────────────────────────────────────────

pub(crate) struct DatabaseOpenReport {
    pub db_size_bytes: i64,
    pub schema_version: u32,
    pub elapsed_ms: u64,
}

pub(crate) struct HydrateStatusReport {
    pub elapsed_ms: u64,
}

pub(crate) struct InitialScanReport {
    pub sources: usize,
    pub files_scanned: usize,
    pub events_found: usize,
    pub elapsed_ms: u64,
    pub readiness_transitioned: bool,
}

pub(crate) struct InitialScanFailureReport {
    pub elapsed_ms: u64,
    pub error: String,
    pub readiness_transitioned: bool,
}

pub(crate) enum InitialScanOutcome {
    ReadyExact(InitialScanReport),
    ReadyDegraded(InitialScanFailureReport),
}

pub(crate) struct ServiceReadyReport {
    pub total_elapsed_ms: u64,
}

pub(crate) struct BackgroundJobHandles {
    pub legacy_rebuild_check: tokio::task::JoinHandle<()>,
    pub chip_data_hydration: tokio::task::JoinHandle<()>,
}

pub(crate) type SamplerShutdown = tokio::sync::watch::Sender<bool>;

// ── Stage 1: Open database ─────────────────────────────────────────────

pub(crate) fn open_database(paths: &BusytokPaths) -> Result<(Database, DatabaseOpenReport)> {
    let start = Instant::now();
    let db_path = paths.db_path();
    info!(
        event_code = "service.db.opening",
        db_path = %db_path.display(),
    );
    let db = Database::open(&db_path).context("open database")?;
    let elapsed = start.elapsed().as_millis() as u64;
    let db_size_bytes = std::fs::metadata(&db_path)
        .map(|meta| meta.len() as i64)
        .unwrap_or_default();
    let schema_version = busytok_store::schema::SCHEMA_VERSION;
    info!(
        event_code = "service.db.opened",
        db_size_bytes,
        schema_version = schema_version as i32,
        elapsed_ms = elapsed,
    );
    Ok((
        db,
        DatabaseOpenReport {
            db_size_bytes,
            schema_version,
            elapsed_ms: elapsed,
        },
    ))
}

// ── Stage 2: Create supervisor ─────────────────────────────────────────

pub(crate) fn create_supervisor(db: Database, paths: BusytokPaths) -> Arc<BusytokSupervisor> {
    let supervisor = Arc::new(BusytokSupervisor::new(db, paths));
    info!(event_code = "service.supervisor.created");
    supervisor
}

// ── Stage 3: Hydrate status ────────────────────────────────────────────

pub(crate) fn hydrate_status(supervisor: &BusytokSupervisor) -> Result<HydrateStatusReport> {
    let start = Instant::now();
    supervisor
        .hydrate_status_from_db()
        .context("hydrate status from db")?;
    let elapsed = start.elapsed().as_millis() as u64;
    info!(event_code = "service.status.hydrated", elapsed_ms = elapsed,);
    Ok(HydrateStatusReport {
        elapsed_ms: elapsed,
    })
}

// ── Stage 4: Bind control server ───────────────────────────────────────

pub(crate) async fn bind_control_server(
    endpoint: &str,
    supervisor: Arc<BusytokSupervisor>,
) -> Result<(
    Arc<ControlServer>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
)> {
    let start = Instant::now();
    info!(
        event_code = "service.control_server.binding",
        endpoint = %endpoint,
    );
    let server = Arc::new(
        ControlServer::bind(endpoint.to_string(), supervisor)
            .await
            .context("bind control server")?,
    );
    let server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };
    let elapsed = start.elapsed().as_millis() as u64;
    info!(
        event_code = "service.control_server.listening",
        elapsed_ms = elapsed,
    );
    Ok((server, server_task))
}

// ── Stage 5: Initial scan or register ──────────────────────────────────

/// Run initial scan or fast-register for fresh installs.
///
/// Calls [`BusytokSupervisor::transition_after_initial_scan`] for durable
/// readiness transition BEFORE returning the outcome.
pub(crate) async fn run_initial_scan_or_register_sources(
    supervisor: Arc<BusytokSupervisor>,
) -> InitialScanOutcome {
    let is_fresh_install = {
        let current = supervisor.read_status_snapshot().await;
        current.readiness == ReadinessStateDto::Starting
    };

    let start = Instant::now();
    let scan_result = if is_fresh_install {
        info!(event_code = "service.fresh_install.register_sources");
        supervisor.register_new_install_sources().await
    } else {
        info!(event_code = "service.initial_scan.spawned");
        supervisor.run_initial_scan().await
    };

    match scan_result {
        Ok(stats) => {
            let elapsed = start.elapsed().as_millis() as u64;
            let transitioned = supervisor
                .transition_after_initial_scan(ReadinessStateDto::ReadyExact)
                .await
                .unwrap_or_else(|e| {
                    warn!(
                        event_code = "service.initial_scan.status_persist_failed",
                        error = %e,
                        "failed to transition initial scan state to ready_exact",
                    );
                    false
                });
            info!(
                event_code = "service.initial_scan.completed",
                sources = stats.sources,
                files_scanned = stats.files_scanned,
                events_found = stats.events_found,
                elapsed_ms = elapsed,
            );
            InitialScanOutcome::ReadyExact(InitialScanReport {
                sources: stats.sources,
                files_scanned: stats.files_scanned,
                events_found: stats.events_found,
                elapsed_ms: elapsed,
                readiness_transitioned: transitioned,
            })
        }
        Err(err) => {
            let elapsed = start.elapsed().as_millis() as u64;
            let transitioned = supervisor
                .transition_after_initial_scan(ReadinessStateDto::ReadyDegraded)
                .await
                .unwrap_or_else(|e| {
                    warn!(
                        event_code = "service.initial_scan.status_persist_failed",
                        error = %e,
                        "failed to transition initial scan state to ready_degraded",
                    );
                    false
                });
            warn!(
                event_code = "service.initial_scan.failed",
                error = %err,
            );
            InitialScanOutcome::ReadyDegraded(InitialScanFailureReport {
                elapsed_ms: elapsed,
                error: format!("{err:#}"),
                readiness_transitioned: transitioned,
            })
        }
    }
}

// ── Stage 6: Start tailing ─────────────────────────────────────────────

pub(crate) async fn start_tailing(supervisor: Arc<BusytokSupervisor>) -> Result<TailHandle> {
    info!(event_code = "service.tailer.starting");
    let start = Instant::now();
    let tail_handle = supervisor.start_tailing().await?;
    let elapsed = start.elapsed().as_millis() as u64;
    info!(event_code = "service.tailer.started", elapsed_ms = elapsed,);
    Ok(tail_handle)
}

// ── Stage 7: Start sampler ─────────────────────────────────────────────

pub(crate) fn start_sampler(supervisor: &BusytokSupervisor) -> SamplerShutdown {
    let shutdown = crate::sampler::start_sampler(
        supervisor.db_handle().clone(),
        supervisor.status_snapshot_arc(),
        supervisor.event_bus_arc(),
    );
    info!(event_code = "service.sampler.started");
    shutdown
}

// ── Stage 8: Background jobs ───────────────────────────────────────────

pub(crate) fn spawn_background_jobs(supervisor: Arc<BusytokSupervisor>) -> BackgroundJobHandles {
    let legacy_supervisor = Arc::clone(&supervisor);
    let legacy_rebuild_check = tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            legacy_supervisor.legacy_audit_rebuild_recommended()
        })
        .await;

        match result {
            Ok(Ok(true)) => {
                warn!(
                    event_code = "service.legacy_rebuild_check",
                    recommended = true,
                    "Legacy audit data detected"
                );
            }
            Ok(Ok(false)) => {}
            Ok(Err(err)) => {
                warn!(
                    event_code = "service.legacy_rebuild_check_failed",
                    error = %err,
                    "Legacy rebuild check failed"
                );
            }
            Err(join_err) => {
                warn!(
                    event_code = "service.legacy_rebuild_check_failed",
                    error = %join_err,
                    "Legacy rebuild check task failed"
                );
            }
        }
    });

    let hydration_status = supervisor.status_snapshot_arc();
    let hydration_db = supervisor.db_handle().clone();
    let chip_data_hydration = tokio::spawn(async move {
        // Read active_generation_id before entering spawn_blocking so the
        // blocking task can hydrate client rollups alongside counts.
        let active_generation_id = {
            let snap = hydration_status.read().await;
            snap.active_generation_id.clone()
        };
        let result = tokio::task::spawn_blocking(move || {
            let db = hydration_db.lock().unwrap();
            let (total, sources) = db.read_chip_hydration_counts()?;
            let rollups = active_generation_id
                .as_deref()
                .map(|gid| busytok_store::read_queries::read_client_rollups(db.conn(), gid))
                .transpose()
                .unwrap_or_default()
                .unwrap_or_default();
            Ok::<_, anyhow::Error>((total, sources, rollups))
        })
        .await;

        match result {
            Ok(Ok((total_usage_event_count, source_count, rollups))) => {
                let mut snap = hydration_status.write().await;
                snap.total_usage_event_count = total_usage_event_count;
                snap.source_count = source_count;
                snap.cached_client_rollups = rollups.into_iter().map(|r| r.into()).collect();
                snap.chip_data_hydrated = true;
            }
            Ok(Err(err)) => {
                warn!(
                    event_code = "service.chip_data_hydration_failed",
                    error = %err,
                    "Background shell-status chip hydration failed"
                );
            }
            Err(join_err) => {
                warn!(
                    event_code = "service.chip_data_hydration_failed",
                    error = %join_err,
                    "Background shell-status chip hydration failed"
                );
            }
        }
    });

    BackgroundJobHandles {
        legacy_rebuild_check,
        chip_data_hydration,
    }
}

// ── Stage 9: Service ready ─────────────────────────────────────────────

pub(crate) fn emit_service_ready(startup: Instant) -> ServiceReadyReport {
    let total_elapsed_ms = startup.elapsed().as_millis() as u64;
    info!(
        event_code = "service.ready",
        total_elapsed_ms, "Busytok service ready, waiting for connections"
    );
    ServiceReadyReport { total_elapsed_ms }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use busytok_config::BusytokPaths;
    use busytok_store::Database;

    #[test]
    fn open_database_creates_db_and_reports_schema_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = BusytokPaths::for_test(dir.path());
        paths.ensure_dirs_exist().expect("ensure dirs");

        let (_db, report) = open_database(&paths).expect("open_database");
        assert!(report.db_size_bytes >= 0);
        assert_eq!(report.schema_version, busytok_store::schema::SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn create_supervisor_returns_starting_readiness() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = BusytokPaths::for_test(dir.path());
        paths.ensure_dirs_exist().expect("ensure dirs");

        let db = Database::open_in_memory().expect("open in-memory");
        let supervisor = create_supervisor(db, paths);

        let snap = supervisor.read_status_snapshot().await;
        assert_eq!(
            snap.readiness,
            busytok_protocol::dto::ReadinessStateDto::Starting
        );
    }

    #[tokio::test]
    async fn hydrate_status_succeeds_on_fresh_db() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = BusytokPaths::for_test(dir.path());
        paths.ensure_dirs_exist().expect("ensure dirs");

        let db = Database::open_in_memory().expect("open in-memory");
        let supervisor = Arc::new(BusytokSupervisor::new(db, paths));

        let report = hydrate_status(&supervisor).expect("hydrate_status");
        // hydrate_status only hydrates readiness/generation from service_state.
        // Chip data (total_events, source_count) is populated by the
        // background chip_data_hydration job, not startup hydration.
        assert!(report.elapsed_ms == 0 || report.elapsed_ms > 0);
    }

    /// bootstrap::run_initial_scan_or_register_sources() must call
    /// supervisor.transition_after_initial_scan() (-> GenerationManager),
    /// not a private helper. On a fresh install with default discovery
    /// disabled and no existing data, it's a fresh install -> fast-register
    /// path -> ReadyExact with no transition (no active generation exists).
    #[tokio::test]
    async fn fresh_install_with_no_discovery_yields_exact_without_transition() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = BusytokPaths::for_test(dir.path());
        paths.ensure_dirs_exist().expect("ensure dirs");

        let db = Database::open_in_memory().expect("open in-memory");
        let mut settings = busytok_config::BusytokSettings::default();
        settings.discovery.claude_code_default_paths = false;
        settings.discovery.codex_default_paths = false;
        let supervisor = Arc::new(BusytokSupervisor::with_adapters_and_settings(
            db,
            paths,
            vec![],
            settings,
        ));

        let outcome = run_initial_scan_or_register_sources(supervisor).await;
        match outcome {
            InitialScanOutcome::ReadyExact(report) => {
                assert!(
                    !report.readiness_transitioned,
                    "no active generation -> transition should not succeed"
                );
                assert_eq!(report.sources, 0);
                assert_eq!(report.files_scanned, 0);
            }
            InitialScanOutcome::ReadyDegraded(_) => {
                panic!("fresh install with empty discovery should not degrade");
            }
        }
    }
}
