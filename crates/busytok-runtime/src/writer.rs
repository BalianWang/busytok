//! Bounded writer actor — the single owner of the SQLite write connection.
//!
//! Receives `WriteCommand` variants via a bounded `tokio::sync::mpsc` channel
//! and processes them sequentially. All database mutations go through this
//! actor so that the write connection is never contended.
//!
//! Each command struct carries all data needed for the write — no extra DB
//! lookups are performed inside the actor loop. DB operations (rusqlite) are
//! wrapped in `tokio::task::spawn_blocking` to avoid blocking the async runtime.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info, warn};

use busytok_aggregator::{rebuild_model_summaries, rebuild_projects, rebuild_sessions};
use busytok_config::BusytokSettings;
use busytok_domain::{
    now_ms, NormalizedUsageEvent, OperationalDiagnosticEvent, ToolEvent, UsageWritePolicy,
};
use busytok_events::{AppEvent, AppEventBus, PublishedEvent};
use busytok_protocol::dto::canonical_invalidation_scopes;
use busytok_store::outbox_queries;
use busytok_store::write_queries;
use busytok_store::{CodexTokenSnapshotRow, Database, LogSourceRow};

use crate::aggregates;
use crate::status::ServiceStatusSnapshot;

// ── Threshold constants ─────────────────────────────────────────────────────

/// Queue depth thresholds that trigger diagnostics.
const QUEUE_WARNING_THRESHOLD: i64 = 64;
const QUEUE_CRITICAL_THRESHOLD: i64 = 96;

/// Aggregate lag thresholds in milliseconds that trigger diagnostics.
const LAG_WARNING_THRESHOLD_MS: i64 = 5_000;
const LAG_CRITICAL_THRESHOLD_MS: i64 = 30_000;

/// Coarse checkpoint interval: persist writer metrics to service_state.
const METRICS_CHECKPOINT_INTERVAL_SECS: u64 = 30;

/// Maximum event count before a pending batch is flushed.
const BATCH_SIZE: usize = 100;
/// Maximum time pending batches wait before being flushed.
const FLUSH_INTERVAL_SECS: u64 = 2;

// ── WriteCommand taxonomy ────────────────────────────────────────────────────

/// Unified command enum for all writer-actor operations.
///
/// Each variant wraps a self-contained command struct. The writer loop pattern-
/// matches on the variant and dispatches to the appropriate handler.
#[derive(Debug)]
pub enum WriteCommand {
    /// Insert a batch of events arriving from the live tailer.
    TailBatch(TailBatchCommand),

    /// Insert a batch of events during a historical rebuild.
    RebuildBatch(RebuildBatchCommand),

    /// Create an audit generation in `building` state.
    GenerationCreate(GenerationCreateCommand),

    /// Enqueue tail replay rows for deferred replay.
    TailReplayBatch(TailReplayBatchCommand),

    /// Record a single event into the tail replay queue.
    RecordTailReplay(RecordTailReplayCommand),

    /// Apply pending replay rows to a target generation.
    ApplyReplayToTarget(ApplyReplayCommand),

    /// Advance a source file checkpoint.
    ProgressCheckpoint(ProgressCheckpointCommand),

    /// Promote a generation: seal the old generation, activate the new one.
    PromotionBarrier(PromotionBarrierCommand),

    /// Record a diagnostic event.
    DiagnosticWrite(DiagnosticWriteCommand),

    /// Persist a settings mutation.
    SettingsWrite(SettingsWriteCommand),

    /// Upsert a discovered log source.
    LogSourceUpsert(LogSourceUpsertCommand),

    /// Replace the realtime summary materialization.
    RealtimeSummaryReplace(RealtimeSummaryReplaceCommand),

    /// Rebuild aggregate rollup tables from canonical usage events.
    RebuildRollups(RebuildRollupsCommand),

    /// Reset failed file checkpoints so tailing can retry them.
    ResetFailedCheckpoints(ResetFailedCheckpointsCommand),

    /// Barrier command used by callers that need all prior writes committed.
    Flush(FlushCommand),

    /// Flush metrics and stop the actor after all prior commands complete.
    Shutdown(ShutdownCommand),
}

// ── Command structs ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TailBatchCommand {
    pub source_id: String,
    pub source_file_id: Option<String>,
    pub source_file_agent: String,
    pub source_file_path: String,
    pub source_file_inode: Option<String>,
    pub events: Vec<NormalizedUsageEvent>,
    pub tool_events: Vec<ToolEvent>,
    pub diagnostic_events: Vec<OperationalDiagnosticEvent>,
    pub codex_snapshots: Vec<CodexTokenSnapshotRow>,
    pub generation_id: String,
    pub checkpoint_offset: Option<u64>,
    pub write_policy: UsageWritePolicy,
}

#[derive(Debug, Clone)]
pub struct RebuildBatchCommand {
    pub source_id: String,
    pub source_file_id: Option<String>,
    pub source_file_agent: String,
    pub source_file_path: String,
    pub source_file_inode: Option<String>,
    pub events: Vec<NormalizedUsageEvent>,
    pub tool_events: Vec<ToolEvent>,
    pub diagnostic_events: Vec<OperationalDiagnosticEvent>,
    pub codex_snapshots: Vec<CodexTokenSnapshotRow>,
    pub generation_id: String,
    pub checkpoint_offset: Option<u64>,
    pub is_final_batch: bool,
    pub write_policy: UsageWritePolicy,
}

#[derive(Debug, Clone)]
pub struct GenerationCreateCommand {
    pub generation_id: String,
}

#[derive(Debug, Clone)]
pub struct TailReplayBatchCommand {
    pub rows: Vec<busytok_store::write_queries::TailReplayEnqueue>,
}

#[derive(Debug, Clone)]
pub struct RecordTailReplayCommand {
    pub source_file_id: String,
    pub event_seq: i64,
    pub event_data_json: String,
}

#[derive(Debug, Clone)]
pub struct ApplyReplayCommand {
    pub target_generation_id: String,
    pub source_file_id: Option<String>,
    pub limit: i64,
}

#[derive(Debug, Clone)]
pub struct ProgressCheckpointCommand {
    pub file_id: String,
    pub source_id: String,
    pub agent: String,
    pub path: String,
    pub inode: Option<String>,
    pub offset_bytes: i64,
    pub size_bytes: i64,
    pub last_mtime_ms: Option<i64>,
    pub state: String,
}

#[derive(Debug, Clone)]
pub struct PromotionBarrierCommand {
    pub from_generation_id: String,
    pub to_generation_id: String,
}

#[derive(Debug, Clone)]
pub struct DiagnosticWriteCommand {
    pub source_id: String,
    pub code: String,
    pub message: String,
    pub severity: String,
    pub details_json: Option<String>,
}

#[derive(Debug)]
pub struct SettingsWriteCommand {
    pub key: String,
    pub value_json: String,
    /// Oneshot channel to signal that the settings have been persisted.
    /// The writer sends `Ok(())` on success or an error message on failure.
    pub respond_tx: oneshot::Sender<Result<(), String>>,
}

#[derive(Debug, Clone)]
pub struct LogSourceUpsertCommand {
    pub row: LogSourceRow,
}

#[derive(Debug, Clone)]
pub struct RealtimeSummaryReplaceCommand {
    pub entries: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct RebuildRollupsCommand {
    pub timezone: String,
    pub respond_tx: oneshot::Sender<Result<(), String>>,
}

#[derive(Debug)]
pub struct ResetFailedCheckpointsCommand {
    pub respond_tx: oneshot::Sender<Result<usize, String>>,
}

#[derive(Debug)]
pub struct FlushCommand {
    pub respond_tx: oneshot::Sender<Result<(), String>>,
}

#[derive(Debug)]
pub struct ShutdownCommand {
    pub respond_tx: oneshot::Sender<Result<(), String>>,
}

// ── WriterHandle ─────────────────────────────────────────────────────────────

/// Handle for sending commands to the writer actor.
///
/// Cloning this handle creates another sender to the same channel.
#[derive(Debug, Clone)]
pub struct WriterHandle {
    tx: mpsc::Sender<WriteCommand>,
}

impl WriterHandle {
    /// Send a command to the writer actor.
    ///
    /// Blocks asynchronously if the channel is full (bounded backpressure).
    pub async fn send(
        &self,
        cmd: WriteCommand,
    ) -> Result<(), mpsc::error::SendError<WriteCommand>> {
        self.tx.send(cmd).await
    }

    /// Try to send a command without blocking.
    ///
    /// Returns `Err(TrySendError::Full(cmd))` if the channel is full.
    pub fn try_send(
        &self,
        cmd: WriteCommand,
    ) -> Result<(), mpsc::error::TrySendError<WriteCommand>> {
        self.tx.try_send(cmd)
    }

    /// Current queue depth (approximate; the channel's `max_capacity - capacity`).
    pub fn queue_depth(&self) -> usize {
        self.tx.max_capacity() - self.tx.capacity()
    }

    /// Maximum channel capacity.
    pub fn capacity(&self) -> usize {
        self.tx.max_capacity()
    }

    /// Wait until all commands sent before this call have been committed.
    pub async fn flush(&self) -> Result<()> {
        let (respond_tx, respond_rx) = oneshot::channel();
        self.send(WriteCommand::Flush(FlushCommand { respond_tx }))
            .await
            .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))?;
        match respond_rx.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => anyhow::bail!("writer flush failed: {e}"),
            Err(_) => anyhow::bail!("writer flush response channel dropped"),
        }
    }

    /// Request a graceful writer shutdown after all prior commands complete.
    pub async fn shutdown(&self) -> Result<()> {
        let (respond_tx, respond_rx) = oneshot::channel();
        self.send(WriteCommand::Shutdown(ShutdownCommand { respond_tx }))
            .await
            .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))?;
        match respond_rx.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => anyhow::bail!("writer shutdown failed: {e}"),
            Err(_) => anyhow::bail!("writer shutdown response channel dropped"),
        }
    }

    /// Rebuild aggregate rollups through the single writer actor.
    pub async fn rebuild_rollups(&self, timezone: String) -> Result<()> {
        let (respond_tx, respond_rx) = oneshot::channel();
        self.send(WriteCommand::RebuildRollups(RebuildRollupsCommand {
            timezone,
            respond_tx,
        }))
        .await
        .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))?;
        match respond_rx.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => anyhow::bail!("writer rollup rebuild failed: {e}"),
            Err(_) => anyhow::bail!("writer rollup rebuild response channel dropped"),
        }
    }

    /// Reset failed checkpoints through the single writer actor.
    pub async fn reset_failed_checkpoints(&self) -> Result<usize> {
        let (respond_tx, respond_rx) = oneshot::channel();
        self.send(WriteCommand::ResetFailedCheckpoints(
            ResetFailedCheckpointsCommand { respond_tx },
        ))
        .await
        .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))?;
        match respond_rx.await {
            Ok(Ok(updated)) => Ok(updated),
            Ok(Err(e)) => anyhow::bail!("writer checkpoint reset failed: {e}"),
            Err(_) => anyhow::bail!("writer checkpoint reset response channel dropped"),
        }
    }
}

