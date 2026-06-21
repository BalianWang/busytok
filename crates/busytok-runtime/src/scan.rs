//! Historical scan: discovers log sources and reads all existing data.
//!
//! `scan_once` performs an initial scan of all discovered log sources.
//! For each source, it upserts the source row, reads each file from its
//! last checkpoint offset, parses lines with the appropriate adapter,
//! enriches cost data from pricing, builds aggregates, and commits
//! atomically via `ingest_store_batch`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tracing::{debug, info, warn};

use busytok_adapters::AgentLogAdapter;
use busytok_aggregator::{
    build_scan_mutations, build_weekly_usage_value, calculate_burn_rate, identify_session_blocks,
    model_rollups_to_rows, project_rollups_to_rows, session_rollups_to_rows, RollupOptions,
};
use busytok_domain::{
    metadata_event_hash, now_ms, AgentKind, CodexTokenSnapshot, MetadataFingerprint,
    NormalizedEvent, NormalizedUsageEvent, OperationalDiagnosticEvent, ParsedLogEvent,
    ReportingTimezone, ToolEvent,
};
use busytok_events::{AppEvent, AppEventBus};
use busytok_pricing::{estimate_cost_with_catalog, load_catalog, CostMode, TokenUsage};
use busytok_store::{CodexTokenSnapshotRow, Database, LogSourceRow, StoreWriteBatch};

use busytok_tailer::{read_file_once, read_inode, ScanFileRequest};

use crate::queue::ScanStats;

pub(crate) fn extract_codex_turn_context_model(line: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(line).ok()?;
    if parsed.get("type").and_then(|v| v.as_str()) != Some("turn_context") {
        return None;
    }

    parsed
        .get("payload")
        .and_then(|payload| payload.get("model"))
        .and_then(|model| model.as_str())
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn effective_codex_model(current_codex_model: &Option<String>) -> Option<String> {
    current_codex_model.clone()
}

fn normalized_codex_event_model(model: Option<String>) -> Option<String> {
    model
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
}

fn explicit_zero_component_codex_delta(snapshot: &CodexTokenSnapshot) -> bool {
    matches!(
        (
            snapshot.delta_input_tokens,
            snapshot.delta_cached_input_tokens,
            snapshot.delta_output_tokens,
            snapshot.delta_reasoning_tokens,
        ),
        (Some(0), Some(0), Some(0), Some(0))
    )
}

pub(crate) fn load_persisted_codex_model(db: &Database, source_file_id: &str) -> Option<String> {
    db.latest_codex_snapshot_model_for_source_file(source_file_id)
        .ok()
        .flatten()
        .or_else(|| {
            db.latest_usage_event_model_for_source_file(source_file_id, AgentKind::Codex.as_str())
                .ok()
                .flatten()
        })
}

fn resolve_codex_event_model(
    snapshot: &CodexTokenSnapshot,
    previous: Option<&CodexTokenSnapshotRow>,
) -> Option<String> {
    normalized_codex_event_model(
        snapshot
            .model
            .clone()
            .or_else(|| previous.and_then(|row| row.model.clone())),
    )
}

/// Returns `true` when `path` has a `.jsonl` extension (case-sensitive,
/// matching the discovery layer's filter).  Files without this extension
/// should never be scanned or tailed.
pub(crate) fn is_jsonl_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext == "jsonl")
}

/// Maximum number of per-line parse-error diagnostic events to emit for a
/// single file. Beyond this threshold, the file is assumed to be unparseable
/// (e.g. a non-JSONL binary) and only a single summary diagnostic is recorded.
const MAX_PARSE_ERRORS_PER_FILE: usize = 100;

pub(crate) fn parse_events_with_codex_model_context(
    db: &Database,
    file_id: &str,
    agent: AgentKind,
    adapter: &(dyn AgentLogAdapter + Send + Sync),
    lines: &[busytok_tailer::TailedLine],
) -> Vec<ParsedLogEvent> {
    let mut parsed_events: Vec<ParsedLogEvent> = Vec::new();
    let mut current_codex_model: Option<String> = if agent == AgentKind::Codex {
        load_persisted_codex_model(db, file_id)
    } else {
        None
    };
    let mut parse_error_count: usize = 0;
    let mut cap_exceeded = false;

    for tailed_line in lines {
        let ctx = &tailed_line.context;
        if agent == AgentKind::Codex {
            if let Some(model) = extract_codex_turn_context_model(&tailed_line.text) {
                current_codex_model = Some(model);
            }
        }
        match adapter.parse_line(ctx, &tailed_line.text) {
            Ok(parsed) => {
                for event in parsed {
                    match event {
                        ParsedLogEvent::CodexTokenSnapshot(mut snap) => {
                            snap.model = normalized_codex_event_model(snap.model);
                            if let Some(ref m) = snap.model {
                                current_codex_model = Some(m.clone());
                            } else {
                                snap.model = effective_codex_model(&current_codex_model);
                            }
                            parsed_events.push(ParsedLogEvent::CodexTokenSnapshot(snap));
                        }
                        other => parsed_events.push(other),
                    }
                }
            }
            Err(e) => {
                parse_error_count += 1;

                // Once the per-file cap is exceeded, suppress further
                // per-line diagnostics to avoid flooding the database with
                // noise from unparseable files (binary, cache, backups).
                if parse_error_count > MAX_PARSE_ERRORS_PER_FILE {
                    if !cap_exceeded {
                        cap_exceeded = true;
                        warn!(
                            file_id = %file_id,
                            source_path = %ctx.source_path,
                            total_lines = lines.len(),
                            parse_errors = parse_error_count,
                            "Parse error cap exceeded for file; suppressing further per-line diagnostics"
                        );
                        let summary = OperationalDiagnosticEvent {
                            id: format!("parse_err_cap_{file_id}"),
                            agent: Some(agent),
                            source_id: Some(String::new()),
                            source_file_id: Some(file_id.to_string()),
                            source_path: Some(ctx.source_path.clone()),
                            source_line: None,
                            category: "parse_error".to_string(),
                            severity: "warning".to_string(),
                            message: format!(
                                "Parse error cap exceeded: {parse_error_count}+ errors in \
                                 {total_lines} lines; file may not be a valid JSONL log",
                                total_lines = lines.len()
                            ),
                            detail_json: None,
                            happened_at_ms: now_ms(),
                            created_at_ms: now_ms(),
                        };
                        parsed_events.push(ParsedLogEvent::Normalized(
                            NormalizedEvent::OperationalDiagnostic(summary),
                        ));
                    }
                    continue;
                }

                warn!("Parse error in {}: {}", tailed_line.context.source_path, e);
                let diag = OperationalDiagnosticEvent {
                    id: format!("parse_err_{}_{}", file_id, ctx.source_line),
                    agent: Some(agent),
                    source_id: Some(String::new()),
                    source_file_id: Some(file_id.to_string()),
                    source_path: Some(ctx.source_path.clone()),
                    source_line: Some(ctx.source_line as i64),
                    category: "parse_error".to_string(),
                    severity: "warning".to_string(),
                    message: format!("Parse error: {e}"),
                    detail_json: None,
                    happened_at_ms: now_ms(),
                    created_at_ms: now_ms(),
                };
                parsed_events.push(ParsedLogEvent::Normalized(
                    NormalizedEvent::OperationalDiagnostic(diag),
                ));
            }
        }
    }

    parsed_events
}

