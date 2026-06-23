//! Store write-query primitives.
//!
//! These are low-level helpers called by the writer actor (Task 5) and
//! rebuild/promotion logic (Task 7). Functions that modify multiple rows
//! use transactions internally to keep operations atomic.

use anyhow::{Context, Result};
use busytok_domain::now_ms;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::BTreeSet;
use tracing::{debug, info};

// ── Usage events batch ───────────────────────────────────────────────────────

/// Idempotent insert used for `WritePolicy::InsertOnce` agents (Codex): a row
/// with the same `id` is silently kept as-is.
const USAGE_INSERT_IGNORE_SQL: &str = "\
    INSERT OR IGNORE INTO usage_events (\
        id, agent, source_file_id, source_path, source_line, \
        source_offset_start, source_offset_end, session_id, turn_id, \
        source_request_id, message_id, timestamp_ms, project_path, \
        project_hash, cwd, model, model_provider, agent_version, \
        client_kind, speed, input_tokens, output_tokens, total_tokens, \
        cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
        reasoning_tokens, thoughts_tokens, tool_tokens, cost_usd, \
        estimated_cost_usd, cost_currency, cost_source, \
        price_catalog_version, is_error, error_type, raw_event_hash, \
        usage_limit_reset_time_ms, created_at_ms, updated_at_ms, \
        generation_id, dedupe_key, is_sidechain\
    ) VALUES (\
        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, \
        ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, \
        ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, \
        ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40, \
        ?41, ?42, ?43\
    )";

/// Upsert used for `WritePolicy::Replace` agents (Claude Code): on a primary
/// key (`id`) conflict, mutable columns are refreshed while `created_at_ms` is
/// preserved. Sidechain-aware cross-row collapse is handled separately via
/// [`remove_other_dedupe_rows`] before this runs.
const USAGE_UPSERT_BY_ID_SQL: &str = "\
    INSERT INTO usage_events (\
        id, agent, source_file_id, source_path, source_line, \
        source_offset_start, source_offset_end, session_id, turn_id, \
        source_request_id, message_id, timestamp_ms, project_path, \
        project_hash, cwd, model, model_provider, agent_version, \
        client_kind, speed, input_tokens, output_tokens, total_tokens, \
        cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
        reasoning_tokens, thoughts_tokens, tool_tokens, cost_usd, \
        estimated_cost_usd, cost_currency, cost_source, \
        price_catalog_version, is_error, error_type, raw_event_hash, \
        usage_limit_reset_time_ms, created_at_ms, updated_at_ms, \
        generation_id, dedupe_key, is_sidechain\
    ) VALUES (\
        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, \
        ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, \
        ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, \
        ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40, \
        ?41, ?42, ?43\
    ) ON CONFLICT(id) DO UPDATE SET \
        speed = excluded.speed, \
        usage_limit_reset_time_ms = excluded.usage_limit_reset_time_ms, \
        input_tokens = excluded.input_tokens, \
        output_tokens = excluded.output_tokens, \
        total_tokens = excluded.total_tokens, \
        cached_input_tokens = excluded.cached_input_tokens, \
        cache_creation_tokens = excluded.cache_creation_tokens, \
        cache_read_tokens = excluded.cache_read_tokens, \
        reasoning_tokens = excluded.reasoning_tokens, \
        thoughts_tokens = excluded.thoughts_tokens, \
        tool_tokens = excluded.tool_tokens, \
        cost_usd = excluded.cost_usd, \
        estimated_cost_usd = excluded.estimated_cost_usd, \
        cost_source = excluded.cost_source, \
        price_catalog_version = excluded.price_catalog_version, \
        raw_event_hash = excluded.raw_event_hash, \
        dedupe_key = excluded.dedupe_key, \
        is_sidechain = excluded.is_sidechain, \
        created_at_ms = usage_events.created_at_ms, \
        updated_at_ms = CASE WHEN excluded.updated_at_ms > usage_events.updated_at_ms \
            THEN excluded.updated_at_ms ELSE usage_events.updated_at_ms END";

/// Bind a single event's 43 columns for either usage-event statement. A macro
/// (rather than a function) sidesteps the multi-input lifetime that a returned
/// `Vec<&dyn ToSql>` would require.
macro_rules! usage_event_params {
    ($event:expr, $generation_id:expr, $dedupe_key:expr) => {
        rusqlite::params![
            $event.id,
            $event.agent.as_str(),
            $event.source_file_id,
            $event.source_path,
            $event.source_line as i64,
            $event.source_offset_start as i64,
            $event.source_offset_end as i64,
            $event.session_id,
            $event.turn_id,
            $event.source_request_id,
            $event.message_id,
            $event.timestamp_ms,
            $event.project_path,
            $event.project_hash,
            $event.cwd,
            $event.model,
            $event.model_provider,
            $event.agent_version,
            $event.client_kind,
            $event.speed,
            $event.input_tokens,
            $event.output_tokens,
            $event.total_tokens,
            $event.cached_input_tokens,
            $event.cache_creation_tokens,
            $event.cache_read_tokens,
            $event.reasoning_tokens,
            $event.thoughts_tokens,
            $event.tool_tokens,
            $event.cost_usd,
            $event.estimated_cost_usd,
            $event.cost_currency,
            $event.cost_source.as_deref().unwrap_or("unknown"),
            $event.price_catalog_version,
            $event.is_error,
            $event.error_type,
            $event.raw_event_hash,
            $event.usage_limit_reset_time_ms,
            $event.created_at_ms,
            $event.updated_at_ms,
            $generation_id,
            $dedupe_key,
            $event.is_sidechain,
        ]
    };
}

/// Existing row sharing a dedupe key, for sidechain-aware winner selection.
#[derive(Clone)]
struct ExistingRow {
    id: String,
    is_sidechain: bool,
    total_tokens: i64,
    tokens: crate::OldEventTokens,
}

/// Fetch the existing row for a `(generation_id, dedupe_key)`, if any.
fn fetch_existing_for_dedupe(
    conn: &Connection,
    generation_id: &str,
    dedupe_key: &str,
) -> Result<Option<ExistingRow>> {
    conn.query_row(
        "SELECT id, is_sidechain, total_tokens, \
                input_tokens, output_tokens, cached_input_tokens, \
                cache_creation_tokens, cache_read_tokens, reasoning_tokens, \
                thoughts_tokens, tool_tokens, cost_usd, estimated_cost_usd \
         FROM usage_events WHERE generation_id = ?1 AND dedupe_key = ?2",
        params![generation_id, dedupe_key],
        |row| {
            Ok(ExistingRow {
                id: row.get(0)?,
                is_sidechain: row.get::<_, i32>(1)? != 0,
                total_tokens: row.get(2)?,
                tokens: crate::OldEventTokens {
                    event_id: row.get(0)?,
                    input_tokens: row.get(3)?,
                    output_tokens: row.get(4)?,
                    total_tokens: row.get(2)?,
                    cached_input_tokens: row.get(5)?,
                    cache_creation_tokens: row.get(6)?,
                    cache_read_tokens: row.get(7)?,
                    reasoning_tokens: row.get(8)?,
                    thoughts_tokens: row.get(9)?,
                    tool_tokens: row.get(10)?,
                    cost_usd: row.get(11)?,
                    estimated_cost_usd: row.get(12)?,
                },
            })
        },
    )
    .optional()
    .map_err(|e| anyhow::anyhow!("failed to fetch existing dedupe row: {e}"))
}

/// Delete any row sharing a dedupe key with a *different* id, so the upcoming
/// upsert leaves exactly one row per dedupe key. Returns the deleted count.
fn remove_other_dedupe_rows(
    conn: &Connection,
    generation_id: &str,
    dedupe_key: &str,
    keep_id: &str,
) -> Result<usize> {
    let deleted = conn.execute(
        "DELETE FROM usage_events WHERE generation_id = ?1 AND dedupe_key = ?2 AND id <> ?3",
        params![generation_id, dedupe_key, keep_id],
    )?;
    Ok(deleted)
}

/// Decide whether a new event should replace an existing row.
///
/// Mirrors ccusage's `should_replace_deduped_entry`: a non-sidechain entry
/// always beats a sidechain one (parent over `/btw` replay); within the same
/// sidechain class, a strictly-higher `total_tokens` wins; ties keep the
/// existing row (ccusage uses strict `>`).
fn new_beats_existing(new: &busytok_domain::NormalizedUsageEvent, existing: &ExistingRow) -> bool {
    if existing.is_sidechain && !new.is_sidechain {
        return true;
    }
    if !existing.is_sidechain && new.is_sidechain {
        return false;
    }
    new.total_tokens > existing.total_tokens
}

/// Outcome of a dedup-aware usage-event batch write.
pub struct DedupOutcome {
    /// Token deltas to feed additive rollup updaters: a full event for each
    /// newly inserted row, plus a `new − old` delta event for each row that
    /// replaced an existing one. Dropped (losing) candidates contribute
    /// nothing — they were never counted.
    pub effective_events: Vec<busytok_domain::NormalizedUsageEvent>,
    /// IDs of rows newly created (new logical events; drives outbox
    /// notifications and per-event publish during rebuild).
    pub inserted_ids: Vec<String>,
    /// Rows newly created (new logical events; drives outbox notifications).
    pub inserted: i64,
    /// Rows that replaced an existing row (same or different id).
    pub replaced: i64,
    /// Candidate events dropped because an existing row won the comparison.
    pub dropped: i64,
}