// ── Default capacity ─────────────────────────────────────────────────────────

/// Default channel capacity for the writer actor.
pub const DEFAULT_WRITER_CAPACITY: usize = 128;

// ── Spawn helpers ────────────────────────────────────────────────────────────

/// Spawn the writer actor (requires an active Tokio runtime).
///
/// Returns a `WriterHandle` for sending commands and a `JoinHandle` for
/// awaiting the actor's completion.
///
/// The actor processes commands in a loop. When all `WriterHandle` clones
/// are dropped, the channel closes, and the actor drains remaining commands
/// before exiting.
///
/// # Panics
///
/// Panics if no Tokio runtime is active. For contexts where a runtime may
/// not be available (e.g. synchronous unit tests that construct a supervisor),
/// use [`try_spawn_writer`] instead.

// ── Batch helpers ─────────────────────────────────────────────────────────────

fn pending_event_count(pending: &[WriteCommand]) -> usize {
    pending
        .iter()
        .map(|cmd| match cmd {
            WriteCommand::TailBatch(c) => c.events.len(),
            WriteCommand::RebuildBatch(c) => c.events.len(),
            _ => 0,
        })
        .sum()
}

async fn accumulate_live_bucket(
    status: &Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    event_bus: &Arc<AppEventBus>,
    inserted_events: &[NormalizedUsageEvent],
) {
    if inserted_events.is_empty() {
        return;
    }
    let now = busytok_domain::now_ms();
    let current_window = (now / 2000) * 2000;

    let mut snap = status.write().await;
    let bucket = &mut snap.live_bucket;

    if bucket.bucket_start_ms != 0
        && bucket.bucket_start_ms != current_window
        && bucket.event_count > 0
    {
        let dto = busytok_protocol::dto::LiveSampleDto {
            bucket_start_ms: bucket.bucket_start_ms,
            tokens_per_sec: bucket.total_tokens as f64 / 2.0,
            cost_per_sec: if bucket.cost_usd > 0.0 {
                Some(bucket.cost_usd / 2.0)
            } else {
                None
            },
            events_per_sec: bucket.event_count as f64 / 2.0,
        };
        let _ = event_bus.publish_ephemeral(AppEvent::LiveSample {
            bucket_start_ms: dto.bucket_start_ms,
            tokens_per_sec: dto.tokens_per_sec,
            cost_per_sec: dto.cost_per_sec,
            events_per_sec: dto.events_per_sec,
            transient: false,
        });
        debug!(
            window = bucket.bucket_start_ms,
            tokens = bucket.total_tokens,
            "live bucket published on window cross"
        );
        *bucket = crate::status::LiveBucket::default();
    }

    bucket.bucket_start_ms = current_window;
    for event in inserted_events {
        bucket.total_tokens += event.total_tokens;
        bucket.cost_usd += event.cost_usd.unwrap_or(0.0);
        bucket.event_count += 1;
    }
}

async fn flush_pending_batches(
    db: &Arc<Mutex<Database>>,
    status: &Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    event_bus: &Arc<AppEventBus>,
    settings: &Arc<Mutex<BusytokSettings>>,
    pending: &mut Vec<WriteCommand>,
) {
    if pending.is_empty() {
        return;
    }

    // Group pending batches by generation_id so events are never attributed
    // to the wrong generation. In steady state all batches share the same
    // generation; during rebuild, tail is paused so there should be exactly
    // one generation. Log a warning if mixed — this indicates a bug in the
    // caller that should not silently corrupt data.
    let mut groups: HashMap<String, Vec<WriteCommand>> = HashMap::new();
    for cmd in pending.drain(..) {
        let gen = match &cmd {
            WriteCommand::TailBatch(c) => c.generation_id.clone(),
            WriteCommand::RebuildBatch(c) => c.generation_id.clone(),
            _ => unreachable!(),
        };
        groups.entry(gen).or_default().push(cmd);
    }

    if groups.len() > 1 {
        warn!(
            count = groups.len(),
            generations = ?groups.keys().collect::<Vec<_>>(),
            "pending batches span multiple generations — flushing each group separately"
        );
    }

    for (gen_id, group) in groups {
        flush_single_generation(db, status, event_bus, settings, group, &gen_id).await;
    }
}

async fn flush_single_generation(
    db: &Arc<Mutex<Database>>,
    status: &Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    event_bus: &Arc<AppEventBus>,
    settings: &Arc<Mutex<BusytokSettings>>,
    group: Vec<WriteCommand>,
    generation_id: &str,
) {
    let mut all_events: Vec<NormalizedUsageEvent> = Vec::new();
    let mut all_policies: Vec<busytok_domain::UsageWritePolicy> = Vec::new();
    let mut all_tools: Vec<ToolEvent> = Vec::new();
    let mut all_diags: Vec<OperationalDiagnosticEvent> = Vec::new();
    let mut all_snapshots: Vec<CodexTokenSnapshotRow> = Vec::new();
    let mut ckpts: HashMap<String, (String, String, String, Option<String>, i64, i64)> =
        HashMap::new();
    let mut source_ids: Vec<String> = Vec::new();
    let mut seen_sources: HashSet<String> = HashSet::new();
    let mut source_file_agent = String::new();
    let mut source_id_for_publish = String::new();
    let mut is_final_rebuild = false;
    let mut first = true;

    for cmd in group {
        match cmd {
            WriteCommand::TailBatch(c) => {
                if first {
                    source_file_agent = c.source_file_agent.clone();
                    source_id_for_publish = c.source_id.clone();
                    first = false;
                }
                // Always update agent for the last-seen (used by RebuildBatch below).
                // For metadata, the first batch's values identify the primary source.
                let src = c.source_id.clone();
                if seen_sources.insert(src.clone()) {
                    source_ids.push(src.clone());
                }
                let event_count = c.events.len();
                let write_policy = c.write_policy;
                all_events.extend(c.events);
                all_policies.extend(std::iter::repeat(write_policy).take(event_count));
                all_tools.extend(c.tool_events);
                all_diags.extend(c.diagnostic_events);
                all_snapshots.extend(c.codex_snapshots);
                if let Some(fid) = c.source_file_id {
                    ckpts.insert(
                        fid,
                        (
                            src,
                            c.source_file_agent,
                            c.source_file_path,
                            c.source_file_inode,
                            c.checkpoint_offset.unwrap_or(0) as i64,
                            c.checkpoint_offset.unwrap_or(0) as i64,
                        ),
                    );
                }
            }
            WriteCommand::RebuildBatch(c) => {
                if first {
                    source_file_agent = c.source_file_agent.clone();
                    source_id_for_publish = c.source_id.clone();
                    first = false;
                }
                is_final_rebuild = c.is_final_batch;
                let src = c.source_id.clone();
                if seen_sources.insert(src.clone()) {
                    source_ids.push(src.clone());
                }
                let event_count = c.events.len();
                let write_policy = c.write_policy;
                all_events.extend(c.events);
                all_policies.extend(std::iter::repeat(write_policy).take(event_count));
                all_tools.extend(c.tool_events);
                all_diags.extend(c.diagnostic_events);
                all_snapshots.extend(c.codex_snapshots);
                if let Some(fid) = c.source_file_id {
                    ckpts.insert(
                        fid,
                        (
                            src,
                            c.source_file_agent,
                            c.source_file_path,
                            c.source_file_inode,
                            c.checkpoint_offset.unwrap_or(0) as i64,
                            c.checkpoint_offset.unwrap_or(0) as i64,
                        ),
                    );
                }
            }
            _ => unreachable!(),
        }
    }

    let total = all_events.len();
    let file_count = ckpts.len();
    let source_count = source_ids.len();

    let db2 = Arc::clone(db);
    let gen_id = generation_id.to_string();
    let agent = source_file_agent.clone();
    let src_id = source_id_for_publish.clone();
    let gen_id_post = gen_id.clone();
    let src_id_post = src_id.clone();
    let current_tz = busytok_domain::ReportingTimezone::parse(&settings.lock().unwrap().timezone)
        .unwrap_or_else(|e| {
            tracing::warn!("failed to parse timezone in writer flush: {e}, falling back to UTC");
            busytok_domain::ReportingTimezone::utc()
        });

    let result = tokio::task::spawn_blocking(move || {
        let db = db2.lock().unwrap();
        let conn = db.conn();
        let tx = conn.unchecked_transaction()?;

        let outcome = write_queries::upsert_usage_events_dedup_aware(
            &*tx,
            &all_events,
            &all_policies,
            &gen_id,
        )?;
        let effective_events = outcome.effective_events;
        let inserted = outcome.inserted;
        let replaced = outcome.replaced;
        let dropped = outcome.dropped;

        insert_supplementary_events(&tx, &all_tools, &all_diags, &all_snapshots)?;
        write_queries::update_materialized_aggregates_from_events(&*tx, &effective_events, &gen_id)?;
        aggregates::apply_event_batch_aggregates(&*tx, &effective_events, &gen_id)?;

        // Maintain daily_usage rollup for IANA timezone queries.
        // Failure here MUST abort the transaction — otherwise IANA read
        // paths diverge from generation-scoped fast-path reads.
        if !effective_events.is_empty() {
            if let Err(e) = write_queries::upsert_daily_usage_for_events(&*tx, &effective_events, &current_tz, &gen_id) {
                tracing::error!(error = %e, "daily_usage upsert failed, rolling back transaction");
                return Err(anyhow::anyhow!("daily_usage upsert failed: {e}"));
            }
        }

        for (fid, (sid, ag, path, inode, off, sz)) in &ckpts {
            write_queries::upsert_source_file_checkpoint(&*tx, fid, sid, ag, path, inode.as_deref(), *off, *sz, None, "active", None)?;
            write_queries::upsert_log_file_checkpoint(&*tx, fid, sid, ag, path, inode.as_deref(), *off, *sz, None, "active", None)?;
        }

        write_queries::refresh_source_summaries_for_inserted_events_tx(&tx, &gen_id, &effective_events, &source_ids)?;

        // Invalidate on inserts OR replacements (a sidechain parent displacing a
        // replay changes totals even when no new row appears).
        let mutated = inserted > 0 || replaced > 0;
        let (seq_start, seq_end) = if mutated {
            outbox_queries::allocate_event_sequence_batch(&*tx, inserted + 2)?
        } else { (0, -1) };

        if mutated {
            let wm = now_ms();
            let scopes = canonical_invalidation_scopes();
            let mut entries: Vec<(i64, String)> = Vec::new();
            for i in 0..inserted {
                let seq = seq_start + i as i64;
                let env = PublishedEvent::durable(
                    AppEvent::UsageEventInserted { event_id: format!("tail-{}-{}", src_id, seq), agent: agent.clone() },
                    seq, gen_id.clone(), wm, scopes.clone(),
                );
                if let Ok(json) = serde_json::to_string(&env) { entries.push((seq, json)); }
            }
            if !entries.is_empty() { let _ = outbox_queries::append_durable_outbox_events(&*tx, &entries); }
        }

        tx.commit()?;
        info!(inserted, replaced, dropped, total, files = file_count, sources = source_count, is_final = is_final_rebuild, gen = %gen_id, "batch flushed");
        Ok::<_, anyhow::Error>((inserted, replaced, seq_start, seq_end, effective_events))
    }).await;

    match result {
        Ok(Ok((inserted, replaced, seq_start, seq_end, effective_events))) => {
            if seq_end > 0 {
                let mut snap = status.write().await;
                snap.latest_event_seq = Some(seq_end);
                snap.total_usage_event_count += inserted;
                snap.chip_data_hydrated = false;
            }
            accumulate_live_bucket(status, event_bus, &effective_events).await;
            // Invalidate on inserts OR replacements: a sidechain parent
            // displacing a replay changes totals without growing the row count.
            if inserted > 0 || replaced > 0 {
                let wm = now_ms();
                let scopes = canonical_invalidation_scopes();
                let di_seq = seq_start + inserted as i64;
                let _ = event_bus.publish(PublishedEvent::durable(
                    AppEvent::DataInvalidated {
                        datasets: scopes.clone(),
                    },
                    di_seq,
                    gen_id_post.clone(),
                    wm,
                    scopes.clone(),
                ));
                let _ = event_bus.publish(PublishedEvent::durable(
                    AppEvent::SummaryUpdated {
                        keys_updated: vec!["usage".into(), "aggregates".into()],
                    },
                    seq_end,
                    gen_id_post,
                    wm,
                    scopes,
                ));
                let _ = event_bus.publish_ephemeral(AppEvent::ScanProgress {
                    source_id: src_id_post,
                    files_scanned: file_count as u64,
                    events_ingested: inserted as u64,
                });
            }
        }
        Ok(Err(e)) => error!(error = %e, "batch flush transaction failed"),
        Err(e) => error!(error = %e, "batch flush spawn_blocking join error"),
    }
}

