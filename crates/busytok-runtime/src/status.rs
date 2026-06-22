//! Service status manager — in-memory snapshot for `shell.status` fast path.
//!
//! The snapshot is updated by the writer actor (queue depth, aggregate lag,
//! latest event seq) and by the scanner/rebuilder (progress). It is read
//! by `shell_status()` without touching the database, providing a sub-1ms
//! response. The snapshot is persisted to the database on a coarse cadence
//! (every 30 seconds) and immediately on warning/critical threshold transitions.

use std::collections::VecDeque;

use busytok_protocol::dto::{LiveSampleDto, ReadinessStateDto, ScanProgressDto};

/// Maximum number of transient samples to retain in the ring buffer.
/// 450 buckets covers 15 minutes at 2-second intervals.
pub const TRANSIENT_RING_BUFFER_CAPACITY: usize = 450;

/// In-memory accumulator for the current 2s throughput bucket.
///
/// Owned by the writer actor (sole mutator). Read by the sampler
/// in exact mode to avoid querying SQLite every 2s.
#[derive(Debug, Clone, Default)]
pub struct LiveBucket {
    pub bucket_start_ms: i64,
    pub total_tokens: i64,
    pub cost_usd: f64,
    pub event_count: i64,
}

/// Cached per-client rollup used by shell.status chip generation.
#[derive(Debug, Clone)]
pub struct CachedClientRollup {
    pub client_kind: String,
    pub active_source_count: i64,
    pub event_count: i64,
}

impl From<busytok_store::read_models::ClientRollupRow> for CachedClientRollup {
    fn from(r: busytok_store::read_models::ClientRollupRow) -> Self {
        Self {
            client_kind: r.client_kind,
            active_source_count: r.active_source_count,
            event_count: r.event_count,
        }
    }
}

/// In-memory snapshot of service-level observability state.
///
/// Updated by the writer actor on each batch commit and by the scanner on
/// progress events. Read by `shell_status()` without a DB round-trip.
///
/// Counter fields (`total_usage_event_count`, `source_count`) are hydrated
/// from the database on first access and then kept up-to-date incrementally
/// by the writer actor. The `chip_data_hydrated` flag controls whether
/// hydration has occurred.
#[derive(Debug, Clone)]
pub struct ServiceStatusSnapshot {
    /// Current readiness state of the read plane.
    pub readiness: ReadinessStateDto,
    /// Active generation ID for the current rebuild cycle, if any.
    pub active_generation_id: Option<String>,
    /// Latest event sequence number seen by the writer.
    pub latest_event_seq: Option<i64>,
    /// Scanner/rebuilder progress, if a scan or rebuild is in flight.
    pub progress: Option<ScanProgressDto>,
    /// Depth of the writer command queue (committed but not yet flushed).
    pub writer_queue_depth: i64,
    /// Estimated aggregate lag between event occurrence and store visibility, in ms.
    pub aggregate_lag_ms: i64,
    /// Human-readable summary of subscription bridge connectivity.
    pub subscription_bridge_connectivity: Option<String>,
    /// Ring buffer of recently published transient samples for `live.window`
    /// bootstrapping and reconnect catch-up.
    pub transient_ring_buffer: VecDeque<LiveSampleDto>,
    /// In-memory 2s bucket for live sampler (exact mode only).
    pub live_bucket: LiveBucket,

    // ── Chip-data counters (hydrated from DB on first access) ──────────
    /// Total number of usage events in the store.
    pub total_usage_event_count: i64,
    /// Number of active (non-removed) log sources.
    pub source_count: i64,
    /// Whether the chip-data counters have been hydrated from the DB.
    pub chip_data_hydrated: bool,
    /// Per-client rollup rows cached from DB on first access.
    pub cached_client_rollups: Vec<CachedClientRollup>,
    /// Cached scan state to avoid COUNT(*) on every shell.status call.
    pub cached_scan_state: Option<String>,
    /// Timestamp when cached_scan_state was last refreshed.
    pub scan_state_cached_at_ms: Option<i64>,
}

impl Default for ServiceStatusSnapshot {
    fn default() -> Self {
        Self {
            readiness: ReadinessStateDto::Starting,
            active_generation_id: None,
            latest_event_seq: None,
            progress: None,
            writer_queue_depth: 0,
            aggregate_lag_ms: 0,
            subscription_bridge_connectivity: None,
            transient_ring_buffer: VecDeque::with_capacity(TRANSIENT_RING_BUFFER_CAPACITY),
            live_bucket: LiveBucket::default(),
            total_usage_event_count: 0,
            source_count: 0,
            chip_data_hydrated: false,
            cached_client_rollups: Vec::new(),
            cached_scan_state: None,
            scan_state_cached_at_ms: None,
        }
    }
}