/// Insert or upsert a batch of usage events with sidechain-aware dedup.
///
/// This is the single write entry point for both the live tailer and the
/// historical rebuild paths. Per-event behavior is driven by [`UsageWritePolicy`]:
///
/// - `InsertOnce` (Codex): idempotent `INSERT OR IGNORE` keyed by `id`.
/// - `Replace` (Claude Code): the event's `dedupe_key` (message-scoped) decides
///   cross-row collapse. A non-sidechain entry replaces a sidechain replay of
///   the same `message_id`; within a class the higher-total entry wins. The
///   displaced row's tokens are captured so rollups receive a `new − old` delta.
///
/// Within a single call, intra-batch collisions on the same dedupe key are
/// resolved in input order so only the final winner is persisted.
pub fn upsert_usage_events_dedup_aware(
    conn: &Connection,
    events: &[busytok_domain::NormalizedUsageEvent],
    policies: &[busytok_domain::UsageWritePolicy],
    generation_id: &str,
) -> Result<DedupOutcome> {
    assert_eq!(
        events.len(),
        policies.len(),
        "events and policies must be parallel slices"
    );
    let mut effective: Vec<busytok_domain::NormalizedUsageEvent> = Vec::new();
    let mut inserted_ids: Vec<String> = Vec::new();
    // dedupe_key -> current in-batch winner, so collisions within one call are
    // resolved against the just-written row instead of a stale DB read.
    let mut winners: std::collections::HashMap<String, ExistingRow> =
        std::collections::HashMap::new();
    let mut inserted = 0i64;
    let mut replaced = 0i64;
    let mut dropped = 0i64;

    for (event, policy) in events.iter().zip(policies.iter()) {
        let dedupe_key = event.dedupe_key.clone().unwrap_or_else(|| event.id.clone());

        // InsertOnce agents are idempotent by id; no cross-event collapse.
        if matches!(policy, busytok_domain::UsageWritePolicy::InsertOnce) {
            let changes = conn
                .execute(
                    USAGE_INSERT_IGNORE_SQL,
                    usage_event_params!(event, generation_id, &event.id),
                )
                .with_context(|| format!("failed to insert usage event {}", event.id))?;
            if changes > 0 {
                effective.push(event.clone());
                inserted_ids.push(event.id.clone());
                inserted += 1;
            }
            continue;
        }

        // Replace policy: sidechain-aware collapse on dedupe_key. The existing
        // row is the in-batch winner if present, else whatever is persisted.
        let existing: Option<ExistingRow> = match winners.get(&dedupe_key).cloned() {
            Some(w) => Some(w),
            None => fetch_existing_for_dedupe(conn, generation_id, &dedupe_key)?,
        };

        let new_wins = match &existing {
            None => true,
            Some(existing_row) => new_beats_existing(event, existing_row),
        };

        if !new_wins {
            dropped += 1;
            if let Some(ref existing_row) = existing {
                debug!(
                    dedupe_key = %dedupe_key,
                    message_id = ?event.message_id,
                    e_is_sc = existing_row.is_sidechain,
                    n_is_sc = event.is_sidechain,
                    e_total = existing_row.total_tokens,
                    n_total = event.total_tokens,
                    "dropped usage event: existing row won"
                );
            } else {
                debug!(
                    dedupe_key = %dedupe_key,
                    message_id = ?event.message_id,
                    "dropped usage event: no existing row"
                );
            }
            continue;
        }

        // New event wins (or is fresh). Remove any displaced different-id row
        // sharing this dedupe key, then upsert the winner by id so the table
        // holds exactly one row per dedupe key.
        let needs_delete = existing.as_ref().map_or(false, |e| e.id != event.id);
        if needs_delete {
            remove_other_dedupe_rows(conn, generation_id, &dedupe_key, &event.id)?;
        }
        conn.execute(
            USAGE_UPSERT_BY_ID_SQL,
            usage_event_params!(event, generation_id, &dedupe_key),
        )
        .with_context(|| format!("failed to upsert usage event {}", event.id))?;

        match existing {
            None => {
                effective.push(event.clone());
                inserted_ids.push(event.id.clone());
                inserted += 1;
            }
            Some(existing_row) => {
                // Rollups previously absorbed the old row's tokens; feed the
                // (new − old) delta so they end at the new totals.
                effective.push(existing_row.tokens.compute_delta(event));
                replaced += 1;
                if !event.is_sidechain && existing_row.is_sidechain {
                    info!(
                        dedupe_key = %dedupe_key,
                        message_id = ?event.message_id,
                        old_total = existing_row.total_tokens,
                        new_total = event.total_tokens,
                        "parent usage replaced sidechain replay"
                    );
                } else {
                    debug!(
                        dedupe_key = %dedupe_key,
                        event_id = %event.id,
                        old_total = existing_row.total_tokens,
                        new_total = event.total_tokens,
                        "replaced usage event with higher-total entry"
                    );
                }
            }
        }

        // Record the winner so a later same-key event in this batch compares
        // against it (and any displaced row is never double-counted).
        winners.insert(dedupe_key.clone(), existing_row_view(event));
    }

    Ok(DedupOutcome {
        effective_events: effective,
        inserted_ids,
        inserted,
        replaced,
        dropped,
    })
}

/// Snapshot an event as the in-batch winner for a dedupe key.
fn existing_row_view(event: &busytok_domain::NormalizedUsageEvent) -> ExistingRow {
    ExistingRow {
        id: event.id.clone(),
        is_sidechain: event.is_sidechain,
        total_tokens: event.total_tokens,
        tokens: crate::OldEventTokens {
            event_id: event.id.clone(),
            input_tokens: event.input_tokens,
            output_tokens: event.output_tokens,
            total_tokens: event.total_tokens,
            cached_input_tokens: event.cached_input_tokens,
            cache_creation_tokens: event.cache_creation_tokens,
            cache_read_tokens: event.cache_read_tokens,
            reasoning_tokens: event.reasoning_tokens,
            thoughts_tokens: event.thoughts_tokens,
            tool_tokens: event.tool_tokens,
            cost_usd: event.cost_usd,
            estimated_cost_usd: event.estimated_cost_usd,
        },
    }
}

/// Insert a batch of normalized usage events with idempotent (`InsertOnce`)
/// semantics and return the count of newly inserted rows. Thin wrapper over
/// [`upsert_usage_events_dedup_aware`] for callers that do not need cross-event
/// collapse (e.g. Codex, and most tests).
pub fn insert_usage_events_batch(
    conn: &Connection,
    events: &[busytok_domain::NormalizedUsageEvent],
    generation_id: &str,
) -> Result<i64> {
    let policies = vec![busytok_domain::UsageWritePolicy::InsertOnce; events.len()];
    Ok(upsert_usage_events_dedup_aware(conn, events, &policies, generation_id)?.inserted)
}

/// Idempotent insert returning the events that were newly created. Used by
/// tests and any path that wants the inserted set back under `InsertOnce`.
pub fn insert_usage_events_batch_returning_inserted(
    conn: &Connection,
    events: &[busytok_domain::NormalizedUsageEvent],
    generation_id: &str,
) -> Result<Vec<busytok_domain::NormalizedUsageEvent>> {
    let policies = vec![busytok_domain::UsageWritePolicy::InsertOnce; events.len()];
    Ok(upsert_usage_events_dedup_aware(conn, events, &policies, generation_id)?.effective_events)
}

// ── Source file checkpoints ──────────────────────────────────────────────────

/// Upsert a source file checkpoint row.
///
/// Creates the row if it does not exist; updates `offset_bytes`, `size_bytes`,
/// `last_mtime_ms`, `inode`, `last_error`, and `updated_at_ms` on conflict.
/// Preserves `first_seen_at_ms` and `created_at_ms` on update.
pub fn upsert_source_file_checkpoint(
    conn: &Connection,
    id: &str,
    source_id: &str,
    agent: &str,
    path: &str,
    inode: Option<&str>,
    offset_bytes: i64,
    size_bytes: i64,
    last_mtime_ms: Option<i64>,
    state: &str,
    last_error: Option<&str>,
) -> Result<()> {
    let now_ms = now_ms();
    conn.execute(
        "INSERT INTO source_file_checkpoints (\
            id, source_id, agent, path, inode, offset_bytes, size_bytes, \
            last_mtime_ms, state, last_error, first_seen_at_ms, \
            last_seen_at_ms, created_at_ms, updated_at_ms\
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
        ON CONFLICT(id) DO UPDATE SET \
            source_id = excluded.source_id, \
            agent = excluded.agent, \
            path = excluded.path, \
            inode = excluded.inode, \
            offset_bytes = excluded.offset_bytes, \
            size_bytes = excluded.size_bytes, \
            last_mtime_ms = excluded.last_mtime_ms, \
            state = excluded.state, \
            last_error = excluded.last_error, \
            last_seen_at_ms = excluded.last_seen_at_ms, \
            updated_at_ms = excluded.updated_at_ms, \
            first_seen_at_ms = source_file_checkpoints.first_seen_at_ms, \
            created_at_ms = source_file_checkpoints.created_at_ms",
        params![
            id,
            source_id,
            agent,
            path,
            inode,
            offset_bytes,
            size_bytes,
            last_mtime_ms,
            state,
            last_error,
            now_ms, // first_seen_at_ms
            now_ms, // last_seen_at_ms
            now_ms, // created_at_ms
            now_ms, // updated_at_ms
        ],
    )
    .context("failed to upsert source file checkpoint")?;
    Ok(())
}