/// Prune usage_events older than 24h for the active generation.
///
/// Reads the active generation from `status`, then spawns a blocking task
/// to delete old events. On success, decrements `total_usage_event_count`.
async fn prune_old_usage_events(
    db: &Arc<Mutex<Database>>,
    status: &Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
) {
    let active_gen = status.read().await.active_generation_id.clone();
    if let Some(ref gen_id) = active_gen {
        let db = Arc::clone(db);
        let gen_id = gen_id.clone();
        let gen_label = gen_id.clone();
        match tokio::task::spawn_blocking(move || {
            let db = db.lock().unwrap();
            write_queries::prune_usage_events(db.conn(), &gen_id)
        })
        .await
        {
            Ok(Ok(deleted)) => {
                if deleted > 0 {
                    debug!(deleted, gen = %gen_label, "pruned old usage events");
                    let mut snap = status.write().await;
                    snap.total_usage_event_count =
                        snap.total_usage_event_count.saturating_sub(deleted);
                }
            }
            Ok(Err(e)) => warn!(error = %e, gen = %gen_label, "usage event pruning query failed"),
            Err(join_err) => {
                warn!(error = ?join_err, gen = %gen_label, "usage event pruning spawn failed")
            }
        }
    }
}

pub fn spawn_writer(
    db: Arc<Mutex<Database>>,
    status: Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    event_bus: Arc<AppEventBus>,
    settings: Arc<Mutex<BusytokSettings>>,
    capacity: usize,
) -> (WriterHandle, tokio::task::JoinHandle<()>) {
    let (handle, join_opt) = try_spawn_writer(db, status, event_bus, settings, capacity);
    let join = join_opt
        .expect("spawn_writer requires a Tokio runtime; use try_spawn_writer for sync contexts");
    (handle, join)
}

/// Try to spawn the writer actor, returning `None` for the join handle when
/// no Tokio runtime is active.
///
/// This is the sync-safe variant used by `BusytokSupervisor::new()`, which
/// may be called from either async or sync contexts.
pub fn try_spawn_writer(
    db: Arc<Mutex<Database>>,
    status: Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    event_bus: Arc<AppEventBus>,
    settings: Arc<Mutex<BusytokSettings>>,
    capacity: usize,
) -> (WriterHandle, Option<tokio::task::JoinHandle<()>>) {
    let (tx, mut rx) = mpsc::channel::<WriteCommand>(capacity);
    let handle = WriterHandle { tx };

    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            let join = rt.spawn(async move {
                info!(capacity, "writer actor started");

                let mut last_metrics_checkpoint = now_ms();
                let mut prev_queue_state: Option<String> = None;
                let mut prev_lag_state: Option<String> = None;
                let mut pending_errors: Vec<String> = Vec::new();
                let mut pending_batches: Vec<WriteCommand> = Vec::new();

                loop {
                    match tokio::time::timeout(Duration::from_secs(FLUSH_INTERVAL_SECS), rx.recv()).await {
                        Ok(Some(cmd)) => {
                            debug!(?cmd, "writer actor received command");
                            match cmd {
                                WriteCommand::TailBatch(_) | WriteCommand::RebuildBatch(_) => {
                                    pending_batches.push(cmd);
                                    if pending_event_count(&pending_batches) >= BATCH_SIZE {
                                        flush_pending_batches(&db, &status, &event_bus, &settings, &mut pending_batches).await;
                                    }
                                }
                                WriteCommand::Flush(cmd) => {
                                    flush_pending_batches(&db, &status, &event_bus, &settings, &mut pending_batches).await;
                                    let result = drain_pending_errors(&mut pending_errors);
                                    let _ = cmd.respond_tx.send(result);
                                }
                                WriteCommand::Shutdown(cmd) => {
                                    flush_pending_batches(&db, &status, &event_bus, &settings, &mut pending_batches).await;
                                    persist_metrics_checkpoint(&db, &status).await;
                                    let result = drain_pending_errors(&mut pending_errors);
                                    let _ = cmd.respond_tx.send(result);
                                    info!("writer actor shutdown requested");
                                    break;
                                }
                                other => {
                                    flush_pending_batches(&db, &status, &event_bus, &settings, &mut pending_batches).await;
                                    if let Err(e) = dispatch_command(&db, &status, &event_bus, &settings, other).await {
                                        error!(error = %e, "writer actor failed to dispatch command");
                                        pending_errors.push(e.to_string());
                                    }
                                }
                            }

                            let queue_depth = rx.len() as i64;
                            update_status_snapshot(&status, queue_depth).await;

                            emit_threshold_diagnostics(
                                &event_bus, &db, &status, queue_depth,
                                &mut prev_queue_state, &mut prev_lag_state,
                            ).await;

                            let now = now_ms();
                            if now - last_metrics_checkpoint > (METRICS_CHECKPOINT_INTERVAL_SECS as i64 * 1000) {
                                persist_metrics_checkpoint(&db, &status).await;
                                prune_old_usage_events(&db, &status).await;
                                if let Err(e) = db.lock().unwrap().checkpoint_wal() {
                                    warn!("WAL checkpoint failed: {e}");
                                }
                                last_metrics_checkpoint = now;
                            }
                        }
                        Ok(None) => {
                            flush_pending_batches(&db, &status, &event_bus, &settings, &mut pending_batches).await;
                            info!("writer actor channel closed, shutting down");
                            break;
                        }
                        Err(_elapsed) => {
                            if !pending_batches.is_empty() {
                                flush_pending_batches(&db, &status, &event_bus, &settings, &mut pending_batches).await;
                            }
                        }
                    }
                }

                persist_metrics_checkpoint(&db, &status).await;
                info!("writer actor stopped");
            });
            (handle, Some(join))
        }
        Err(_) => {
            // No Tokio runtime — don't spawn (sync test context).
            warn!("no tokio runtime active; writer actor not spawned (sync context)");
            (handle, None)
        }
    }
}

fn drain_pending_errors(pending_errors: &mut Vec<String>) -> std::result::Result<(), String> {
    if pending_errors.is_empty() {
        return Ok(());
    }
    let count = pending_errors.len();
    let message = if count == 1 {
        pending_errors.remove(0)
    } else {
        let joined = pending_errors
            .iter()
            .enumerate()
            .map(|(idx, err)| format!("{}. {}", idx + 1, err))
            .collect::<Vec<_>>()
            .join("; ");
        pending_errors.clear();
        format!("{count} writer errors since previous barrier: {joined}")
    };
    Err(message)
}

