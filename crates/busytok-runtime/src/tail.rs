//! Live file watching: tails discovered log files for incremental data.
//!
//! `start_tailing` sets up a `FileWatchService` to monitor discovered
//! root directories. When a file changes, it reads from the last
//! checkpoint offset, parses new lines, and stores the results.
//!
//! Dynamic file discovery: when a `Created` event arrives for a path
//! that is not yet tracked but falls under a watched source's root_path,
//! the new file is added to `file_to_source` and processed from offset 0.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use busytok_adapters::AgentLogAdapter;
use busytok_aggregator::{
    build_scan_mutations, model_rollups_to_rows, project_rollups_to_rows, session_rollups_to_rows,
    RollupOptions,
};
use busytok_config::BusytokSettings;
use busytok_domain::{
    AgentKind, CodexTokenSnapshot, NormalizedEvent, NormalizedUsageEvent,
    OperationalDiagnosticEvent, ParsedLogEvent, ReportingTimezone, ToolEvent,
};
use busytok_events::{AppEvent, AppEventBus};
use busytok_protocol::dto::canonical_invalidation_scopes;
use busytok_store::{Database, StoreWriteBatch};
use busytok_tailer::{FileChangeKind, FileWatchService, ScanFileRequest};

use busytok_pricing::CostMode;

use crate::scan::{
    build_full_realtime_summary, derive_file_id, enrich_cost, parse_events_with_codex_model_context,
};

/// Type alias for boxed adapter with Send + Sync bounds.
type BoxedAdapter = Box<dyn AgentLogAdapter + Send + Sync>;

/// Result of starting the tailing service.
pub struct TailHandle {
    /// Join handle for the tail worker task.
    pub join_handle: tokio::task::JoinHandle<()>,
    /// Shutdown signal sender.
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
}