/// Upsert a log file row used by source health summaries.
///
/// Preserves `first_seen_at_ms` and `created_at_ms` on update while advancing
/// the latest checkpoint state, offsets, and error metadata.
pub fn upsert_log_file_checkpoint(
    conn: &Connection,
    id: &str,
    source_id: &str,
    agent: &str,
    path: &str,
    inode: Option<&str>,
    offset_bytes: i64,
    size_bytes: i64,
    last_mtime_ms: Option<i64>,
    state: &str,
    last_error: Option<&str>,
) -> Result<()> {
    let now_ms = now_ms();
    conn.execute(
        "INSERT INTO log_files (\
            id, source_id, agent, path, inode, size_bytes, offset_bytes, \
            last_mtime_ms, first_seen_at_ms, last_seen_at_ms, state, last_error, \
            created_at_ms, updated_at_ms\
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10, ?11, ?9, ?9) \
        ON CONFLICT(id) DO UPDATE SET \
            source_id = excluded.source_id, \
            agent = excluded.agent, \
            path = excluded.path, \
            inode = excluded.inode, \
            size_bytes = excluded.size_bytes, \
            offset_bytes = excluded.offset_bytes, \
            last_mtime_ms = excluded.last_mtime_ms, \
            last_seen_at_ms = excluded.last_seen_at_ms, \
            state = excluded.state, \
            last_error = excluded.last_error, \
            updated_at_ms = excluded.updated_at_ms, \
            first_seen_at_ms = log_files.first_seen_at_ms, \
            created_at_ms = log_files.created_at_ms",
        params![
            id,
            source_id,
            agent,
            path,
            inode,
            size_bytes,
            offset_bytes,
            last_mtime_ms,
            now_ms,
            state,
            last_error,
        ],
    )
    .context("failed to upsert log file checkpoint")?;
    Ok(())
}

/// Rebuild all materialized source and diagnostic summaries for a generation.
pub fn rebuild_source_summaries(conn: &Connection, generation_id: &str) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let result = rebuild_source_summaries_tx(&tx, generation_id);
    match result {
        Ok(()) => {
            tx.commit()?;
            Ok(())
        }
        Err(err) => {
            let _ = tx.rollback();
            Err(err)
        }
    }
}

/// Rebuild all materialized source and diagnostic summaries inside an existing
/// transaction, clearing stale rows for the target generation first.
pub fn rebuild_source_summaries_tx(
    tx: &rusqlite::Transaction<'_>,
    generation_id: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM source_health_summary WHERE generation_id = ?1",
        params![generation_id],
    )
    .context("failed to clear source summaries for generation")?;
    let source_ids = {
        let mut stmt = tx.prepare(
            "SELECT id FROM log_sources WHERE status NOT IN ('removed', 'unknown') ORDER BY id",
        )?;
        let source_ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        source_ids
    };

    refresh_source_summaries_for_sources_tx(tx, generation_id, &source_ids)
}

/// Refresh materialized source and diagnostic summaries for a specific set of sources.
pub fn refresh_source_summaries_for_sources<S: AsRef<str>>(
    conn: &Connection,
    generation_id: &str,
    source_ids: &[S],
) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let result = refresh_source_summaries_for_sources_tx(&tx, generation_id, source_ids);
    match result {
        Ok(()) => {
            tx.commit()?;
            Ok(())
        }
        Err(err) => {
            let _ = tx.rollback();
            Err(err)
        }
    }
}

/// Transaction-scoped source summary refresh used by writer-owned transactions.
pub fn refresh_source_summaries_for_sources_tx<S: AsRef<str>>(
    tx: &rusqlite::Transaction<'_>,
    generation_id: &str,
    source_ids: &[S],
) -> Result<()> {
    let source_ids = source_ids
        .iter()
        .map(|source_id| source_id.as_ref())
        .filter(|source_id| !source_id.is_empty())
        .collect::<BTreeSet<_>>();
    if source_ids.is_empty() {
        return Ok(());
    }

    let now_ms = now_ms();
    let mut delete_source_summary = tx
        .prepare("DELETE FROM source_health_summary WHERE generation_id = ?1 AND source_id = ?2")?;
    let mut insert_source_summary = tx.prepare(
        "INSERT INTO source_health_summary
         (generation_id, source_id, agent, root_path, source_type, status,
          configured_by_user, last_scan_at_ms, file_count, parsed_file_count,
          event_count, last_error, latest_activity_at_ms,
          created_at_ms, updated_at_ms)
         SELECT ?1, s.id, s.agent, s.root_path, s.source_type, s.status,
                s.configured_by_user, s.last_scan_completed_at_ms,
                COALESCE(files.file_count, 0),
                COALESCE(files.parsed_file_count, 0),
                COALESCE(events.event_count, 0),
                s.last_error,
                events.latest_activity_at_ms,
                ?2, ?2
         FROM log_sources s
         LEFT JOIN (
           SELECT source_id,
                  COUNT(*) AS file_count,
                  SUM(CASE WHEN offset_bytes > 0 THEN 1 ELSE 0 END) AS parsed_file_count
           FROM log_files
           GROUP BY source_id
         ) files ON files.source_id = s.id
         LEFT JOIN (
           SELECT lf.source_id,
                  COUNT(*) AS event_count,
                  MAX(ue.timestamp_ms) AS latest_activity_at_ms
           FROM log_files lf
           JOIN usage_events ue ON ue.source_file_id = lf.id
           WHERE ue.generation_id = ?1
           GROUP BY lf.source_id
         ) events ON events.source_id = s.id
         WHERE s.id = ?3 AND s.status NOT IN ('removed', 'unknown')",
    )?;

    for source_id in source_ids {
        delete_source_summary
            .execute(params![generation_id, source_id])
            .with_context(|| format!("failed to delete source summary for {source_id}"))?;
        insert_source_summary
            .execute(params![generation_id, now_ms, source_id])
            .with_context(|| format!("failed to refresh source summary for {source_id}"))?;
    }

    Ok(())
}

fn affected_source_ids_for_inserted_events<S: AsRef<str>>(
    conn: &Connection,
    inserted_events: &[busytok_domain::NormalizedUsageEvent],
    fallback_source_ids: &[S],
) -> Result<Vec<String>> {
    let mut source_ids = fallback_source_ids
        .iter()
        .map(|source_id| source_id.as_ref().trim())
        .filter(|source_id| !source_id.is_empty())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    let source_file_ids = inserted_events
        .iter()
        .map(|event| event.source_file_id.trim())
        .filter(|source_file_id| !source_file_id.is_empty())
        .collect::<BTreeSet<_>>();
    if source_file_ids.is_empty() {
        return Ok(source_ids.into_iter().collect());
    }

    let mut source_stmt = conn.prepare("SELECT source_id FROM log_files WHERE id = ?1")?;
    for source_file_id in source_file_ids {
        if let Some(source_id) = source_stmt
            .query_row(params![source_file_id], |row| row.get::<_, String>(0))
            .optional()
            .with_context(|| {
                format!("failed to resolve source_id for source_file_id {source_file_id}")
            })?
        {
            if !source_id.trim().is_empty() {
                source_ids.insert(source_id);
            }
        }
    }

    Ok(source_ids.into_iter().collect())
}

/// Refresh summaries for the sources actually touched by a batch of inserted
/// events, with optional fallback source IDs for batches that insert no rows.
pub fn refresh_source_summaries_for_inserted_events_tx<S: AsRef<str>>(
    tx: &rusqlite::Transaction<'_>,
    generation_id: &str,
    inserted_events: &[busytok_domain::NormalizedUsageEvent],
    fallback_source_ids: &[S],
) -> Result<()> {
    let affected_source_ids =
        affected_source_ids_for_inserted_events(tx, inserted_events, fallback_source_ids)?;
    refresh_source_summaries_for_sources_tx(tx, generation_id, &affected_source_ids)
}