fn codex_usage_event_id(
    session_id: &str,
    timestamp_ms: i64,
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    total_tokens: i64,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codex-usage");
    hasher.update(b"\x00");
    hasher.update(session_id.as_bytes());
    hasher.update(b"\x00");
    hasher.update(timestamp_ms.to_le_bytes());
    hasher.update(b"\x00");
    hasher.update(input_tokens.to_le_bytes());
    hasher.update(cached_input_tokens.to_le_bytes());
    hasher.update(output_tokens.to_le_bytes());
    hasher.update(reasoning_tokens.to_le_bytes());
    hasher.update(total_tokens.to_le_bytes());
    format!("codex-usage:{}", hex::encode(hasher.finalize()))
}

/// Derive a file ID from a file path.
///
/// Uses SHA-256 to produce a collision-resistant, deterministic file ID
/// from the path string. Returns the hex digest prefixed with "file_".
pub fn derive_file_id(path: &Path) -> String {
    let display = path.display().to_string();
    let mut hasher = Sha256::new();
    hasher.update(display.as_bytes());
    let result = hasher.finalize();
    format!("file_{}", hex::encode(result))
}

/// Enrich a usage event with estimated cost from the pricing catalog.
///
/// The `mode` controls how `estimate_cost_usd` behaves:
/// - `Auto` — prefer `cost_usd` from the source, fall back to token calculation
/// - `Calculate` — always compute from token counts, ignore source-provided cost
/// - `Display` — always use source-provided cost, return 0 when absent
///
/// If the event already has `cost_usd` from the source and mode is `Auto`,
/// that value is preferred. The event `speed` field (e.g. "fast") is passed
/// to the pricing function so the fast-mode multiplier can be applied.
pub fn enrich_cost(event: &mut NormalizedUsageEvent, mode: CostMode) {
    let model = event.model.as_deref().unwrap_or("");
    let usage = TokenUsage {
        input_tokens: event.input_tokens as u64,
        output_tokens: event.output_tokens as u64,
        cached_input_tokens: event.cached_input_tokens as u64,
        cache_creation_tokens: event.cache_creation_tokens as u64,
        reasoning_tokens: event.reasoning_tokens as u64,
    };
    let speed = event.speed.as_deref();

    // Load a single snapshot for both cost estimation and version stamping.
    let catalog = load_catalog();

    if event.estimated_cost_usd.is_none() {
        if let Some(estimate) =
            estimate_cost_with_catalog(&catalog, model, usage, event.cost_usd, speed, mode)
        {
            event.estimated_cost_usd = Some(estimate);
            if event.cost_usd.is_none() {
                event.cost_source = Some("estimated".to_string());
                event.cost_usd = Some(estimate);
            }
        }
    }

    event.price_catalog_version = Some(catalog.version.clone());
}

/// Convert a LogSourceType to its string representation.
fn source_type_str(st: &busytok_domain::LogSourceType) -> &'static str {
    match st {
        busytok_domain::LogSourceType::Jsonl => "jsonl",
        busytok_domain::LogSourceType::SQLite => "sqlite",
        busytok_domain::LogSourceType::Directory => "directory",
    }
}

fn build_log_source_row(
    source: &busytok_discovery::DiscoveredLogSource,
    now_ms: i64,
    started_at_ms: Option<i64>,
    completed_at_ms: Option<i64>,
    first_seen_at_ms: i64,
    created_at_ms: i64,
) -> LogSourceRow {
    LogSourceRow {
        id: source.source_id.clone(),
        agent: source.agent.as_str().to_string(),
        source_type: source_type_str(&source.source_type).to_string(),
        root_path: source.root_path.display().to_string(),
        configured_by_user: source.configured_by_user as i32,
        default_discovery_enabled: 1,
        status: "active".to_string(),
        last_scan_started_at_ms: started_at_ms,
        last_scan_completed_at_ms: completed_at_ms,
        last_error: None,
        first_seen_at_ms,
        last_seen_at_ms: now_ms,
        created_at_ms,
        updated_at_ms: now_ms,
    }
}

fn sorted_files_by_earliest_timestamp(files: &[PathBuf]) -> Vec<PathBuf> {
    let mut files_with_ts: Vec<(&PathBuf, Option<i64>)> = files
        .iter()
        .map(|f| (f, get_earliest_timestamp_ms(f)))
        .collect();
    files_with_ts.sort_by(|a, b| match (a.1, b.1) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(a_ts), Some(b_ts)) => a_ts.cmp(&b_ts),
    });
    files_with_ts.into_iter().map(|(p, _)| p.clone()).collect()
}

struct PartitionedParsedEvents {
    usage_events: Vec<NormalizedUsageEvent>,
    tool_events: Vec<ToolEvent>,
    diagnostic_events: Vec<OperationalDiagnosticEvent>,
    parse_errors: Vec<String>,
    codex_snapshots: Vec<CodexTokenSnapshot>,
    parse_error_count: usize,
}