/// Spawn a writer actor for testing backpressure with a custom capacity.
///
/// Opens an in-memory database, creates a status snapshot and event bus,
/// then spawns the writer actor. Requires an active Tokio runtime.
pub fn spawn_test_writer_with_capacity(
    capacity: usize,
) -> (WriterHandle, tokio::task::JoinHandle<()>) {
    let db = Database::open_in_memory().expect("failed to open in-memory db");
    let db = Arc::new(Mutex::new(db));
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let settings = Arc::new(Mutex::new(BusytokSettings::default()));

    spawn_writer(db, status, event_bus, settings, capacity)
}

// ── Command dispatch ─────────────────────────────────────────────────────────

/// Route a command to the appropriate handler.
async fn dispatch_command(
    db: &Arc<Mutex<Database>>,
    status: &Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    event_bus: &Arc<AppEventBus>,
    settings: &Arc<Mutex<BusytokSettings>>,
    cmd: WriteCommand,
) -> Result<()> {
    match cmd {
        WriteCommand::TailBatch(_) | WriteCommand::RebuildBatch(_) => {
            unreachable!(
                "TailBatch/RebuildBatch are handled by flush_pending_batches in the actor loop"
            )
        }
        WriteCommand::GenerationCreate(c) => handle_generation_create(db, c).await,
        WriteCommand::TailReplayBatch(c) => handle_tail_replay_batch(db, c).await,
        WriteCommand::RecordTailReplay(c) => handle_record_tail_replay(db, c).await,
        WriteCommand::ApplyReplayToTarget(c) => handle_apply_replay_to_target(db, c).await,
        WriteCommand::ProgressCheckpoint(c) => handle_progress_checkpoint(db, c).await,
        WriteCommand::PromotionBarrier(c) => handle_promotion_barrier(db, status, c).await,
        WriteCommand::DiagnosticWrite(c) => handle_diagnostic_write(db, event_bus, c).await,
        WriteCommand::SettingsWrite(c) => handle_settings_write(settings, c).await,
        WriteCommand::LogSourceUpsert(c) => handle_log_source_upsert(db, c).await,
        WriteCommand::RealtimeSummaryReplace(c) => handle_realtime_summary_replace(db, c).await,
        WriteCommand::RebuildRollups(c) => handle_rebuild_rollups(db, event_bus, status, c).await,
        WriteCommand::ResetFailedCheckpoints(c) => handle_reset_failed_checkpoints(db, c).await,
        WriteCommand::Flush(_) | WriteCommand::Shutdown(_) => {
            unreachable!("Flush and Shutdown are handled by the writer actor loop")
        }
    }
}

// ── Command handlers ─────────────────────────────────────────────────────────

fn insert_supplementary_events(
    tx: &rusqlite::Transaction<'_>,
    tool_events: &[ToolEvent],
    diagnostic_events: &[OperationalDiagnosticEvent],
    codex_snapshots: &[CodexTokenSnapshotRow],
) -> Result<()> {
    for tool in tool_events {
        tx.execute(
            "INSERT OR IGNORE INTO tool_events (\
                id, agent, source_file_id, source_path, source_line, \
                source_offset_start, source_offset_end, session_id, \
                message_id, tool_name, status, timestamp_ms, \
                project_hash, created_at_ms\
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            rusqlite::params![
                tool.id,
                tool.agent.as_str(),
                tool.source_file_id,
                tool.source_path,
                tool.source_line as i64,
                tool.source_offset_start as i64,
                tool.source_offset_end as i64,
                tool.session_id,
                tool.message_id,
                tool.tool_name,
                tool.status,
                tool.timestamp_ms,
                tool.project_hash,
                tool.created_at_ms,
            ],
        )?;
    }

    for diag in diagnostic_events {
        tx.execute(
            "INSERT OR REPLACE INTO diagnostic_events (\
                id, agent, source_id, source_file_id, source_path, source_line, \
                severity, code, message, details_json, happened_at_ms, created_at_ms\
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                diag.id,
                diag.agent.as_ref().map(|a| a.as_str()),
                diag.source_id,
                diag.source_file_id,
                diag.source_path.as_deref(),
                diag.source_line,
                diag.severity,
                diag.category,
                diag.message,
                diag.detail_json,
                diag.happened_at_ms,
                diag.created_at_ms,
            ],
        )?;
    }

    for snap in codex_snapshots {
        tx.execute(
            "INSERT INTO codex_token_snapshots (\
                id, source_file_id, source_line, source_offset_start, source_offset_end, \
                session_id, turn_id, token_event_ordinal, \
                input_tokens, cached_input_tokens, output_tokens, reasoning_tokens, total_tokens, \
                model, raw_usage_json, emitted_event_id, created_at_ms, updated_at_ms\
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18) \
            ON CONFLICT(id) DO UPDATE SET \
                input_tokens = excluded.input_tokens, \
                cached_input_tokens = excluded.cached_input_tokens, \
                output_tokens = excluded.output_tokens, \
                reasoning_tokens = excluded.reasoning_tokens, \
                total_tokens = excluded.total_tokens, \
                model = excluded.model, \
                raw_usage_json = excluded.raw_usage_json, \
                emitted_event_id = excluded.emitted_event_id, \
                updated_at_ms = excluded.updated_at_ms",
            rusqlite::params![
                snap.id,
                snap.source_file_id,
                snap.source_line,
                snap.source_offset_start,
                snap.source_offset_end,
                snap.session_id,
                snap.turn_id,
                snap.token_event_ordinal,
                snap.input_tokens,
                snap.cached_input_tokens,
                snap.output_tokens,
                snap.reasoning_tokens,
                snap.total_tokens,
                snap.model,
                snap.raw_usage_json,
                snap.emitted_event_id,
                snap.created_at_ms,
                snap.updated_at_ms,
            ],
        )?;
    }

    Ok(())
}