/// Resolve the current active generation, preferring `service_state` when it
/// has already been updated by the runtime.
pub fn current_active_generation_id(conn: &Connection) -> Result<Option<String>> {
    let from_service_state = conn
        .query_row(
            "SELECT NULLIF(active_generation_id, '') FROM service_state WHERE id = 1",
            [],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    if from_service_state.is_some() {
        return Ok(from_service_state);
    }

    conn.query_row(
        "SELECT generation_id FROM audit_generations \
         WHERE is_active = 1 \
         ORDER BY promoted_at_ms DESC, updated_at_ms DESC \
         LIMIT 1",
        [],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Record a diagnostic event using either a connection or a transaction.
pub fn record_diagnostic_event(
    conn: &Connection,
    event: &busytok_domain::OperationalDiagnosticEvent,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO diagnostic_events (\
            id, agent, source_id, source_file_id, source_path, source_line, \
            severity, code, message, details_json, happened_at_ms, created_at_ms\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            event.id,
            event.agent.as_ref().map(|a| a.as_str()),
            event.source_id,
            event.source_file_id,
            event.source_path.as_deref(),
            event.source_line,
            event.severity,
            event.category,
            event.message,
            event.detail_json,
            event.happened_at_ms,
            event.created_at_ms,
        ],
    )
    .context("failed to record diagnostic event")?;
    Ok(())
}

/// Upsert a log source using either a connection or a transaction.
pub fn upsert_log_source(
    conn: &Connection,
    source: &crate::repository::LogSourceRow,
) -> Result<()> {
    conn.execute(
        "INSERT INTO log_sources (\
            id, agent, source_type, root_path, configured_by_user, \
            default_discovery_enabled, status, last_scan_started_at_ms, \
            last_scan_completed_at_ms, last_error, first_seen_at_ms, \
            last_seen_at_ms, created_at_ms, updated_at_ms\
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
        ON CONFLICT(id) DO UPDATE SET \
            agent = excluded.agent, \
            source_type = excluded.source_type, \
            root_path = excluded.root_path, \
            status = excluded.status, \
            last_scan_started_at_ms = CASE \
                WHEN excluded.last_scan_started_at_ms IS NOT NULL THEN excluded.last_scan_started_at_ms \
                ELSE log_sources.last_scan_started_at_ms END, \
            last_scan_completed_at_ms = excluded.last_scan_completed_at_ms, \
            last_error = excluded.last_error, \
            last_seen_at_ms = excluded.last_seen_at_ms, \
            updated_at_ms = excluded.updated_at_ms, \
            first_seen_at_ms = log_sources.first_seen_at_ms, \
            created_at_ms = log_sources.created_at_ms",
        params![
            source.id,
            source.agent,
            source.source_type,
            source.root_path,
            source.configured_by_user,
            source.default_discovery_enabled,
            source.status,
            source.last_scan_started_at_ms,
            source.last_scan_completed_at_ms,
            source.last_error,
            source.first_seen_at_ms,
            source.last_seen_at_ms,
            source.created_at_ms,
            source.updated_at_ms,
        ],
    )
    .context("failed to upsert log source")?;
    Ok(())
}

// ── Generation observations ──────────────────────────────────────────────────

/// Insert a generation-to-file observation record.
///
/// Records that a source file was observed during a particular generation's
/// scan, with the offset/size/mtime at that point in time.
pub fn insert_generation_observation(
    conn: &Connection,
    generation_id: &str,
    source_file_id: &str,
    offset_bytes: i64,
    size_bytes: i64,
    last_mtime_ms: Option<i64>,
    scan_status: Option<&str>,
    scan_errors: Option<&str>,
) -> Result<()> {
    let now_ms = now_ms();
    conn.execute(
        "INSERT OR REPLACE INTO generation_file_observations (\
            generation_id, source_file_id, observed_at_ms, \
            offset_bytes, size_bytes, last_mtime_ms, \
            scan_status, scan_errors\
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            generation_id,
            source_file_id,
            now_ms,
            offset_bytes,
            size_bytes,
            last_mtime_ms,
            scan_status,
            scan_errors,
        ],
    )
    .context("failed to insert generation observation")?;
    Ok(())
}

// ── Tail replay queue ────────────────────────────────────────────────────────

/// Enqueue rows onto the tail replay queue.
///
/// Each entry is a JSON-encoded event + metadata to be replayed into the
/// target generation during a rebuild.
pub fn enqueue_tail_replay_rows(conn: &Connection, rows: &[TailReplayEnqueue]) -> Result<()> {
    let now_ms = now_ms();
    let sql = "\
        INSERT INTO tail_replay_queue (\
            source_file_id, event_seq, event_data_json, attempts, \
            last_attempt_at_ms, status, created_at_ms, updated_at_ms\
        ) VALUES (?1, ?2, ?3, 0, NULL, 'pending', ?4, ?4)";

    for row in rows {
        conn.execute(
            sql,
            params![
                row.source_file_id,
                row.event_seq,
                row.event_data_json,
                now_ms
            ],
        )
        .with_context(|| {
            format!(
                "failed to enqueue tail replay row for source_file_id={}",
                row.source_file_id
            )
        })?;
    }
    Ok(())
}

/// A single row to enqueue onto the tail replay queue.
#[derive(Debug, Clone)]
pub struct TailReplayEnqueue {
    pub source_file_id: String,
    pub event_seq: i64,
    pub event_data_json: String,
}

/// Apply replay rows from the tail replay queue to a target generation.
///
/// Reads pending replay rows (optionally scoped to a source file), writes them
/// as usage events tagged with the target generation, and marks the replay rows
/// as processed (`status = 'applied'`). Runs inside a single transaction.
pub fn apply_replay_rows_to_target_generation(
    conn: &Connection,
    target_generation_id: &str,
    source_file_id: Option<&str>,
    limit: i64,
) -> Result<i64> {
    let tx = conn
        .unchecked_transaction()
        .context("failed to begin replay apply transaction")?;

    // Select pending replay rows and apply them
    let (replay_ids, applied) = {
        let select_sql = if source_file_id.is_some() {
            "SELECT id, event_data_json FROM tail_replay_queue \
             WHERE status = 'pending' AND source_file_id = ?1 \
             ORDER BY event_seq ASC LIMIT ?2"
        } else {
            "SELECT id, event_data_json FROM tail_replay_queue \
             WHERE status = 'pending' \
             ORDER BY event_seq ASC LIMIT ?1"
        };

        let mut stmt = tx.prepare(select_sql)?;

        let mut replay_ids: Vec<i64> = Vec::new();
        let mut applied = 0i64;

        if let Some(sfid) = source_file_id {
            let rows = stmt.query_map(params![sfid, limit], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (replay_id, data_json) = row?;
                replay_ids.push(replay_id);
                applied += apply_single_replay(&tx, target_generation_id, &data_json)?;
            }
        } else {
            let rows = stmt.query_map(params![limit], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (replay_id, data_json) = row?;
                replay_ids.push(replay_id);
                applied += apply_single_replay(&tx, target_generation_id, &data_json)?;
            }
        }

        (replay_ids, applied)
    };
    // stmt and rows are dropped here, releasing the borrow on tx

    // Mark applied replay rows as processed
    if !replay_ids.is_empty() {
        let now_ms = now_ms();
        for id in &replay_ids {
            tx.execute(
                "UPDATE tail_replay_queue \
                 SET status = 'applied', attempts = attempts + 1, \
                     last_attempt_at_ms = ?2, updated_at_ms = ?2 \
                 WHERE id = ?1",
                params![id, now_ms],
            )
            .context("failed to mark replay row as applied")?;
        }
    }

    tx.commit()
        .context("failed to commit replay apply transaction")?;

    Ok(applied)
}

/// Apply a single JSON-encoded event to the target generation.
///
/// Parses the event JSON and inserts it into `usage_events` tagged with the
/// target generation. Returns the number of inserted events (0 or 1).
fn apply_single_replay(
    tx: &rusqlite::Transaction,
    target_generation_id: &str,
    data_json: &str,
) -> Result<i64> {
    // Tail replay events carry a partial JSON snapshot (not a full serialised
    // NormalizedUsageEvent), so we extract fields manually and issue a direct
    // INSERT OR IGNORE using the canonical 43-column schema (includes
    // is_sidechain to prevent schema mismatch).
    let value: serde_json::Value =
        serde_json::from_str(data_json).context("failed to parse replay event JSON")?;

    let event_id = value["id"].as_str().unwrap_or("replay-unknown").to_string();

    let changes = tx
        .execute(
            USAGE_INSERT_IGNORE_SQL,
            rusqlite::params![
                event_id,
                value["agent"].as_str().unwrap_or("claude_code"),
                value["source_file_id"].as_str().unwrap_or(""),
                value["source_path"].as_str().unwrap_or(""),
                value["source_line"].as_i64().unwrap_or(0),
                value["source_offset_start"].as_i64().unwrap_or(0),
                value["source_offset_end"].as_i64().unwrap_or(0),
                value["session_id"].as_str().unwrap_or(""),
                value["turn_id"].as_str().unwrap_or(""),
                value["source_request_id"].as_str().unwrap_or(""),
                value["message_id"].as_str().unwrap_or(""),
                value["timestamp_ms"].as_i64().unwrap_or(0),
                value["project_path"].as_str().unwrap_or(""),
                value["project_hash"].as_str().unwrap_or(""),
                value["cwd"].as_str().unwrap_or(""),
                value["model"].as_str().unwrap_or(""),
                value["model_provider"].as_str().unwrap_or(""),
                value["agent_version"].as_str().unwrap_or(""),
                value["client_kind"].as_str().unwrap_or(""),
                value["speed"].as_str(),
                value["input_tokens"].as_i64().unwrap_or(0),
                value["output_tokens"].as_i64().unwrap_or(0),
                value["total_tokens"].as_i64().unwrap_or(0),
                value["cached_input_tokens"].as_i64().unwrap_or(0),
                value["cache_creation_tokens"].as_i64().unwrap_or(0),
                value["cache_read_tokens"].as_i64().unwrap_or(0),
                value["reasoning_tokens"].as_i64().unwrap_or(0),
                value["thoughts_tokens"].as_i64().unwrap_or(0),
                value["tool_tokens"].as_i64().unwrap_or(0),
                value["cost_usd"].as_f64(),
                value["estimated_cost_usd"].as_f64(),
                value["cost_currency"].as_str().unwrap_or("USD"),
                value["cost_source"].as_str().unwrap_or("unknown"),
                value["price_catalog_version"].as_str().unwrap_or(""),
                value["is_error"].as_i64().unwrap_or(0),
                value["error_type"].as_str(),
                value["raw_event_hash"].as_str().unwrap_or(""),
                value["usage_limit_reset_time_ms"].as_i64(),
                value["created_at_ms"].as_i64().unwrap_or(0),
                value["updated_at_ms"].as_i64().unwrap_or(0),
                target_generation_id,
                value["id"].as_str().unwrap_or(""),
                0i32, // is_sidechain → DEFAULT 0 (replay events have no sidechain flag)
            ],
        )
        .context("failed to insert replay usage event")?;

    Ok(changes as i64)
}

// ── Materialized aggregate updates ───────────────────────────────────────────

/// Update materialized aggregate buckets from a set of usage events.
///
/// Groups events into 2s/hour/day buckets and upserts totals into the
/// `usage_buckets_2s`, `usage_buckets_hour`, and `usage_buckets_day` tables.
/// Runs inside a transaction.
pub fn update_materialized_aggregates_from_events(
    conn: &Connection,
    events: &[busytok_domain::NormalizedUsageEvent],
    generation_id: &str,
) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }

    let now_ms = now_ms();

    // Group events into bucket-sized windows
    for event in events {
        let agent = event.agent.as_str();
        let model = event.model.as_deref().unwrap_or("");

        // 2s bucket
        let bucket_2s = (event.timestamp_ms / 2000) * 2000;
        conn.execute(
            "INSERT INTO usage_buckets_2s (\
                generation_id, bucket_start_ms, agent, model, \
                input_tokens, output_tokens, total_tokens, \
                cost_usd, cost_status, event_count, \
                created_at_ms, updated_at_ms\
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
                      CASE WHEN ?8 IS NOT NULL THEN 'exact' ELSE 'unavailable' END, \
                      ?9, ?10, ?10) \
            ON CONFLICT(generation_id, bucket_start_ms, agent, model) DO UPDATE SET \
                input_tokens = input_tokens + excluded.input_tokens, \
                output_tokens = output_tokens + excluded.output_tokens, \
                total_tokens = total_tokens + excluded.total_tokens, \
                cost_usd = CASE WHEN cost_usd IS NOT NULL AND excluded.cost_usd IS NOT NULL \
                                THEN cost_usd + excluded.cost_usd \
                                WHEN cost_usd IS NOT NULL THEN cost_usd \
                                ELSE excluded.cost_usd END, \
                cost_status = CASE \
                                WHEN cost_status = 'partial' OR excluded.cost_status = 'partial' THEN 'partial' \
                                WHEN cost_status = excluded.cost_status THEN cost_status \
                                WHEN cost_status = 'unknown' THEN excluded.cost_status \
                                WHEN excluded.cost_status = 'unknown' THEN cost_status \
                                ELSE 'partial' END, \
                event_count = event_count + excluded.event_count, \
                updated_at_ms = excluded.updated_at_ms",
            params![
                generation_id,
                bucket_2s,
                agent,
                model,
                event.input_tokens,
                event.output_tokens,
                event.total_tokens,
                event.cost_usd,
                1i64,
                now_ms,
            ],
        )
        .with_context(|| format!("failed to upsert usage_buckets_2s for event {}", event.id))?;

        // Hour bucket
        let bucket_hour = (event.timestamp_ms / 3_600_000) * 3_600_000;
        conn.execute(
            "INSERT INTO usage_buckets_hour (\
                generation_id, bucket_start_ms, agent, model, \
                input_tokens, output_tokens, total_tokens, \
                cost_usd, cost_status, event_count, \
                created_at_ms, updated_at_ms\
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
                      CASE WHEN ?8 IS NOT NULL THEN 'exact' ELSE 'unavailable' END, \
                      ?9, ?10, ?10) \
            ON CONFLICT(generation_id, bucket_start_ms, agent, model) DO UPDATE SET \
                input_tokens = input_tokens + excluded.input_tokens, \
                output_tokens = output_tokens + excluded.output_tokens, \
                total_tokens = total_tokens + excluded.total_tokens, \
                cost_usd = CASE WHEN cost_usd IS NOT NULL AND excluded.cost_usd IS NOT NULL \
                                THEN cost_usd + excluded.cost_usd \
                                WHEN cost_usd IS NOT NULL THEN cost_usd \
                                ELSE excluded.cost_usd END, \
                cost_status = CASE \
                                WHEN cost_status = 'partial' OR excluded.cost_status = 'partial' THEN 'partial' \
                                WHEN cost_status = excluded.cost_status THEN cost_status \
                                WHEN cost_status = 'unknown' THEN excluded.cost_status \
                                WHEN excluded.cost_status = 'unknown' THEN cost_status \
                                ELSE 'partial' END, \
                event_count = event_count + excluded.event_count, \
                updated_at_ms = excluded.updated_at_ms",
            params![
                generation_id,
                bucket_hour,
                agent,
                model,
                event.input_tokens,
                event.output_tokens,
                event.total_tokens,
                event.cost_usd,
                1i64,
                now_ms,
            ],
        )
        .with_context(|| format!("failed to upsert usage_buckets_hour for event {}", event.id))?;

        // Day bucket
        let bucket_day = (event.timestamp_ms / 86_400_000) * 86_400_000;
        conn.execute(
            "INSERT INTO usage_buckets_day (\
                generation_id, bucket_start_ms, agent, model, \
                input_tokens, output_tokens, total_tokens, \
                cost_usd, cost_status, event_count, \
                created_at_ms, updated_at_ms\
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
                      CASE WHEN ?8 IS NOT NULL THEN 'exact' ELSE 'unavailable' END, \
                      ?9, ?10, ?10) \
            ON CONFLICT(generation_id, bucket_start_ms, agent, model) DO UPDATE SET \
                input_tokens = input_tokens + excluded.input_tokens, \
                output_tokens = output_tokens + excluded.output_tokens, \
                total_tokens = total_tokens + excluded.total_tokens, \
                cost_usd = CASE WHEN cost_usd IS NOT NULL AND excluded.cost_usd IS NOT NULL \
                                THEN cost_usd + excluded.cost_usd \
                                WHEN cost_usd IS NOT NULL THEN cost_usd \
                                ELSE excluded.cost_usd END, \
                cost_status = CASE \
                                WHEN cost_status = 'partial' OR excluded.cost_status = 'partial' THEN 'partial' \
                                WHEN cost_status = excluded.cost_status THEN cost_status \
                                WHEN cost_status = 'unknown' THEN excluded.cost_status \
                                WHEN excluded.cost_status = 'unknown' THEN cost_status \
                                ELSE 'partial' END, \
                event_count = event_count + excluded.event_count, \
                updated_at_ms = excluded.updated_at_ms",
            params![
                generation_id,
                bucket_day,
                agent,
                model,
                event.input_tokens,
                event.output_tokens,
                event.total_tokens,
                event.cost_usd,
                1i64,
                now_ms,
            ],
        )
        .with_context(|| format!("failed to upsert usage_buckets_day for event {}", event.id))?;
    }

    // Persist dimension summary tables (by project, model, session, client).
    for event in events {
        let agent = event.agent.as_str();
        let model = event.model.as_deref().unwrap_or("");
        let day_secs = (event.timestamp_ms / 1000 / 86_400) * 86_400;
        let date = {
            // Convert epoch seconds to YYYY-MM-DD.
            let days = day_secs / 86_400;
            let mut y: i64 = 1970;
            let mut remaining = days;
            loop {
                let days_in_year = if (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0) {
                    366
                } else {
                    365
                };
                if remaining < days_in_year {
                    break;
                }
                remaining -= days_in_year;
                y += 1;
            }
            let month_days = if (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0) {
                [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
            } else {
                [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
            };
            let mut m: u32 = 1;
            for &md in month_days.iter() {
                if remaining < md as i64 {
                    break;
                }
                remaining -= md as i64;
                m += 1;
            }
            format!("{:04}-{:02}-{:02}", y, m, remaining + 1)
        };

        // by project
        let project_id = event.project_hash.as_deref().unwrap_or("");
        conn.execute(
            "INSERT INTO usage_by_project_day \
             (generation_id, date, project_id, project_path, agent, model, \
              input_tokens, output_tokens, total_tokens, \
              cost_usd, cost_status, event_count, last_active_at_ms, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, \
                     CASE WHEN ?10 IS NOT NULL THEN 'exact' ELSE 'unavailable' END, \
                     1, ?11, ?12, ?12) \
             ON CONFLICT(generation_id, date, project_id, agent, model) DO UPDATE SET \
               project_path = COALESCE(excluded.project_path, project_path), \
               input_tokens = input_tokens + excluded.input_tokens, \
               output_tokens = output_tokens + excluded.output_tokens, \
               total_tokens = total_tokens + excluded.total_tokens, \
               cost_usd = CASE WHEN cost_usd IS NOT NULL AND excluded.cost_usd IS NOT NULL \
                               THEN cost_usd + excluded.cost_usd \
                               WHEN cost_usd IS NOT NULL THEN cost_usd \
                               ELSE excluded.cost_usd END, \
               cost_status = CASE \
                               WHEN cost_status = 'partial' OR excluded.cost_status = 'partial' THEN 'partial' \
                               WHEN cost_status = excluded.cost_status THEN cost_status \
                               WHEN cost_status = 'unknown' THEN excluded.cost_status \
                               WHEN excluded.cost_status = 'unknown' THEN cost_status \
                               ELSE 'partial' END, \
               event_count = event_count + excluded.event_count, \
               last_active_at_ms = MAX(COALESCE(last_active_at_ms, 0), COALESCE(excluded.last_active_at_ms, 0)), \
               updated_at_ms = excluded.updated_at_ms",
            params![
                generation_id,
                &date,
                project_id,
                event.project_path.as_deref(),
                agent,
                model,
                event.input_tokens,
                event.output_tokens,
                event.total_tokens,
                event.cost_usd,
                event.timestamp_ms,
                now_ms,
            ],
        )
        .with_context(|| {
            format!(
                "failed to upsert usage_by_project_day for event {}",
                event.id
            )
        })?;

        // by model
        conn.execute(
            "INSERT INTO usage_by_model_day \
             (generation_id, date, agent, model, \
              input_tokens, output_tokens, total_tokens, \
              cost_usd, cost_status, event_count, last_active_at_ms, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
                     CASE WHEN ?8 IS NOT NULL THEN 'exact' ELSE 'unavailable' END, \
                     1, ?9, ?10, ?10) \
             ON CONFLICT(generation_id, date, agent, model) DO UPDATE SET \
               input_tokens = input_tokens + excluded.input_tokens, \
               output_tokens = output_tokens + excluded.output_tokens, \
               total_tokens = total_tokens + excluded.total_tokens, \
               cost_usd = CASE WHEN cost_usd IS NOT NULL AND excluded.cost_usd IS NOT NULL \
                               THEN cost_usd + excluded.cost_usd \
                               WHEN cost_usd IS NOT NULL THEN cost_usd \
                               ELSE excluded.cost_usd END, \
               cost_status = CASE \
                               WHEN cost_status = 'partial' OR excluded.cost_status = 'partial' THEN 'partial' \
                               WHEN cost_status = excluded.cost_status THEN cost_status \
                               WHEN cost_status = 'unknown' THEN excluded.cost_status \
                               WHEN excluded.cost_status = 'unknown' THEN cost_status \
                               ELSE 'partial' END, \
               event_count = event_count + excluded.event_count, \
               last_active_at_ms = MAX(COALESCE(last_active_at_ms, 0), COALESCE(excluded.last_active_at_ms, 0)), \
               updated_at_ms = excluded.updated_at_ms",
            params![
                generation_id,
                &date,
                agent,
                model,
                event.input_tokens,
                event.output_tokens,
                event.total_tokens,
                event.cost_usd,
                event.timestamp_ms,
                now_ms,
            ],
        )
        .with_context(|| format!("failed to upsert usage_by_model_day for event {}", event.id))?;

        // by session
        let session_id = if event.session_id.is_empty() {
            ""
        } else {
            &event.session_id
        };
        conn.execute(
            "INSERT INTO usage_by_session_day \
             (generation_id, date, session_id, agent, client_kind, project_path, project_hash, model, \
              input_tokens, output_tokens, total_tokens, \
              cost_usd, cost_status, event_count, last_active_at_ms, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, \
                     CASE WHEN ?12 IS NOT NULL THEN 'exact' ELSE 'unavailable' END, \
                     1, ?13, ?14, ?14) \
             ON CONFLICT(generation_id, date, session_id, agent) DO UPDATE SET \
               client_kind = COALESCE(excluded.client_kind, client_kind), \
               project_path = COALESCE(excluded.project_path, project_path), \
               project_hash = COALESCE(excluded.project_hash, project_hash), \
               model = COALESCE(excluded.model, model), \
               input_tokens = input_tokens + excluded.input_tokens, \
               output_tokens = output_tokens + excluded.output_tokens, \
               total_tokens = total_tokens + excluded.total_tokens, \
               cost_usd = CASE WHEN cost_usd IS NOT NULL AND excluded.cost_usd IS NOT NULL \
                               THEN cost_usd + excluded.cost_usd \
                               WHEN cost_usd IS NOT NULL THEN cost_usd \
                               ELSE excluded.cost_usd END, \
               cost_status = CASE \
                               WHEN cost_status = 'partial' OR excluded.cost_status = 'partial' THEN 'partial' \
                               WHEN cost_status = excluded.cost_status THEN cost_status \
                               WHEN cost_status = 'unknown' THEN excluded.cost_status \
                               WHEN excluded.cost_status = 'unknown' THEN cost_status \
                               ELSE 'partial' END, \
               event_count = event_count + excluded.event_count, \
               last_active_at_ms = MAX(COALESCE(last_active_at_ms, 0), COALESCE(excluded.last_active_at_ms, 0)), \
               updated_at_ms = excluded.updated_at_ms",
            params![
                generation_id,
                &date,
                session_id,
                agent,
                event.client_kind.as_deref(),
                event.project_path.as_deref(),
                event.project_hash.as_deref(),
                event.model.as_deref(),
                event.input_tokens,
                event.output_tokens,
                event.total_tokens,
                event.cost_usd,
                event.timestamp_ms,
                now_ms,
            ],
        )
        .with_context(|| {
            format!(
                "failed to upsert usage_by_session_day for event {}",
                event.id
            )
        })?;

        // by client
        let client_kind = event.client_kind.as_deref().unwrap_or("");
        conn.execute(
            "INSERT INTO usage_by_client_day \
             (generation_id, date, client_kind, agent, \
              input_tokens, output_tokens, total_tokens, \
              cost_usd, cost_status, event_count, last_active_at_ms, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
                     CASE WHEN ?8 IS NOT NULL THEN 'exact' ELSE 'unavailable' END, \
                     1, ?9, ?10, ?10) \
             ON CONFLICT(generation_id, date, client_kind, agent) DO UPDATE SET \
               input_tokens = input_tokens + excluded.input_tokens, \
               output_tokens = output_tokens + excluded.output_tokens, \
               total_tokens = total_tokens + excluded.total_tokens, \
               cost_usd = CASE WHEN cost_usd IS NOT NULL AND excluded.cost_usd IS NOT NULL \
                               THEN cost_usd + excluded.cost_usd \
                               WHEN cost_usd IS NOT NULL THEN cost_usd \
                               ELSE excluded.cost_usd END, \
               cost_status = CASE \
                               WHEN cost_status = 'partial' OR excluded.cost_status = 'partial' THEN 'partial' \
                               WHEN cost_status = excluded.cost_status THEN cost_status \
                               WHEN cost_status = 'unknown' THEN excluded.cost_status \
                               WHEN excluded.cost_status = 'unknown' THEN cost_status \
                               ELSE 'partial' END, \
               event_count = event_count + excluded.event_count, \
               last_active_at_ms = MAX(COALESCE(last_active_at_ms, 0), COALESCE(excluded.last_active_at_ms, 0)), \
               updated_at_ms = excluded.updated_at_ms",
            params![
                generation_id,
                &date,
                client_kind,
                agent,
                event.input_tokens,
                event.output_tokens,
                event.total_tokens,
                event.cost_usd,
                event.timestamp_ms,
                now_ms,
            ],
        )
        .with_context(|| {
            format!(
                "failed to upsert usage_by_client_day for event {}",
                event.id
            )
        })?;
    }

    Ok(())
}