/// Start the tailing loop for live file monitoring.
///
/// This spawns a background task that:
/// 1. Watches all discovered root directories for file changes
/// 2. When a file changes, reads from its last checkpoint offset
/// 3. Parses new lines with the appropriate adapter
/// 4. Enriches events with pricing
/// 5. Commits results atomically (including rollups)
/// 6. After commit, rebuilds realtime_summary from full corpus
///
/// Returns a `TailHandle` that can be used to shut down the tailer.
pub async fn start_tailing(
    db: std::sync::Arc<Mutex<Database>>,
    adapters: Vec<BoxedAdapter>,
    sources: Vec<busytok_discovery::DiscoveredLogSource>,
    event_bus: std::sync::Arc<AppEventBus>,
    settings: Arc<Mutex<BusytokSettings>>,
    writer_handle: crate::writer::WriterHandle,
    generation_id: String,
) -> Result<TailHandle> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Build a map from path -> (source_id, agent) for all known files.
    let file_to_source: std::sync::Mutex<std::collections::HashMap<PathBuf, (String, AgentKind)>> =
        std::sync::Mutex::new(std::collections::HashMap::new());
    {
        let mut map = file_to_source.lock().unwrap();
        for source in &sources {
            for file_path in &source.files {
                map.insert(file_path.clone(), (source.source_id.clone(), source.agent));
            }
        }
    }

    // Files that received a watcher event since the last periodic rescan.
    // The rescan only touches these active files, not the full historical set.
    let recently_touched: std::sync::Mutex<std::collections::HashSet<PathBuf>> =
        std::sync::Mutex::new(std::collections::HashSet::new());

    // Build a list of (root_path, source_id, agent) for matching new files
    // against their source.
    let source_roots: Vec<(PathBuf, String, AgentKind)> = sources
        .iter()
        .map(|s| (s.root_path.clone(), s.source_id.clone(), s.agent))
        .collect();

    // Start the file watcher.
    let mut watcher = FileWatchService::new().context("failed to create file watch service")?;

    // Watch each source root directory.
    for source in &sources {
        if source.root_path.exists() {
            watcher
                .watch_path(&source.root_path)
                .with_context(|| format!("failed to watch {}", source.root_path.display()))?;
        }
    }

    // Close the startup race between historical scan and watcher activation:
    // after watches are installed, read every known file once from its current
    // checkpoint. Appends that landed after initial scan but before the watch
    // was active are caught here; appends after this point are covered by the
    // watcher events already being registered.
    {
        let wh = &writer_handle;
        let gen_id = &generation_id;
        for source in &sources {
            for file_path in &source.files {
                let cmd = {
                    let db_guard = db.lock().unwrap();
                    prepare_tail_batch_command(
                        &db_guard,
                        &adapters,
                        file_path,
                        &source.source_id,
                        source.agent,
                        gen_id,
                    )
                };
                match cmd {
                    Ok(Some(write_cmd)) => {
                        wh.send(write_cmd)
                            .await
                            .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))
                            .with_context(|| {
                                format!(
                                    "failed to enqueue startup catch-up for {}",
                                    file_path.display()
                                )
                            })?;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        warn!(
                            path = %file_path.display(),
                            error = %e,
                            "startup tail catch-up failed"
                        );
                    }
                }
            }
        }
        wh.flush()
            .await
            .context("failed to flush startup tail catch-up")?;
    }

    // Seed recently_touched with all known files so the first periodic rescan
    // covers them. This catches the case where a file's first write after
    // startup never triggers a watcher notification.
    {
        // Lock recently_touched first to match the rescan path ordering.
        let mut touched = recently_touched.lock().unwrap();
        let map_guard = file_to_source.lock().unwrap();
        for path in map_guard.keys() {
            touched.insert(path.clone());
        }
    }

    info!("Tailer started, watching {} source roots", sources.len());

    // Spawn the main tail worker that polls the watcher and processes changes.
    let join_handle = tokio::spawn(async move {
        // Periodic fallback: re-read tracked files to catch changes that were
        // missed by filesystem notifications. macOS can coalesce or delay
        // Modified events for actively-written files (e.g. Codex sessions).
        const PERIODIC_RESCAN_INTERVAL: Duration = Duration::from_secs(30);
        let mut last_rescan = std::time::Instant::now();
        // Coalesce threshold: when the writer queue depth exceeds 80% of
        // capacity, back off briefly to let the writer drain. This prevents
        // the tailer from flooding the writer channel faster than it can
        // commit.
        let warn_threshold = crate::writer::DEFAULT_WRITER_CAPACITY * 8 / 10;

        loop {
            // Poll for file change events.
            let events = watcher.poll_events(Duration::from_millis(500));
            for event in events {
                match event {
                    Ok(change) => {
                        if change.kind == FileChangeKind::Modified
                            || change.kind == FileChangeKind::Created
                        {
                            // Check if the path is already tracked (release lock before any await).
                            let source_info = {
                                let mut map_guard = file_to_source.lock().unwrap();
                                let info = if let Some(info) = map_guard.get(&change.path) {
                                    Some(info.clone())
                                } else if change.kind == FileChangeKind::Created {
                                    // Only consider .jsonl files — non-jsonl files
                                    // (backups, caches, etc.) must never be scanned.
                                    if !crate::scan::is_jsonl_file(&change.path) {
                                        None
                                    } else if let Some((_, source_id, agent)) = source_roots
                                        .iter()
                                        .find(|(root, _, _)| change.path.starts_with(root))
                                    {
                                        map_guard.insert(
                                            change.path.clone(),
                                            (source_id.clone(), *agent),
                                        );
                                        Some((source_id.clone(), *agent))
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };
                                info
                            }; // map_guard dropped here

                            if let Some((source_id, agent)) = source_info {
                                // Remember this file for periodic rescan fallback.
                                {
                                    let mut touched = recently_touched.lock().unwrap();
                                    touched.insert(change.path.clone());
                                }
                                {
                                    let wh = &writer_handle;
                                    let gen_id = &generation_id;
                                    // When writer queue is near capacity, apply backpressure
                                    // by sleeping briefly — the bounded channel will naturally
                                    // throttle producers. Coalescing across multiple file
                                    // changes requires accumulating events from multiple
                                    // poll_events() cycles (future enhancement).
                                    if wh.queue_depth() >= warn_threshold {
                                        warn!(
                                            queue_depth = wh.queue_depth(),
                                            capacity = wh.capacity(),
                                            "tail writer queue near capacity; applying backpressure"
                                        );
                                        tokio::time::sleep(Duration::from_millis(50)).await;
                                    }

                                    // Phase 1: read data and build command (sync, under DB lock).
                                    let cmd = {
                                        let db_guard = db.lock().unwrap();
                                        prepare_tail_batch_command(
                                            &db_guard,
                                            &adapters,
                                            &change.path,
                                            &source_id,
                                            agent,
                                            gen_id,
                                        )
                                    };
                                    // Phase 2: send through writer (async, no DB lock).
                                    match cmd {
                                        Ok(Some(write_cmd)) => {
                                            if let Err(e) = wh.send(write_cmd).await {
                                                warn!(
                                                    "Failed to send tail batch for {}: {}",
                                                    change.path.display(),
                                                    e
                                                );
                                            } else {
                                                // Publish progress.
                                                let _ = event_bus.publish_ephemeral(
                                                    AppEvent::ScanProgress {
                                                        source_id: source_id.clone(),
                                                        files_scanned: 1,
                                                        events_ingested: 0,
                                                    },
                                                );
                                            }
                                        }
                                        Ok(None) => {
                                            // No new data in this file.
                                        }
                                        Err(e) => {
                                            warn!(
                                                "Tail processing error for {}: {}",
                                                change.path.display(),
                                                e
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Watcher error: {}", e);
                    }
                }
            }

            // Check for shutdown signal.
            if *shutdown_rx.borrow() {
                info!("Tail worker received shutdown signal");
                break;
            }

            // Periodic fallback: re-read recently-touched files to catch changes
            // missed by filesystem notification coalescing. Only scans files
            // that received a watcher event since the last rescan cycle, not
            // the full historical set.
            if last_rescan.elapsed() >= PERIODIC_RESCAN_INTERVAL {
                // Read active set in a block so the touched guard is dropped
                // before any await in the empty-path.
                let active: Vec<(PathBuf, String, AgentKind)> = 'build: {
                    let touched = recently_touched.lock().unwrap();
                    if touched.is_empty() {
                        drop(touched);
                        break 'build Vec::new();
                    }
                    let map_guard = file_to_source.lock().unwrap();
                    build_rescan_candidates(&touched, &map_guard)
                };
                if active.is_empty() {
                    last_rescan = std::time::Instant::now();
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                // Clear after snapshotting — next cycle only sees new touches.
                recently_touched.lock().unwrap().clear();

                // Skip rescan when writer is congested. Do NOT reset
                // last_rescan — the next loop iteration (~650ms) will retry.
                {
                    if writer_handle.queue_depth() >= warn_threshold {
                        debug!(
                            queue_depth = writer_handle.queue_depth(),
                            "skipping periodic rescan: writer queue near capacity"
                        );
                        requeue_skipped_candidates(&mut *recently_touched.lock().unwrap(), &active);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                }
                last_rescan = std::time::Instant::now();

                let mut rescan_found = 0u64;
                for (path, source_id, agent) in &active {
                    // Check shutdown inside the rescan loop so we don't block
                    // graceful exit when many files are tracked.
                    if *shutdown_rx.borrow() {
                        info!("Tail worker received shutdown signal during periodic rescan");
                        break;
                    }
                    {
                        let wh = &writer_handle;
                        let gen_id = &generation_id;
                        let cmd = {
                            let db_guard = db.lock().unwrap();
                            prepare_tail_batch_command(
                                &db_guard, &adapters, path, source_id, *agent, gen_id,
                            )
                        };
                        match cmd {
                            Ok(Some(write_cmd)) => {
                                if let Err(e) = wh.send(write_cmd).await {
                                    warn!(
                                        "Periodic rescan send failed for {}: {}",
                                        path.display(),
                                        e
                                    );
                                } else {
                                    rescan_found += 1;
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                warn!("Periodic rescan error for {}: {}", path.display(), e);
                            }
                        }
                    }
                }
                if *shutdown_rx.borrow() {
                    info!("Tail worker received shutdown signal after periodic rescan");
                    break;
                }
                if rescan_found > 0 {
                    info!(
                        files_found = rescan_found,
                        files_checked = active.len(),
                        "periodic fallback rescan recovered missed changes"
                    );
                } else {
                    debug!(
                        files_checked = active.len(),
                        "periodic fallback rescan found no new data"
                    );
                }
            }

            // Small sleep to avoid busy-looping.
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    Ok(TailHandle {
        join_handle,
        shutdown_tx,
    })
}

/// Process a single file change event.
fn process_file_change(
    db: &Database,
    adapters: &[BoxedAdapter],
    path: &Path,
    source_id: &str,
    agent: AgentKind,
    event_bus: &AppEventBus,
    timezone: &str,
    generation_id: &str,
) -> Result<()> {
    debug!("Processing tail item for {}", path.display());

    let rtz = ReportingTimezone::parse(timezone).unwrap_or_else(|e| {
        tracing::warn!(
            "failed to parse timezone '{}' in process_file_change: {e}",
            timezone
        );
        ReportingTimezone::utc()
    });
    let rollup_opts = RollupOptions {
        timezone: rtz.clone(),
    };

    // Find the adapter for this file based on the agent type.
    // This is more reliable than filename-based heuristics.
    let adapter = adapters.iter().find(|a| {
        let adapter_agent = a.agent();
        match agent {
            AgentKind::ClaudeCode => matches!(adapter_agent, AgentKind::ClaudeCode),
            AgentKind::Codex => matches!(adapter_agent, AgentKind::Codex),
        }
    });
    if adapter.is_none() {
        debug!(
            "No adapter for tail item {} (agent {:?})",
            path.display(),
            agent
        );
        return Ok(());
    }
    let adapter = adapter.unwrap();
    let write_policy = adapter.write_policy();
    let file_id = derive_file_id(path);

    // Get the current checkpoint offset and previous inode.
    let log_file_row = db.get_log_file(&file_id).ok().flatten();
    let offset = log_file_row
        .as_ref()
        .map(|r| r.offset_bytes as u64)
        .unwrap_or(0);
    let previous_inode = log_file_row.as_ref().and_then(|r| r.inode.clone());

    // Read from the last offset, with truncation/rotation detection.
    let request = ScanFileRequest {
        source_id: source_id.to_string(),
        source_file_id: file_id.clone(),
        path: path.to_path_buf(),
        resume_offset: offset,
        previous_inode,
    };

    // Read the current inode for storage (before moving request).
    let current_inode = busytok_tailer::read_inode(path);

    let batch = busytok_tailer::read_file_once(request)
        .with_context(|| format!("failed to read tailed file {}", path.display()))?;

    if batch.lines.is_empty() {
        return Ok(());
    }

    let mut parsed_events =
        parse_events_with_codex_model_context(db, &file_id, agent, adapter.as_ref(), &batch.lines);
    for parsed in &mut parsed_events {
        if let ParsedLogEvent::Normalized(NormalizedEvent::OperationalDiagnostic(diag)) = parsed {
            if diag.source_id.as_deref() == Some("") {
                diag.source_id = Some(source_id.to_string());
            }
        }
    }

    // Partition ParsedLogEvent into Normalized events and Codex snapshots.
    let mut usage_events: Vec<NormalizedUsageEvent> = Vec::new();
    let mut diagnostic_events: Vec<OperationalDiagnosticEvent> = Vec::new();
    let mut parse_errors: Vec<String> = Vec::new();
    let mut codex_snapshots: Vec<CodexTokenSnapshot> = Vec::new();
    let mut tool_events: Vec<ToolEvent> = Vec::new();

    for parsed in parsed_events {
        match parsed {
            ParsedLogEvent::Normalized(ne) => match ne {
                NormalizedEvent::Usage(mut u) => {
                    enrich_cost(&mut u, CostMode::Auto);
                    usage_events.push(*u);
                }
                NormalizedEvent::OperationalDiagnostic(d) => {
                    if d.category == "parse_error" {
                        parse_errors.push(d.message.clone());
                    }
                    diagnostic_events.push(d);
                }
                NormalizedEvent::Tool(t) => {
                    tool_events.push(t);
                }
            },
            ParsedLogEvent::CodexTokenSnapshot(snap) => {
                codex_snapshots.push(snap);
            }
        }
    }

    // Convert Codex snapshots to usage deltas.
    let (codex_events, codex_snapshot_rows) =
        crate::scan::build_codex_delta_events(db, &codex_snapshots, batch.was_reset)
            .context("failed to build Codex delta events")?;
    for mut event in codex_events {
        enrich_cost(&mut event, CostMode::Auto);
        usage_events.push(event);
    }

    let usage_count = usage_events.len();

    // Capture event IDs, agents, and a by-id map before moving
    // events into the store batch. The closure uses these to
    // compute rollups from truly inserted events inside the
    // transaction, keeping events + rollups + checkpoint atomic.
    let all_event_info: Vec<(String, String)> = usage_events
        .iter()
        .map(|e| (e.id.clone(), e.agent.as_str().to_string()))
        .collect();

    // Capture Codex model resolutions for cross-batch backfill
    // before events are moved into store_batch.
    let codex_model_resolutions = crate::scan::collect_codex_model_resolutions(&usage_events);

    let store_batch = StoreWriteBatch {
        source_id: source_id.to_string(),
        source_file_id: Some(file_id.clone()),
        source_file_agent: agent.as_str().to_string(),
        source_file_path: path.display().to_string(),
        source_file_inode: current_inode,
        checkpoint_offset: Some(batch.checkpoint_offset),
        usage_events: usage_events
            .into_iter()
            .map(|e| (e, write_policy))
            .collect(),
        tool_events,
        diagnostic_events,
        codex_snapshots: codex_snapshot_rows,
        daily_usage_rows: Vec::new(),
        model_usage_rows: Vec::new(),
        realtime_summary_rows: Vec::new(),
        session_rows: Vec::new(),
        project_rows: Vec::new(),
        model_summary_rows: Vec::new(),
    };

    let ro = rollup_opts;
    let ingest_result = db
        .ingest_store_batch(store_batch, generation_id, |effective_events, gen_id| {
            // `effective_events` already folds in new−old deltas for replacements.
            if effective_events.is_empty() {
                return Ok(busytok_store::RollupRows::default());
            }
            let mutations = build_scan_mutations(effective_events, ro, gen_id)
                .context("failed to build tail rollup mutations")?;
            Ok(busytok_store::RollupRows {
                daily_usage_rows: mutations.daily_usage,
                model_usage_rows: Vec::new(),
                session_rows: session_rollups_to_rows(&mutations.session_rollups),
                project_rows: project_rollups_to_rows(&mutations.project_rollups),
                model_summary_rows: model_rollups_to_rows(&mutations.model_rollups),
            })
        })
        .with_context(|| format!("failed to ingest tail batch for {}", path.display()))?;

    // Cross-batch backfill: if this batch resolved a Codex model,
    // update earlier events from previous batches that had NULL model.
    let mut backfill_changed = false;
    if !codex_model_resolutions.is_empty() {
        backfill_changed |=
            crate::scan::backfill_cross_batch_codex_models(db, &codex_model_resolutions);
    }
    // Also handle batches where only turn_context arrived (no usage events).
    if agent == AgentKind::Codex {
        backfill_changed |=
            crate::scan::cross_batch_backfill_from_turn_context(db, &batch.lines, &file_id);
    }
    // If backfill changed any events, rebuild model-dependent aggregates.
    if backfill_changed {
        crate::scan::rebuild_model_aggregates(db, timezone);
    }

    // Publish UsageEventInserted only for truly inserted events.
    let inserted_ids: std::collections::HashSet<&str> = ingest_result
        .inserted_event_ids
        .iter()
        .map(|s| s.as_str())
        .collect();
    for (event_id, agent) in &all_event_info {
        if inserted_ids.contains(event_id.as_str()) {
            let _ = event_bus.publish_ephemeral(AppEvent::UsageEventInserted {
                event_id: event_id.clone(),
                agent: agent.clone(),
            });
        }
    }

    // Publish DataInvalidated — overview, activity, and clients pages need refresh.
    let _ = event_bus.publish_ephemeral(AppEvent::DataInvalidated {
        datasets: canonical_invalidation_scopes(),
    });

    // Publish Error events for parse errors.
    for err_msg in &parse_errors {
        let _ = event_bus.publish_ephemeral(AppEvent::Error {
            message: err_msg.clone(),
            source: Some("tail".to_string()),
        });
    }

    // Rebuild realtime summary from full corpus (post-transaction cache).
    let all_events = db
        .all_usage_events()
        .context("failed to read usage events for realtime summary")?;
    let realtime_summary = build_full_realtime_summary(&all_events, &rtz, Some(db), &[])
        .context("failed to build full realtime summary")?;
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
        .context("failed to write realtime summary after tail")?;
    let _ = event_bus.publish_ephemeral(AppEvent::SummaryUpdated {
        keys_updated: vec!["realtime_summary".to_string()],
    });

    let _ = event_bus.publish_ephemeral(AppEvent::ScanProgress {
        source_id: source_id.to_string(),
        files_scanned: 1,
        events_ingested: usage_count as u64,
    });

    Ok(())
}

/// Prepare a `TailBatchCommand` for a single file change.
///
/// This is the writer-actor variant of [`process_file_change`]. It performs
/// all parsing and enrichment using the database for reads (no awaits), then
/// returns a `TailBatchCommand` ready to be sent through the writer handle.
///
/// Callers should:
/// 1. Lock the DB
/// 2. Call this function to build the command (sync, no awaits)
/// 3. Release the DB lock
/// 4. Send the command through the writer handle (async)
///
/// This avoids holding the DB mutex across `.await` points, which is
/// necessary because `Database` contains `RefCell` and is `!Sync`.
pub fn prepare_tail_batch_command(
    db: &Database,
    adapters: &[BoxedAdapter],
    path: &Path,
    source_id: &str,
    agent: AgentKind,
    generation_id: &str,
) -> Result<Option<crate::writer::WriteCommand>> {
    debug!("Preparing tail batch command for {}", path.display());

    let adapter = adapters.iter().find(|a| {
        let adapter_agent = a.agent();
        match agent {
            AgentKind::ClaudeCode => matches!(adapter_agent, AgentKind::ClaudeCode),
            AgentKind::Codex => matches!(adapter_agent, AgentKind::Codex),
        }
    });
    if adapter.is_none() {
        debug!(
            "No adapter for tail item {} (agent {:?})",
            path.display(),
            agent
        );
        return Ok(None);
    }
    let adapter = adapter.unwrap();
    let write_policy = adapter.write_policy();
    let file_id = derive_file_id(path);

    // Get the current checkpoint offset and previous inode (reads from DB).
    let log_file_row = db.get_log_file(&file_id).ok().flatten();
    let offset = log_file_row
        .as_ref()
        .map(|r| r.offset_bytes as u64)
        .unwrap_or(0);
    let previous_inode = log_file_row.as_ref().and_then(|r| r.inode.clone());

    // Read from the last offset.
    let request = ScanFileRequest {
        source_id: source_id.to_string(),
        source_file_id: file_id.clone(),
        path: path.to_path_buf(),
        resume_offset: offset,
        previous_inode,
    };

    let current_inode = busytok_tailer::read_inode(path);

    let batch = busytok_tailer::read_file_once(request)
        .with_context(|| format!("failed to read tailed file {}", path.display()))?;

    if batch.lines.is_empty() {
        return Ok(None);
    }

    let mut parsed_events =
        parse_events_with_codex_model_context(db, &file_id, agent, adapter.as_ref(), &batch.lines);
    for parsed in &mut parsed_events {
        if let ParsedLogEvent::Normalized(NormalizedEvent::OperationalDiagnostic(diag)) = parsed {
            if diag.source_id.as_deref() == Some("") {
                diag.source_id = Some(source_id.to_string());
            }
        }
    }

    // Partition.
    let mut usage_events: Vec<NormalizedUsageEvent> = Vec::new();
    let mut diagnostic_events: Vec<OperationalDiagnosticEvent> = Vec::new();
    let mut codex_snapshots: Vec<CodexTokenSnapshot> = Vec::new();
    let mut tool_events: Vec<ToolEvent> = Vec::new();

    for parsed in parsed_events {
        match parsed {
            ParsedLogEvent::Normalized(ne) => match ne {
                NormalizedEvent::Usage(mut u) => {
                    enrich_cost(&mut u, CostMode::Auto);
                    usage_events.push(*u);
                }
                NormalizedEvent::OperationalDiagnostic(d) => {
                    diagnostic_events.push(d);
                }
                NormalizedEvent::Tool(t) => {
                    tool_events.push(t);
                }
            },
            ParsedLogEvent::CodexTokenSnapshot(snap) => {
                codex_snapshots.push(snap);
            }
        }
    }

    // Convert Codex snapshots to usage deltas.
    let (codex_events, codex_snapshot_rows) =
        crate::scan::build_codex_delta_events(db, &codex_snapshots, batch.was_reset)
            .context("failed to build Codex delta events")?;
    for mut event in codex_events {
        enrich_cost(&mut event, CostMode::Auto);
        usage_events.push(event);
    }

    // Build the command.
    let cmd = crate::writer::WriteCommand::TailBatch(crate::writer::TailBatchCommand {
        source_id: source_id.to_string(),
        source_file_id: Some(file_id.clone()),
        source_file_agent: agent.as_str().to_string(),
        source_file_path: path.display().to_string(),
        source_file_inode: current_inode,
        events: usage_events,
        tool_events,
        diagnostic_events,
        codex_snapshots: codex_snapshot_rows,
        generation_id: generation_id.to_string(),
        checkpoint_offset: Some(batch.checkpoint_offset),
        write_policy,
    });

    Ok(Some(cmd))
}

/// Rescan a specific file that was reported as changed.
///
/// This is the handler for `rescan_changed_files` in the supervisor.
pub fn rescan_changed_files(
    db: &Database,
    adapters: &[BoxedAdapter],
    source_id: &str,
    agent: AgentKind,
    file_path: &Path,
    timezone: &str,
    generation_id: &str,
) -> Result<()> {
    process_file_change(
        db,
        adapters,
        file_path,
        source_id,
        agent,
        &AppEventBus::new(64),
        timezone,
        generation_id,
    )
}

/// Build the rescan candidate list from `recently_touched` and `file_to_source`.
///
/// Returns matched candidates. Paths in `touched` that are not in
/// `file_to_source` are silently excluded.
pub(crate) fn build_rescan_candidates(
    touched: &std::collections::HashSet<PathBuf>,
    file_to_source: &std::collections::HashMap<PathBuf, (String, AgentKind)>,
) -> Vec<(PathBuf, String, AgentKind)> {
    touched
        .iter()
        .filter_map(|p| {
            file_to_source
                .get(p)
                .map(|(s, a)| (p.clone(), s.clone(), *a))
        })
        .collect()
}

/// Re-add candidate paths back to the touched set when a rescan cycle is
/// skipped due to writer backpressure.
pub(crate) fn requeue_skipped_candidates(
    touched: &mut std::collections::HashSet<PathBuf>,
    candidates: &[(PathBuf, String, AgentKind)],
) {
    for (path, _, _) in candidates {
        touched.insert(path.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Arc;

    use busytok_adapters::CodexAdapter;
    use busytok_config::BusytokPaths;
    use busytok_domain::LogSourceType;

    use crate::supervisor::BusytokSupervisor;

    #[test]
    fn process_file_change_codex_inherits_model_from_heartbeat_snapshot_baseline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("rollout-tail.jsonl");
        let source = busytok_discovery::DiscoveredLogSource {
            agent: AgentKind::Codex,
            source_id: "test-tail".to_string(),
            root_path: dir.path().to_path_buf(),
            files: vec![file_path.clone()],
            source_type: LogSourceType::Jsonl,
            configured_by_user: false,
        };

        let turn_context = r#"{"timestamp":"2026-05-20T07:16:20.000Z","type":"turn_context","payload":{"model":"gpt-5.3-codex-spark"}}"#;
        let heartbeat = r#"{"timestamp":"2026-05-20T07:16:22.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":5,"total_tokens":165},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":165}}}}"#;
        let total_only = r#"{"timestamp":"2026-05-20T07:16:24.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":130,"cached_input_tokens":15,"output_tokens":70,"reasoning_output_tokens":8,"total_tokens":208}}}}"#;

        {
            let mut f = std::fs::File::create(&file_path).expect("create file");
            writeln!(f, "{turn_context}").expect("write turn_context");
            writeln!(f, "{heartbeat}").expect("write heartbeat");
        }

        let db = busytok_store::Database::open_in_memory().expect("db open");
        let supervisor = BusytokSupervisor::new(db, BusytokPaths::new());
        let initial = supervisor
            .run_scan_with_sources(vec![source.clone()])
            .expect("initial heartbeat scan");
        assert_eq!(initial.events_found, 0);

        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&file_path)
                .expect("open append");
            writeln!(f, "{total_only}").expect("write total-only line");
        }

        let db_handle: Arc<std::sync::Mutex<Database>> = supervisor.db_handle().clone();
        let adapters: Vec<BoxedAdapter> = vec![Box::new(CodexAdapter)];
        let event_bus = AppEventBus::new(32);

        {
            let db = db_handle.lock().unwrap();
            process_file_change(
                &db,
                &adapters,
                &file_path,
                "test-tail",
                AgentKind::Codex,
                &event_bus,
                "UTC",
                "gen-test",
            )
            .expect("tail processing should succeed");
        }

        let db = db_handle.lock().unwrap();
        let events = db.all_usage_events().expect("get all events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].model.as_deref(), Some("gpt-5.3-codex-spark"));
        assert_eq!(events[0].total_tokens, 43);
    }

    /// Cross-batch backfill: batch 1 has token_count without model,
    /// batch 2 has turn_context with model. After batch 2 is processed,
    /// batch 1's event should be backfilled in the DB.
    #[test]
    fn process_file_change_codex_cross_batch_model_backfill() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("rollout-cross-batch.jsonl");
        let source = busytok_discovery::DiscoveredLogSource {
            agent: AgentKind::Codex,
            source_id: "test-cross-batch".to_string(),
            root_path: dir.path().to_path_buf(),
            files: vec![file_path.clone()],
            source_type: LogSourceType::Jsonl,
            configured_by_user: false,
        };

        // Batch 1: two token_count lines WITHOUT info.model.
        // The second snapshot produces a delta event (43 tokens) with NULL model.
        let heartbeat1 = r#"{"timestamp":"2026-05-20T07:16:22.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":5,"total_tokens":165},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":165}}}}"#;
        let heartbeat2 = r#"{"timestamp":"2026-05-20T07:16:23.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":130,"cached_input_tokens":15,"output_tokens":70,"reasoning_output_tokens":8,"total_tokens":208},"last_token_usage":{"input_tokens":30,"cached_input_tokens":5,"output_tokens":20,"reasoning_output_tokens":3,"total_tokens":58}}}}"#;

        {
            let mut f = std::fs::File::create(&file_path).expect("create file");
            writeln!(f, "{heartbeat1}").expect("write heartbeat1");
            writeln!(f, "{heartbeat2}").expect("write heartbeat2");
        }

        let db = busytok_store::Database::open_in_memory().expect("db open");
        let supervisor = BusytokSupervisor::new(db, BusytokPaths::new());
        let initial = supervisor
            .run_scan_with_sources(vec![source.clone()])
            .expect("initial scan");
        assert_eq!(
            initial.events_found, 1,
            "batch 1 should produce 1 delta event"
        );

        // Verify batch 1's event has NULL model.
        let db_handle: Arc<std::sync::Mutex<Database>> = supervisor.db_handle().clone();
        {
            let db = db_handle.lock().unwrap();
            let events = db.all_usage_events().expect("get all events");
            assert_eq!(events.len(), 1);
            assert!(
                events[0].model.is_none(),
                "batch 1 event should have NULL model before backfill"
            );
        }

        // Batch 2: turn_context with model arrives in a later tail read.
        let turn_context = r#"{"timestamp":"2026-05-20T07:16:24.000Z","type":"turn_context","payload":{"model":"gpt-5.4"}}"#;

        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&file_path)
                .expect("open append");
            writeln!(f, "{turn_context}").expect("write turn_context");
        }

        let adapters: Vec<BoxedAdapter> = vec![Box::new(CodexAdapter)];
        let event_bus = AppEventBus::new(32);

        {
            let db = db_handle.lock().unwrap();
            process_file_change(
                &db,
                &adapters,
                &file_path,
                "test-cross-batch",
                AgentKind::Codex,
                &event_bus,
                "UTC",
                "gen-test",
            )
            .expect("tail processing should succeed");
        }

        // After batch 2, the cross-batch backfill should have updated
        // batch 1's event with the resolved model.
        let db = db_handle.lock().unwrap();
        let events = db.all_usage_events().expect("get all events");
        assert_eq!(
            events.len(),
            1,
            "turn_context alone should not produce a new usage event"
        );
        assert_eq!(
            events[0].model.as_deref(),
            Some("gpt-5.4"),
            "batch 1 event should be backfilled with model from batch 2's turn_context"
        );
    }

    /// Regression: after cross-batch backfill updates the `model` field on
    /// existing usage_events, the aggregate tables (`model_summary`,
    /// `daily_usage`, `sessions`) must be rebuilt so rankings/models views
    /// don't keep showing the stale NULL/empty model grouping.
    #[test]
    fn process_file_change_codex_cross_batch_backfill_rebuilds_aggregates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("rollout-agg-rebuild.jsonl");
        let source = busytok_discovery::DiscoveredLogSource {
            agent: AgentKind::Codex,
            source_id: "test-agg-rebuild".to_string(),
            root_path: dir.path().to_path_buf(),
            files: vec![file_path.clone()],
            source_type: LogSourceType::Jsonl,
            configured_by_user: false,
        };

        // Batch 1: two heartbeats WITHOUT model → 1 delta event with NULL model.
        let heartbeat1 = r#"{"timestamp":"2026-05-20T07:16:22.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":5,"total_tokens":165},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":165}}}}"#;
        let heartbeat2 = r#"{"timestamp":"2026-05-20T07:16:23.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":130,"cached_input_tokens":15,"output_tokens":70,"reasoning_output_tokens":8,"total_tokens":208},"last_token_usage":{"input_tokens":30,"cached_input_tokens":5,"output_tokens":20,"reasoning_output_tokens":3,"total_tokens":58}}}}"#;

        {
            let mut f = std::fs::File::create(&file_path).expect("create file");
            writeln!(f, "{heartbeat1}").expect("write heartbeat1");
            writeln!(f, "{heartbeat2}").expect("write heartbeat2");
        }

        let db = busytok_store::Database::open_in_memory().expect("db open");
        let supervisor = BusytokSupervisor::new(db, BusytokPaths::new());
        let initial = supervisor
            .run_scan_with_sources(vec![source.clone()])
            .expect("initial scan");
        assert_eq!(
            initial.events_found, 1,
            "batch 1 should produce 1 delta event"
        );

        let db_handle: Arc<std::sync::Mutex<Database>> = supervisor.db_handle().clone();

        // After batch 1: aggregates should NOT have the resolved model yet.
        // `model_summary` skips empty-model events entirely, so there should
        // be no gpt-5.4 row. `daily_usage` stores empty string for NULL model.
        {
            let db = db_handle.lock().unwrap();
            let model_summaries = db.model_summary_rows().expect("model_summary rows");
            assert!(
                !model_summaries.iter().any(|m| m.model == "gpt-5.4"),
                "before backfill, model_summary should NOT have a gpt-5.4 row"
            );
            let daily = db.daily_usage_rows().expect("daily_usage rows");
            assert!(
                daily.iter().any(|d| d.model.is_empty()),
                "before backfill, daily_usage should have an empty-model row"
            );
        }

        // Batch 2: turn_context with model → triggers cross-batch backfill
        // + aggregate rebuild.
        let turn_context = r#"{"timestamp":"2026-05-20T07:16:24.000Z","type":"turn_context","payload":{"model":"gpt-5.4"}}"#;
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&file_path)
                .expect("open append");
            writeln!(f, "{turn_context}").expect("write turn_context");
        }

        let adapters: Vec<BoxedAdapter> = vec![Box::new(CodexAdapter)];
        let event_bus = AppEventBus::new(32);
        {
            let db = db_handle.lock().unwrap();
            process_file_change(
                &db,
                &adapters,
                &file_path,
                "test-agg-rebuild",
                AgentKind::Codex,
                &event_bus,
                "UTC",
                "gen-test",
            )
            .expect("tail processing should succeed");
        }

        // After batch 2: aggregates should be rebuilt with the resolved model.
        let db = db_handle.lock().unwrap();
        let events = db.all_usage_events().expect("get all events");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].model.as_deref(),
            Some("gpt-5.4"),
            "event model should be backfilled"
        );

        let model_summaries = db.model_summary_rows().expect("model_summary rows");
        assert!(
            model_summaries.iter().any(|m| m.model == "gpt-5.4"),
            "after backfill, model_summary should have a gpt-5.4 row"
        );
        assert!(
            !model_summaries.iter().any(|m| m.model.is_empty()),
            "after backfill, model_summary should NOT have an empty-model row"
        );

        let daily = db.daily_usage_rows().expect("daily_usage rows");
        assert!(
            daily.iter().any(|d| d.model == "gpt-5.4"),
            "after backfill, daily_usage should have a gpt-5.4 row"
        );
        assert!(
            !daily.iter().any(|d| d.model.is_empty()),
            "after backfill, daily_usage should NOT have an empty-model row"
        );
    }

    #[test]
    fn build_rescan_candidates_filters_by_touched_set() {
        use std::collections::{HashMap, HashSet};

        let file_a = PathBuf::from("/a.jsonl");
        let file_b = PathBuf::from("/b.jsonl");
        let file_c = PathBuf::from("/c.jsonl");

        let file_to_source: HashMap<PathBuf, (String, AgentKind)> = [
            (file_a.clone(), ("src-1".into(), AgentKind::ClaudeCode)),
            (file_b.clone(), ("src-2".into(), AgentKind::Codex)),
            (file_c.clone(), ("src-3".into(), AgentKind::ClaudeCode)),
        ]
        .into_iter()
        .collect();

        // Only a and c were touched.
        let touched: HashSet<PathBuf> = [file_a.clone(), file_c.clone()].into_iter().collect();

        let candidates = build_rescan_candidates(&touched, &file_to_source);

        assert_eq!(candidates.len(), 2);
        let paths: HashSet<PathBuf> = candidates.iter().map(|(p, _, _)| p.clone()).collect();
        assert!(paths.contains(&file_a));
        assert!(paths.contains(&file_c));
        assert!(!paths.contains(&file_b));
    }

    #[test]
    fn build_rescan_candidates_ignores_unknown_paths() {
        use std::collections::{HashMap, HashSet};

        let file_to_source: HashMap<PathBuf, (String, AgentKind)> = [(
            PathBuf::from("/known.jsonl"),
            ("s".into(), AgentKind::Codex),
        )]
        .into_iter()
        .collect();

        let touched: HashSet<PathBuf> = [PathBuf::from("/unknown.jsonl")].into_iter().collect();

        let candidates = build_rescan_candidates(&touched, &file_to_source);
        assert!(candidates.is_empty());
    }

    #[test]
    fn requeue_skipped_candidates_re_adds_paths() {
        use std::collections::HashSet;

        let mut touched: HashSet<PathBuf> = HashSet::new();
        let candidates = vec![
            (PathBuf::from("/a.jsonl"), "s1".into(), AgentKind::Codex),
            (
                PathBuf::from("/b.jsonl"),
                "s2".into(),
                AgentKind::ClaudeCode,
            ),
        ];

        requeue_skipped_candidates(&mut touched, &candidates);

        assert!(touched.contains(Path::new("/a.jsonl")));
        assert!(touched.contains(Path::new("/b.jsonl")));
    }
}