impl ServiceStatusSnapshot {
    /// Create a new snapshot with default (Starting) readiness.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a transient sample into the ring buffer, evicting the oldest
    /// entry if capacity is exceeded.
    pub fn push_transient_sample(&mut self, sample: LiveSampleDto) {
        if self.transient_ring_buffer.len() >= TRANSIENT_RING_BUFFER_CAPACITY {
            self.transient_ring_buffer.pop_front();
        }
        self.transient_ring_buffer.push_back(sample);
    }

    /// Return a snapshot of the transient ring buffer.
    pub fn transient_samples(&self) -> Vec<LiveSampleDto> {
        self.transient_ring_buffer.iter().cloned().collect()
    }

    /// Apply a runtime health update from the writer actor.
    ///
    /// This is called at a coarse interval (every ~30 s) or immediately
    /// when a warning/critical threshold is crossed.
    pub fn apply_runtime_health_update(
        &mut self,
        writer_queue_depth: i64,
        aggregate_lag_ms: i64,
        latest_event_seq: Option<i64>,
    ) {
        self.writer_queue_depth = writer_queue_depth;
        self.aggregate_lag_ms = aggregate_lag_ms;
        if latest_event_seq.is_some() {
            self.latest_event_seq = latest_event_seq;
        }
    }

    /// Apply a progress update from the scanner or rebuilder.
    pub fn apply_progress_update(&mut self, progress: ScanProgressDto) {
        self.progress = Some(progress);
    }

    /// Mark the progress phase as complete (clears progress).
    pub fn clear_progress(&mut self) {
        self.progress = None;
    }

    /// Apply a durable state transition (e.g. readiness, generation_id).
    pub fn apply_durable_transition(
        &mut self,
        readiness: ReadinessStateDto,
        active_generation_id: Option<String>,
    ) {
        self.readiness = readiness;
        self.active_generation_id = active_generation_id;
    }

    /// Hydrate snapshot fields from a service state row in the database.
    ///
    /// Called during startup to seed the snapshot with persisted state.
    pub fn hydrate_from_service_state_row(&mut self, row: &ServiceStateRow) {
        self.latest_event_seq = row.latest_event_seq;
        self.readiness = row.readiness;
        self.active_generation_id = row.active_generation_id.clone();
        self.writer_queue_depth = row.writer_queue_depth;
        self.aggregate_lag_ms = row.aggregate_lag_ms;
    }

    /// Hydrate chip-data counters from the database.
    ///
    /// Called once on first `shell_status` access after startup or after a
    /// rebuild to avoid COUNT queries on the hot path. Subsequent calls
    /// are a no-op until `chip_data_hydrated` is reset.
    pub fn hydrate_chip_data(&mut self, conn: &rusqlite::Connection) {
        if self.chip_data_hydrated {
            return;
        }
        self.total_usage_event_count = conn
            .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
            .unwrap_or(0);
        self.source_count = conn
            .query_row(
                "SELECT COUNT(*) FROM log_sources WHERE status != 'removed'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if let Some(ref gen_id) = self.active_generation_id {
            self.cached_client_rollups =
                busytok_store::read_queries::read_client_rollups(conn, gen_id)
                    .map(|rows| rows.into_iter().map(|r| r.into()).collect())
                    .unwrap_or_default();
        }
        self.chip_data_hydrated = true;
    }

    /// Reset chip-data hydration flag so counters are re-read from DB on
    /// next access. Called after a rebuild replaces all event data.
    pub fn invalidate_chip_data(&mut self) {
        self.chip_data_hydrated = false;
        self.cached_client_rollups = Vec::new();
    }
}

/// A lightweight row representation for service state persisted in the
/// service_state table. Used during startup hydration.
///
/// Fresh-install defaults are defined by `busytok_store::read_models::ServiceStateRow::default()`.
/// This struct is only ever built in `supervisor::hydrate_status_from_db` by
/// mapping from the store-side row — it has no standalone `Default` to avoid
/// duplicating the authoritative fallback values.
#[derive(Debug, Clone)]
pub struct ServiceStateRow {
    pub latest_event_seq: Option<i64>,
    pub readiness: ReadinessStateDto,
    pub active_generation_id: Option<String>,
    pub writer_queue_depth: i64,
    pub aggregate_lag_ms: i64,
}