// ── Daily usage incremental maintenance ──────────────────────────────────────

/// Upsert `daily_usage` rows for a batch of effective-delta events.
///
/// This is the single shared entry point for all daily_usage maintenance.
/// `events` MUST be the effective-delta set (inserted-only for InsertOnce,
/// or inserted + replaced-delta for Replace). All rows are tagged with
/// `generation_id`.
///
/// Callers: writer flush, rebuild, and ingest_store_batch (via callback).
pub fn upsert_daily_usage_for_events(
    conn: &Connection,
    events: &[busytok_domain::NormalizedUsageEvent],
    rtz: &busytok_domain::ReportingTimezone,
    generation_id: &str,
) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }

    for event in events {
        let date = rtz
            .local_date_for_timestamp_ms(event.timestamp_ms)
            .with_context(|| format!("failed to compute date for event {}", event.id))?;
        let tz_name = rtz.canonical_name();
        let agent = event.agent.as_str();
        let project_hash = event.project_hash.as_deref().unwrap_or("");
        let model = event.model.as_deref().unwrap_or("");

        conn.execute(
            "INSERT INTO daily_usage (\
                date, timezone, agent, project_hash, model, \
                input_tokens, output_tokens, total_tokens, \
                cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
                reasoning_tokens, thoughts_tokens, tool_tokens, \
                cost_usd, estimated_cost_usd, event_count, generation_id\
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18) \
            ON CONFLICT(date, timezone, agent, project_hash, model, generation_id) DO UPDATE SET \
                input_tokens = input_tokens + excluded.input_tokens, \
                output_tokens = output_tokens + excluded.output_tokens, \
                total_tokens = total_tokens + excluded.total_tokens, \
                cached_input_tokens = cached_input_tokens + excluded.cached_input_tokens, \
                cache_creation_tokens = cache_creation_tokens + excluded.cache_creation_tokens, \
                cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens, \
                reasoning_tokens = reasoning_tokens + excluded.reasoning_tokens, \
                thoughts_tokens = thoughts_tokens + excluded.thoughts_tokens, \
                tool_tokens = tool_tokens + excluded.tool_tokens, \
                cost_usd = CASE WHEN cost_usd IS NOT NULL AND excluded.cost_usd IS NOT NULL \
                                THEN cost_usd + excluded.cost_usd \
                                WHEN cost_usd IS NOT NULL THEN cost_usd \
                                ELSE excluded.cost_usd END, \
                estimated_cost_usd = CASE \
                    WHEN estimated_cost_usd IS NOT NULL AND excluded.estimated_cost_usd IS NOT NULL \
                    THEN estimated_cost_usd + excluded.estimated_cost_usd \
                    WHEN estimated_cost_usd IS NOT NULL THEN estimated_cost_usd \
                    ELSE excluded.estimated_cost_usd END, \
                event_count = event_count + excluded.event_count",
            rusqlite::params![
                date,
                tz_name,
                agent,
                project_hash,
                model,
                event.input_tokens,
                event.output_tokens,
                event.total_tokens,
                event.cached_input_tokens,
                event.cache_creation_tokens,
                event.cache_read_tokens,
                event.reasoning_tokens,
                event.thoughts_tokens,
                event.tool_tokens,
                event.cost_usd,
                event.estimated_cost_usd,
                1i64,
                generation_id,
            ],
        )
        .with_context(|| format!("failed to upsert daily_usage for event {}", event.id))?;
    }
    Ok(())
}