async fn handle_generation_create(
    db: &Arc<Mutex<Database>>,
    cmd: GenerationCreateCommand,
) -> Result<()> {
    let db = Arc::clone(db);

    tokio::task::spawn_blocking(move || {
        let db = db.lock().unwrap();
        crate::rebuild::create_generation(&db, &cmd.generation_id)?;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    Ok(())
}

async fn handle_tail_replay_batch(
    db: &Arc<Mutex<Database>>,
    cmd: TailReplayBatchCommand,
) -> Result<()> {
    let db = Arc::clone(db);

    tokio::task::spawn_blocking(move || {
        let db = db.lock().unwrap();
        let conn = db.conn();
        write_queries::enqueue_tail_replay_rows(conn, &cmd.rows)?;
        debug!(count = cmd.rows.len(), "tail replay rows enqueued");
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    Ok(())
}

async fn handle_record_tail_replay(
    db: &Arc<Mutex<Database>>,
    cmd: RecordTailReplayCommand,
) -> Result<()> {
    let db = Arc::clone(db);

    tokio::task::spawn_blocking(move || {
        let db = db.lock().unwrap();
        let conn = db.conn();
        write_queries::enqueue_tail_replay_rows(
            conn,
            &[busytok_store::write_queries::TailReplayEnqueue {
                source_file_id: cmd.source_file_id,
                event_seq: cmd.event_seq,
                event_data_json: cmd.event_data_json,
            }],
        )?;
        debug!("single tail replay row recorded");
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    Ok(())
}

async fn handle_apply_replay_to_target(
    db: &Arc<Mutex<Database>>,
    cmd: ApplyReplayCommand,
) -> Result<()> {
    let db = Arc::clone(db);

    tokio::task::spawn_blocking(move || {
        let db = db.lock().unwrap();
        let conn = db.conn();
        let applied = write_queries::apply_replay_rows_to_target_generation(
            conn,
            &cmd.target_generation_id,
            cmd.source_file_id.as_deref(),
            cmd.limit,
        )?;
        info!(applied, "replay rows applied to target generation");
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    Ok(())
}

async fn handle_progress_checkpoint(
    db: &Arc<Mutex<Database>>,
    cmd: ProgressCheckpointCommand,
) -> Result<()> {
    let db = Arc::clone(db);

    tokio::task::spawn_blocking(move || {
        let db = db.lock().unwrap();
        let conn = db.conn();
        let tx = conn.unchecked_transaction()?;
        write_queries::upsert_source_file_checkpoint(
            &*tx,
            &cmd.file_id,
            &cmd.source_id,
            &cmd.agent,
            &cmd.path,
            cmd.inode.as_deref(),
            cmd.offset_bytes,
            cmd.size_bytes,
            cmd.last_mtime_ms,
            &cmd.state,
            None,
        )?;
        write_queries::upsert_log_file_checkpoint(
            &*tx,
            &cmd.file_id,
            &cmd.source_id,
            &cmd.agent,
            &cmd.path,
            cmd.inode.as_deref(),
            cmd.offset_bytes,
            cmd.size_bytes,
            cmd.last_mtime_ms,
            &cmd.state,
            None,
        )?;
        if let Some(generation_id) = write_queries::current_active_generation_id(&tx)? {
            write_queries::refresh_source_summaries_for_sources_tx(
                &tx,
                &generation_id,
                &[cmd.source_id.as_str()],
            )?;
        }
        tx.commit()?;
        debug!(file_id = %cmd.file_id, offset = cmd.offset_bytes, "checkpoint updated");
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    Ok(())
}

async fn handle_promotion_barrier(
    db: &Arc<Mutex<Database>>,
    status: &Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    cmd: PromotionBarrierCommand,
) -> Result<()> {
    // Delegate to the full barrier in rebuild.rs — this ensures replay
    // drain, consistency check, and atomic promotion are always applied,
    // regardless of whether the barrier is triggered via the writer
    // channel or called directly from the supervisor.
    let result = crate::rebuild::execute_promotion_barrier(db, status, &cmd.to_generation_id)?;

    if !result.promoted {
        anyhow::bail!(
            "promotion barrier refused: {}",
            result
                .degradation_reason
                .unwrap_or_else(|| "unknown".to_string())
        );
    }

    Ok(())
}

async fn handle_diagnostic_write(
    db: &Arc<Mutex<Database>>,
    event_bus: &Arc<AppEventBus>,
    cmd: DiagnosticWriteCommand,
) -> Result<()> {
    let db = Arc::clone(db);
    let event_bus = Arc::clone(event_bus);

    // Monotonic counter for unique diagnostic event IDs across the actor lifetime.
    static DIAG_COUNTER: AtomicU64 = AtomicU64::new(0);

    // Clone cmd fields before they're moved into the closure.
    let severity = cmd.severity.clone();
    let message = cmd.message.clone();
    let source_id = cmd.source_id.clone();

    tokio::task::spawn_blocking(move || {
        let db = db.lock().unwrap();
        let now = now_ms();
        let seq = DIAG_COUNTER.fetch_add(1, Ordering::Relaxed);
        let diag_source_id = cmd.source_id.clone();

        let diag = busytok_domain::OperationalDiagnosticEvent {
            id: format!("diag-{}-{}-{}", diag_source_id, now, seq),
            agent: None,
            source_id: Some(diag_source_id.clone()),
            source_file_id: None,
            source_path: None,
            source_line: None,
            category: cmd.code,
            severity: cmd.severity,
            message: cmd.message,
            detail_json: cmd.details_json,
            happened_at_ms: now,
            created_at_ms: now,
        };

        let conn = db.conn();
        let tx = conn.unchecked_transaction()?;
        write_queries::record_diagnostic_event(&tx, &diag)?;
        if let Some(generation_id) = write_queries::current_active_generation_id(&tx)? {
            write_queries::refresh_source_summaries_for_sources_tx(
                &tx,
                &generation_id,
                &[diag_source_id.as_str()],
            )?;
        }
        tx.commit()?;

        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    // Publish event for severity > info
    if severity == "error" || severity == "warning" {
        let _ = event_bus.publish_ephemeral(AppEvent::Error {
            message,
            source: Some(source_id),
        });
    }

    Ok(())
}

async fn handle_settings_write(
    settings: &Arc<Mutex<BusytokSettings>>,
    cmd: SettingsWriteCommand,
) -> Result<()> {
    let result = {
        let mut settings = settings.lock().unwrap();
        // Parse the value_json and apply to the appropriate settings field.
        match cmd.key.as_str() {
            "timezone" => {
                settings.timezone = cmd.value_json.clone();
            }
            "week_starts_on" => {
                if let Ok(v) = cmd.value_json.parse::<u8>() {
                    settings.week_starts_on = v;
                } else {
                    let _ = cmd.respond_tx.send(Err(format!(
                        "invalid week_starts_on value: {}",
                        cmd.value_json
                    )));
                    return Ok(());
                }
            }
            other => {
                let _ = cmd
                    .respond_tx
                    .send(Err(format!("unsupported settings key: {other}")));
                return Ok(());
            }
        }
        Ok::<(), String>(())
    };

    // Send the response back.
    let _ = cmd.respond_tx.send(result.map_err(|e| e));
    Ok(())
}

async fn handle_log_source_upsert(
    db: &Arc<Mutex<Database>>,
    cmd: LogSourceUpsertCommand,
) -> Result<()> {
    let db = Arc::clone(db);

    tokio::task::spawn_blocking(move || {
        let db = db.lock().unwrap();
        let conn = db.conn();
        let tx = conn.unchecked_transaction()?;
        write_queries::upsert_log_source(&tx, &cmd.row)?;
        if let Some(generation_id) = write_queries::current_active_generation_id(&tx)? {
            write_queries::refresh_source_summaries_for_sources_tx(
                &tx,
                &generation_id,
                &[cmd.row.id.as_str()],
            )?;
        }
        tx.commit()?;
        debug!(source_id = %cmd.row.id, "log source upserted");
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    Ok(())
}

async fn handle_realtime_summary_replace(
    db: &Arc<Mutex<Database>>,
    cmd: RealtimeSummaryReplaceCommand,
) -> Result<()> {
    let db = Arc::clone(db);

    tokio::task::spawn_blocking(move || {
        let db = db.lock().unwrap();
        db.replace_realtime_summary(&cmd.entries)?;
        debug!(entries = cmd.entries.len(), "realtime summary replaced");
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    Ok(())
}

async fn handle_rebuild_rollups(
    db: &Arc<Mutex<Database>>,
    event_bus: &Arc<AppEventBus>,
    status: &Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    cmd: RebuildRollupsCommand,
) -> Result<()> {
    let db = Arc::clone(db);
    let timezone = cmd.timezone.clone();
    let tz_for_log = timezone.clone();
    let active_gen_opt = status.read().await.active_generation_id.clone();

    info!(
        event_code = "timezone.rebuild.started",
        timezone = %tz_for_log,
        active_generation = ?active_gen_opt,
        "rollup rebuild started"
    );

    let result = tokio::task::spawn_blocking(move || -> std::result::Result<RebuildOutcome, String> {
        let rtz = busytok_domain::ReportingTimezone::parse(&timezone)
            .map_err(|e| format!("invalid timezone '{}': {e}", timezone))?;
        let db = db.lock().unwrap();

        let active_gen = match &active_gen_opt {
            Some(g) => g.clone(),
            None => {
                let total: i64 = db
                    .conn()
                    .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
                    .map_err(|e| format!("failed to count usage_events: {e}"))?;
                if total == 0 {
                    return Ok(RebuildOutcome::NoOp);
                }
                return Err(format!(
                    "rebuild requires an active generation (found {total} usage events without an active generation; service may be mid-bootstrap)"
                ));
            }
        };

        // Rebuild daily_usage from ONLY the active generation's events.
        let all_events = db.usage_events_for_generation(&active_gen)
            .map_err(|e| e.to_string())?;

        // Rebuild daily_usage via the shared upsert function (single event→row mapping).
        db.conn().execute("DELETE FROM daily_usage", [])
            .map_err(|e| format!("failed to truncate daily_usage: {e}"))?;
        write_queries::upsert_daily_usage_for_events(db.conn(), &all_events, &rtz, &active_gen)
            .map_err(|e| format!("failed to rebuild daily_usage: {e}"))?;

        let session_rows = rebuild_sessions(&all_events, &timezone);
        db.replace_sessions(&session_rows)
            .map_err(|e| e.to_string())?;

        let project_rows = rebuild_projects(&all_events, &timezone);
        db.replace_projects(&project_rows)
            .map_err(|e| e.to_string())?;

        let model_summary_rows = rebuild_model_summaries(&all_events);
        db.replace_model_summaries(&model_summary_rows)
            .map_err(|e| e.to_string())?;

        let realtime_summary =
            crate::scan::build_full_realtime_summary(&all_events, &rtz, Some(&db), &[])
                .map_err(|e| e.to_string())?;
        let summary_entries: Vec<(String, String)> = realtime_summary
            .into_iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    serde_json::to_string(&v).unwrap_or_else(|e| {
                        warn!("failed to serialize realtime summary key={k}: {e}");
                        "{}".to_string()
                    }),
                )
            })
            .collect();
        db.replace_realtime_summary(&summary_entries)
            .map_err(|e| e.to_string())?;

        let (seq, _) = outbox_queries::allocate_event_sequence_batch(db.conn(), 1)
            .map_err(|e| e.to_string())?;
        Ok(RebuildOutcome::Completed { seq })
    })
    .await
    .map_err(|e| format!("spawn_blocking join error: {e}"))
    .and_then(|r| r);

    match result {
        Ok(RebuildOutcome::Completed { seq }) => {
            info!(
                event_code = "timezone.rebuild.complete",
                timezone = %tz_for_log,
                "rollup rebuild complete"
            );
            {
                let mut snap = status.write().await;
                snap.latest_event_seq = Some(seq);
            }
            let scopes = canonical_invalidation_scopes();
            let generation_id = status
                .try_read()
                .ok()
                .and_then(|snap| snap.active_generation_id.clone())
                .unwrap_or_default();
            let _ = event_bus.publish(PublishedEvent::durable(
                AppEvent::DataInvalidated {
                    datasets: scopes.clone(),
                },
                seq,
                generation_id,
                now_ms(),
                scopes,
            ));
            let _ = cmd.respond_tx.send(Ok(()));
            Ok(())
        }
        Ok(RebuildOutcome::NoOp) => {
            info!(
                event_code = "timezone.rebuild.noop",
                timezone = %tz_for_log,
                "rollup rebuild skipped: no active generation and no usage events"
            );
            let _ = cmd.respond_tx.send(Ok(()));
            Ok(())
        }
        Err(err) => {
            warn!(
                event_code = "timezone.rebuild.failed",
                timezone = %tz_for_log,
                error = %err,
                "rollup rebuild failed"
            );
            // The structured error has already been delivered through
            // cmd.respond_tx. Returning Ok(()) here prevents the writer loop
            // from pushing this same error onto pending_errors, which would
            // otherwise surface as a stale "shutdown failed: <rebuild error>"
            // on the next flush/shutdown and mask whatever real state the
            // caller is asking about.
            let _ = cmd.respond_tx.send(Err(err));
            Ok(())
        }
    }
}

enum RebuildOutcome {
    Completed { seq: i64 },
    NoOp,
}

async fn handle_reset_failed_checkpoints(
    db: &Arc<Mutex<Database>>,
    cmd: ResetFailedCheckpointsCommand,
) -> Result<()> {
    let db = Arc::clone(db);

    let result = tokio::task::spawn_blocking(move || {
        let db = db.lock().unwrap();
        let conn = db.conn();
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let source_ids = {
            let mut stmt = tx
                .prepare(
                    "SELECT DISTINCT source_id FROM log_files \
                     WHERE state = 'error' OR state = 'failed' ORDER BY source_id",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        let updated = tx
            .execute(
                "UPDATE log_files SET offset_bytes = 0, state = 'active', last_error = NULL \
                 WHERE state = 'error' OR state = 'failed'",
                [],
            )
            .map_err(|e| e.to_string())?;
        if updated > 0 {
            if let Some(generation_id) =
                write_queries::current_active_generation_id(&tx).map_err(|e| e.to_string())?
            {
                write_queries::refresh_source_summaries_for_sources_tx(
                    &tx,
                    &generation_id,
                    &source_ids,
                )
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok::<usize, String>(updated)
    })
    .await
    .map_err(|e| format!("spawn_blocking join error: {e}"))
    .and_then(|r| r);

    match result {
        Ok(updated) => {
            let _ = cmd.respond_tx.send(Ok(updated));
            Ok(())
        }
        Err(err) => {
            let _ = cmd.respond_tx.send(Err(err.clone()));
            anyhow::bail!(err)
        }
    }
}

// ── Status snapshot update ───────────────────────────────────────────────────

async fn update_status_snapshot(
    status: &Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    queue_depth: i64,
) {
    let mut snap = status.write().await;
    snap.writer_queue_depth = queue_depth;
}

// ── Threshold diagnostics ────────────────────────────────────────────────────

/// Emit and clear threshold-crossing diagnostics for writer queue depth and
/// aggregate lag. When values cross above a threshold, a diagnostic event is
/// published. When they recover below, a clearing event is published.
async fn emit_threshold_diagnostics(
    event_bus: &Arc<AppEventBus>,
    db: &Arc<Mutex<Database>>,
    status: &Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    queue_depth: i64,
    prev_queue_state: &mut Option<String>,
    prev_lag_state: &mut Option<String>,
) {
    // ── Queue depth threshold ──────────────────────────────────────────────
    let current_q_state = if queue_depth >= QUEUE_CRITICAL_THRESHOLD {
        Some("critical".to_string())
    } else if queue_depth >= QUEUE_WARNING_THRESHOLD {
        Some("warning".to_string())
    } else {
        None
    };

    if *prev_queue_state != current_q_state {
        match &current_q_state {
            Some(severity) => {
                // Crossed above a threshold.
                let threshold = if severity == "critical" {
                    QUEUE_CRITICAL_THRESHOLD
                } else {
                    QUEUE_WARNING_THRESHOLD
                };
                warn!(
                    queue_depth,
                    threshold,
                    severity = severity.as_str(),
                    "writer queue depth crossed threshold"
                );
                let _ = event_bus.publish_ephemeral(AppEvent::WriterQueueThreshold {
                    queue_depth,
                    threshold,
                    severity: severity.clone(),
                });

                // Persist immediately on threshold crossing.
                persist_metrics_checkpoint(db, status).await;
            }
            None => {
                // Recovered below all thresholds — emit clearing event.
                let prev_severity = prev_queue_state.as_deref().unwrap_or("warning");
                info!(queue_depth, "writer queue depth recovered below thresholds");
                let _ = event_bus.publish_ephemeral(AppEvent::WriterQueueThreshold {
                    queue_depth,
                    threshold: QUEUE_WARNING_THRESHOLD,
                    severity: format!("recovered_from_{}", prev_severity),
                });
            }
        }
        *prev_queue_state = current_q_state;
    }

    // ── Aggregate lag threshold ───────────────────────────────────────────
    let snap = status.read().await;
    let lag_ms = snap.aggregate_lag_ms;

    let current_l_state = if lag_ms >= LAG_CRITICAL_THRESHOLD_MS {
        Some("critical".to_string())
    } else if lag_ms >= LAG_WARNING_THRESHOLD_MS {
        Some("warning".to_string())
    } else {
        None
    };

    if *prev_lag_state != current_l_state {
        match &current_l_state {
            Some(severity) => {
                let threshold = if severity == "critical" {
                    LAG_CRITICAL_THRESHOLD_MS
                } else {
                    LAG_WARNING_THRESHOLD_MS
                };
                warn!(
                    lag_ms,
                    threshold,
                    severity = severity.as_str(),
                    "writer aggregate lag crossed threshold"
                );
                let _ = event_bus.publish_ephemeral(AppEvent::WriterLagThreshold {
                    lag_ms,
                    threshold,
                    severity: severity.clone(),
                });

                // Persist immediately on threshold crossing.
                persist_metrics_checkpoint(db, status).await;
            }
            None => {
                let prev_severity = prev_lag_state.as_deref().unwrap_or("warning");
                info!(lag_ms, "writer aggregate lag recovered below thresholds");
                let _ = event_bus.publish_ephemeral(AppEvent::WriterLagThreshold {
                    lag_ms,
                    threshold: LAG_WARNING_THRESHOLD_MS,
                    severity: format!("recovered_from_{}", prev_severity),
                });
            }
        }
        *prev_lag_state = current_l_state;
    }
}

// ── Metrics checkpoint ───────────────────────────────────────────────────────

/// Persist writer metrics (queue depth, lag) to the service_state table.
async fn persist_metrics_checkpoint(
    db: &Arc<Mutex<Database>>,
    status: &Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
) {
    let db = Arc::clone(db);
    let status = Arc::clone(status);

    let (queue_depth, lag_ms, latest_event_seq) = {
        let snap = status.read().await;
        (
            snap.writer_queue_depth,
            snap.aggregate_lag_ms,
            snap.latest_event_seq,
        )
    };

    let _ = tokio::task::spawn_blocking(move || {
        let db = db.lock().unwrap();
        let conn = db.conn();
        outbox_queries::checkpoint_writer_metrics(conn, queue_depth, lag_ms, latest_event_seq)
            .map_err(|e| {
                error!(error = %e, "failed to persist writer metrics checkpoint");
                e
            })
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::ServiceStatusSnapshot;
    use busytok_domain::NormalizedUsageEvent;
    use busytok_events::AppEventBus;
    use std::sync::Arc;

    fn make_test_event(id: &str, tokens: i64, cost: Option<f64>) -> NormalizedUsageEvent {
        let mut evt =
            NormalizedUsageEvent::minimal_for_test(id, busytok_domain::AgentKind::ClaudeCode);
        evt.total_tokens = tokens;
        evt.input_tokens = tokens / 2;
        evt.output_tokens = tokens / 2;
        evt.cost_usd = cost;
        evt.timestamp_ms = busytok_domain::now_ms();
        evt
    }

    // ── accumulate_live_bucket tests ──────────────────────────────────────

    #[tokio::test]
    async fn first_batch_accumulates_without_publishing() {
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let event_bus = Arc::new(AppEventBus::new(8));
        let mut rx = event_bus.subscribe();

        let events = vec![make_test_event("e1", 100, Some(0.01))];
        accumulate_live_bucket(&status, &event_bus, &events).await;

        let snap = status.read().await;
        assert_eq!(snap.live_bucket.total_tokens, 100);
        assert!((snap.live_bucket.cost_usd - 0.01).abs() < 0.001);
        assert_eq!(snap.live_bucket.event_count, 1);
        assert!(snap.live_bucket.bucket_start_ms > 0);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn same_window_appends_accumulate() {
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let event_bus = Arc::new(AppEventBus::new(8));
        let now = busytok_domain::now_ms();
        {
            let mut snap = status.write().await;
            snap.live_bucket.bucket_start_ms = (now / 2000) * 2000;
            snap.live_bucket.total_tokens = 200;
            snap.live_bucket.event_count = 2;
        }
        let events = vec![make_test_event("e2", 100, None)];
        accumulate_live_bucket(&status, &event_bus, &events).await;
        let snap = status.read().await;
        assert_eq!(snap.live_bucket.total_tokens, 300);
        assert_eq!(snap.live_bucket.event_count, 3);
    }

    #[tokio::test]
    async fn new_window_publishes_old_bucket_and_resets() {
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let event_bus = Arc::new(AppEventBus::new(8));
        let mut rx = event_bus.subscribe();
        let old_window = 1000;
        {
            let mut snap = status.write().await;
            snap.live_bucket.bucket_start_ms = old_window;
            snap.live_bucket.total_tokens = 500;
            snap.live_bucket.cost_usd = 0.05;
            snap.live_bucket.event_count = 5;
        }
        let events = vec![make_test_event("e3", 100, Some(0.01))];
        accumulate_live_bucket(&status, &event_bus, &events).await;

        let published = rx
            .try_recv()
            .expect("old bucket should be published on window cross");
        if let AppEvent::LiveSample {
            bucket_start_ms,
            tokens_per_sec,
            transient,
            ..
        } = &published.event
        {
            assert_eq!(*bucket_start_ms, old_window);
            assert!((*tokens_per_sec - 250.0).abs() < 0.01);
            assert!(!transient);
        } else {
            panic!("expected LiveSample");
        }
        let snap = status.read().await;
        assert_eq!(snap.live_bucket.total_tokens, 100);
        assert_eq!(snap.live_bucket.event_count, 1);
    }

    #[tokio::test]
    async fn empty_events_is_noop() {
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let event_bus = Arc::new(AppEventBus::new(8));
        let mut rx = event_bus.subscribe();
        accumulate_live_bucket(&status, &event_bus, &[]).await;
        let snap = status.read().await;
        assert_eq!(snap.live_bucket.total_tokens, 0);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn default_zero_bucket_start_does_not_trigger_window_cross() {
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let event_bus = Arc::new(AppEventBus::new(8));
        let mut rx = event_bus.subscribe();
        let events = vec![make_test_event("e5", 100, None)];
        accumulate_live_bucket(&status, &event_bus, &events).await;
        assert!(rx.try_recv().is_err(), "no publish on fresh default bucket");
        let snap = status.read().await;
        assert_eq!(snap.live_bucket.total_tokens, 100);
    }

    // ── flush_pending_batches tests ───────────────────────────────────────

    #[tokio::test]
    async fn flush_pending_batches_empty_is_noop() {
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let event_bus = Arc::new(AppEventBus::new(8));
        let settings = Arc::new(Mutex::new(BusytokSettings::default()));
        let mut pending: Vec<WriteCommand> = Vec::new();
        flush_pending_batches(&db, &status, &event_bus, &settings, &mut pending).await;
        assert!(pending.is_empty());
    }

    // ── pending_event_count tests ─────────────────────────────────────────

    #[test]
    fn pending_event_count_sums_correctly() {
        let cmd1 = WriteCommand::TailBatch(TailBatchCommand {
            source_id: "s1".into(),
            source_file_id: None,
            source_file_agent: "a".into(),
            source_file_path: "p".into(),
            source_file_inode: None,
            events: vec![
                make_test_event("e1", 100, None),
                make_test_event("e2", 200, None),
            ],
            tool_events: vec![],
            diagnostic_events: vec![],
            codex_snapshots: vec![],
            generation_id: "g".into(),
            checkpoint_offset: None,
            write_policy: busytok_domain::UsageWritePolicy::InsertOnce,
        });
        let cmd2 = WriteCommand::TailBatch(TailBatchCommand {
            source_id: "s2".into(),
            source_file_id: None,
            source_file_agent: "a".into(),
            source_file_path: "p".into(),
            source_file_inode: None,
            events: vec![make_test_event("e3", 300, None)],
            tool_events: vec![],
            diagnostic_events: vec![],
            codex_snapshots: vec![],
            generation_id: "g".into(),
            checkpoint_offset: None,
            write_policy: busytok_domain::UsageWritePolicy::InsertOnce,
        });
        assert_eq!(pending_event_count(&[cmd1, cmd2]), 3);
    }

    #[test]
    fn pending_event_count_counts_rebuild_batch_events() {
        // Covers the `WriteCommand::RebuildBatch(c) => c.events.len()` arm (L364).
        let cmd = WriteCommand::RebuildBatch(RebuildBatchCommand {
            source_id: "s1".into(),
            source_file_id: None,
            source_file_agent: "a".into(),
            source_file_path: "p".into(),
            source_file_inode: None,
            events: vec![
                make_test_event("r1", 100, None),
                make_test_event("r2", 200, None),
                make_test_event("r3", 300, None),
            ],
            tool_events: vec![],
            diagnostic_events: vec![],
            codex_snapshots: vec![],
            generation_id: "g".into(),
            checkpoint_offset: None,
            is_final_batch: false,
            write_policy: busytok_domain::UsageWritePolicy::InsertOnce,
        });
        assert_eq!(pending_event_count(&[cmd]), 3);
    }

    #[test]
    fn pending_event_count_returns_zero_for_non_batch_commands() {
        // Covers the `_ => 0` arm (L365).
        let cmds = vec![
            WriteCommand::GenerationCreate(GenerationCreateCommand {
                generation_id: "g1".into(),
            }),
            WriteCommand::TailReplayBatch(TailReplayBatchCommand { rows: vec![] }),
            WriteCommand::RecordTailReplay(RecordTailReplayCommand {
                source_file_id: "f".into(),
                event_seq: 1,
                event_data_json: "{}".into(),
            }),
            WriteCommand::ProgressCheckpoint(ProgressCheckpointCommand {
                file_id: "f".into(),
                source_id: "s".into(),
                agent: "a".into(),
                path: "p".into(),
                inode: None,
                offset_bytes: 0,
                size_bytes: 0,
                last_mtime_ms: None,
                state: "ok".into(),
            }),
        ];
        assert_eq!(pending_event_count(&cmds), 0);
    }

    // ── insert_supplementary_events tests (L889-987) ──────────────────────
    // Exercises the three insertion loops (tool_events, diagnostic_events,
    // codex_snapshots) against a real in-memory database transaction.

    #[test]
    fn insert_supplementary_events_inserts_all_kinds() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();

        let tool = ToolEvent {
            id: "tool-1".to_string(),
            agent: busytok_domain::AgentKind::ClaudeCode,
            source_file_id: "sf-1".to_string(),
            source_path: "/tmp/x.jsonl".to_string(),
            source_line: 10,
            source_offset_start: 0,
            source_offset_end: 100,
            session_id: "sess-1".to_string(),
            message_id: Some("msg-1".to_string()),
            tool_name: "Read".to_string(),
            status: Some("ok".to_string()),
            timestamp_ms: Some(1_000),
            project_hash: Some("ph".to_string()),
            created_at_ms: 1_000,
        };

        let diag = OperationalDiagnosticEvent {
            id: "diag-1".to_string(),
            agent: Some(busytok_domain::AgentKind::ClaudeCode),
            source_id: Some("s-1".to_string()),
            source_file_id: Some("sf-1".to_string()),
            source_path: Some("/tmp/x.jsonl".to_string()),
            source_line: Some(5),
            category: "parser".to_string(),
            severity: "warning".to_string(),
            message: "malformed line".to_string(),
            detail_json: Some(r#"{"line":5}"#.to_string()),
            happened_at_ms: 1_000,
            created_at_ms: 1_000,
        };

        let now = busytok_domain::now_ms();
        let snap = CodexTokenSnapshotRow {
            id: "snap-1".to_string(),
            source_file_id: "sf-1".to_string(),
            source_line: 1,
            source_offset_start: 0,
            source_offset_end: 100,
            session_id: "sess-1".to_string(),
            turn_id: Some("t-1".to_string()),
            token_event_ordinal: 0,
            input_tokens: 50,
            cached_input_tokens: 0,
            output_tokens: 50,
            reasoning_tokens: 0,
            total_tokens: 100,
            model: Some("gpt-4".to_string()),
            raw_usage_json: "{}".to_string(),
            emitted_event_id: Some("evt-1".to_string()),
            created_at_ms: now,
            updated_at_ms: now,
        };

        let tx = conn.unchecked_transaction().unwrap();
        insert_supplementary_events(&tx, &[tool], &[diag], &[snap]).unwrap();
        tx.commit().unwrap();

        // Verify tool_events row.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Verify diagnostic_events row.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM diagnostic_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Verify codex_token_snapshots row.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM codex_token_snapshots", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn insert_supplementary_events_empty_inputs_is_noop() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        let tx = conn.unchecked_transaction().unwrap();
        // Passing all-empty slices should succeed without inserting anything.
        insert_supplementary_events(&tx, &[], &[], &[]).unwrap();
        tx.commit().unwrap();
    }

    // ── prune_old_usage_events tests (lines 682-711) ─────────────────────

    #[tokio::test]
    async fn prune_old_usage_events_no_active_generation_is_noop() {
        // Covers the early-return path when active_generation_id is None.
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        // active_generation_id is None by default.
        let before = status.read().await.total_usage_event_count;
        prune_old_usage_events(&db, &status).await;
        let after = status.read().await.total_usage_event_count;
        assert_eq!(before, after, "no prune should happen without active gen");
    }

    #[tokio::test]
    async fn prune_old_usage_events_with_active_gen_and_no_events_returns_zero() {
        // Covers the Ok(Ok(0)) branch where deleted == 0 (no decrement).
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        {
            let mut snap = status.write().await;
            snap.active_generation_id = Some("gen-no-events".to_string());
            snap.total_usage_event_count = 5;
        }
        prune_old_usage_events(&db, &status).await;
        let after = status.read().await.total_usage_event_count;
        assert_eq!(after, 5, "count should not change when 0 events pruned");
    }

    #[tokio::test]
    async fn prune_old_usage_events_deletes_old_events_and_decrements_count() {
        // Covers the Ok(Ok(deleted > 0)) branch with decrement.
        let db = Database::open_in_memory().unwrap();
        let now = now_ms();
        db.conn()
            .execute(
                "INSERT INTO audit_generations (generation_id, state, started_at_ms, is_active, \
                 created_at_ms, updated_at_ms) VALUES ('gen-prune', 'promoted', ?1, 1, ?1, ?1)",
                rusqlite::params![now],
            )
            .unwrap();
        let mut evt = NormalizedUsageEvent::minimal_for_test(
            "evt-old",
            busytok_domain::AgentKind::ClaudeCode,
        );
        evt.timestamp_ms = now - 86_400_000 - 1_000; // 25h ago
        evt.total_tokens = 100;
        db.write_usage_event(&evt, UsageWritePolicy::InsertOnce)
            .unwrap();
        db.conn()
            .execute(
                "UPDATE usage_events SET generation_id = 'gen-prune' WHERE id = 'evt-old'",
                [],
            )
            .unwrap();
        let db = Arc::new(Mutex::new(db));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        {
            let mut snap = status.write().await;
            snap.active_generation_id = Some("gen-prune".to_string());
            snap.total_usage_event_count = 1;
        }
        prune_old_usage_events(&db, &status).await;
        let snap = status.read().await;
        assert_eq!(
            snap.total_usage_event_count, 0,
            "should decrement after prune"
        );
        let count: i64 = db
            .lock()
            .unwrap()
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM usage_events WHERE id = 'evt-old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "old event should be pruned");
    }

    #[tokio::test]
    async fn prune_old_usage_events_keeps_recent_events() {
        // Additional coverage: events within 24h should not be pruned.
        let db = Database::open_in_memory().unwrap();
        let now = now_ms();
        db.conn()
            .execute(
                "INSERT INTO audit_generations (generation_id, state, started_at_ms, is_active, \
                 created_at_ms, updated_at_ms) VALUES ('gen-keep', 'promoted', ?1, 1, ?1, ?1)",
                rusqlite::params![now],
            )
            .unwrap();
        let mut evt = NormalizedUsageEvent::minimal_for_test(
            "evt-recent",
            busytok_domain::AgentKind::ClaudeCode,
        );
        evt.timestamp_ms = now - 1_000; // 1s ago
        evt.total_tokens = 50;
        db.write_usage_event(&evt, UsageWritePolicy::InsertOnce)
            .unwrap();
        db.conn()
            .execute(
                "UPDATE usage_events SET generation_id = 'gen-keep' WHERE id = 'evt-recent'",
                [],
            )
            .unwrap();
        let db = Arc::new(Mutex::new(db));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        {
            let mut snap = status.write().await;
            snap.active_generation_id = Some("gen-keep".to_string());
            snap.total_usage_event_count = 1;
        }
        prune_old_usage_events(&db, &status).await;
        let snap = status.read().await;
        assert_eq!(
            snap.total_usage_event_count, 1,
            "recent event should not be pruned"
        );
    }

    // ── flush_pending_batches multi-generation warning (lines 447-453) ────

    #[tokio::test]
    async fn flush_pending_batches_with_multiple_generations_logs_warning() {
        // Covers the groups.len() > 1 branch.
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let event_bus = Arc::new(AppEventBus::new(8));
        let settings = Arc::new(Mutex::new(BusytokSettings::default()));
        {
            let db = db.lock().unwrap();
            let now = now_ms();
            db.conn()
                .execute(
                    "INSERT INTO audit_generations (generation_id, state, started_at_ms, is_active, \
                     created_at_ms, updated_at_ms) \
                     VALUES ('gen-multi-a', 'promoted', ?1, 0, ?1, ?1), \
                            ('gen-multi-b', 'promoted', ?1, 0, ?1, ?1)",
                    rusqlite::params![now],
                )
                .unwrap();
        }
        let mut pending: Vec<WriteCommand> = vec![
            WriteCommand::TailBatch(TailBatchCommand {
                source_id: "src-a".into(),
                source_file_id: None,
                source_file_agent: "claude_code".into(),
                source_file_path: "/tmp/a.jsonl".into(),
                source_file_inode: None,
                events: vec![make_test_event("evt-a", 10, None)],
                tool_events: vec![],
                diagnostic_events: vec![],
                codex_snapshots: vec![],
                generation_id: "gen-multi-a".into(),
                checkpoint_offset: None,
                write_policy: UsageWritePolicy::InsertOnce,
            }),
            WriteCommand::TailBatch(TailBatchCommand {
                source_id: "src-b".into(),
                source_file_id: None,
                source_file_agent: "claude_code".into(),
                source_file_path: "/tmp/b.jsonl".into(),
                source_file_inode: None,
                events: vec![make_test_event("evt-b", 20, None)],
                tool_events: vec![],
                diagnostic_events: vec![],
                codex_snapshots: vec![],
                generation_id: "gen-multi-b".into(),
                checkpoint_offset: None,
                write_policy: UsageWritePolicy::InsertOnce,
            }),
        ];
        flush_pending_batches(&db, &status, &event_bus, &settings, &mut pending).await;
        assert!(pending.is_empty());
        let db = db.lock().unwrap();
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM usage_events WHERE id IN ('evt-a','evt-b')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    // ── flush_single_generation invalid timezone fallback (lines 565-567) ─

    #[tokio::test]
    async fn flush_single_generation_invalid_timezone_falls_back_to_utc() {
        // Covers lines 565-567: settings.timezone parse error → UTC fallback.
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let event_bus = Arc::new(AppEventBus::new(8));
        let settings = Arc::new(Mutex::new(BusytokSettings::default()));
        settings.lock().unwrap().timezone = "Mars/Olympus".to_string();
        {
            let db = db.lock().unwrap();
            let now = now_ms();
            db.conn()
                .execute(
                    "INSERT INTO audit_generations (generation_id, state, started_at_ms, is_active, \
                     created_at_ms, updated_at_ms) \
                     VALUES ('gen-tz-bad', 'promoted', ?1, 0, ?1, ?1)",
                    rusqlite::params![now],
                )
                .unwrap();
        }
        let group = vec![WriteCommand::TailBatch(TailBatchCommand {
            source_id: "src-tz".into(),
            source_file_id: None,
            source_file_agent: "claude_code".into(),
            source_file_path: "/tmp/tz.jsonl".into(),
            source_file_inode: None,
            events: vec![make_test_event("evt-tz", 100, None)],
            tool_events: vec![],
            diagnostic_events: vec![],
            codex_snapshots: vec![],
            generation_id: "gen-tz-bad".into(),
            checkpoint_offset: None,
            write_policy: UsageWritePolicy::InsertOnce,
        })];
        flush_single_generation(&db, &status, &event_bus, &settings, group, "gen-tz-bad").await;
        let db = db.lock().unwrap();
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM usage_events WHERE id = 'evt-tz'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "event should be persisted despite bad timezone");
    }

    // ── emit_threshold_diagnostics queue paths (lines 1553-1588) ──────────

    #[tokio::test]
    async fn emit_threshold_diagnostics_queue_warning_fires() {
        // Covers the Some("warning") branch.
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let event_bus = Arc::new(AppEventBus::new(8));
        let mut rx = event_bus.subscribe();
        let mut prev_queue_state: Option<String> = None;
        let mut prev_lag_state: Option<String> = None;
        emit_threshold_diagnostics(
            &event_bus,
            &db,
            &status,
            70, // above warning (64), below critical (96)
            &mut prev_queue_state,
            &mut prev_lag_state,
        )
        .await;
        let evt = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout waiting for event")
            .expect("event channel closed");
        if let AppEvent::WriterQueueThreshold { severity, .. } = evt.event {
            assert_eq!(severity, "warning");
        } else {
            panic!("expected WriterQueueThreshold event");
        }
        assert_eq!(prev_queue_state, Some("warning".to_string()));
    }

    #[tokio::test]
    async fn emit_threshold_diagnostics_queue_critical_fires() {
        // Covers the Some("critical") branch.
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let event_bus = Arc::new(AppEventBus::new(8));
        let mut rx = event_bus.subscribe();
        let mut prev_queue_state: Option<String> = None;
        let mut prev_lag_state: Option<String> = None;
        emit_threshold_diagnostics(
            &event_bus,
            &db,
            &status,
            100, // above critical (96)
            &mut prev_queue_state,
            &mut prev_lag_state,
        )
        .await;
        let evt = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout waiting for event")
            .expect("event channel closed");
        if let AppEvent::WriterQueueThreshold { severity, .. } = evt.event {
            assert_eq!(severity, "critical");
        } else {
            panic!("expected WriterQueueThreshold event");
        }
        assert_eq!(prev_queue_state, Some("critical".to_string()));
    }

    #[tokio::test]
    async fn emit_threshold_diagnostics_queue_recovery_fires() {
        // Covers the None branch (recovery from previous warning).
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let event_bus = Arc::new(AppEventBus::new(8));
        let mut rx = event_bus.subscribe();
        let mut prev_queue_state: Option<String> = Some("warning".to_string());
        let mut prev_lag_state: Option<String> = None;
        emit_threshold_diagnostics(
            &event_bus,
            &db,
            &status,
            0, // below all thresholds
            &mut prev_queue_state,
            &mut prev_lag_state,
        )
        .await;
        let evt = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout waiting for event")
            .expect("event channel closed");
        if let AppEvent::WriterQueueThreshold { severity, .. } = evt.event {
            assert!(
                severity.starts_with("recovered_from_"),
                "expected recovery severity, got: {severity}"
            );
        } else {
            panic!("expected WriterQueueThreshold event");
        }
        assert_eq!(prev_queue_state, None);
    }

    #[tokio::test]
    async fn emit_threshold_diagnostics_lag_recovery_fires_directly() {
        // Covers the lag None branch (recovery from previous warning).
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let event_bus = Arc::new(AppEventBus::new(8));
        let mut rx = event_bus.subscribe();
        {
            let mut snap = status.write().await;
            snap.aggregate_lag_ms = 6_000;
        }
        let mut prev_queue_state: Option<String> = None;
        let mut prev_lag_state: Option<String> = None;
        // First call: triggers warning.
        emit_threshold_diagnostics(
            &event_bus,
            &db,
            &status,
            0,
            &mut prev_queue_state,
            &mut prev_lag_state,
        )
        .await;
        // Drain the warning event.
        let _ = rx.try_recv();
        // Now set lag to 0 — triggers recovery.
        {
            let mut snap = status.write().await;
            snap.aggregate_lag_ms = 0;
        }
        emit_threshold_diagnostics(
            &event_bus,
            &db,
            &status,
            0,
            &mut prev_queue_state,
            &mut prev_lag_state,
        )
        .await;
        let evt = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout waiting for event")
            .expect("event channel closed");
        if let AppEvent::WriterLagThreshold { severity, .. } = evt.event {
            assert!(
                severity.starts_with("recovered_from_"),
                "expected recovery severity, got: {severity}"
            );
        } else {
            panic!("expected WriterLagThreshold event");
        }
        assert_eq!(prev_lag_state, None);
    }

    // ── handle_log_source_upsert with active generation (lines 1276-1282) ─

    #[tokio::test]
    async fn handle_log_source_upsert_with_active_generation_refreshes_summary() {
        let db = Database::open_in_memory().unwrap();
        let now = now_ms();
        db.conn()
            .execute(
                "INSERT INTO audit_generations (generation_id, state, started_at_ms, is_active, \
                 created_at_ms, updated_at_ms) VALUES ('gen-lsu', 'promoted', ?1, 1, ?1, ?1)",
                rusqlite::params![now],
            )
            .unwrap();
        let db = Arc::new(Mutex::new(db));
        let cmd = LogSourceUpsertCommand {
            row: LogSourceRow {
                id: "src-lsu".to_string(),
                agent: "claude_code".to_string(),
                source_type: "jsonl".to_string(),
                root_path: "/tmp/lsu".to_string(),
                configured_by_user: 1,
                default_discovery_enabled: 1,
                status: "active".to_string(),
                last_scan_started_at_ms: Some(now),
                last_scan_completed_at_ms: Some(now),
                last_error: None,
                first_seen_at_ms: now,
                last_seen_at_ms: now,
                created_at_ms: now,
                updated_at_ms: now,
            },
        };
        handle_log_source_upsert(&db, cmd).await.expect("upsert");
        let db = db.lock().unwrap();
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM source_health_summary \
                 WHERE generation_id = 'gen-lsu' AND source_id = 'src-lsu'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "source_health_summary should be refreshed");
    }

    // ── handle_rebuild_rollups NoOp branch (lines 1427-1434) ──────────────

    #[tokio::test]
    async fn handle_rebuild_rollups_noop_when_no_events_and_no_active_gen() {
        // Covers the NoOp branch: no active generation and no usage events.
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
        let event_bus = Arc::new(AppEventBus::new(8));
        let (respond_tx, respond_rx) = oneshot::channel();
        let cmd = RebuildRollupsCommand {
            timezone: "UTC".to_string(),
            respond_tx,
        };
        handle_rebuild_rollups(&db, &event_bus, &status, cmd)
            .await
            .expect("should succeed");
        let result = respond_rx.await.expect("response should be sent");
        assert!(result.is_ok(), "NoOp should respond with Ok(())");
    }
}