fn partition_parsed_events(
    parsed_events: Vec<ParsedLogEvent>,
    source_id: &str,
) -> PartitionedParsedEvents {
    let mut usage_events = Vec::new();
    let mut tool_events = Vec::new();
    let mut diagnostic_events = Vec::new();
    let mut parse_errors = Vec::new();
    let mut codex_snapshots = Vec::new();
    let mut parse_error_count = 0;

    for parsed in parsed_events {
        match parsed {
            ParsedLogEvent::Normalized(ne) => match ne {
                NormalizedEvent::Usage(mut u) => {
                    enrich_cost(&mut u, CostMode::Auto);
                    usage_events.push(*u);
                }
                NormalizedEvent::OperationalDiagnostic(mut d) => {
                    if d.category == "parse_error" {
                        parse_error_count += 1;
                        parse_errors.push(d.message.clone());
                    }
                    if d.source_id.as_deref() == Some("") {
                        d.source_id = Some(source_id.to_string());
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

    PartitionedParsedEvents {
        usage_events,
        tool_events,
        diagnostic_events,
        parse_errors,
        codex_snapshots,
        parse_error_count,
    }
}

/// Perform a full historical scan of all discovered sources.
///
/// This is the main entry point for initial data ingestion. It:
/// 1. Upserts all log source rows
/// 2. For each file, reads from its last checkpoint offset
/// 3. Parses lines with the appropriate adapter
/// 4. Enriches events with pricing
/// 5. Builds aggregate mutations
/// 6. Commits atomically via ingest_store_batch (including rollups)
/// 7. After all files, rebuilds realtime_summary from full corpus
///
/// Returns scan statistics.
pub fn scan_once(
    db: &Database,
    adapters: &[Box<dyn AgentLogAdapter + Send + Sync>],
    sources: &[busytok_discovery::DiscoveredLogSource],
    event_bus: &AppEventBus,
    rtz: &ReportingTimezone,
    generation_id: &str,
) -> Result<ScanStats> {
    let mut stats = ScanStats {
        sources: sources.len(),
        ..Default::default()
    };

    let rollup_opts = RollupOptions { timezone: rtz.clone() };

    // Upsert all log source rows.
    for source in sources {
        let now_ms = now_ms();
        let row = build_log_source_row(source, now_ms, Some(now_ms), None, now_ms, now_ms);
        db.upsert_log_source(&row)
            .with_context(|| format!("failed to upsert source {}", source.source_id))?;
        let _ = event_bus.publish_ephemeral(AppEvent::ScanProgress {
            source_id: source.source_id.clone(),
            files_scanned: 0,
            events_ingested: 0,
        });
    }

    // Process each source and its files.
    for source in sources {
        let source_id = &source.source_id;
        let agent = &source.agent;

        let sorted_files = sorted_files_by_earliest_timestamp(&source.files);

        for file_path in &sorted_files {
            let file_id = derive_file_id(file_path);

            // Find the adapter for this file based on source.agent.
            // This is more reliable than filename-based heuristics.
            let adapter = adapters.iter().find(|a| {
                let adapter_agent = a.agent();
                match source.agent {
                    busytok_domain::AgentKind::ClaudeCode => {
                        matches!(adapter_agent, busytok_domain::AgentKind::ClaudeCode)
                    }
                    busytok_domain::AgentKind::Codex => {
                        matches!(adapter_agent, busytok_domain::AgentKind::Codex)
                    }
                }
            });

            if adapter.is_none() {
                debug!("No adapter found for file {}", file_path.display());
                continue;
            }
            let adapter = adapter.unwrap();

            // Defense-in-depth: skip non-.jsonl files, even if they were
            // erroneously included in the source file list.
            if !is_jsonl_file(file_path) {
                debug!("Skipping non-JSONL file {}", file_path.display());
                continue;
            }

            let write_policy = adapter.write_policy();

            // Get the current checkpoint offset and previous inode.
            let log_file_row = db.get_log_file(&file_id).ok().flatten();
            let offset = log_file_row
                .as_ref()
                .map(|r| r.offset_bytes as u64)
                .unwrap_or(0);
            let previous_inode = log_file_row.as_ref().and_then(|r| r.inode.clone());

            // Read the file from the last offset, with truncation/rotation detection.
            let request = ScanFileRequest {
                source_id: source_id.clone(),
                source_file_id: file_id.clone(),
                path: file_path.clone(),
                resume_offset: offset,
                previous_inode,
            };

            let batch = read_file_once(request)
                .with_context(|| format!("failed to read file {}", file_path.display()))?;

            // Read the current inode for storage.
            let current_inode = read_inode(file_path);

            if batch.lines.is_empty() {
                debug!("No new lines in {}", file_path.display());
                continue;
            }

            let parsed_events = parse_events_with_codex_model_context(
                db,
                &file_id,
                *agent,
                adapter.as_ref(),
                &batch.lines,
            );
            let partitioned = partition_parsed_events(parsed_events, source_id);
            let mut usage_events = partitioned.usage_events;
            let tool_events = partitioned.tool_events;
            let diagnostic_events = partitioned.diagnostic_events;
            let parse_errors = partitioned.parse_errors;
            let codex_snapshots = partitioned.codex_snapshots;
            let parse_error_count = partitioned.parse_error_count;

            // Convert Codex snapshots to usage deltas.
            let (codex_events, codex_snapshot_rows) =
                build_codex_delta_events(db, &codex_snapshots, batch.was_reset)
                    .context("failed to build Codex delta events")?;
            for mut event in codex_events {
                enrich_cost(&mut event, CostMode::Auto);
                usage_events.push(event);
            }

            // Capture event IDs, agents, and a by-id map before moving
            // events into the store batch. The closure uses these to
            // compute rollups from truly inserted events inside the
            // transaction, keeping events + rollups + checkpoint atomic.
            let all_event_info: Vec<(String, String)> = usage_events
                .iter()
                .map(|e| (e.id.clone(), e.agent.as_str().to_string()))
                .collect();
            let events_by_id: std::collections::HashMap<
                String,
                busytok_domain::NormalizedUsageEvent,
            > = usage_events
                .iter()
                .map(|e| (e.id.clone(), e.clone()))
                .collect();

            let usage_count = usage_events.len();

            let store_batch = StoreWriteBatch {
                source_id: source_id.clone(),
                source_file_id: Some(file_id.clone()),
                source_file_agent: agent.as_str().to_string(),
                source_file_path: file_path.display().to_string(),
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

            let ro = rollup_opts.clone();
            let scan_gen = generation_id.to_string();
            let ingest_result = db.ingest_store_batch(store_batch, generation_id, |inserted_events, replaced_old, gen_id| {
                // Build rollups from inserted events (full values) and
                // replaced events (delta = new - old).
                let mut rollup_events: Vec<busytok_domain::NormalizedUsageEvent> = inserted_events
                    .iter()
                    .map(|(_, event)| (*event).clone())
                    .collect();

                for old in replaced_old {
                    if let Some(new_event) = events_by_id.get(&old.event_id) {
                        rollup_events.push(old.compute_delta(new_event));
                    } else {
                        tracing::error!(
                            old_event_id = %old.event_id,
                            "replaced event has no corresponding new event in batch — rollup may be stale"
                        );
                    }
                }

                if rollup_events.is_empty() {
                    return Ok(busytok_store::RollupRows::default());
                }
                let mutations = build_scan_mutations(&rollup_events, ro.clone(), gen_id)
                    .context("failed to build rollup mutations")?;
                Ok(busytok_store::RollupRows {
                    daily_usage_rows: mutations.daily_usage,
                    model_usage_rows: Vec::new(),
                    session_rows: session_rollups_to_rows(&mutations.session_rollups),
                    project_rows: project_rollups_to_rows(&mutations.project_rollups),
                    model_summary_rows: model_rollups_to_rows(&mutations.model_rollups),
                })
            }).with_context(|| format!("failed to ingest batch for {}", file_path.display()))?;

            stats.files_scanned += 1;
            stats.events_found += usage_count;
            stats.diagnostics_found += parse_error_count;

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

            // Publish Error events for parse errors.
            for err_msg in &parse_errors {
                let _ = event_bus.publish_ephemeral(AppEvent::Error {
                    message: err_msg.clone(),
                    source: Some("scan".to_string()),
                });
            }

            let _ = event_bus.publish_ephemeral(AppEvent::ScanProgress {
                source_id: source_id.clone(),
                files_scanned: stats.files_scanned as u64,
                events_ingested: stats.events_found as u64,
            });
        }

        // Update the source's last_scan_completed_at_ms.
        let now_ms = now_ms();
        let row = build_log_source_row(source, now_ms, None, Some(now_ms), 0, 0);
        db.upsert_log_source(&row)
            .with_context(|| format!("failed to update source {}", source.source_id))?;
    }

    // Rebuild realtime summary from full corpus (post-transaction cache).
    let all_events = db
        .all_usage_events()
        .context("failed to read usage events for realtime summary")?;

    let transcript_paths: Vec<PathBuf> = sources
        .iter()
        .flat_map(|src| src.files.iter().cloned())
        .collect();
    let realtime_summary =
        build_full_realtime_summary(&all_events, rtz, Some(db), &transcript_paths)
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
        .context("failed to write realtime summary")?;
    let _ = event_bus.publish_ephemeral(AppEvent::SummaryUpdated {
        keys_updated: vec!["realtime_summary".to_string()],
    });

    info!(
        "Scan completed: {} sources, {} files, {} events",
        stats.sources, stats.files_scanned, stats.events_found
    );

    Ok(stats)
}

/// Build a full realtime summary including billing blocks, context window
/// info, and weekly usage aggregates, in addition to the base summary.
///
/// This is used by scan and tail to ensure consistent summary content across
/// all paths. `db` is optional and used for weekly usage; `transcript_paths`
/// is used for context window calculation.
pub fn build_full_realtime_summary(
    events: &[NormalizedUsageEvent],
    rtz: &ReportingTimezone,
    db: Option<&Database>,
    transcript_paths: &[PathBuf],
) -> Result<HashMap<String, serde_json::Value>> {
    let mut summary = busytok_aggregator::build_realtime_summary(events, rtz)?;

    // Add billing blocks (active session blocks with burn rate).
    let active_blocks: Vec<serde_json::Value> = identify_session_blocks(events, 5)
        .iter()
        .filter(|b| b.is_active)
        .map(|b| {
            let burn = calculate_burn_rate(b);
            serde_json::json!({
                "id": b.id,
                "start_time_ms": b.start_time_ms,
                "end_time_ms": b.end_time_ms,
                "is_active": b.is_active,
                "total_tokens": b.total_tokens,
                "cost_usd": b.cost_usd,
                "event_count": b.event_count,
                "burn_rate": burn.map(|br| serde_json::json!({
                    "tokens_per_minute": br.tokens_per_minute,
                    "cost_per_hour": br.cost_per_hour,
                    "status": format!("{:?}", br.status),
                })),
            })
        })
        .collect();
    summary.insert(
        "active_blocks".to_string(),
        serde_json::Value::Array(active_blocks),
    );

    // Prefer a fresh transcript-derived context window when transcript paths
    // are available. Otherwise preserve any previously computed value so tail
    // and rebuild paths do not erase it.
    if let Some(ci) = transcript_paths
        .iter()
        .find_map(|f| busytok_adapters::calculate_context_from_transcript(f))
    {
        summary.insert(
            "context_window_info".to_string(),
            serde_json::json!({
                "input_tokens": ci.input_tokens,
                "output_tokens": ci.output_tokens,
                "context_limit": ci.context_limit,
            }),
        );
    } else if let Some(db) = db {
        if let Ok(existing) = db.read_realtime_summary() {
            if let Some(existing_json) = existing.get("context_window_info") {
                if let Ok(existing_value) = serde_json::from_str::<serde_json::Value>(existing_json)
                {
                    summary.insert("context_window_info".to_string(), existing_value);
                }
            }
        }
    }

    // Add weekly usage aggregates if a database is available.
    if let Some(db) = db {
        let rollup_opts = RollupOptions { timezone: rtz.clone() };
        if let Ok(events) = db.all_usage_events() {
            if let Ok(weekly) = build_weekly_usage_value(&events, rollup_opts) {
                if let Some(arr) = weekly.as_array() {
                    if !arr.is_empty() {
                        summary.insert("weekly_usage".to_string(), weekly);
                    }
                }
            }
        }
    }

    Ok(summary)
}

/// Compute the delta between a current Codex snapshot and the previous one.
///
/// Follows the ccusage `subtractRawUsage` pattern: delta = current - previous.
/// First snapshot has no previous, so delta = total itself.
/// Reasoning tokens are informational and NOT added to output_tokens.
fn compute_codex_delta(
    current: &CodexTokenSnapshot,
    previous: Option<&CodexTokenSnapshotRow>,
) -> (i64, i64, i64, i64, i64) {
    let prev = previous.map(|p| {
        (
            p.input_tokens,
            p.cached_input_tokens,
            p.output_tokens,
            p.reasoning_tokens,
            p.total_tokens,
        )
    });

    let (prev_input, prev_cached, prev_output, prev_reasoning, prev_total) =
        prev.unwrap_or((0, 0, 0, 0, 0));

    let delta_input = (current.input_tokens - prev_input).max(0);
    let delta_cached = (current.cached_input_tokens - prev_cached).max(0);
    let delta_output = (current.output_tokens - prev_output).max(0);
    let delta_reasoning = (current.reasoning_tokens - prev_reasoning).max(0);
    let delta_total = (current.total_tokens - prev_total).max(0);

    (
        delta_input,
        delta_cached,
        delta_output,
        delta_reasoning,
        delta_total,
    )
}

fn codex_snapshot_delta(
    current: &CodexTokenSnapshot,
    previous: Option<&CodexTokenSnapshotRow>,
) -> (i64, i64, i64, i64, i64) {
    if let (
        Some(delta_input),
        Some(delta_cached),
        Some(delta_output),
        Some(delta_reasoning),
        Some(delta_total),
    ) = (
        current.delta_input_tokens,
        current.delta_cached_input_tokens,
        current.delta_output_tokens,
        current.delta_reasoning_tokens,
        current.delta_total_tokens,
    ) {
        return (
            delta_input.max(0),
            delta_cached.max(0),
            delta_output.max(0),
            delta_reasoning.max(0),
            delta_total.max(0),
        );
    }

    compute_codex_delta(current, previous)
}

/// Convert a batch of Codex cumulative snapshots into delta usage events
/// and snapshot persistence rows.
///
/// Within a batch, ordinal and previous-snapshot state is tracked in-memory
/// because DB queries won't see uncommitted snapshots from earlier in the
/// same batch.
pub fn build_codex_delta_events(
    db: &busytok_store::Database,
    snapshots: &[CodexTokenSnapshot],
    was_reset: bool,
) -> Result<(Vec<NormalizedUsageEvent>, Vec<CodexTokenSnapshotRow>)> {
    if snapshots.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    // Pre-load existing state from DB for all scopes we'll need.
    // If the file was restarted (rotation/truncation), skip persisted
    // baselines: the new cumulative counters may be lower, and old
    // snapshots no longer represent a valid baseline for deltas.
    type ScopeKey = (String, String, Option<String>);
    let mut scope_state: std::collections::HashMap<ScopeKey, (i64, Option<CodexTokenSnapshotRow>)> =
        std::collections::HashMap::new();

    for snap in snapshots {
        let key = (
            snap.source_file_id.clone(),
            snap.session_id.clone(),
            snap.turn_id.clone(),
        );
        if !scope_state.contains_key(&key) {
            let turn_id_str = snap.turn_id.as_deref().unwrap_or("");
            let ordinal = if was_reset {
                1 // start ordinal from 1 after reset
            } else {
                db.next_codex_ordinal(&snap.source_file_id, &snap.session_id, turn_id_str)
                    .context("failed to get next codex ordinal")?
            };
            let previous = if was_reset {
                None // no baseline after file restart
            } else {
                db.get_latest_codex_snapshot(&snap.source_file_id, &snap.session_id, turn_id_str)
                    .context("failed to get latest codex snapshot")?
            };
            scope_state.insert(key, (ordinal, previous));
        }
    }

    let mut events = Vec::with_capacity(snapshots.len());
    let mut rows = Vec::with_capacity(snapshots.len());

    for snap in snapshots {
        let key = (
            snap.source_file_id.clone(),
            snap.session_id.clone(),
            snap.turn_id.clone(),
        );
        let (ordinal, previous) = scope_state.get_mut(&key).expect("scope entry missing");

        let current_ordinal = *ordinal;
        *ordinal += 1;

        let (delta_input, delta_cached, delta_output, delta_reasoning, delta_total) =
            codex_snapshot_delta(snap, previous.as_ref());

        let now = now_ms();
        let emitted_event_id = if explicit_zero_component_codex_delta(snap) {
            None
        } else {
            let model = resolve_codex_event_model(snap, previous.as_ref());
            let event_id = codex_usage_event_id(
                &snap.session_id,
                snap.timestamp_ms,
                delta_input,
                delta_cached,
                delta_output,
                delta_reasoning,
                delta_total,
            );
            let fingerprint = MetadataFingerprint::new("codex", &snap.session_id)
                .turn_id(snap.turn_id.as_deref().unwrap_or(""))
                .tokens(delta_input, delta_output)
                .total_tokens(delta_total);
            let raw_event_hash = metadata_event_hash(&fingerprint);

            let event = NormalizedUsageEvent {
                id: event_id.clone(),
                agent: AgentKind::Codex,
                source_file_id: snap.source_file_id.clone(),
                source_path: snap.source_path.clone(),
                source_line: snap.source_line,
                source_offset_start: snap.source_offset_start,
                source_offset_end: snap.source_offset_end,
                session_id: snap.session_id.clone(),
                turn_id: snap.turn_id.clone(),
                source_request_id: None,
                message_id: None,
                timestamp_ms: snap.timestamp_ms,
                project_path: None,
                project_hash: None,
                cwd: None,
                model,
                model_provider: snap.model_provider.clone(),
                agent_version: None,
                client_kind: Some("codex".to_string()),
                speed: None,
                input_tokens: delta_input,
                output_tokens: delta_output,
                total_tokens: delta_total,
                cached_input_tokens: delta_cached,
                cache_creation_tokens: 0,
                cache_read_tokens: delta_cached,
                reasoning_tokens: delta_reasoning,
                thoughts_tokens: 0,
                tool_tokens: 0,
                cost_usd: None,           // cumulative cost cannot be used as delta
                estimated_cost_usd: None, // populated by enrich_cost below
                cost_currency: Some("USD".to_string()),
                cost_source: Some("unknown".to_string()),
                price_catalog_version: None,
                is_error: false,
                error_type: None,
                usage_limit_reset_time_ms: None,
                raw_event_hash,
                created_at_ms: now,
                updated_at_ms: now,
            };
            events.push(event);
            Some(event_id)
        };

        rows.push(CodexTokenSnapshotRow::from_domain(
            snap,
            current_ordinal,
            emitted_event_id.clone(),
        ));

        // Update in-memory previous for next iteration in this batch.
        *previous = Some(CodexTokenSnapshotRow {
            id: format!(
                "codex-snap:{}:{}:{}:{}",
                snap.source_file_id,
                snap.session_id,
                snap.turn_id.as_deref().unwrap_or("none"),
                current_ordinal
            ),
            source_file_id: snap.source_file_id.clone(),
            source_line: snap.source_line as i64,
            source_offset_start: snap.source_offset_start as i64,
            source_offset_end: snap.source_offset_end as i64,
            session_id: snap.session_id.clone(),
            turn_id: snap.turn_id.clone(),
            token_event_ordinal: current_ordinal,
            input_tokens: snap.input_tokens,
            cached_input_tokens: snap.cached_input_tokens,
            output_tokens: snap.output_tokens,
            reasoning_tokens: snap.reasoning_tokens,
            total_tokens: snap.total_tokens,
            model: snap.model.clone(),
            raw_usage_json: snap.raw_usage_json.clone(),
            emitted_event_id,
            created_at_ms: now,
            updated_at_ms: now,
        });
    }

    Ok((events, rows))
}

/// Read the earliest timestamp from a JSONL file by scanning the first few lines.
///
/// Opens the file, reads line by line, and attempts to parse an RFC 3339
/// timestamp from the `"timestamp"` field of the first JSON object that has one.
/// Returns the timestamp in milliseconds since Unix epoch.
fn get_earliest_timestamp_ms(path: &Path) -> Option<i64> {
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut earliest: Option<i64> = None;
    // Cap scan to 1000 lines to avoid reading huge files.
    let max_lines = 1000;

    for (line_idx, line) in reader.lines().flatten().enumerate() {
        if line_idx >= max_lines {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Try to parse as JSON and extract timestamp
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(ts_str) = val.get("timestamp").and_then(|v| v.as_str()) {
                if let Ok(dt) = time::OffsetDateTime::parse(
                    ts_str,
                    &time::format_description::well_known::Rfc3339,
                ) {
                    let ms = dt.unix_timestamp() * 1000 + dt.millisecond() as i64;
                    match earliest {
                        None => earliest = Some(ms),
                        Some(ref mut e) if ms < *e => *e = ms,
                        _ => {}
                    }
                }
            }
        }
        // NOTE: intentionally NOT breaking on first match so we find the
        // true minimum timestamp across the first 1000 lines.
    }

    earliest
}

/// Perform a full historical scan using the bounded writer actor.
///
/// This is the writer-actor variant of [`scan_once`]. Instead of writing
/// directly to the database, it parses and enriches events, then sends
/// `RebuildBatch` and `ProgressCheckpoint` commands through the writer
/// handle. The writer actor owns the single SQLite write connection,
/// guaranteeing sequential, un-contended writes.
///
/// This function requires an active Tokio runtime. When no runtime is
/// available, callers should fall back to the synchronous [`scan_once`].
pub async fn scan_once_via_writer(
    db: &Database,
    adapters: &[Box<dyn AgentLogAdapter + Send + Sync>],
    sources: &[busytok_discovery::DiscoveredLogSource],
    event_bus: &AppEventBus,
    rtz: &ReportingTimezone,
    writer_handle: &crate::writer::WriterHandle,
    generation_id: &str,
) -> Result<ScanStats> {
    let mut stats = ScanStats {
        sources: sources.len(),
        ..Default::default()
    };

    // Upsert all log source rows through the writer so the scan path never
    // contends for the SQLite write connection.
    for source in sources {
        let now_ms = now_ms();
        let row = build_log_source_row(source, now_ms, Some(now_ms), None, now_ms, now_ms);
        writer_handle
            .send(crate::writer::WriteCommand::LogSourceUpsert(
                crate::writer::LogSourceUpsertCommand { row },
            ))
            .await
            .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))
            .with_context(|| format!("failed to enqueue source {}", source.source_id))?;
        let _ = event_bus.publish_ephemeral(AppEvent::ScanProgress {
            source_id: source.source_id.clone(),
            files_scanned: 0,
            events_ingested: 0,
        });
    }

    // Process each source and its files.
    for source in sources {
        let source_id = &source.source_id;
        let agent = &source.agent;

        let sorted_files = sorted_files_by_earliest_timestamp(&source.files);

        for file_path in &sorted_files {
            let file_id = derive_file_id(file_path);

            let adapter = adapters.iter().find(|a| {
                let adapter_agent = a.agent();
                match source.agent {
                    busytok_domain::AgentKind::ClaudeCode => {
                        matches!(adapter_agent, busytok_domain::AgentKind::ClaudeCode)
                    }
                    busytok_domain::AgentKind::Codex => {
                        matches!(adapter_agent, busytok_domain::AgentKind::Codex)
                    }
                }
            });

            if adapter.is_none() {
                debug!("No adapter found for file {}", file_path.display());
                continue;
            }
            let adapter = adapter.unwrap();

            // Defense-in-depth: skip non-.jsonl files, even if they were
            // erroneously included in the source file list.
            if !is_jsonl_file(file_path) {
                debug!("Skipping non-JSONL file {}", file_path.display());
                continue;
            }

            let write_policy = adapter.write_policy();

            // Get the current checkpoint offset and previous inode.
            let log_file_row = db.get_log_file(&file_id).ok().flatten();
            let offset = log_file_row
                .as_ref()
                .map(|r| r.offset_bytes as u64)
                .unwrap_or(0);
            let previous_inode = log_file_row.as_ref().and_then(|r| r.inode.clone());

            // Read the file.
            let request = ScanFileRequest {
                source_id: source_id.clone(),
                source_file_id: file_id.clone(),
                path: file_path.clone(),
                resume_offset: offset,
                previous_inode,
            };

            let batch = read_file_once(request)
                .with_context(|| format!("failed to read file {}", file_path.display()))?;

            let current_inode = read_inode(file_path);

            if batch.lines.is_empty() {
                debug!("No new lines in {}", file_path.display());
                continue;
            }

            let parsed_events = parse_events_with_codex_model_context(
                db,
                &file_id,
                *agent,
                adapter.as_ref(),
                &batch.lines,
            );
            let partitioned = partition_parsed_events(parsed_events, source_id);
            let mut usage_events = partitioned.usage_events;
            let tool_events = partitioned.tool_events;
            let diagnostic_events = partitioned.diagnostic_events;
            let parse_errors = partitioned.parse_errors;
            let codex_snapshots = partitioned.codex_snapshots;
            let parse_error_count = partitioned.parse_error_count;

            // Convert Codex snapshots to usage deltas.
            let (codex_events, codex_snapshot_rows) =
                build_codex_delta_events(db, &codex_snapshots, batch.was_reset)
                    .context("failed to build Codex delta events")?;
            for mut event in codex_events {
                enrich_cost(&mut event, CostMode::Auto);
                usage_events.push(event);
            }

            let usage_count = usage_events.len();

            // Capture event IDs and agents before sending to writer.
            let all_event_info: Vec<(String, String)> = usage_events
                .iter()
                .map(|e| (e.id.clone(), e.agent.as_str().to_string()))
                .collect();

            // Send RebuildBatch command through the writer actor.
            let cmd =
                crate::writer::WriteCommand::RebuildBatch(crate::writer::RebuildBatchCommand {
                    source_id: source_id.clone(),
                    source_file_id: Some(file_id.clone()),
                    source_file_agent: agent.as_str().to_string(),
                    source_file_path: file_path.display().to_string(),
                    source_file_inode: current_inode,
                    events: usage_events,
                    tool_events,
                    diagnostic_events,
                    codex_snapshots: codex_snapshot_rows,
                    generation_id: generation_id.to_string(),
                    checkpoint_offset: Some(batch.checkpoint_offset),
                    is_final_batch: false,
                    write_policy,
                });
            writer_handle
                .send(cmd)
                .await
                .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))?;

            stats.files_scanned += 1;
            stats.events_found += usage_count;
            stats.diagnostics_found += parse_error_count;

            // Publish progress.
            for (event_id, agent) in &all_event_info {
                let _ = event_bus.publish_ephemeral(AppEvent::UsageEventInserted {
                    event_id: event_id.clone(),
                    agent: agent.clone(),
                });
            }
            for err_msg in &parse_errors {
                let _ = event_bus.publish_ephemeral(AppEvent::Error {
                    message: err_msg.clone(),
                    source: Some("scan".to_string()),
                });
            }

            let _ = event_bus.publish_ephemeral(AppEvent::ScanProgress {
                source_id: source_id.clone(),
                files_scanned: stats.files_scanned as u64,
                events_ingested: stats.events_found as u64,
            });
        }

        // Update the source's last_scan_completed_at_ms.
        let now_ms = now_ms();
        let row = build_log_source_row(source, now_ms, None, Some(now_ms), 0, 0);
        writer_handle
            .send(crate::writer::WriteCommand::LogSourceUpsert(
                crate::writer::LogSourceUpsertCommand { row },
            ))
            .await
            .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))
            .with_context(|| format!("failed to enqueue source completion {}", source.source_id))?;
    }

    // Rebuild realtime summary from the full corpus after all event writes
    // have landed. Without this barrier the summary can lag behind the scan
    // completion signal because the writer actor is asynchronous.
    writer_handle
        .flush()
        .await
        .context("failed to flush writer before realtime summary rebuild")?;

    let all_events = db
        .all_usage_events()
        .context("failed to read usage events for realtime summary")?;

    let transcript_paths: Vec<PathBuf> = sources
        .iter()
        .flat_map(|src| src.files.iter().cloned())
        .collect();
    let realtime_summary =
        build_full_realtime_summary(&all_events, rtz, Some(db), &transcript_paths)
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
    writer_handle
        .send(crate::writer::WriteCommand::RealtimeSummaryReplace(
            crate::writer::RealtimeSummaryReplaceCommand {
                entries: summary_entries,
            },
        ))
        .await
        .map_err(|e| anyhow::anyhow!("writer channel send error: {e}"))
        .context("failed to enqueue realtime summary replacement")?;
    writer_handle
        .flush()
        .await
        .context("failed to flush writer after realtime summary rebuild")?;
    let _ = event_bus.publish_ephemeral(AppEvent::SummaryUpdated {
        keys_updated: vec!["realtime_summary".to_string()],
    });

    info!(
        "Scan (via writer) completed: {} sources, {} files, {} events",
        stats.sources, stats.files_scanned, stats.events_found
    );

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use busytok_domain::AgentKind;
    use busytok_store::Database;

    #[test]
    fn derive_file_id_from_absolute_path() {
        let id = derive_file_id(Path::new("/home/user/.claude/projects/test/session.jsonl"));
        assert!(id.starts_with("file_"));
        // SHA-256 hex digest is 64 characters; total length is 5 (prefix) + 64.
        assert_eq!(id.len(), 69);
        // No slashes in the hex digest.
        assert!(!id.contains('/'));
    }

    #[test]
    fn derive_file_id_from_relative_path() {
        let id = derive_file_id(Path::new("test/file.jsonl"));
        assert!(id.starts_with("file_"));
        assert_eq!(id.len(), 69);
    }

    #[test]
    fn derive_file_id_is_collision_resistant() {
        // These paths would collide under the old underscore-replacement scheme
        // (both would produce "file_a_b_c"), but SHA-256 distinguishes them.
        let id1 = derive_file_id(Path::new("a/b_c"));
        let id2 = derive_file_id(Path::new("a/b/c"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn derive_file_id_is_deterministic() {
        let id1 = derive_file_id(Path::new("/some/path/to/file.jsonl"));
        let id2 = derive_file_id(Path::new("/some/path/to/file.jsonl"));
        assert_eq!(id1, id2);
    }

    #[test]
    fn enrich_cost_sets_catalog_version() {
        let mut event = NormalizedUsageEvent::minimal_for_test("test-1", AgentKind::ClaudeCode);
        event.model = Some("claude-sonnet-4-6".to_string());
        event.input_tokens = 100;
        event.output_tokens = 50;

        enrich_cost(&mut event, CostMode::Auto);

        assert!(event.price_catalog_version.is_some());
        assert!(event.estimated_cost_usd.is_some());
        assert!(event.estimated_cost_usd.unwrap() > 0.0);
    }

    #[test]
    fn enrich_cost_prefers_source_cost() {
        let mut event = NormalizedUsageEvent::minimal_for_test("test-2", AgentKind::ClaudeCode);
        event.cost_usd = Some(0.001);
        event.cost_source = Some("source".to_string());
        event.model = Some("claude-sonnet-4-6".to_string());
        event.input_tokens = 100;
        event.output_tokens = 50;

        enrich_cost(&mut event, CostMode::Auto);

        assert_eq!(event.cost_usd, Some(0.001));
        assert!(event.estimated_cost_usd.is_some());
        assert_eq!(event.cost_source.as_deref(), Some("source"));
        assert!(event.price_catalog_version.is_some());
    }

    #[test]
    fn enrich_cost_unknown_model() {
        let mut event = NormalizedUsageEvent::minimal_for_test("test-3", AgentKind::ClaudeCode);
        event.model = Some("nonexistent-model".to_string());
        event.input_tokens = 1000;
        event.output_tokens = 500;

        enrich_cost(&mut event, CostMode::Auto);

        assert!(event.estimated_cost_usd.is_none());
        assert!(event.price_catalog_version.is_some());
    }

    #[test]
    fn enrich_cost_sets_estimated_provenance_when_no_source_cost() {
        let mut event = NormalizedUsageEvent::minimal_for_test("test-4", AgentKind::ClaudeCode);
        event.cost_usd = None;
        event.cost_source = Some("unknown".to_string());
        event.model = Some("claude-sonnet-4-6".to_string());
        event.input_tokens = 100;
        event.output_tokens = 50;

        enrich_cost(&mut event, CostMode::Auto);

        assert!(event.cost_usd.is_some());
        assert!(event.estimated_cost_usd.is_some());
        assert_eq!(event.cost_source.as_deref(), Some("estimated"));
    }

    #[test]
    fn build_full_realtime_summary_preserves_existing_context_window_when_transcripts_absent() {
        let db = Database::open_in_memory().expect("db");
        db.replace_realtime_summary(&[(
            "context_window_info".to_string(),
            serde_json::json!({
                "input_tokens": 1234,
                "output_tokens": 567,
                "context_limit": 200000
            })
            .to_string(),
        )])
        .expect("seed realtime summary");

        let rtz = ReportingTimezone::parse("Asia/Shanghai").unwrap();
        let summary =
            build_full_realtime_summary(&[], &rtz, Some(&db), &[]).expect("summary");

        let context = summary
            .get("context_window_info")
            .expect("context_window_info should be preserved");
        assert_eq!(context["input_tokens"], 1234);
        assert_eq!(context["output_tokens"], 567);
        assert_eq!(context["context_limit"], 200000);
    }

    #[test]
    fn build_full_realtime_summary_prefers_fresh_transcript_context_over_stored_value() {
        let db = Database::open_in_memory().expect("db");
        db.replace_realtime_summary(&[(
            "context_window_info".to_string(),
            serde_json::json!({
                "input_tokens": 1,
                "output_tokens": 2,
                "context_limit": 3
            })
            .to_string(),
        )])
        .expect("seed realtime summary");

        let dir = tempfile::tempdir().expect("tempdir");
        let transcript_path = dir.path().join("transcript.jsonl");
        std::fs::write(
            &transcript_path,
            "{\"type\":\"assistant\",\"message\":{\"usage\":{\"input_tokens\":100,\"output_tokens\":50,\"cache_creation_input_tokens\":10,\"cache_read_input_tokens\":5}}}\n",
        )
        .expect("write transcript");

        let rtz = ReportingTimezone::parse("Asia/Shanghai").unwrap();
        let summary =
            build_full_realtime_summary(&[], &rtz, Some(&db), &[transcript_path])
                .expect("summary");

        let context = summary
            .get("context_window_info")
            .expect("context_window_info should be present");
        assert_eq!(context["input_tokens"], 115);
        assert_eq!(context["output_tokens"], 50);
        assert_eq!(context["context_limit"], 200000);
    }

    #[test]
    fn codex_usage_event_id_does_not_depend_on_model_resolution() {
        let first = codex_usage_event_id("sess-1", 1_747_712_000_000, 10, 2, 5, 1, 18);
        let second = codex_usage_event_id("sess-1", 1_747_712_000_000, 10, 2, 5, 1, 18);
        assert_eq!(
            first, second,
            "Codex usage event identity should stay stable when model resolution changes across runs"
        );
    }

    // ── is_jsonl_file ──────────────────────────────────────────────────

    #[test]
    fn is_jsonl_file_accepts_jsonl_extension() {
        assert!(is_jsonl_file(Path::new("session.jsonl")));
        assert!(is_jsonl_file(Path::new("/a/b/c/session.jsonl")));
    }

    #[test]
    fn is_jsonl_file_rejects_other_extensions() {
        assert!(!is_jsonl_file(Path::new("session.log")));
        assert!(!is_jsonl_file(Path::new("config.json")));
        assert!(!is_jsonl_file(Path::new("file.txt")));
        assert!(!is_jsonl_file(Path::new("file.jsonl.lock")));
        assert!(!is_jsonl_file(Path::new("file.backup.1779511134661")));
        assert!(!is_jsonl_file(Path::new("file@v1")));
    }

    #[test]
    fn is_jsonl_file_rejects_no_extension() {
        assert!(!is_jsonl_file(Path::new("README")));
        assert!(!is_jsonl_file(Path::new("/path/to/file")));
    }

    #[test]
    fn is_jsonl_file_is_case_sensitive() {
        // Matching the discovery layer: only lowercase "jsonl" is accepted.
        assert!(!is_jsonl_file(Path::new("session.JSONL")));
        assert!(!is_jsonl_file(Path::new("session.JsOnL")));
    }

    // ── parse error cap ────────────────────────────────────────────────

    use busytok_adapters::{ClaudeCodeAdapter, CodexAdapter};
    use busytok_tailer::TailedLine;

    fn make_tailed_line(text: &str, line_num: u64) -> TailedLine {
        TailedLine {
            text: text.to_string(),
            context: busytok_domain::ParseContext {
                source_file_id: "f-cap".to_string(),
                source_path: "/fake/cap-test".to_string(),
                inode: None,
                source_line: line_num,
                source_offset_start: 0,
                source_offset_end: text.len() as u64,
                replay_sequence: 0,
            },
        }
    }

    #[test]
    fn parse_error_cap_stops_at_max_and_emits_summary() {
        let db = Database::open_in_memory().unwrap();
        let adapter = ClaudeCodeAdapter;
        let file_id = "file-cap-test";

        // Build 105 lines of garbage so lines 0..99 generate per-line
        // diagnostics and lines 100..104 are suppressed behind one summary.
        let lines: Vec<_> = (0..105)
            .map(|i| make_tailed_line(&format!("{{not valid json {i}}}"), i))
            .collect();

        let events = parse_events_with_codex_model_context(
            &db,
            file_id,
            AgentKind::ClaudeCode,
            &adapter,
            &lines,
        );

        // 100 per-line + 1 summary = 101
        assert_eq!(events.len(), 101);

        // Last event must be the summary.
        if let ParsedLogEvent::Normalized(NormalizedEvent::OperationalDiagnostic(diag)) =
            &events[100]
        {
            assert_eq!(diag.id, format!("parse_err_cap_{file_id}"));
            assert_eq!(diag.severity, "warning");
            assert!(
                diag.message.contains("105"),
                "summary should mention total line count: {}",
                diag.message
            );
        } else {
            panic!("last event should be the cap summary diagnostic");
        }
    }

    #[test]
    fn parse_error_cap_not_triggered_below_max() {
        let db = Database::open_in_memory().unwrap();
        let adapter = ClaudeCodeAdapter;
        let file_id = "file-below-cap";

        let lines: Vec<_> = (0..50)
            .map(|i| make_tailed_line(&format!("{{bad json {i}}}"), i))
            .collect();

        let events = parse_events_with_codex_model_context(
            &db,
            file_id,
            AgentKind::ClaudeCode,
            &adapter,
            &lines,
        );

        // 50 per-line diagnostics, no summary
        assert_eq!(events.len(), 50);
        for event in &events {
            if let ParsedLogEvent::Normalized(NormalizedEvent::OperationalDiagnostic(diag)) = event
            {
                assert!(
                    !diag.id.starts_with("parse_err_cap_"),
                    "cap summary should not appear below threshold"
                );
            }
        }
    }

    #[test]
    fn codex_model_inherits_from_previous_token_count_info_model() {
        let db = Database::open_in_memory().unwrap();
        let adapter = CodexAdapter;
        let file_id = "model-inherit-test-file";

        // First token_count has info.model; second doesn't.
        let line1 = r#"{"timestamp":"2026-05-20T07:16:22.000Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5.4","total_token_usage":{"input_tokens":100,"output_tokens":50,"total_tokens":150}}}}"#;
        let line2 = r#"{"timestamp":"2026-05-20T07:16:23.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":200,"output_tokens":100,"total_tokens":300}}}}"#;

        let lines = vec![make_tailed_line(line1, 0), make_tailed_line(line2, 1)];

        let events =
            parse_events_with_codex_model_context(&db, file_id, AgentKind::Codex, &adapter, &lines);

        let snapshots: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                ParsedLogEvent::CodexTokenSnapshot(s) => Some(s),
                _ => None,
            })
            .collect();

        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].model.as_deref(), Some("gpt-5.4"));
        assert_eq!(
            snapshots[1].model.as_deref(),
            Some("gpt-5.4"),
            "second snapshot should inherit model from the first"
        );
    }
}