/// Upsert pre-built daily_usage rows (lower-level, takes rows directly).
///
/// Used by `ingest_store_batch` where rows are already produced by
/// `build_scan_mutations`. This is the shared SQL for daily_usage upsert —
/// no other code path should contain the ON CONFLICT SQL for daily_usage.
pub fn upsert_daily_usage_rows(
    conn: &Connection,
    rows: &[crate::repository::DailyUsageRow],
) -> Result<()> {
    for row in rows {
        conn.execute(
            "INSERT INTO daily_usage (\
                date, timezone, agent, project_hash, model, \
                input_tokens, output_tokens, total_tokens, \
                cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
                reasoning_tokens, thoughts_tokens, tool_tokens, \
                cost_usd, estimated_cost_usd, event_count, generation_id\
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18) \
            ON CONFLICT(date, timezone, agent, project_hash, model, generation_id) DO UPDATE SET \
                input_tokens = input_tokens + excluded.input_tokens, \
                output_tokens = output_tokens + excluded.output_tokens, \
                total_tokens = total_tokens + excluded.total_tokens, \
                cached_input_tokens = cached_input_tokens + excluded.cached_input_tokens, \
                cache_creation_tokens = cache_creation_tokens + excluded.cache_creation_tokens, \
                cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens, \
                reasoning_tokens = reasoning_tokens + excluded.reasoning_tokens, \
                thoughts_tokens = thoughts_tokens + excluded.thoughts_tokens, \
                tool_tokens = tool_tokens + excluded.tool_tokens, \
                cost_usd = CASE WHEN cost_usd IS NOT NULL AND excluded.cost_usd IS NOT NULL \
                                THEN cost_usd + excluded.cost_usd \
                                WHEN cost_usd IS NOT NULL THEN cost_usd \
                                ELSE excluded.cost_usd END, \
                estimated_cost_usd = CASE \
                    WHEN estimated_cost_usd IS NOT NULL AND excluded.estimated_cost_usd IS NOT NULL \
                    THEN estimated_cost_usd + excluded.estimated_cost_usd \
                    WHEN estimated_cost_usd IS NOT NULL THEN estimated_cost_usd \
                    ELSE excluded.estimated_cost_usd END, \
                event_count = event_count + excluded.event_count",
            params![
                row.date,
                row.timezone,
                row.agent,
                row.project_hash,
                row.model,
                row.input_tokens,
                row.output_tokens,
                row.total_tokens,
                row.cached_input_tokens,
                row.cache_creation_tokens,
                row.cache_read_tokens,
                row.reasoning_tokens,
                row.thoughts_tokens,
                row.tool_tokens,
                row.cost_usd,
                row.estimated_cost_usd,
                row.event_count,
                row.generation_id,
            ],
        ).with_context(|| format!("failed to upsert daily_usage row for date={}", row.date))?;
    }
    Ok(())
}

// ── Diagnostic event pruning ─────────────────────────────────────────────────

/// Prune diagnostic events older than the given timestamp.
///
/// Returns the number of rows deleted.
pub fn prune_diagnostic_events(conn: &Connection, older_than_ms: i64) -> Result<i64> {
    let mut deleted = conn
        .execute(
            "DELETE FROM diagnostic_events WHERE created_at_ms < ?1",
            params![older_than_ms],
        )
        .context("failed to prune diagnostic events by age")? as i64;

    // Row-count cap: delete oldest rows beyond 10,000.
    let max_rows: i64 = 10_000;
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM diagnostic_events", [], |row| {
            row.get(0)
        })
        .context("failed to count diagnostic events")?;

    if count > max_rows {
        let excess = count - max_rows;
        deleted += conn
            .execute(
                "DELETE FROM diagnostic_events WHERE id IN (\
                    SELECT id FROM diagnostic_events \
                    ORDER BY created_at_ms ASC LIMIT ?1\
                )",
                params![excess],
            )
            .context("failed to prune diagnostic events by count")? as i64;
    }

    Ok(deleted)
}

/// Prune usage events older than 24h, scoped to a single generation
/// so non-active generation audit data is preserved.
pub fn prune_usage_events(conn: &Connection, generation_id: &str) -> Result<i64> {
    let cutoff = busytok_domain::now_ms() - 86_400_000;
    conn.execute(
        "DELETE FROM usage_events WHERE generation_id = ?1 AND timestamp_ms < ?2",
        params![generation_id, cutoff],
    )
    .map(|n| n as i64)
    .context("failed to prune usage events")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use busytok_domain::{AgentKind, NormalizedUsageEvent};

    fn make_test_event(id: &str, timestamp_ms: i64, tokens: i64) -> NormalizedUsageEvent {
        let mut event = NormalizedUsageEvent::minimal_for_test(id, AgentKind::ClaudeCode);
        event.timestamp_ms = timestamp_ms;
        event.total_tokens = tokens;
        event.input_tokens = tokens / 2;
        event.output_tokens = tokens / 2;
        event
    }

    #[test]
    fn insert_usage_events_batch_inserts_events() {
        let db = Database::open_in_memory().unwrap();
        let evt = make_test_event("evt-1", 1000, 100);
        let count = insert_usage_events_batch(db.conn(), &[evt], "gen-1").unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn insert_usage_events_batch_ignores_duplicates() {
        let db = Database::open_in_memory().unwrap();
        let evt = make_test_event("evt-1", 1000, 100);
        let count1 = insert_usage_events_batch(db.conn(), &[evt.clone()], "gen-1").unwrap();
        assert_eq!(count1, 1);
        let count2 = insert_usage_events_batch(db.conn(), &[evt], "gen-1").unwrap();
        assert_eq!(count2, 0, "duplicate should be ignored");
    }

    #[test]
    fn upsert_source_file_checkpoint_creates_and_updates() {
        let db = Database::open_in_memory().unwrap();
        // First insert
        upsert_source_file_checkpoint(
            db.conn(),
            "file-1",
            "src-1",
            "claude_code",
            "/tmp/file.jsonl",
            Some("inode-1"),
            100,
            500,
            Some(1000),
            "active",
            None,
        )
        .unwrap();

        // Verify it exists
        let offset: i64 = db
            .conn()
            .query_row(
                "SELECT offset_bytes FROM source_file_checkpoints WHERE id = 'file-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(offset, 100);

        // Update (upsert with new offset)
        upsert_source_file_checkpoint(
            db.conn(),
            "file-1",
            "src-1",
            "claude_code",
            "/tmp/file.jsonl",
            Some("inode-1"),
            200,
            600,
            Some(2000),
            "active",
            None,
        )
        .unwrap();

        let offset2: i64 = db
            .conn()
            .query_row(
                "SELECT offset_bytes FROM source_file_checkpoints WHERE id = 'file-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(offset2, 200);
    }

    #[test]
    fn insert_generation_observation_persists() {
        let db = Database::open_in_memory().unwrap();
        insert_generation_observation(
            db.conn(),
            "gen-1",
            "file-1",
            100,
            500,
            Some(1000),
            Some("ok"),
            None,
        )
        .unwrap();

        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM generation_file_observations WHERE generation_id = 'gen-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn enqueue_tail_replay_rows_adds_to_queue() {
        let db = Database::open_in_memory().unwrap();
        let rows = vec![TailReplayEnqueue {
            source_file_id: "file-1".to_string(),
            event_seq: 1,
            event_data_json: r#"{"id":"evt-1","total_tokens":100}"#.to_string(),
        }];
        enqueue_tail_replay_rows(db.conn(), &rows).unwrap();

        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM tail_replay_queue WHERE status = 'pending'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn prune_diagnostic_events_deletes_old_events() {
        let db = Database::open_in_memory().unwrap();
        // Insert a diagnostic event directly
        let diag = busytok_domain::OperationalDiagnosticEvent {
            id: "diag-1".to_string(),
            agent: None,
            source_id: Some("src-1".to_string()),
            source_file_id: None,
            source_path: None,
            source_line: None,
            category: "test".to_string(),
            severity: "info".to_string(),
            message: "test message".to_string(),
            detail_json: None,
            happened_at_ms: 1000,
            created_at_ms: 1000,
        };
        db.record_diagnostic_event(&diag).unwrap();

        // Prune events older than 2000 (our event is at 1000)
        let deleted = prune_diagnostic_events(db.conn(), 2000).unwrap();
        assert_eq!(deleted, 1);
    }

    #[test]
    fn update_materialized_aggregates_persists_to_buckets() {
        let db = Database::open_in_memory().unwrap();
        let mut evt = make_test_event("evt-bucket", 5000, 100);
        evt.model = Some("test-model".to_string());
        let tx = db.conn().unchecked_transaction().unwrap();
        update_materialized_aggregates_from_events(&tx, &[evt], "gen-1").unwrap();
        tx.commit().unwrap();

        // Check 2s bucket
        let count_2s: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM usage_buckets_2s WHERE generation_id = 'gen-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_2s, 1);

        // Check hour bucket
        let count_hour: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM usage_buckets_hour WHERE generation_id = 'gen-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_hour, 1);

        // Check day bucket
        let count_day: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM usage_buckets_day WHERE generation_id = 'gen-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_day, 1);
    }

    // ── Property-style aggregate consistency test ────────────────────────

    #[test]
    fn materialized_bucket_totals_equal_summed_canonical_totals() {
        let db = Database::open_in_memory().unwrap();
        let generation_id = "gen-consistency";

        // Insert multiple varied event batches
        let batches: Vec<Vec<NormalizedUsageEvent>> = vec![
            vec![
                {
                    let mut e =
                        NormalizedUsageEvent::minimal_for_test("evt-a1", AgentKind::ClaudeCode);
                    e.timestamp_ms = 1000;
                    e.total_tokens = 100;
                    e.input_tokens = 60;
                    e.output_tokens = 40;
                    e.cost_usd = Some(0.01);
                    e.model = Some("claude-sonnet".to_string());
                    e
                },
                {
                    let mut e = NormalizedUsageEvent::minimal_for_test("evt-a2", AgentKind::Codex);
                    e.timestamp_ms = 3000;
                    e.total_tokens = 200;
                    e.input_tokens = 100;
                    e.output_tokens = 100;
                    e.cost_usd = None;
                    e.model = Some("gpt-5".to_string());
                    e
                },
            ],
            vec![
                {
                    let mut e =
                        NormalizedUsageEvent::minimal_for_test("evt-b1", AgentKind::ClaudeCode);
                    e.timestamp_ms = 5000;
                    e.total_tokens = 50;
                    e.input_tokens = 30;
                    e.output_tokens = 20;
                    e.cost_usd = Some(0.005);
                    e.model = Some("claude-sonnet".to_string());
                    e
                },
                {
                    let mut e = NormalizedUsageEvent::minimal_for_test("evt-b2", AgentKind::Codex);
                    e.timestamp_ms = 7000;
                    e.total_tokens = 300;
                    e.input_tokens = 150;
                    e.output_tokens = 150;
                    e.cost_usd = Some(0.03);
                    e.model = Some("gpt-5".to_string());
                    e
                },
            ],
        ];

        // Write all events via the batch insert
        for batch in &batches {
            insert_usage_events_batch(db.conn(), batch, generation_id).unwrap();
        }

        // Compute canonical totals from usage_events
        let canonical: (i64, i64, i64, i64, Option<f64>) = db
            .conn()
            .query_row(
                "SELECT \
                    COALESCE(SUM(total_tokens), 0), \
                    COALESCE(SUM(input_tokens), 0), \
                    COALESCE(SUM(output_tokens), 0), \
                    COUNT(*), \
                    SUM(cost_usd) \
                 FROM usage_events \
                 WHERE generation_id = ?1",
                params![generation_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();

        // Now update materialized aggregates (wrapped in a transaction so that
        // the writer actor can call this from within its own outer transaction).
        let all_events: Vec<NormalizedUsageEvent> = batches.into_iter().flatten().collect();
        let tx = db.conn().unchecked_transaction().unwrap();
        update_materialized_aggregates_from_events(&tx, &all_events, generation_id).unwrap();
        tx.commit().unwrap();

        // Check 2s bucket totals match canonical
        let buck_2s: (i64, i64) = db
            .conn()
            .query_row(
                "SELECT COALESCE(SUM(total_tokens), 0), COALESCE(SUM(event_count), 0) \
                 FROM usage_buckets_2s WHERE generation_id = ?1",
                params![generation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            buck_2s.0, canonical.0,
            "2s bucket total_tokens must equal canonical total_tokens"
        );
        assert_eq!(
            buck_2s.1, canonical.3,
            "2s bucket event_count must equal canonical event_count"
        );

        // Check hour bucket totals match canonical
        let buck_hour: (i64, i64) = db
            .conn()
            .query_row(
                "SELECT COALESCE(SUM(total_tokens), 0), COALESCE(SUM(event_count), 0) \
                 FROM usage_buckets_hour WHERE generation_id = ?1",
                params![generation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            buck_hour.0, canonical.0,
            "hour bucket total_tokens must equal canonical total_tokens"
        );
        assert_eq!(
            buck_hour.1, canonical.3,
            "hour bucket event_count must equal canonical event_count"
        );

        // Check day bucket totals match canonical
        let buck_day: (i64, i64) = db
            .conn()
            .query_row(
                "SELECT COALESCE(SUM(total_tokens), 0), COALESCE(SUM(event_count), 0) \
                 FROM usage_buckets_day WHERE generation_id = ?1",
                params![generation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            buck_day.0, canonical.0,
            "day bucket total_tokens must equal canonical total_tokens"
        );
        assert_eq!(
            buck_day.1, canonical.3,
            "day bucket event_count must equal canonical event_count"
        );
    }

    #[test]
    fn prune_usage_events_deletes_old_in_active_gen_preserves_inactive() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        let now = busytok_domain::now_ms();
        conn.execute(
            "INSERT INTO audit_generations (generation_id, state, is_active, started_at_ms, created_at_ms, updated_at_ms) \
             VALUES ('gen-a', 'promoted', 1, ?1, ?1, ?1), ('gen-b', 'sealed', 0, ?1, ?1, ?1)",
            params![now],
        ).unwrap();

        let old_ts = now - 90_000_000;
        let recent_ts = now - 1_000;
        let old_evt = make_test_event("old-active", old_ts, 100);
        let recent_evt = make_test_event("recent-active", recent_ts, 200);
        let old_inactive = make_test_event("old-inactive", old_ts, 300);

        insert_usage_events_batch(conn, &[old_evt], "gen-a").unwrap();
        insert_usage_events_batch(conn, &[recent_evt], "gen-a").unwrap();
        insert_usage_events_batch(conn, &[old_inactive], "gen-b").unwrap();

        let deleted = prune_usage_events(conn, "gen-a").unwrap();
        assert!(deleted >= 1);

        let active_ids: Vec<String> = conn
            .prepare("SELECT id FROM usage_events WHERE generation_id = 'gen-a'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(!active_ids.contains(&"old-active".to_string()));
        assert!(active_ids.contains(&"recent-active".to_string()));

        let inactive_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_events WHERE generation_id = 'gen-b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(inactive_count, 1);
    }

    #[test]
    fn prune_usage_events_empty_table_is_noop() {
        let db = Database::open_in_memory().unwrap();
        let now = busytok_domain::now_ms();
        db.conn().execute(
            "INSERT INTO audit_generations (generation_id, state, is_active, started_at_ms, created_at_ms, updated_at_ms) \
             VALUES ('gen-a', 'promoted', 1, ?1, ?1, ?1)",
            params![now],
        ).unwrap();
        let deleted = prune_usage_events(db.conn(), "gen-a").unwrap();
        assert_eq!(deleted, 0);
    }

    // ── upsert_daily_usage_for_events tests ────────────────────────────────

    #[test]
    fn upsert_daily_usage_for_events_inserts_rows() {
        let db = Database::open_in_memory().unwrap();
        let rtz = busytok_domain::ReportingTimezone::parse("Asia/Shanghai").unwrap();
        let evt = make_test_event("evt-daily-1", 1_781_193_600_000, 100);
        // 2026-06-12 00:00 CST

        upsert_daily_usage_for_events(db.conn(), &[evt], &rtz, "gen-test").unwrap();

        let (tokens, count): (i64, i64) = db.conn().query_row(
            "SELECT total_tokens, event_count FROM daily_usage WHERE timezone = 'Asia/Shanghai'",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(tokens, 100);
        assert_eq!(count, 1);
    }

    #[test]
    fn upsert_daily_usage_for_events_accumulates_on_conflict() {
        let db = Database::open_in_memory().unwrap();
        let rtz = busytok_domain::ReportingTimezone::parse("UTC").unwrap();
        let ts = 1_700_000_000_000;
        let evt1 = make_test_event("evt-a", ts, 100);
        let evt2 = make_test_event("evt-b", ts, 200);

        upsert_daily_usage_for_events(db.conn(), &[evt1], &rtz, "gen-test").unwrap();
        upsert_daily_usage_for_events(db.conn(), &[evt2], &rtz, "gen-test").unwrap();

        let (tokens, count): (i64, i64) = db
            .conn()
            .query_row(
                "SELECT total_tokens, event_count FROM daily_usage WHERE timezone = 'UTC'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(tokens, 300, "tokens should accumulate");
        assert_eq!(count, 2, "event_count should increment");
    }

    #[test]
    fn upsert_daily_usage_for_events_respects_timezone_date() {
        let db = Database::open_in_memory().unwrap();
        // Same UTC timestamp, different timezones → different dates
        let ts = 1_781_193_600_000i64; // 2026-06-11 16:00 UTC = 2026-06-12 00:00 CST
        let rtz_shanghai = busytok_domain::ReportingTimezone::parse("Asia/Shanghai").unwrap();
        let rtz_utc = busytok_domain::ReportingTimezone::parse("UTC").unwrap();
        let mut evt = make_test_event("evt-tz", ts, 50);

        upsert_daily_usage_for_events(db.conn(), &[evt.clone()], &rtz_shanghai, "gen-test")
            .unwrap();
        upsert_daily_usage_for_events(db.conn(), &[evt.clone()], &rtz_utc, "gen-test").unwrap();

        let sh_date: String = db
            .conn()
            .query_row(
                "SELECT date FROM daily_usage WHERE timezone = 'Asia/Shanghai'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let utc_date: String = db
            .conn()
            .query_row(
                "SELECT date FROM daily_usage WHERE timezone = 'UTC'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sh_date, "2026-06-12");
        assert_eq!(utc_date, "2026-06-11");
    }

    #[test]
    fn upsert_daily_usage_for_events_empty_is_noop() {
        let db = Database::open_in_memory().unwrap();
        let rtz = busytok_domain::ReportingTimezone::parse("UTC").unwrap();
        upsert_daily_usage_for_events(db.conn(), &[], &rtz, "gen-test").unwrap();
    }

    /// Regression: writer flush must not commit usage_events unless daily_usage
    /// upsert also succeeds. Simulates the failure path by rolling back the tx.
    #[test]
    fn daily_usage_failure_rolls_back_usage_events_same_transaction() {
        let db = Database::open_in_memory().unwrap();
        let rtz = busytok_domain::ReportingTimezone::parse("UTC").unwrap();
        let evt = make_test_event("evt-daily-atomic", 1_700_000_000_000, 100);

        let tx = db.conn().unchecked_transaction().unwrap();
        let inserted =
            insert_usage_events_batch_returning_inserted(&tx, &[evt.clone()], "gen-daily-atomic")
                .unwrap();
        assert_eq!(inserted.len(), 1, "event should be inserted in tx");

        upsert_daily_usage_for_events(&tx, &inserted, &rtz, "gen-test").unwrap();

        // Simulate a failure after daily_usage was written —
        // the writer now returns Err to roll back the whole tx.
        drop(tx); // rollback (the writer would do this via ?)

        let event_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM usage_events WHERE id = 'evt-daily-atomic'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            event_count, 0,
            "rolled-back event must not be visible outside tx"
        );

        let daily_count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM daily_usage", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            daily_count, 0,
            "rolled-back daily_usage must not be visible outside tx"
        );
    }
}
