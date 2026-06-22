//! Store read-query primitives.
//!
//! These functions are called by the read service (Task 8) to hydrate UI
//! responses. They take a `&rusqlite::Connection` and return focused
//! `read_models` structs. All queries are synchronous and read-only.

use anyhow::{Context, Result};
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use time::OffsetDateTime;

use crate::read_models::*;

const MILLIS_PER_DAY: i64 = 86_400_000;
const SUMMARY_DAY_BUCKET_THRESHOLD_MS: i64 = 14 * MILLIS_PER_DAY;

#[derive(Debug, Deserialize, Serialize)]
struct SortCursor {
    sort: i64,
    key: String,
}

fn encode_sort_cursor(sort: i64, key: &str) -> Result<String> {
    let payload = SortCursor {
        sort,
        key: key.to_string(),
    };
    let bytes = serde_json::to_vec(&payload).context("failed to serialize cursor")?;
    Ok(hex::encode(bytes))
}

fn parse_sort_cursor(cursor: Option<&str>) -> Result<Option<SortCursor>> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let bytes = hex::decode(cursor).context("invalid cursor encoding")?;
    let payload: SortCursor = serde_json::from_slice(&bytes).context("invalid cursor payload")?;
    Ok(Some(payload))
}

fn append_source_health_status_filter(
    sql: &mut String,
    args: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    status_filter: Option<&str>,
) {
    let Some(status_filter) = status_filter.filter(|value| !value.is_empty()) else {
        return;
    };
    let idx = args.len() + 1;
    match status_filter {
        "scanning_or_active" => {
            sql.push_str(&format!(" AND s.status IN (?{idx}, ?{})", idx + 1));
            args.push(Box::new("scanning".to_string()));
            args.push(Box::new("active".to_string()));
        }
        "idle" => {
            sql.push_str(" AND s.status NOT IN ('error', 'warning', 'scanning', 'active')");
        }
        other => {
            sql.push_str(&format!(" AND s.status = ?{idx}"));
            args.push(Box::new(other.to_string()));
        }
    }
}

fn breakdown_filter_column(filter: BreakdownFilterField) -> &'static str {
    match filter {
        BreakdownFilterField::Project => "project_hash",
        BreakdownFilterField::Model => "model",
        BreakdownFilterField::Session => "session_id",
    }
}

fn utc_date_from_epoch_ms(epoch_ms: i64) -> Result<String> {
    let timestamp = OffsetDateTime::from_unix_timestamp(epoch_ms.div_euclid(1000))
        .context("invalid epoch timestamp")?;
    Ok(timestamp.date().to_string())
}

fn floor_utc_day(epoch_ms: i64) -> i64 {
    epoch_ms.div_euclid(MILLIS_PER_DAY) * MILLIS_PER_DAY
}

fn ceil_utc_day(epoch_ms: i64) -> i64 {
    let floored = floor_utc_day(epoch_ms);
    if epoch_ms == floored {
        floored
    } else {
        floored + MILLIS_PER_DAY
    }
}

struct ExactRangeSegments {
    raw_prefix_end_ms: i64,
    raw_suffix_start_ms: i64,
    day_start_date: String,
    day_end_date: String,
}

fn exact_range_segments(start_ms: i64, end_ms: i64) -> Result<ExactRangeSegments> {
    let full_day_start_ms = ceil_utc_day(start_ms);
    let full_day_end_ms = floor_utc_day(end_ms);

    if full_day_start_ms < full_day_end_ms {
        Ok(ExactRangeSegments {
            raw_prefix_end_ms: full_day_start_ms,
            raw_suffix_start_ms: full_day_end_ms,
            day_start_date: utc_date_from_epoch_ms(full_day_start_ms)?,
            day_end_date: utc_date_from_epoch_ms(full_day_end_ms)?,
        })
    } else {
        Ok(ExactRangeSegments {
            raw_prefix_end_ms: end_ms,
            raw_suffix_start_ms: end_ms,
            day_start_date: "9999-12-31".to_string(),
            day_end_date: "9999-12-31".to_string(),
        })
    }
}

// ── Service state ────────────────────────────────────────────────────────────

/// Read the current service state joined with the latest event sequence number.
///
/// If the singleton `service_state` row does not exist yet (fresh install),
/// returns a default row with readiness `"starting"` so the service can boot.
pub fn read_service_state(conn: &Connection) -> Result<ServiceStateRow> {
    let row = conn.query_row(
        "SELECT \
                COALESCE(s.writer_queue_depth, 0), \
                COALESCE(s.aggregate_lag_ms, 0), \
                s.readiness, \
                s.active_generation_id, \
                s.last_exact_rebuild_at_ms, \
                COALESCE(s.updated_at_ms, 0), \
                e.latest_event_seq \
             FROM service_state s \
             LEFT JOIN event_sequence_state e ON e.id = 1 \
             WHERE s.id = 1",
        [],
        |row| {
            Ok(ServiceStateRow {
                writer_queue_depth: row.get(0)?,
                aggregate_lag_ms: row.get(1)?,
                readiness: row.get(2)?,
                active_generation_id: row.get(3)?,
                last_exact_rebuild_at_ms: row.get(4)?,
                updated_at_ms: row.get(5)?,
                latest_event_seq: row.get(6)?,
            })
        },
    );

    match row {
        Ok(r) => Ok(r),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(ServiceStateRow::default()),
        Err(e) => Err(e).context("failed to read service state"),
    }
}

// ── Overview ─────────────────────────────────────────────────────────────────

/// Read aggregate usage totals for the given generation and time range.
///
/// Returns zeros for an empty range. The `generation_id` is used to scope
/// results to a specific rebuild generation.
pub fn read_overview_summary(
    conn: &Connection,
    generation_id: &str,
    range: &RangeWindow,
) -> Result<OverviewSummaryRow> {
    let table = summary_bucket_table(range);
    let sql = format!(
        "SELECT \
            COALESCE(SUM(total_tokens), 0), \
            SUM(cost_usd), \
            COALESCE(SUM(event_count), 0), \
            COALESCE(SUM(CASE \
                WHEN cost_status IN ('exact', 'estimated', 'partial') \
                THEN event_count ELSE 0 END), 0), \
            COALESCE(SUM(CASE \
                WHEN cost_status IN ('unavailable', 'unknown', 'partial') \
                THEN event_count ELSE 0 END), 0) \
         FROM {table} \
         WHERE generation_id = ?1 \
           AND bucket_start_ms >= ?2 AND bucket_start_ms < ?3"
    );
    let row = conn
        .query_row(
            &sql,
            params![generation_id, range.start_ms, range.end_ms],
            |row| {
                Ok(overview_summary_row_from_parts(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .with_context(|| format!("failed to read overview summary from {table}"))?;

    Ok(row)
}

/// Read exact overview summary totals directly from `usage_events`.
pub fn read_overview_summary_exact(
    conn: &Connection,
    generation_id: &str,
    range: &RangeWindow,
) -> Result<OverviewSummaryRow> {
    conn.query_row(
        "SELECT
            COALESCE(SUM(total_tokens), 0),
            SUM(cost_usd),
            COALESCE(COUNT(*), 0),
            COALESCE(COUNT(cost_usd), 0),
            COALESCE(COUNT(*), 0) - COALESCE(COUNT(cost_usd), 0)
         FROM usage_events
         WHERE generation_id = ?1
           AND timestamp_ms >= ?2 AND timestamp_ms < ?3",
        params![generation_id, range.start_ms, range.end_ms],
        |row| {
            Ok(overview_summary_row_from_parts(
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )
    .context("failed to read exact overview summary")
}

fn summary_bucket_table(range: &RangeWindow) -> &'static str {
    if range.end_ms.saturating_sub(range.start_ms) >= SUMMARY_DAY_BUCKET_THRESHOLD_MS
        && is_utc_day_aligned(range.start_ms)
        && is_utc_day_aligned(range.end_ms)
    {
        "usage_buckets_day"
    } else {
        "usage_buckets_hour"
    }
}

fn is_utc_day_aligned(epoch_ms: i64) -> bool {
    epoch_ms.rem_euclid(MILLIS_PER_DAY) == 0
}

fn overview_summary_row_from_parts(
    total_tokens: i64,
    raw_cost: Option<f64>,
    event_count: i64,
    with_cost: i64,
    without_cost: i64,
) -> OverviewSummaryRow {
    let has_cost = with_cost > 0;
    let has_no_cost = without_cost > 0;
    let total_cost_usd = match (raw_cost, has_cost) {
        (Some(cost), true) if cost > 0.0 => Some(cost),
        (Some(_), true) => Some(0.0),
        _ => None,
    };

    OverviewSummaryRow {
        total_tokens,
        total_cost_usd,
        event_count,
        has_cost,
        has_no_cost,
    }
}

/// Read trend bucket data for an exact time window.
///
/// Returns one `OverviewTrendBucketRow` per `(bucket_start_ms, agent, model)`
/// combination found in the `usage_buckets_2s` table for the given generation
/// and time range.
pub fn read_live_window_exact(
    conn: &Connection,
    generation_id: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<OverviewTrendBucketRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT \
                bucket_start_ms, \
                agent, \
                model, \
                COALESCE(SUM(total_tokens), 0), \
                COALESCE(SUM(cost_usd), 0), \
                COALESCE(SUM(event_count), 0) \
             FROM usage_buckets_2s \
             WHERE generation_id = ?1 \
               AND bucket_start_ms >= ?2 AND bucket_start_ms < ?3 \
             GROUP BY bucket_start_ms, agent, model \
             ORDER BY bucket_start_ms ASC",
        )
        .context("failed to prepare read_live_window_exact query")?;

    let rows = stmt.query_map(params![generation_id, start_ms, end_ms], |row| {
        let tokens: i64 = row.get(3)?;
        let cost: f64 = row.get(4)?;
        let event_count: i64 = row.get(5)?;
        let has_cost = cost > 0.0;
        let has_no_cost = tokens > 0 && !has_cost;
        let cost_usd = if has_cost { Some(cost) } else { None };
        Ok(OverviewTrendBucketRow {
            key: format!(
                "{}-{}-{}",
                row.get::<_, i64>(0)?,    // bucket_start_ms
                row.get::<_, String>(1)?, // agent
                row.get::<_, String>(2)?, // model
            ),
            start_ms: row.get(0)?,
            end_ms: row.get::<_, i64>(0)? + 2000,
            tokens,
            cost_usd,
            event_count,
            has_cost,
            has_no_cost,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

// ── Activity ─────────────────────────────────────────────────────────────────

/// Read a paginated activity list from usage_events.
///
/// Returns events ordered by `timestamp_ms DESC, id DESC`.
pub fn read_activity_list(
    conn: &Connection,
    generation_id: &str,
    start_ms: i64,
    end_ms: i64,
    limit: i64,
    cursor: Option<(i64, String)>,
) -> Result<Vec<ActivityListRow>> {
    let sql = if cursor.is_some() {
        "SELECT \
            id, \
            timestamp_ms, \
            COALESCE(client_kind, '') AS client_kind, \
            COALESCE(session_id, '') AS session_id, \
            COALESCE(source_file_id, '') AS source_file_id, \
            COALESCE(source_path, '') AS source_path, \
            project_hash, \
            project_path, \
            model, \
            total_tokens, \
            input_tokens, \
            cached_input_tokens, \
            cost_usd, \
            is_error \
         FROM usage_events \
         WHERE generation_id = ?1 \
           AND timestamp_ms >= ?2 AND timestamp_ms < ?3 \
           AND (timestamp_ms < ?5 OR (timestamp_ms = ?5 AND id < ?6)) \
         ORDER BY timestamp_ms DESC, id DESC \
         LIMIT ?4"
    } else {
        "SELECT \
            id, \
            timestamp_ms, \
            COALESCE(client_kind, '') AS client_kind, \
            COALESCE(session_id, '') AS session_id, \
            COALESCE(source_file_id, '') AS source_file_id, \
            COALESCE(source_path, '') AS source_path, \
            project_hash, \
            project_path, \
            model, \
            total_tokens, \
            input_tokens, \
            cached_input_tokens, \
            cost_usd, \
            is_error \
         FROM usage_events \
         WHERE generation_id = ?1 \
           AND timestamp_ms >= ?2 AND timestamp_ms < ?3 \
         ORDER BY timestamp_ms DESC, id DESC \
         LIMIT ?4"
    };

    let mut stmt = conn
        .prepare(&sql)
        .context("failed to prepare read_activity_list query")?;

    let map_row = |row: &rusqlite::Row<'_>| {
        Ok(ActivityListRow {
            id: row.get(0)?,
            happened_at_ms: row.get(1)?,
            client_kind: row.get(2)?,
            session_id: row.get(3)?,
            source_file_id: row.get(4)?,
            source_path: row.get(5)?,
            project_hash: row.get(6)?,
            project_path: row.get(7)?,
            model: row.get(8)?,
            total_tokens: row.get(9)?,
            input_tokens: row.get(10)?,
            cached_input_tokens: row.get(11)?,
            cost_usd: row.get(12)?,
            is_error: row.get::<_, i32>(13)? != 0,
        })
    };

    let rows = match &cursor {
        Some((cursor_ts, cursor_id)) => stmt.query_map(
            params![
                generation_id,
                start_ms,
                end_ms,
                limit,
                cursor_ts,
                cursor_id.as_str()
            ],
            map_row,
        )?,
        None => stmt.query_map(params![generation_id, start_ms, end_ms, limit], map_row)?,
    };

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Read bounded activity rows filtered by a breakdown detail dimension.
pub fn read_breakdown_activity_list(
    conn: &Connection,
    generation_id: &str,
    filter: BreakdownFilterField,
    filter_value: &str,
    start_ms: i64,
    end_ms: i64,
    limit: i64,
) -> Result<Vec<ActivityListRow>> {
    let field = breakdown_filter_column(filter);
    let sql = format!(
        "SELECT id, timestamp_ms, \
                COALESCE(client_kind, ''), \
                COALESCE(session_id, ''), \
                COALESCE(source_file_id, ''), \
                COALESCE(source_path, ''), \
                project_hash, project_path, model, \
                total_tokens, input_tokens, cached_input_tokens, \
                cost_usd, is_error \
         FROM usage_events \
         WHERE generation_id = ?1 \
           AND timestamp_ms >= ?2 AND timestamp_ms < ?3 \
           AND {field} = ?4 \
         ORDER BY timestamp_ms DESC, id DESC \
         LIMIT ?5"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            generation_id,
            start_ms,
            end_ms,
            filter_value,
            limit.clamp(1, 1000)
        ],
        |row| {
            Ok(ActivityListRow {
                id: row.get(0)?,
                happened_at_ms: row.get(1)?,
                client_kind: row.get(2)?,
                session_id: row.get(3)?,
                source_file_id: row.get(4)?,
                source_path: row.get(5)?,
                project_hash: row.get(6)?,
                project_path: row.get(7)?,
                model: row.get(8)?,
                total_tokens: row.get(9)?,
                input_tokens: row.get(10)?,
                cached_input_tokens: row.get(11)?,
                cost_usd: row.get(12)?,
                is_error: row.get::<_, i32>(13)? != 0,
            })
        },
    )?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// Read full detail for a single usage event by its ID.
pub fn read_activity_detail(
    conn: &Connection,
    event_id: &str,
    generation_id: &str,
) -> Result<ActivityDetailRow> {
    conn.query_row(
        "SELECT \
            id, agent, source_file_id, source_path, source_line, \
            source_offset_start, source_offset_end, session_id, turn_id, \
            source_request_id, message_id, timestamp_ms, project_path, \
            project_hash, cwd, model, model_provider, agent_version, \
            client_kind, speed, input_tokens, output_tokens, total_tokens, \
            cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
            reasoning_tokens, thoughts_tokens, tool_tokens, cost_usd, \
            estimated_cost_usd, cost_currency, cost_source, \
            price_catalog_version, is_error, error_type, raw_event_hash, \
            usage_limit_reset_time_ms, generation_id \
         FROM usage_events \
         WHERE id = ?1 AND generation_id = ?2",
        params![event_id, generation_id],
        |row| {
            Ok(ActivityDetailRow {
                id: row.get(0)?,
                agent: row.get(1)?,
                source_file_id: row.get(2)?,
                source_path: row.get(3)?,
                source_line: row.get(4)?,
                source_offset_start: row.get(5)?,
                source_offset_end: row.get(6)?,
                session_id: row.get(7)?,
                turn_id: row.get(8)?,
                source_request_id: row.get(9)?,
                message_id: row.get(10)?,
                timestamp_ms: row.get(11)?,
                project_path: row.get(12)?,
                project_hash: row.get(13)?,
                cwd: row.get(14)?,
                model: row.get(15)?,
                model_provider: row.get(16)?,
                agent_version: row.get(17)?,
                client_kind: row.get(18)?,
                speed: row.get(19)?,
                input_tokens: row.get(20)?,
                output_tokens: row.get(21)?,
                total_tokens: row.get(22)?,
                cached_input_tokens: row.get(23)?,
                cache_creation_tokens: row.get(24)?,
                cache_read_tokens: row.get(25)?,
                reasoning_tokens: row.get(26)?,
                thoughts_tokens: row.get(27)?,
                tool_tokens: row.get(28)?,
                cost_usd: row.get(29)?,
                estimated_cost_usd: row.get(30)?,
                cost_currency: row.get(31)?,
                cost_source: row.get(32)?,
                price_catalog_version: row.get(33)?,
                is_error: row.get::<_, i32>(34)? != 0,
                error_type: row.get(35)?,
                raw_event_hash: row.get(36)?,
                usage_limit_reset_time_ms: row.get(37)?,
                generation_id: row.get(38)?,
            })
        },
    )
    .context("failed to read activity detail")
}

/// Read source metadata joined through `log_files` for an activity event.
pub fn read_activity_source_info(
    conn: &Connection,
    source_file_id: &str,
) -> Result<Option<ActivitySourceInfoRow>> {
    conn.query_row(
        "SELECT lf.source_id, ls.agent, ls.root_path
         FROM log_files lf
         INNER JOIN log_sources ls ON ls.id = lf.source_id
         WHERE lf.id = ?1",
        params![source_file_id],
        |row| {
            Ok(ActivitySourceInfoRow {
                source_id: row.get(0)?,
                agent: row.get(1)?,
                root_path: row.get(2)?,
            })
        },
    )
    .optional()
    .context("failed to read activity source info")
}

/// Read materialized source health rows for the Clients page.
pub fn read_source_health_summaries(
    conn: &Connection,
    generation_id: &str,
    limit: i64,
    cursor: Option<String>,
    client_id_filter: Option<&str>,
    status_filter: Option<&str>,
) -> Result<CursorPage<SourceHealthSummaryRow>> {
    let page_size = limit.clamp(1, 500);
    let query_limit = page_size + 1;
    let cursor = parse_sort_cursor(cursor.as_deref())?;
    let mut sql = String::from(
        "SELECT \
            s.source_id, s.agent, s.root_path, s.source_type, s.status, \
            s.configured_by_user, s.last_scan_at_ms, s.file_count, \
            s.parsed_file_count, s.event_count, s.last_error, \
            s.latest_activity_at_ms \
         FROM source_health_summary s \
         WHERE s.generation_id = ?1",
    );
    let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(generation_id.to_string())];

    if let Some(client_id) = client_id_filter.filter(|value| !value.is_empty()) {
        let idx = args.len() + 1;
        sql.push_str(&format!(" AND s.agent = ?{idx}"));
        args.push(Box::new(client_id.to_string()));
    }

    append_source_health_status_filter(&mut sql, &mut args, status_filter);

    if let Some(cursor) = cursor {
        let idx = args.len() + 1;
        sql.push_str(&format!(
            " AND (COALESCE(s.last_scan_at_ms, 0) < ?{idx} \
               OR (COALESCE(s.last_scan_at_ms, 0) = ?{idx2} AND s.source_id < ?{idx3}))",
            idx2 = idx + 1,
            idx3 = idx + 2,
        ));
        args.push(Box::new(cursor.sort));
        args.push(Box::new(cursor.sort));
        args.push(Box::new(cursor.key));
    }

    let limit_idx = args.len() + 1;
    sql.push_str(&format!(
        " ORDER BY COALESCE(s.last_scan_at_ms, 0) DESC, s.source_id DESC LIMIT ?{limit_idx}"
    ));
    args.push(Box::new(query_limit));

    let arg_refs: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|arg| arg.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(arg_refs.as_slice(), |row| {
        Ok(SourceHealthSummaryRow {
            source_id: row.get(0)?,
            agent: row.get(1)?,
            root_path: row.get(2)?,
            source_type: row.get(3)?,
            status: row.get(4)?,
            configured_by_user: row.get::<_, i32>(5)? != 0,
            last_scan_at_ms: row.get(6)?,
            file_count: row.get(7)?,
            parsed_file_count: row.get(8)?,
            event_count: row.get(9)?,
            last_error: row.get(10)?,
            latest_activity_at_ms: row.get(11)?,
        })
    })?;

    let mut items = Vec::new();
    for row in rows {
        items.push(row?);
    }
    let next_cursor = if items.len() > page_size as usize {
        items.pop().expect("extra cursor row exists");
        let last = items.last().expect("visible cursor row exists");
        Some(encode_sort_cursor(
            last.last_scan_at_ms.unwrap_or(0),
            &last.source_id,
        )?)
    } else {
        None
    };

    Ok(CursorPage { items, next_cursor })
}

/// Read filtered source-summary totals for the clients snapshot header.
pub fn read_source_health_summary_totals(
    conn: &Connection,
    generation_id: &str,
    client_id_filter: Option<&str>,
    status_filter: Option<&str>,
) -> Result<SourceHealthSummaryTotalsRow> {
    let mut sql = String::from(
        "SELECT \
            COUNT(*) AS source_count, \
            COALESCE(SUM(CASE WHEN s.status = 'active' THEN 1 ELSE 0 END), 0) AS active_source_count \
         FROM source_health_summary s \
         WHERE s.generation_id = ?1",
    );
    let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(generation_id.to_string())];

    if let Some(client_id) = client_id_filter.filter(|value| !value.is_empty()) {
        let idx = args.len() + 1;
        sql.push_str(&format!(" AND s.agent = ?{idx}"));
        args.push(Box::new(client_id.to_string()));
    }

    append_source_health_status_filter(&mut sql, &mut args, status_filter);

    let arg_refs: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|arg| arg.as_ref()).collect();
    conn.query_row(&sql, arg_refs.as_slice(), |row| {
        Ok(SourceHealthSummaryTotalsRow {
            source_count: row.get(0)?,
            active_source_count: row.get(1)?,
        })
    })
    .context("failed to read source health summary totals")
}

/// Read client summary card rollups from materialized source summaries.
pub fn read_client_rollups(conn: &Connection, generation_id: &str) -> Result<Vec<ClientRollupRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT \
                s.agent AS client_kind, \
                COALESCE(SUM(CASE WHEN s.status = 'active' THEN 1 ELSE 0 END), 0) AS active_source_count, \
                COALESCE(SUM(s.event_count), 0) AS event_count, \
                MAX(s.last_scan_at_ms) AS last_scan_at_ms \
             FROM source_health_summary s \
             WHERE s.generation_id = ?1 \
             GROUP BY s.agent \
             ORDER BY event_count DESC, client_kind ASC",
        )
        .context("failed to prepare client rollup query")?;
    let rows = stmt.query_map(params![generation_id], |row| {
        Ok(ClientRollupRow {
            client_kind: row.get(0)?,
            active_source_count: row.get(1)?,
            event_count: row.get(2)?,
            last_scan_at_ms: row.get(3)?,
        })
    })?;

    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// Read a single client source detail row by source ID.
pub fn read_client_source_detail(
    conn: &Connection,
    source_id: &str,
) -> Result<Option<ClientSourceDetailRow>> {
    conn.query_row(
        "SELECT s.id, s.agent, s.root_path, s.source_type, \
                s.status, s.configured_by_user, \
                s.last_scan_completed_at_ms, \
                COALESCE(sf.file_count, 0), \
                COALESCE(sf.parsed_count, 0), \
                COALESCE(uec.event_count, 0), \
                s.last_error \
         FROM log_sources s \
         LEFT JOIN ( \
             SELECT source_id, COUNT(*) AS file_count, \
                    COUNT(CASE WHEN offset_bytes > 0 THEN 1 END) AS parsed_count \
             FROM log_files GROUP BY source_id \
         ) sf ON sf.source_id = s.id \
         LEFT JOIN ( \
             SELECT lf.source_id, COUNT(*) AS event_count \
             FROM log_files lf INNER JOIN usage_events ue ON ue.source_file_id = lf.id \
             GROUP BY lf.source_id \
         ) uec ON uec.source_id = s.id \
         WHERE s.id = ?1 AND s.status != 'removed'",
        params![source_id],
        |row| {
            Ok(ClientSourceDetailRow {
                source_id: row.get(0)?,
                agent: row.get(1)?,
                root_path: row.get(2)?,
                source_type: row.get(3)?,
                status: row.get(4)?,
                configured_by_user: row.get::<_, i32>(5)? != 0,
                last_scan_at_ms: row.get(6)?,
                file_count: row.get(7)?,
                parsed_file_count: row.get(8)?,
                event_count: row.get(9)?,
                last_error: row.get(10)?,
            })
        },
    )
    .optional()
    .context("failed to read client source detail")
}

/// Read bounded recent activity items for a source.
pub fn read_client_source_recent_activity(
    conn: &Connection,
    source_id: &str,
    limit: i64,
) -> Result<Vec<ActivityListRow>> {
    let mut stmt = conn.prepare(
        "SELECT ue.id, ue.timestamp_ms, \
                COALESCE(ue.client_kind, ''), \
                COALESCE(ue.session_id, ''), \
                COALESCE(ue.source_file_id, ''), \
                COALESCE(ue.source_path, ''), \
                ue.project_hash, ue.project_path, ue.model, \
                ue.total_tokens, ue.input_tokens, ue.cached_input_tokens, \
                ue.cost_usd, ue.is_error \
         FROM usage_events ue \
         INNER JOIN log_files lf ON lf.id = ue.source_file_id \
         WHERE lf.source_id = ?1 \
         ORDER BY ue.timestamp_ms DESC, ue.id DESC \
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![source_id, limit.clamp(1, 100)], |row| {
        Ok(ActivityListRow {
            id: row.get(0)?,
            happened_at_ms: row.get(1)?,
            client_kind: row.get(2)?,
            session_id: row.get(3)?,
            source_file_id: row.get(4)?,
            source_path: row.get(5)?,
            project_hash: row.get(6)?,
            project_path: row.get(7)?,
            model: row.get(8)?,
            total_tokens: row.get(9)?,
            input_tokens: row.get(10)?,
            cached_input_tokens: row.get(11)?,
            cost_usd: row.get(12)?,
            is_error: row.get::<_, i32>(13)? != 0,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// Read token component totals for a model detail view.
pub fn read_model_token_breakdown(
    conn: &Connection,
    generation_id: &str,
    model: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<ModelTokenBreakdownRow> {
    conn.query_row(
        "SELECT COALESCE(SUM(input_tokens), 0), \
                COALESCE(SUM(output_tokens), 0), \
                COALESCE(SUM(cached_input_tokens), 0), \
                COALESCE(SUM(reasoning_tokens), 0) \
         FROM usage_events \
         WHERE generation_id = ?1 \
           AND timestamp_ms >= ?2 AND timestamp_ms < ?3 \
           AND model = ?4",
        params![generation_id, start_ms, end_ms, model],
        |row| {
            Ok(ModelTokenBreakdownRow {
                input_tokens: row.get(0)?,
                output_tokens: row.get(1)?,
                cached_input_tokens: row.get(2)?,
                reasoning_tokens: row.get(3)?,
            })
        },
    )
    .context("failed to read model token breakdown")
}

/// Read bounded source contexts for a session detail view.
pub fn read_session_source_context(
    conn: &Connection,
    generation_id: &str,
    session_id: &str,
    limit: i64,
) -> Result<Vec<ActivitySourceInfoRow>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT lf.source_id, ls.agent, ls.root_path \
         FROM usage_events ue \
         INNER JOIN log_files lf ON lf.id = ue.source_file_id \
         INNER JOIN log_sources ls ON ls.id = lf.source_id \
         WHERE ue.generation_id = ?1 \
           AND ue.session_id = ?2 \
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(
        params![generation_id, session_id, limit.clamp(1, 20)],
        |row| {
            Ok(ActivitySourceInfoRow {
                source_id: row.get(0)?,
                agent: row.get(1)?,
                root_path: row.get(2)?,
            })
        },
    )?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

// ── Settings ─────────────────────────────────────────────────────────────────

/// Read settings diagnostics: current queue depth, aggregate lag, readiness,
/// and recent subscription/diagnostic events for the given generation.
pub fn read_settings_diagnostics(
    conn: &Connection,
    _generation_id: &str,
) -> Result<SettingsDiagnosticsRow> {
    // Read service state for queue/lag metrics.
    let state = read_service_state(conn)?;

    // Read recent diagnostic events (last 20, DESC).
    let mut stmt = conn
        .prepare(
            "SELECT id, severity, \
                COALESCE(code, '') AS code, \
                message, \
                happened_at_ms \
             FROM diagnostic_events \
             ORDER BY happened_at_ms DESC \
             LIMIT 20",
        )
        .context("failed to prepare diagnostics query")?;

    let recent_events: Vec<SettingsDiagnosticEventRow> = stmt
        .query_map([], |row| {
            Ok(SettingsDiagnosticEventRow {
                id: row.get(0)?,
                severity: row.get(1)?,
                code: row.get(2)?,
                message: row.get(3)?,
                happened_at_ms: row.get(4)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(SettingsDiagnosticsRow {
        writer_queue_depth: state.writer_queue_depth,
        aggregate_lag_ms: state.aggregate_lag_ms,
        readiness: state.readiness,
        active_generation_id: state.active_generation_id,
        recent_events,
    })
}

// ── Breakdown ────────────────────────────────────────────────────────────────

/// Read a cursor-paginated breakdown list from materialized dimension tables.
pub fn read_breakdown_list(
    conn: &Connection,
    generation_id: &str,
    dimension: BreakdownDimension,
    start_date: &str,
    end_date: &str,
    limit: i64,
    cursor: Option<String>,
) -> Result<CursorPage<BreakdownGroupRow>> {
    let spec = breakdown_dimension_spec(dimension);
    let page_size = limit.clamp(1, 500);
    let query_limit = page_size + 1;
    let cursor = parse_sort_cursor(cursor.as_deref())?;
    let cursor_clause = if cursor.is_some() {
        "WHERE (sort_value < ?5 OR (sort_value = ?5 AND group_key < ?6))"
    } else {
        ""
    };
    let sql = format!(
        "WITH grouped AS ( \
            SELECT {key_col} AS group_key, \
                   {label_expr} AS label, \
                   NULL AS subtitle, \
                   COALESCE(SUM(total_tokens), 0) AS total_tokens, \
                   SUM(cost_usd) AS total_cost_usd, \
                   COALESCE(SUM(event_count), 0) AS event_count, \
                   MAX(last_active_at_ms) AS last_active_at_ms, \
                   {sort_expr} AS sort_value, \
                   COALESCE(SUM(CASE WHEN cost_status IN ('exact', 'estimated', 'partial') THEN event_count ELSE 0 END), 0) > 0 AS has_cost, \
                   COALESCE(SUM(CASE WHEN cost_status IN ('unavailable', 'unknown', 'partial') THEN event_count ELSE 0 END), 0) > 0 AS has_no_cost \
            FROM {table} \
            WHERE generation_id = ?1 AND date >= ?2 AND date <= ?3 \
              AND {key_col} IS NOT NULL AND {key_col} != '' \
            GROUP BY {key_col} \
         ) \
         SELECT group_key, label, subtitle, total_tokens, total_cost_usd, event_count, \
                last_active_at_ms, has_cost, has_no_cost, sort_value \
         FROM grouped \
         {cursor_clause} \
         ORDER BY sort_value DESC, group_key DESC \
         LIMIT ?4",
        table = spec.table,
        key_col = spec.key_col,
        label_expr = spec.label_expr,
        sort_expr = spec.sort_expr,
        cursor_clause = cursor_clause,
    );

    let mut stmt = conn.prepare(&sql)?;
    let map_row = |row: &rusqlite::Row<'_>| {
        Ok(BreakdownGroupRow {
            group_key: row.get(0)?,
            label: row.get(1)?,
            subtitle: row.get::<_, Option<String>>(2)?,
            total_tokens: row.get(3)?,
            total_cost_usd: row.get(4)?,
            event_count: row.get(5)?,
            last_active_at_ms: row.get(6)?,
            has_cost: row.get::<_, i32>(7)? != 0,
            has_no_cost: row.get::<_, i32>(8)? != 0,
            extra_values: Vec::new(),
        })
    };

    let mut items = Vec::new();
    if let Some(cursor) = cursor {
        let rows = stmt.query_map(
            params![
                generation_id,
                start_date,
                end_date,
                query_limit,
                cursor.sort,
                cursor.key
            ],
            map_row,
        )?;
        for row in rows {
            items.push(row?);
        }
    } else {
        let rows = stmt.query_map(
            params![generation_id, start_date, end_date, query_limit],
            map_row,
        )?;
        for row in rows {
            items.push(row?);
        }
    }

    populate_breakdown_extra_values(
        conn,
        generation_id,
        dimension,
        start_date,
        end_date,
        &mut items,
    )?;

    let next_cursor = if items.len() > page_size as usize {
        items.pop().expect("extra cursor row exists");
        let last = items.last().expect("visible cursor row exists");
        let sort = match dimension {
            BreakdownDimension::Session => last.last_active_at_ms.unwrap_or(0),
            BreakdownDimension::Project | BreakdownDimension::Model => last.total_tokens,
        };
        Some(encode_sort_cursor(sort, &last.group_key)?)
    } else {
        None
    };

    Ok(CursorPage { items, next_cursor })
}

/// Read aggregate totals for a breakdown list response.
pub fn read_breakdown_totals(
    conn: &Connection,
    generation_id: &str,
    dimension: BreakdownDimension,
    start_date: &str,
    end_date: &str,
) -> Result<BreakdownTotalsRow> {
    let spec = breakdown_dimension_spec(dimension);
    let sql = format!(
        "WITH grouped AS ( \
            SELECT {key_col} AS group_key, \
                   COALESCE(SUM(total_tokens), 0) AS total_tokens, \
                   SUM(cost_usd) AS total_cost_usd, \
                   COALESCE(SUM(event_count), 0) AS event_count, \
                   COALESCE(SUM(CASE WHEN cost_status IN ('exact', 'estimated', 'partial') THEN event_count ELSE 0 END), 0) > 0 AS has_cost, \
                   COALESCE(SUM(CASE WHEN cost_status IN ('unavailable', 'unknown', 'partial') THEN event_count ELSE 0 END), 0) > 0 AS has_no_cost \
            FROM {table} \
            WHERE generation_id = ?1 AND date >= ?2 AND date <= ?3 \
              AND {key_col} IS NOT NULL AND {key_col} != '' \
            GROUP BY {key_col} \
         ) \
         SELECT \
            COALESCE(COUNT(*), 0), \
            COALESCE(SUM(total_tokens), 0), \
            SUM(total_cost_usd), \
            COALESCE(SUM(CASE WHEN has_cost THEN 1 ELSE 0 END), 0) > 0, \
            COALESCE(SUM(CASE WHEN has_no_cost THEN 1 ELSE 0 END), 0) > 0 \
         FROM grouped",
        table = spec.table,
        key_col = spec.key_col,
    );
    conn.query_row(&sql, params![generation_id, start_date, end_date], |row| {
        Ok(BreakdownTotalsRow {
            grouped_count: row.get(0)?,
            total_tokens: row.get(1)?,
            total_cost_usd: row.get(2)?,
            has_cost: row.get::<_, i32>(3)? != 0,
            has_no_cost: row.get::<_, i32>(4)? != 0,
        })
    })
    .context("failed to read breakdown totals")
}

/// Read top sessions for a project from materialized session-day rows.
pub fn read_project_top_sessions(
    conn: &Connection,
    generation_id: &str,
    project_hash: &str,
    start_date: &str,
    end_date: &str,
    limit: i64,
) -> Result<Vec<BreakdownGroupRow>> {
    let mut stmt = conn.prepare(
        "SELECT \
            session_id, \
            session_id, \
            COALESCE(SUM(total_tokens), 0), \
            SUM(cost_usd), \
            COALESCE(SUM(event_count), 0), \
            MAX(last_active_at_ms), \
            COALESCE(SUM(CASE WHEN cost_status IN ('exact', 'estimated', 'partial') THEN event_count ELSE 0 END), 0) > 0, \
            COALESCE(SUM(CASE WHEN cost_status IN ('unavailable', 'unknown', 'partial') THEN event_count ELSE 0 END), 0) > 0, \
            MAX(client_kind), \
            MAX(project_path) \
         FROM usage_by_session_day \
         WHERE generation_id = ?1 \
           AND project_hash = ?2 \
           AND date >= ?3 AND date <= ?4 \
           AND session_id IS NOT NULL AND session_id != '' \
         GROUP BY session_id \
         ORDER BY total_tokens DESC, session_id DESC \
         LIMIT ?5",
    )?;
    let rows = stmt.query_map(
        params![
            generation_id,
            project_hash,
            start_date,
            end_date,
            limit.clamp(1, 50)
        ],
        |row| {
            Ok(BreakdownGroupRow {
                group_key: row.get(0)?,
                label: row.get(1)?,
                subtitle: None,
                total_tokens: row.get(2)?,
                total_cost_usd: row.get(3)?,
                event_count: row.get(4)?,
                last_active_at_ms: row.get(5)?,
                has_cost: row.get::<_, i32>(6)? != 0,
                has_no_cost: row.get::<_, i32>(7)? != 0,
                extra_values: vec![row.get(8)?, row.get(9)?],
            })
        },
    )?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

struct BreakdownDimensionSpec {
    table: &'static str,
    key_col: &'static str,
    label_expr: &'static str,
    sort_expr: &'static str,
}

fn breakdown_dimension_spec(dimension: BreakdownDimension) -> BreakdownDimensionSpec {
    match dimension {
        BreakdownDimension::Project => BreakdownDimensionSpec {
            table: "usage_by_project_day",
            key_col: "project_id",
            label_expr: "COALESCE(MAX(project_path), project_id)",
            sort_expr: "COALESCE(SUM(total_tokens), 0)",
        },
        BreakdownDimension::Model => BreakdownDimensionSpec {
            table: "usage_by_model_day",
            key_col: "model",
            label_expr: "model",
            sort_expr: "COALESCE(SUM(total_tokens), 0)",
        },
        BreakdownDimension::Session => BreakdownDimensionSpec {
            table: "usage_by_session_day",
            key_col: "session_id",
            label_expr: "session_id",
            sort_expr: "COALESCE(MAX(last_active_at_ms), 0)",
        },
    }
}

fn populate_breakdown_extra_values(
    conn: &Connection,
    generation_id: &str,
    dimension: BreakdownDimension,
    start_date: &str,
    end_date: &str,
    items: &mut [BreakdownGroupRow],
) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    match dimension {
        BreakdownDimension::Project => populate_project_breakdown_extra_values(
            conn,
            generation_id,
            start_date,
            end_date,
            items,
        ),
        BreakdownDimension::Model => {
            populate_model_breakdown_extra_values(conn, generation_id, start_date, end_date, items)
        }
        BreakdownDimension::Session => populate_session_breakdown_extra_values(
            conn,
            generation_id,
            start_date,
            end_date,
            items,
        ),
    }
}

fn populate_project_breakdown_extra_values(
    conn: &Connection,
    generation_id: &str,
    start_date: &str,
    end_date: &str,
    items: &mut [BreakdownGroupRow],
) -> Result<()> {
    let key_to_index = breakdown_item_index(items);
    let mut item_keys = key_to_index.keys().cloned().collect::<Vec<_>>();
    item_keys.sort();
    let values_sql = breakdown_values_sql(item_keys.len());
    let generation_param = item_keys.len() + 1;
    let start_param = generation_param + 1;
    let end_param = start_param + 1;
    let sql = format!(
        "WITH requested(project_id) AS (VALUES {values_sql}), ranked AS ( \
            SELECT \
                p.project_id, \
                p.model, \
                SUM(p.total_tokens) AS total_tokens, \
                ROW_NUMBER() OVER (PARTITION BY p.project_id ORDER BY SUM(p.total_tokens) DESC, p.model ASC) AS row_num \
            FROM usage_by_project_day p \
            INNER JOIN requested r ON r.project_id = p.project_id \
            WHERE p.generation_id = ?{generation_param} \
              AND p.date >= ?{start_param} AND p.date <= ?{end_param} \
              AND p.model IS NOT NULL AND p.model != '' \
            GROUP BY p.project_id, p.model \
         ) \
         SELECT project_id, model FROM ranked WHERE row_num = 1"
    );
    let mut params_vec = item_keys
        .iter()
        .cloned()
        .map(Value::Text)
        .collect::<Vec<_>>();
    params_vec.push(Value::Text(generation_id.to_string()));
    params_vec.push(Value::Text(start_date.to_string()));
    params_vec.push(Value::Text(end_date.to_string()));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params_vec.iter()), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (project_id, model) = row?;
        if let Some(index) = key_to_index.get(&project_id) {
            items[*index].extra_values = vec![Some(model)];
        }
    }
    Ok(())
}

fn populate_model_breakdown_extra_values(
    conn: &Connection,
    generation_id: &str,
    start_date: &str,
    end_date: &str,
    items: &mut [BreakdownGroupRow],
) -> Result<()> {
    let key_to_index = breakdown_item_index(items);
    let mut item_keys = key_to_index.keys().cloned().collect::<Vec<_>>();
    item_keys.sort();
    let values_sql = breakdown_values_sql(item_keys.len());
    let generation_param = item_keys.len() + 1;
    let start_param = generation_param + 1;
    let end_param = start_param + 1;

    let client_sql = format!(
        "WITH requested(model) AS (VALUES {values_sql}) \
         SELECT m.model, GROUP_CONCAT(DISTINCT m.agent) \
         FROM usage_by_model_day m \
         INNER JOIN requested r ON r.model = m.model \
         WHERE m.generation_id = ?{generation_param} \
           AND m.date >= ?{start_param} AND m.date <= ?{end_param} \
         GROUP BY m.model"
    );
    let mut client_params = item_keys
        .iter()
        .cloned()
        .map(Value::Text)
        .collect::<Vec<_>>();
    client_params.push(Value::Text(generation_id.to_string()));
    client_params.push(Value::Text(start_date.to_string()));
    client_params.push(Value::Text(end_date.to_string()));
    let mut client_stmt = conn.prepare(&client_sql)?;
    let client_rows = client_stmt.query_map(params_from_iter(client_params.iter()), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    for row in client_rows {
        let (model, client_labels) = row?;
        if let Some(index) = key_to_index.get(&model) {
            items[*index].extra_values = vec![client_labels, None];
        }
    }

    let project_sql = format!(
        "WITH requested(model) AS (VALUES {values_sql}), ranked AS ( \
            SELECT \
                p.model, \
                COALESCE(MAX(p.project_path), p.project_id) AS project_label, \
                SUM(p.total_tokens) AS total_tokens, \
                ROW_NUMBER() OVER (PARTITION BY p.model ORDER BY SUM(p.total_tokens) DESC, COALESCE(MAX(p.project_path), p.project_id) ASC) AS row_num \
            FROM usage_by_project_day p \
            INNER JOIN requested r ON r.model = p.model \
            WHERE p.generation_id = ?{generation_param} \
              AND p.date >= ?{start_param} AND p.date <= ?{end_param} \
              AND p.model IS NOT NULL AND p.model != '' \
            GROUP BY p.model, p.project_id \
         ) \
         SELECT model, project_label FROM ranked WHERE row_num = 1"
    );
    let mut project_params = item_keys
        .iter()
        .cloned()
        .map(Value::Text)
        .collect::<Vec<_>>();
    project_params.push(Value::Text(generation_id.to_string()));
    project_params.push(Value::Text(start_date.to_string()));
    project_params.push(Value::Text(end_date.to_string()));
    let mut project_stmt = conn.prepare(&project_sql)?;
    let project_rows = project_stmt.query_map(params_from_iter(project_params.iter()), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    for row in project_rows {
        let (model, project_label) = row?;
        if let Some(index) = key_to_index.get(&model) {
            if items[*index].extra_values.is_empty() {
                items[*index].extra_values = vec![None, project_label];
            } else {
                items[*index].extra_values[1] = project_label;
            }
        }
    }
    Ok(())
}

fn populate_session_breakdown_extra_values(
    conn: &Connection,
    generation_id: &str,
    start_date: &str,
    end_date: &str,
    items: &mut [BreakdownGroupRow],
) -> Result<()> {
    let key_to_index = breakdown_item_index(items);
    let mut item_keys = key_to_index.keys().cloned().collect::<Vec<_>>();
    item_keys.sort();
    let values_sql = breakdown_values_sql(item_keys.len());
    let generation_param = item_keys.len() + 1;
    let start_param = generation_param + 1;
    let end_param = start_param + 1;
    let sql = format!(
        "WITH requested(session_id) AS (VALUES {values_sql}) \
         SELECT \
            s.session_id, \
            MAX(s.client_kind), \
            MAX(s.project_path), \
            MAX(s.project_hash) \
         FROM usage_by_session_day s \
         INNER JOIN requested r ON r.session_id = s.session_id \
         WHERE s.generation_id = ?{generation_param} \
           AND s.date >= ?{start_param} AND s.date <= ?{end_param} \
         GROUP BY s.session_id"
    );
    let mut params_vec = item_keys
        .iter()
        .cloned()
        .map(Value::Text)
        .collect::<Vec<_>>();
    params_vec.push(Value::Text(generation_id.to_string()));
    params_vec.push(Value::Text(start_date.to_string()));
    params_vec.push(Value::Text(end_date.to_string()));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params_vec.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    for row in rows {
        let (session_id, client_kind, project_path, project_hash) = row?;
        if let Some(index) = key_to_index.get(&session_id) {
            items[*index].extra_values = vec![client_kind, project_path, project_hash];
        }
    }
    Ok(())
}

fn breakdown_item_index(items: &[BreakdownGroupRow]) -> HashMap<String, usize> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| (item.group_key.clone(), index))
        .collect()
}

fn breakdown_values_sql(len: usize) -> String {
    std::iter::repeat_n("(?)", len)
        .collect::<Vec<_>>()
        .join(", ")
}

// ── Overview Trend ────────────────────────────────────────────────────────────
///
/// Read monthly trend buckets from `usage_buckets_day` for the last 12 months.
/// Returns one row per (bucket_start_ms, agent, model) aggregated by month.
pub fn read_overview_trend(
    conn: &Connection,
    generation_id: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<OverviewTrendBucketRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT \
                bucket_start_ms, \
                agent, \
                model, \
                COALESCE(SUM(total_tokens), 0), \
                COALESCE(SUM(cost_usd), 0), \
                COALESCE(SUM(event_count), 0) \
             FROM usage_buckets_day \
             WHERE bucket_start_ms >= ?1 AND bucket_start_ms < ?2 \
               AND generation_id = ?3 \
             GROUP BY bucket_start_ms, agent, model \
             ORDER BY bucket_start_ms ASC",
        )
        .context("failed to prepare read_overview_trend query")?;

    let rows = stmt.query_map(params![start_ms, end_ms, generation_id], |row| {
        let tokens: i64 = row.get(3)?;
        let cost: f64 = row.get(4)?;
        let event_count: i64 = row.get(5)?;
        let has_cost = cost > 0.0;
        let has_no_cost = tokens > 0 && !has_cost;
        let cost_usd = if has_cost { Some(cost) } else { None };
        Ok(OverviewTrendBucketRow {
            key: format!(
                "{}-{}-{}",
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ),
            start_ms: row.get(0)?,
            end_ms: row.get::<_, i64>(0)? + 86_400_000, // one day
            tokens,
            cost_usd,
            event_count,
            has_cost,
            has_no_cost,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Read hourly trend rows from `usage_buckets_hour`.
pub fn read_overview_trend_hourly(
    conn: &Connection,
    generation_id: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<OverviewTrendBucketRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT
                bucket_start_ms,
                agent,
                model,
                COALESCE(SUM(total_tokens), 0),
                SUM(cost_usd),
                COALESCE(SUM(event_count), 0),
                COALESCE(SUM(CASE
                    WHEN cost_status IN ('exact', 'estimated', 'partial')
                    THEN event_count ELSE 0 END), 0),
                COALESCE(SUM(CASE
                    WHEN cost_status IN ('unavailable', 'unknown', 'partial')
                    THEN event_count ELSE 0 END), 0)
             FROM usage_buckets_hour
             WHERE bucket_start_ms >= ?1 AND bucket_start_ms < ?2
               AND generation_id = ?3
             GROUP BY bucket_start_ms, agent, model
             ORDER BY bucket_start_ms ASC",
        )
        .context("failed to prepare read_overview_trend_hourly query")?;

    let rows = stmt.query_map(params![start_ms, end_ms, generation_id], |row| {
        let has_cost = row.get::<_, i64>(6)? > 0;
        let has_no_cost = row.get::<_, i64>(7)? > 0;
        Ok(OverviewTrendBucketRow {
            key: format!(
                "{}-{}-{}",
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ),
            start_ms: row.get(0)?,
            end_ms: row.get::<_, i64>(0)? + 3_600_000,
            tokens: row.get(3)?,
            cost_usd: row.get(4)?,
            event_count: row.get(5)?,
            has_cost,
            has_no_cost,
        })
    })?;

    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

const OVERVIEW_EXACT_WINDOW_CHUNK_SIZE: usize = 300;

/// Read exact overview aggregates over explicit time windows from `usage_events`.
pub fn read_overview_window_aggregates_exact(
    conn: &Connection,
    generation_id: &str,
    windows: &[OverviewExactWindow],
) -> Result<Vec<OverviewTrendBucketRow>> {
    if windows.is_empty() {
        return Ok(Vec::new());
    }

    let mut results = Vec::with_capacity(windows.len());

    for chunk in windows.chunks(OVERVIEW_EXACT_WINDOW_CHUNK_SIZE) {
        let values_sql = (0..chunk.len())
            .map(|idx| {
                let base = idx * 3 + 1;
                format!("(?{}, ?{}, ?{})", base, base + 1, base + 2)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let generation_param = chunk.len() * 3 + 1;
        let sql = format!(
            "WITH exact_windows(key, start_ms, end_ms) AS (VALUES {values_sql})
             SELECT
                exact_windows.key,
                exact_windows.start_ms,
                exact_windows.end_ms,
                COALESCE(SUM(usage_events.total_tokens), 0),
                SUM(usage_events.cost_usd),
                COALESCE(COUNT(usage_events.id), 0),
                COALESCE(COUNT(usage_events.cost_usd), 0),
                COALESCE(COUNT(usage_events.id), 0) - COALESCE(COUNT(usage_events.cost_usd), 0)
             FROM exact_windows
             LEFT JOIN usage_events
               ON usage_events.generation_id = ?{generation_param}
              AND usage_events.timestamp_ms >= exact_windows.start_ms
              AND usage_events.timestamp_ms < exact_windows.end_ms
             GROUP BY exact_windows.key, exact_windows.start_ms, exact_windows.end_ms
             ORDER BY exact_windows.start_ms ASC"
        );

        let mut query_params = Vec::with_capacity(chunk.len() * 3 + 1);
        for window in chunk {
            query_params.push(Value::Text(window.key.clone()));
            query_params.push(Value::Integer(window.start_ms));
            query_params.push(Value::Integer(window.end_ms));
        }
        query_params.push(Value::Text(generation_id.to_string()));

        let mut stmt = conn
            .prepare(&sql)
            .context("failed to prepare read_overview_window_aggregates_exact query")?;
        let rows = stmt.query_map(params_from_iter(query_params.iter()), |row| {
            let has_cost = row.get::<_, i64>(6)? > 0;
            let has_no_cost = row.get::<_, i64>(7)? > 0;
            let raw_cost: Option<f64> = row.get(4)?;
            let cost_usd = match (raw_cost, has_cost) {
                (Some(cost), true) if cost > 0.0 => Some(cost),
                (Some(_), true) => Some(0.0),
                _ => None,
            };

            Ok(OverviewTrendBucketRow {
                key: row.get(0)?,
                start_ms: row.get(1)?,
                end_ms: row.get(2)?,
                tokens: row.get(3)?,
                cost_usd,
                event_count: row.get(5)?,
                has_cost,
                has_no_cost,
            })
        })?;

        for row in rows {
            results.push(row?);
        }
    }

    Ok(results)
}

/// Read exact breakdown trend aggregates over explicit windows with a safe field filter.
pub fn read_breakdown_window_aggregates_exact(
    conn: &Connection,
    generation_id: &str,
    filter: BreakdownFilterField,
    filter_value: &str,
    windows: &[OverviewExactWindow],
) -> Result<Vec<OverviewTrendBucketRow>> {
    if windows.is_empty() {
        return Ok(Vec::new());
    }

    let field = breakdown_filter_column(filter);
    let mut results = Vec::with_capacity(windows.len());

    for chunk in windows.chunks(OVERVIEW_EXACT_WINDOW_CHUNK_SIZE) {
        let values_sql = (0..chunk.len())
            .map(|idx| {
                let base = idx * 3 + 1;
                format!("(?{}, ?{}, ?{})", base, base + 1, base + 2)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let generation_param = chunk.len() * 3 + 1;
        let filter_param = generation_param + 1;
        let sql = format!(
            "WITH exact_windows(key, start_ms, end_ms) AS (VALUES {values_sql})
             SELECT
                exact_windows.key,
                exact_windows.start_ms,
                exact_windows.end_ms,
                COALESCE(SUM(usage_events.total_tokens), 0),
                SUM(usage_events.cost_usd),
                COALESCE(COUNT(usage_events.id), 0),
                COALESCE(COUNT(usage_events.cost_usd), 0),
                COALESCE(COUNT(usage_events.id), 0) - COALESCE(COUNT(usage_events.cost_usd), 0)
             FROM exact_windows
             LEFT JOIN usage_events
               ON usage_events.generation_id = ?{generation_param}
              AND usage_events.{field} = ?{filter_param}
              AND usage_events.timestamp_ms >= exact_windows.start_ms
              AND usage_events.timestamp_ms < exact_windows.end_ms
             GROUP BY exact_windows.key, exact_windows.start_ms, exact_windows.end_ms
             ORDER BY exact_windows.start_ms ASC"
        );

        let mut query_params = Vec::with_capacity(chunk.len() * 3 + 2);
        for window in chunk {
            query_params.push(Value::Text(window.key.clone()));
            query_params.push(Value::Integer(window.start_ms));
            query_params.push(Value::Integer(window.end_ms));
        }
        query_params.push(Value::Text(generation_id.to_string()));
        query_params.push(Value::Text(filter_value.to_string()));

        let mut stmt = conn
            .prepare(&sql)
            .context("failed to prepare read_breakdown_window_aggregates_exact query")?;
        let rows = stmt.query_map(params_from_iter(query_params.iter()), |row| {
            let has_cost = row.get::<_, i64>(6)? > 0;
            let has_no_cost = row.get::<_, i64>(7)? > 0;
            let raw_cost: Option<f64> = row.get(4)?;
            let cost_usd = match (raw_cost, has_cost) {
                (Some(cost), true) if cost > 0.0 => Some(cost),
                (Some(_), true) => Some(0.0),
                _ => None,
            };

            Ok(OverviewTrendBucketRow {
                key: row.get(0)?,
                start_ms: row.get(1)?,
                end_ms: row.get(2)?,
                tokens: row.get(3)?,
                cost_usd,
                event_count: row.get(5)?,
                has_cost,
                has_no_cost,
            })
        })?;

        for row in rows {
            results.push(row?);
        }
    }

    Ok(results)
}

// ── Overview Heatmap ──────────────────────────────────────────────────────────

/// Read daily aggregate heatmap data from `usage_buckets_day`.
/// Returns one row per date with total tokens, cost, and event count.
pub fn read_overview_heatmap(
    conn: &Connection,
    generation_id: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<OverviewHeatmapDayRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT \
                bucket_start_ms, \
                COALESCE(SUM(total_tokens), 0), \
                COALESCE(SUM(cost_usd), 0), \
                COALESCE(SUM(event_count), 0) \
             FROM usage_buckets_day \
             WHERE bucket_start_ms >= ?1 AND bucket_start_ms < ?2 \
               AND generation_id = ?3 \
             GROUP BY bucket_start_ms \
             ORDER BY bucket_start_ms ASC",
        )
        .context("failed to prepare read_overview_heatmap query")?;

    let mut rows = stmt.query(params![start_ms, end_ms, generation_id])?;
    let mut results = Vec::new();
    while let Some(row) = rows.next()? {
        let bucket_start_ms: i64 = row.get(0)?;
        let tokens: i64 = row.get(1)?;
        let cost: f64 = row.get(2)?;
        let event_count: i64 = row.get(3)?;
        let has_cost = cost > 0.0;
        let has_no_cost = tokens > 0 && !has_cost;
        results.push(OverviewHeatmapDayRow {
            date: utc_date_from_epoch_ms(bucket_start_ms)?,
            tokens,
            cost_usd: if has_cost { Some(cost) } else { None },
            event_count,
            has_cost,
            has_no_cost,
        });
    }
    Ok(results)
}

// ── Overview Rankings ─────────────────────────────────────────────────────────

/// Read top-ranked models by total token usage within a time range.
pub fn read_overview_rankings_models(
    conn: &Connection,
    generation_id: &str,
    start_ms: i64,
    end_ms: i64,
    limit: i64,
) -> Result<Vec<RankingRow>> {
    read_rankings_exact_range(
        conn,
        RankingsDimensionSpec {
            table: "usage_by_model_day",
            table_key_col: "model",
            table_label_expr: "model",
            raw_key_col: "model",
            raw_label_expr: "MAX(model)",
            order_by: "total_tokens DESC, group_key DESC",
        },
        generation_id,
        start_ms,
        end_ms,
        limit,
    )
}

/// Read top-ranked models by total cost within a time range.
pub fn read_overview_rankings_models_by_cost(
    conn: &Connection,
    generation_id: &str,
    start_ms: i64,
    end_ms: i64,
    limit: i64,
) -> Result<Vec<RankingRow>> {
    read_rankings_exact_range(
        conn,
        RankingsDimensionSpec {
            table: "usage_by_model_day",
            table_key_col: "model",
            table_label_expr: "model",
            raw_key_col: "model",
            raw_label_expr: "MAX(model)",
            order_by: "total_cost_usd DESC, group_key DESC",
        },
        generation_id,
        start_ms,
        end_ms,
        limit,
    )
}

/// Read top-ranked projects by total token usage within a time range.
pub fn read_overview_rankings_projects(
    conn: &Connection,
    generation_id: &str,
    start_ms: i64,
    end_ms: i64,
    limit: i64,
) -> Result<Vec<RankingRow>> {
    read_rankings_exact_range(
        conn,
        RankingsDimensionSpec {
            table: "usage_by_project_day",
            table_key_col: "project_id",
            table_label_expr: "COALESCE(MAX(project_path), project_id)",
            raw_key_col: "project_hash",
            raw_label_expr: "MAX(project_path)",
            order_by: "total_tokens DESC, group_key DESC",
        },
        generation_id,
        start_ms,
        end_ms,
        limit,
    )
}

/// Read top-ranked sessions by total token usage within a time range.
pub fn read_overview_rankings_sessions(
    conn: &Connection,
    generation_id: &str,
    start_ms: i64,
    end_ms: i64,
    limit: i64,
) -> Result<Vec<RankingRow>> {
    read_rankings_exact_range(
        conn,
        RankingsDimensionSpec {
            table: "usage_by_session_day",
            table_key_col: "session_id",
            table_label_expr: "session_id",
            raw_key_col: "session_id",
            raw_label_expr: "MAX(session_id)",
            order_by: "total_tokens DESC, group_key DESC",
        },
        generation_id,
        start_ms,
        end_ms,
        limit,
    )
}

struct RankingsDimensionSpec {
    table: &'static str,
    table_key_col: &'static str,
    table_label_expr: &'static str,
    raw_key_col: &'static str,
    raw_label_expr: &'static str,
    order_by: &'static str,
}

fn read_rankings_exact_range(
    conn: &Connection,
    spec: RankingsDimensionSpec,
    generation_id: &str,
    start_ms: i64,
    end_ms: i64,
    limit: i64,
) -> Result<Vec<RankingRow>> {
    let segments = exact_range_segments(start_ms, end_ms)?;
    let sql = format!(
        "WITH materialized AS ( \
            SELECT {table_key_col} AS group_key, \
                   {table_label_expr} AS label, \
                   COALESCE(SUM(total_tokens), 0) AS total_tokens, \
                   SUM(cost_usd) AS total_cost_usd, \
                   COALESCE(SUM(event_count), 0) AS event_count, \
                   COALESCE(SUM(CASE WHEN cost_status IN ('exact', 'estimated', 'partial') THEN event_count ELSE 0 END), 0) > 0 AS has_cost, \
                   COALESCE(SUM(CASE WHEN cost_status IN ('unavailable', 'unknown', 'partial') THEN event_count ELSE 0 END), 0) > 0 AS has_no_cost \
            FROM {table} \
            WHERE generation_id = ?1 AND date >= ?2 AND date < ?3 \
              AND {table_key_col} IS NOT NULL AND {table_key_col} != '' \
            GROUP BY {table_key_col} \
         ), raw_prefix AS ( \
            SELECT COALESCE({raw_key_col}, '') AS group_key, \
                   {raw_label_expr} AS label, \
                   COALESCE(SUM(total_tokens), 0) AS total_tokens, \
                   SUM(cost_usd) AS total_cost_usd, \
                   COUNT(*) AS event_count, \
                   COALESCE(SUM(CASE WHEN cost_usd IS NOT NULL THEN 1 ELSE 0 END), 0) > 0 AS has_cost, \
                   COALESCE(SUM(CASE WHEN cost_usd IS NULL THEN 1 ELSE 0 END), 0) > 0 AS has_no_cost \
            FROM usage_events \
            WHERE generation_id = ?1 \
              AND timestamp_ms >= ?4 AND timestamp_ms < ?5 \
              AND COALESCE({raw_key_col}, '') != '' \
            GROUP BY COALESCE({raw_key_col}, '') \
         ), raw_suffix AS ( \
            SELECT COALESCE({raw_key_col}, '') AS group_key, \
                   {raw_label_expr} AS label, \
                   COALESCE(SUM(total_tokens), 0) AS total_tokens, \
                   SUM(cost_usd) AS total_cost_usd, \
                   COUNT(*) AS event_count, \
                   COALESCE(SUM(CASE WHEN cost_usd IS NOT NULL THEN 1 ELSE 0 END), 0) > 0 AS has_cost, \
                   COALESCE(SUM(CASE WHEN cost_usd IS NULL THEN 1 ELSE 0 END), 0) > 0 AS has_no_cost \
            FROM usage_events \
            WHERE generation_id = ?1 \
              AND timestamp_ms >= ?6 AND timestamp_ms < ?7 \
              AND COALESCE({raw_key_col}, '') != '' \
            GROUP BY COALESCE({raw_key_col}, '') \
         ) \
         SELECT group_key, \
                COALESCE(MAX(label), group_key) AS label, \
                COALESCE(SUM(total_tokens), 0) AS total_tokens, \
                SUM(total_cost_usd) AS total_cost_usd, \
                COALESCE(SUM(event_count), 0) AS event_count, \
                COALESCE(SUM(CASE WHEN has_cost THEN 1 ELSE 0 END), 0) > 0, \
                COALESCE(SUM(CASE WHEN has_no_cost THEN 1 ELSE 0 END), 0) > 0 \
         FROM ( \
            SELECT * FROM materialized \
            UNION ALL \
            SELECT * FROM raw_prefix \
            UNION ALL \
            SELECT * FROM raw_suffix \
         ) ranked \
         WHERE group_key != '' \
         GROUP BY group_key \
         ORDER BY {order_by} \
         LIMIT ?8",
        table = spec.table,
        table_key_col = spec.table_key_col,
        table_label_expr = spec.table_label_expr,
        raw_key_col = spec.raw_key_col,
        raw_label_expr = spec.raw_label_expr,
        order_by = spec.order_by,
    );
    let mut stmt = conn
        .prepare(&sql)
        .with_context(|| format!("failed to prepare rankings query from {}", spec.table))?;
    let rows = stmt.query_map(
        params![
            generation_id,
            segments.day_start_date,
            segments.day_end_date,
            start_ms,
            segments.raw_prefix_end_ms,
            segments.raw_suffix_start_ms,
            end_ms,
            limit.clamp(1, 50),
        ],
        |row| {
            Ok(RankingRow {
                group_key: row.get(0)?,
                label: row.get(1)?,
                total_tokens: row.get(2)?,
                total_cost_usd: row.get(3)?,
                event_count: row.get(4)?,
                has_cost: row.get::<_, i32>(5)? != 0,
                has_no_cost: row.get::<_, i32>(6)? != 0,
            })
        },
    )?;

    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

// ── Activity Recent ───────────────────────────────────────────────────────────

/// Read recent activity events from usage_events, ordered by timestamp descending.
pub fn read_activity_recent(
    conn: &Connection,
    _generation_id: &str,
    start_ms: i64,
    end_ms: i64,
    limit: u32,
) -> Result<Vec<ActivityListRow>> {
    read_activity_list(conn, _generation_id, start_ms, end_ms, limit as i64, None)
}

// ── Daily usage read queries (IANA timezone path) ───────────────────────────
//
// `daily_usage` is generation-scoped — its PRIMARY KEY includes generation_id
// and all reads filter by the active generation. This matches the contract of
// every other read-plane materialized table (usage_buckets_*, usage_by_*_day,
// source_health_summary). IANA overview paths now agree with the fast path on
// generation boundaries.

/// Read trend data from the materialized `daily_usage` table for IANA timezones.
///
/// Filters by timezone, date range, and generation_id. This is the primary
/// read path for IANA timezones on week/month/year ranges.
pub fn read_overview_trend_from_daily_usage(
    conn: &Connection,
    timezone: &str,
    start_date: &str,
    end_date: &str,
    generation_id: &str,
) -> Result<Vec<DailyUsageTrendRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT
                date,
                COALESCE(SUM(total_tokens), 0),
                SUM(cost_usd),
                COALESCE(SUM(event_count), 0),
                COALESCE(SUM(CASE WHEN cost_usd IS NOT NULL THEN event_count ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN cost_usd IS NULL THEN event_count ELSE 0 END), 0)
             FROM daily_usage
             WHERE timezone = ?1
               AND date >= ?2 AND date <= ?3
               AND generation_id = ?4
             GROUP BY date
             ORDER BY date ASC",
        )
        .context("failed to prepare read_overview_trend_from_daily_usage query")?;

    let rows = stmt.query_map(
        params![timezone, start_date, end_date, generation_id],
        |row| {
            let has_cost = row.get::<_, i64>(4)? > 0;
            let has_no_cost = row.get::<_, i64>(5)? > 0;
            let raw_cost: Option<f64> = row.get(2)?;
            let cost_usd = match (raw_cost, has_cost) {
                (Some(cost), true) if cost > 0.0 => Some(cost),
                _ => None,
            };
            Ok(DailyUsageTrendRow {
                date: row.get(0)?,
                tokens: row.get(1)?,
                cost_usd,
                event_count: row.get(3)?,
                has_cost,
                has_no_cost,
            })
        },
    )?;

    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// Read summary totals from the materialized `daily_usage` table.
pub fn read_overview_summary_from_daily_usage(
    conn: &Connection,
    timezone: &str,
    start_date: &str,
    end_date: &str,
    generation_id: &str,
) -> Result<OverviewSummaryRow> {
    conn.query_row(
        "SELECT
            COALESCE(SUM(total_tokens), 0),
            SUM(cost_usd),
            COALESCE(SUM(event_count), 0),
            COALESCE(SUM(CASE WHEN cost_usd IS NOT NULL THEN event_count ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN cost_usd IS NULL THEN event_count ELSE 0 END), 0)
         FROM daily_usage
         WHERE timezone = ?1
           AND date >= ?2 AND date <= ?3
           AND generation_id = ?4",
        params![timezone, start_date, end_date, generation_id],
        |row| {
            let with_cost: i64 = row.get(3)?;
            let without_cost: i64 = row.get(4)?;
            Ok(OverviewSummaryRow {
                total_tokens: row.get(0)?,
                total_cost_usd: row.get(1)?,
                event_count: row.get(2)?,
                has_cost: with_cost > 0,
                has_no_cost: without_cost > 0,
            })
        },
    )
    .context("failed to read overview summary from daily_usage")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use busytok_domain::now_ms;

    fn seeded_db_with_service_state() -> Database {
        let db = Database::open_in_memory().unwrap();
        let now_ms = now_ms();
        // Seed service_state singleton with known values
        db.conn()
            .execute(
                "INSERT OR REPLACE INTO service_state \
                 (id, writer_queue_depth, aggregate_lag_ms, readiness, \
                  active_generation_id, last_exact_rebuild_at_ms, updated_at_ms) \
                 VALUES (1, 0, 0, 'starting', NULL, NULL, ?1)",
                params![now_ms],
            )
            .unwrap();
        // Seed event_sequence_state with latest_event_seq = 42
        db.conn()
            .execute(
                "INSERT OR REPLACE INTO event_sequence_state \
                 (id, latest_event_seq, latest_event_timestamp_ms, updated_at_ms) \
                 VALUES (1, 42, NULL, ?1)",
                params![now_ms],
            )
            .unwrap();
        db
    }

    fn seeded_db_with_active_generation() -> Database {
        let db = Database::open_in_memory().unwrap();
        let now_ms = now_ms();
        // Use timestamps that fall within day_range (now - 24h, now).
        let t1 = now_ms - 3_600_000; // 1 hour ago
        let t2 = now_ms - 2_400_000; // 40 min ago
        let t3 = now_ms - 600_000; // 10 min ago
                                   // Seed service_state
        db.conn()
            .execute(
                "INSERT OR REPLACE INTO service_state \
                 (id, writer_queue_depth, aggregate_lag_ms, readiness, \
                  active_generation_id, last_exact_rebuild_at_ms, updated_at_ms) \
                 VALUES (1, 0, 0, 'ready_exact', 'gen-1', NULL, ?1)",
                params![now_ms],
            )
            .unwrap();
        // Seed event_sequence_state
        db.conn()
            .execute(
                "INSERT OR REPLACE INTO event_sequence_state \
                 (id, latest_event_seq, latest_event_timestamp_ms, updated_at_ms) \
                 VALUES (1, 42, NULL, ?1)",
                params![now_ms],
            )
            .unwrap();
        // Seed usage events with known tokens for gen-1
        let events = [
            ("evt-1", t1, 400i64, Some(0.05f64)),
            ("evt-2", t2, 500i64, Some(0.06f64)),
            ("evt-3", t3, 334i64, None::<f64>),
        ];
        for (id, ts, tokens, cost) in &events {
            db.conn()
                .execute(
                    "INSERT INTO usage_events \
                     (id, agent, source_file_id, source_path, source_line, \
                      source_offset_start, source_offset_end, session_id, \
                      timestamp_ms, model, total_tokens, cost_usd, cost_source, \
                      raw_event_hash, is_error, generation_id, dedupe_key, created_at_ms, updated_at_ms) \
                     VALUES (?1, 'claude_code', 'f1', '/tmp/test.jsonl', 0, \
                             0, 0, 's1', ?2, 'claude-sonnet', ?3, ?4, 'unknown', \
                             '', 0, 'gen-1', ?1, ?5, ?5)",
                    params![id, ts, tokens, cost, now_ms],
                )
                .unwrap();
        }
        for (id, ts, tokens, cost) in &events {
            let bucket_hour = (*ts / 3_600_000) * 3_600_000;
            db.conn()
                .execute(
                    "INSERT INTO usage_buckets_hour \
                     (generation_id, bucket_start_ms, agent, model, total_tokens, cost_usd, \
                      cost_status, event_count, created_at_ms, updated_at_ms) \
                     VALUES ('gen-1', ?1, 'claude_code', 'claude-sonnet', ?2, ?3, \
                             CASE WHEN ?3 IS NULL THEN 'unavailable' ELSE 'exact' END, 1, ?4, ?4) \
                     ON CONFLICT(generation_id, bucket_start_ms, agent, model) DO UPDATE SET \
                       total_tokens = total_tokens + excluded.total_tokens, \
                       cost_usd = CASE WHEN cost_usd IS NOT NULL AND excluded.cost_usd IS NOT NULL \
                                       THEN cost_usd + excluded.cost_usd \
                                       WHEN cost_usd IS NOT NULL THEN cost_usd \
                                       ELSE excluded.cost_usd END, \
                       cost_status = 'partial', \
                       event_count = event_count + excluded.event_count",
                    params![bucket_hour, tokens, cost, now_ms],
                )
                .unwrap();
            db.conn()
                .execute(
                    "INSERT INTO usage_by_model_day \
                     (generation_id, date, agent, model, total_tokens, cost_usd, \
                      event_count, last_active_at_ms, created_at_ms, updated_at_ms) \
                     VALUES ('gen-1', ?1, 'claude_code', 'claude-sonnet', ?2, ?3, 1, ?4, ?5, ?5) \
                     ON CONFLICT(generation_id, date, agent, model) DO UPDATE SET \
                       total_tokens = total_tokens + excluded.total_tokens, \
                       cost_usd = CASE WHEN cost_usd IS NOT NULL AND excluded.cost_usd IS NOT NULL \
                                       THEN cost_usd + excluded.cost_usd \
                                       WHEN cost_usd IS NOT NULL THEN cost_usd \
                                       ELSE excluded.cost_usd END, \
                       event_count = event_count + excluded.event_count, \
                       last_active_at_ms = MAX(COALESCE(last_active_at_ms, 0), excluded.last_active_at_ms)",
                    params![utc_date_from_epoch_ms(*ts).unwrap(), tokens, cost, ts, now_ms],
                )
                .unwrap();
            let _ = id;
        }
        db
    }

    fn seeded_db_with_diagnostics() -> Database {
        let db = Database::open_in_memory().unwrap();
        let now_ms = now_ms();
        // Seed service_state with known queue depth
        db.conn()
            .execute(
                "INSERT OR REPLACE INTO service_state \
                 (id, writer_queue_depth, aggregate_lag_ms, readiness, \
                  active_generation_id, last_exact_rebuild_at_ms, updated_at_ms) \
                 VALUES (1, 3, 1500, 'ready_exact', 'gen-1', NULL, ?1)",
                params![now_ms],
            )
            .unwrap();
        // Seed event_sequence_state
        db.conn()
            .execute(
                "INSERT OR REPLACE INTO event_sequence_state \
                 (id, latest_event_seq, latest_event_timestamp_ms, updated_at_ms) \
                 VALUES (1, 10, NULL, ?1)",
                params![now_ms],
            )
            .unwrap();
        // Seed diagnostic events including subscription_disconnected
        for (i, (code, sev)) in [
            ("subscription_connected", "info"),
            ("subscription_disconnected", "info"),
            ("writer_batch_committed", "info"),
        ]
        .iter()
        .enumerate()
        {
            db.conn()
                .execute(
                    "INSERT INTO diagnostic_events \
                     (id, severity, code, message, happened_at_ms, created_at_ms) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                    params![
                        format!("diag-{}", i),
                        sev,
                        code,
                        format!("event {}", code),
                        now_ms - (3000 - i as i64 * 1000),
                    ],
                )
                .unwrap();
        }
        db
    }

    fn day_range() -> RangeWindow {
        let now = now_ms();
        RangeWindow::new(now - 86_400_000, now)
    }

    #[test]
    fn read_service_state_returns_latest_event_sequence() {
        let db = seeded_db_with_service_state();
        let state = read_service_state(db.conn()).unwrap();
        assert_eq!(state.latest_event_seq, Some(42));
    }

    #[test]
    fn read_service_state_returns_defaults_when_no_rows() {
        let db = Database::open_in_memory().unwrap();
        // No service_state or event_sequence_state rows exist — fresh install
        let state = read_service_state(db.conn()).unwrap();
        assert_eq!(state.readiness.as_deref(), Some("starting"));
        assert_eq!(state.active_generation_id, None);
        assert_eq!(state.latest_event_seq, None);
        assert_eq!(state.writer_queue_depth, 0);
        assert_eq!(state.aggregate_lag_ms, 0);
    }

    #[test]
    fn read_overview_summary_returns_zero_for_empty_range() {
        let db = Database::open_in_memory().unwrap();
        let range = RangeWindow::new(0, 1000);
        let summary = read_overview_summary(db.conn(), "gen-1", &range).unwrap();
        assert_eq!(summary.total_tokens, 0);
        assert_eq!(summary.event_count, 0);
        assert_eq!(summary.total_cost_usd, None);
        assert!(!summary.has_cost);
        assert!(!summary.has_no_cost);
    }

    #[test]
    fn overview_summary_reads_materialized_totals_for_active_generation() {
        let db = seeded_db_with_active_generation();
        let dto = read_overview_summary(db.conn(), "gen-1", &day_range()).unwrap();
        // 400 + 500 + 334 = 1234
        assert_eq!(dto.total_tokens, 1234);
        assert_eq!(dto.event_count, 3);
        assert!(dto.has_cost);
        assert!(dto.has_no_cost);
    }

    #[test]
    fn settings_diagnostics_reads_queue_lag_and_subscription_events() {
        let db = seeded_db_with_diagnostics();
        let dto = read_settings_diagnostics(db.conn(), "gen-1").unwrap();
        assert_eq!(dto.writer_queue_depth, 3);
        assert_eq!(dto.aggregate_lag_ms, 1500);
        assert!(dto
            .recent_events
            .iter()
            .any(|row| row.code == "subscription_disconnected"));
    }

    #[test]
    fn read_live_window_exact_returns_empty_for_no_data() {
        let db = Database::open_in_memory().unwrap();
        let rows = read_live_window_exact(db.conn(), "gen-1", 0, 10000).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn read_activity_list_returns_empty_for_no_data() {
        let db = Database::open_in_memory().unwrap();
        let rows = read_activity_list(db.conn(), "gen-1", 0, 10000, 10, None).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn read_activity_detail_returns_full_event() {
        let db = seeded_db_with_active_generation();
        let detail = read_activity_detail(db.conn(), "evt-1", "gen-1").unwrap();
        assert_eq!(detail.id, "evt-1");
        assert_eq!(detail.total_tokens, 400);
        assert_eq!(detail.agent, "claude_code");
    }

    #[test]
    fn read_breakdown_list_groups_by_model() {
        let db = seeded_db_with_active_generation();
        let rows = read_breakdown_list(
            db.conn(),
            "gen-1",
            BreakdownDimension::Model,
            &utc_date_from_epoch_ms(day_range().start_ms).unwrap(),
            &utc_date_from_epoch_ms(day_range().end_ms + MILLIS_PER_DAY).unwrap(),
            10,
            None,
        )
        .unwrap();
        // All 3 events share "claude-sonnet"
        assert_eq!(rows.items.len(), 1);
        assert_eq!(rows.items[0].group_key, "claude-sonnet");
        assert_eq!(rows.items[0].total_tokens, 1234);
    }

    // ── daily_usage read path tests ────────────────────────────────────────

    #[test]
    fn read_overview_trend_from_daily_usage_returns_aggregated_rows() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        // Seed daily_usage with two days of data for Asia/Shanghai
        conn.execute(
            "INSERT INTO daily_usage (date, timezone, agent, project_hash, model, \
             input_tokens, output_tokens, total_tokens, cached_input_tokens, \
             cache_creation_tokens, cache_read_tokens, reasoning_tokens, \
             thoughts_tokens, tool_tokens, cost_usd, estimated_cost_usd, event_count, generation_id) \
             VALUES ('2026-06-10', 'Asia/Shanghai', 'claude_code', '', 'claude-sonnet', \
             100, 200, 300, 0, 0, 0, 0, 0, 0, 0.05, NULL, 1, 'gen-test')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO daily_usage (date, timezone, agent, project_hash, model, \
             input_tokens, output_tokens, total_tokens, cached_input_tokens, \
             cache_creation_tokens, cache_read_tokens, reasoning_tokens, \
             thoughts_tokens, tool_tokens, cost_usd, estimated_cost_usd, event_count, generation_id) \
             VALUES ('2026-06-11', 'Asia/Shanghai', 'claude_code', '', 'claude-sonnet', \
             150, 250, 400, 0, 0, 0, 0, 0, 0, NULL, NULL, 1, 'gen-test')",
            [],
        ).unwrap();
        // Different timezone should not be included
        conn.execute(
            "INSERT INTO daily_usage (date, timezone, agent, project_hash, model, \
             input_tokens, output_tokens, total_tokens, cached_input_tokens, \
             cache_creation_tokens, cache_read_tokens, reasoning_tokens, \
             thoughts_tokens, tool_tokens, cost_usd, estimated_cost_usd, event_count, generation_id) \
             VALUES ('2026-06-10', 'UTC', 'claude_code', '', 'claude-sonnet', \
             999, 999, 999, 0, 0, 0, 0, 0, 0, NULL, NULL, 1, 'gen-test')",
            [],
        ).unwrap();

        let rows = read_overview_trend_from_daily_usage(
            conn,
            "Asia/Shanghai",
            "2026-06-10",
            "2026-06-11",
            "gen-test",
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].date, "2026-06-10");
        assert_eq!(rows[0].tokens, 300);
        assert_eq!(rows[0].cost_usd, Some(0.05));
        assert_eq!(rows[1].date, "2026-06-11");
        assert_eq!(rows[1].tokens, 400);
        assert_eq!(rows[1].cost_usd, None);
    }

    #[test]
    fn read_overview_summary_from_daily_usage_aggregates_across_dates() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        conn.execute(
            "INSERT INTO daily_usage (date, timezone, agent, project_hash, model, \
             input_tokens, output_tokens, total_tokens, cached_input_tokens, \
             cache_creation_tokens, cache_read_tokens, reasoning_tokens, \
             thoughts_tokens, tool_tokens, cost_usd, estimated_cost_usd, event_count, generation_id) \
             VALUES ('2026-06-10', 'Asia/Shanghai', 'claude_code', '', 'claude-sonnet', \
             100, 200, 300, 0, 0, 0, 0, 0, 0, 0.05, NULL, 1, 'gen-test')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO daily_usage (date, timezone, agent, project_hash, model, \
             input_tokens, output_tokens, total_tokens, cached_input_tokens, \
             cache_creation_tokens, cache_read_tokens, reasoning_tokens, \
             thoughts_tokens, tool_tokens, cost_usd, estimated_cost_usd, event_count, generation_id) \
             VALUES ('2026-06-11', 'Asia/Shanghai', 'claude_code', '', 'claude-sonnet', \
             150, 250, 400, 0, 0, 0, 0, 0, 0, 0.06, NULL, 1, 'gen-test')",
            [],
        ).unwrap();

        let summary = read_overview_summary_from_daily_usage(
            conn,
            "Asia/Shanghai",
            "2026-06-10",
            "2026-06-11",
            "gen-test",
        )
        .unwrap();
        assert_eq!(summary.total_tokens, 700);
        assert_eq!(summary.event_count, 2);
        assert!(summary.has_cost);
        assert!(!summary.has_no_cost);
    }

    /// Proves that `daily_usage` is generation-scoped: events written with
    /// one generation_id are NOT visible in reads scoped to another generation.
    #[test]
    fn daily_usage_generation_isolation() {
        use crate::write_queries::{
            insert_usage_events_batch_returning_inserted, upsert_daily_usage_for_events,
        };
        use busytok_domain::ReportingTimezone;

        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        let rtz = ReportingTimezone::parse("Asia/Shanghai").unwrap();
        let ts = 1_781_200_000_000i64; // 2026-06-12 ~00:00 CST

        let mut evt_a = busytok_domain::NormalizedUsageEvent::minimal_for_test(
            "evt-gen-iso-a",
            busytok_domain::AgentKind::ClaudeCode,
        );
        evt_a.timestamp_ms = ts;
        evt_a.total_tokens = 100;

        let mut evt_b = busytok_domain::NormalizedUsageEvent::minimal_for_test(
            "evt-gen-iso-b",
            busytok_domain::AgentKind::ClaudeCode,
        );
        evt_b.timestamp_ms = ts;
        evt_b.total_tokens = 200;

        // Write evt_a to Gen-A, evt_b to Gen-B.
        let inserted_a =
            insert_usage_events_batch_returning_inserted(conn, &[evt_a], "Gen-A").unwrap();
        upsert_daily_usage_for_events(conn, &inserted_a, &rtz, "Gen-A").unwrap();

        let inserted_b =
            insert_usage_events_batch_returning_inserted(conn, &[evt_b], "Gen-B").unwrap();
        upsert_daily_usage_for_events(conn, &inserted_b, &rtz, "Gen-B").unwrap();

        let date = rtz.local_date_for_timestamp_ms(ts).unwrap();

        // Gen-A read: only sees evt_a (100 tokens).
        let summary_a = read_overview_summary_from_daily_usage(
            conn,
            rtz.canonical_name(),
            &date,
            &date,
            "Gen-A",
        )
        .unwrap();
        assert_eq!(summary_a.total_tokens, 100);
        assert_eq!(summary_a.event_count, 1);

        // Gen-B read: only sees evt_b (200 tokens).
        let summary_b = read_overview_summary_from_daily_usage(
            conn,
            rtz.canonical_name(),
            &date,
            &date,
            "Gen-B",
        )
        .unwrap();
        assert_eq!(summary_b.total_tokens, 200);
        assert_eq!(summary_b.event_count, 1);
    }

    /// Contract: fast-path (generation-scoped) and IANA (daily_usage) overview
    /// reads MUST agree when both read from the same event set under UTC.
    ///
    /// Preconditions for agreement:
    ///   1. Same events exist in both `usage_buckets_hour` and `daily_usage`
    ///   2. Same timezone (UTC, where day boundaries match the bucket boundaries)
    ///   3. Same generation_id scopes the fast-path read
    ///
    /// Divergence condition: when `daily_usage` accumulates events from
    /// generations that are NOT the active generation (e.g. after a promotion
    /// without a rebuild), the IANA path sees more events than the fast path.
    #[test]
    fn overview_fast_path_and_iana_path_agree_on_same_fixture_utc() {
        use crate::read_models::RangeWindow;
        use crate::write_queries::{
            insert_usage_events_batch_returning_inserted,
            update_materialized_aggregates_from_events, upsert_daily_usage_for_events,
        };
        use busytok_domain::ReportingTimezone;

        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        let gen_id = "gen-contract-test";
        let rtz = ReportingTimezone::parse("UTC").unwrap();

        // Two events in the same UTC day (2026-06-12).
        let ts1 = 1_781_222_400_000i64; // 2026-06-12 00:00 UTC
        let ts2 = 1_781_265_600_000i64; // 2026-06-12 12:00 UTC
        let range = RangeWindow::new(ts1, ts1 + 86_400_000); // full day

        let mut evt1 = busytok_domain::NormalizedUsageEvent::minimal_for_test(
            "evt-contract-1",
            busytok_domain::AgentKind::ClaudeCode,
        );
        evt1.timestamp_ms = ts1;
        evt1.total_tokens = 100;
        evt1.input_tokens = 60;
        evt1.output_tokens = 40;
        evt1.cost_usd = Some(0.01);

        let mut evt2 = busytok_domain::NormalizedUsageEvent::minimal_for_test(
            "evt-contract-2",
            busytok_domain::AgentKind::Codex,
        );
        evt2.timestamp_ms = ts2;
        evt2.total_tokens = 200;
        evt2.input_tokens = 100;
        evt2.output_tokens = 100;
        evt2.cost_usd = Some(0.02);

        let inserted = insert_usage_events_batch_returning_inserted(
            conn,
            &[evt1.clone(), evt2.clone()],
            gen_id,
        )
        .unwrap();
        assert_eq!(inserted.len(), 2);

        // Populate both materialized paths from the same event set.
        update_materialized_aggregates_from_events(conn, &inserted, gen_id).unwrap();
        upsert_daily_usage_for_events(conn, &inserted, &rtz, "gen-contract-test").unwrap();

        // Fast-path read (generation-scoped, from usage_buckets_hour).
        let fast = read_overview_summary(conn, gen_id, &range).unwrap();

        // IANA-path read (from daily_usage, filtered by UTC timezone).
        let date = rtz.local_date_for_timestamp_ms(ts1).unwrap();
        let iana = read_overview_summary_from_daily_usage(
            conn,
            rtz.canonical_name(),
            &date,
            &date,
            "gen-contract-test",
        )
        .unwrap();

        // Under UTC, both paths must agree on totals for the same day.
        assert_eq!(
            fast.total_tokens, iana.total_tokens,
            "fast-path and IANA-path must agree on total_tokens for same fixture (UTC)"
        );
        assert_eq!(
            fast.event_count, iana.event_count,
            "fast-path and IANA-path must agree on event_count for same fixture (UTC)"
        );
        // Cost agreement is more nuanced (cost_usd uses SUM, not COALESCE in
        // fast path vs IANA path), so we only check that both agree on
        // has_cost / has_no_cost.
        assert_eq!(fast.has_cost, iana.has_cost);
        assert_eq!(fast.has_no_cost, iana.has_no_cost);

        // — Divergence demonstration: add events to a DIFFERENT generation and
        //   only update daily_usage (simulating cross-generation accumulation).
        let mut evt3 = busytok_domain::NormalizedUsageEvent::minimal_for_test(
            "evt-contract-3",
            busytok_domain::AgentKind::ClaudeCode,
        );
        evt3.timestamp_ms = ts1 + 3_600_000; // same UTC day, different gen
        evt3.total_tokens = 50;
        evt3.cost_usd = Some(0.005);

        let inserted_gen2 =
            insert_usage_events_batch_returning_inserted(conn, &[evt3], "gen-other").unwrap();
        assert_eq!(inserted_gen2.len(), 1);

        // Only update daily_usage (not usage_buckets_hour for gen-contract-test).
        upsert_daily_usage_for_events(conn, &inserted_gen2, &rtz, "gen-other").unwrap();

        // Fast path (gen-contract-test scope) still sees 2 events.
        let fast_after = read_overview_summary(conn, gen_id, &range).unwrap();
        assert_eq!(fast_after.total_tokens, 300); // unchanged

        // IANA path (global daily_usage) sees 3 events — the cross-generation
        // event is included because daily_usage is NOT generation-scoped.
        let iana_after = read_overview_summary_from_daily_usage(
            conn,
            rtz.canonical_name(),
            &date,
            &date,
            "gen-contract-test",
        )
        .unwrap();
        assert_eq!(iana_after.total_tokens, 300); // 100+200 (cross-gen evt excluded)
        assert_eq!(iana_after.event_count, 2);

        // Generation isolation: cross-generation events inserted with
        // gen-other are NOT visible in gen-contract-test-scoped reads.
        // daily_usage is now generation-scoped, matching usage_buckets_*.
    }

    /// Contract: fast-path trend and IANA daily_usage trend must agree on the
    /// same event set under UTC. Same preconditions and divergence conditions
    /// as the summary contract test above.
    #[test]
    fn overview_trend_fast_path_and_iana_path_agree_on_same_fixture_utc() {
        use crate::write_queries::{
            insert_usage_events_batch_returning_inserted,
            update_materialized_aggregates_from_events, upsert_daily_usage_for_events,
        };
        use busytok_domain::ReportingTimezone;

        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        let gen_id = "gen-trend-contract";
        let rtz = ReportingTimezone::parse("UTC").unwrap();

        // Two events in the same UTC day (2026-06-12).
        let ts1 = 1_781_222_400_000i64;
        let ts2 = 1_781_265_600_000i64;

        let mut evt1 = busytok_domain::NormalizedUsageEvent::minimal_for_test(
            "evt-trend-1",
            busytok_domain::AgentKind::ClaudeCode,
        );
        evt1.timestamp_ms = ts1;
        evt1.total_tokens = 150;
        evt1.cost_usd = Some(0.015);

        let mut evt2 = busytok_domain::NormalizedUsageEvent::minimal_for_test(
            "evt-trend-2",
            busytok_domain::AgentKind::Codex,
        );
        evt2.timestamp_ms = ts2;
        evt2.total_tokens = 250;
        evt2.cost_usd = Some(0.025);

        let inserted =
            insert_usage_events_batch_returning_inserted(conn, &[evt1, evt2], gen_id).unwrap();
        assert_eq!(inserted.len(), 2);

        update_materialized_aggregates_from_events(conn, &inserted, gen_id).unwrap();
        upsert_daily_usage_for_events(conn, &inserted, &rtz, gen_id).unwrap();

        // Fast-path trend: sum across all hourly buckets.
        let fast = read_overview_trend_hourly(conn, gen_id, ts1, ts1 + 86_400_000).unwrap();
        let fast_tokens: i64 = fast.iter().map(|r| r.tokens).sum();
        let fast_count: i64 = fast.iter().map(|r| r.event_count).sum();
        assert_eq!(fast_tokens, 400);
        assert_eq!(fast_count, 2);

        // IANA trend: sum across daily rows.
        let date = rtz.local_date_for_timestamp_ms(ts1).unwrap();
        let iana = read_overview_trend_from_daily_usage(
            conn,
            rtz.canonical_name(),
            &date,
            &date,
            "gen-trend-contract",
        )
        .unwrap();
        let iana_tokens: i64 = iana.iter().map(|r| r.tokens).sum();
        let iana_count: i64 = iana.iter().map(|r| r.event_count).sum();
        assert_eq!(iana_tokens, 400);
        assert_eq!(iana_count, 2);
        assert_eq!(
            fast_tokens, iana_tokens,
            "trend fast-path and IANA-path must agree on tokens for same fixture (UTC)"
        );

        // Divergence: add cross-generation event to daily_usage only.
        let mut evt3 = busytok_domain::NormalizedUsageEvent::minimal_for_test(
            "evt-trend-3",
            busytok_domain::AgentKind::ClaudeCode,
        );
        evt3.timestamp_ms = ts1 + 3_600_000;
        evt3.total_tokens = 50;
        evt3.cost_usd = Some(0.005);
        let inserted_gen2 =
            insert_usage_events_batch_returning_inserted(conn, &[evt3], "gen-other").unwrap();
        upsert_daily_usage_for_events(conn, &inserted_gen2, &rtz, "gen-other").unwrap();

        let fast2 = read_overview_trend_hourly(conn, gen_id, ts1, ts1 + 86_400_000).unwrap();
        let fast_tokens2: i64 = fast2.iter().map(|r| r.tokens).sum();
        assert_eq!(fast_tokens2, 400); // unchanged, gen-scoped

        let iana2 = read_overview_trend_from_daily_usage(
            conn,
            rtz.canonical_name(),
            &date,
            &date,
            "gen-trend-contract",
        )
        .unwrap();
        let iana_tokens2: i64 = iana2.iter().map(|r| r.tokens).sum();
        assert_eq!(iana_tokens2, 400); // gen-scoped: cross-gen event is excluded
    }

    /// Contract: IANA heatmap reads from daily_usage (same underlying query as
    /// trend). This test verifies the daily_usage heatmap read path produces
    /// consistent per-date aggregates. Cross-generation semantics are inherited
    /// from the trend contract test above.
    #[test]
    fn overview_heatmap_iana_path_reads_from_daily_usage_consistently() {
        use crate::write_queries::{
            insert_usage_events_batch_returning_inserted, upsert_daily_usage_for_events,
        };
        use busytok_domain::ReportingTimezone;

        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        let rtz = ReportingTimezone::parse("America/New_York").unwrap();

        // Event at 2026-06-12 04:00 UTC = 2026-06-12 00:00 EDT
        let ts1 = 1_781_246_400_000i64;
        // Event at 2026-06-12 16:00 UTC = 2026-06-12 12:00 EDT
        let ts2 = 1_781_289_600_000i64;

        let mut evt1 = busytok_domain::NormalizedUsageEvent::minimal_for_test(
            "evt-heatmap-1",
            busytok_domain::AgentKind::ClaudeCode,
        );
        evt1.timestamp_ms = ts1;
        evt1.total_tokens = 100;
        evt1.cost_usd = Some(0.01);

        let mut evt2 = busytok_domain::NormalizedUsageEvent::minimal_for_test(
            "evt-heatmap-2",
            busytok_domain::AgentKind::ClaudeCode,
        );
        evt2.timestamp_ms = ts2;
        evt2.total_tokens = 300;
        evt2.cost_usd = Some(0.03);

        let inserted =
            insert_usage_events_batch_returning_inserted(conn, &[evt1, evt2], "gen-heatmap")
                .unwrap();
        upsert_daily_usage_for_events(conn, &inserted, &rtz, "gen-heatmap").unwrap();

        // Both events fall in the same IANA date (2026-06-12 EDT).
        let date1 = rtz.local_date_for_timestamp_ms(ts1).unwrap();
        let date2 = rtz.local_date_for_timestamp_ms(ts2).unwrap();
        assert_eq!(date1, date2, "both events in same IANA date");

        let rows = read_overview_trend_from_daily_usage(
            conn,
            rtz.canonical_name(),
            &date1,
            &date1,
            "gen-heatmap",
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tokens, 400); // 100 + 300 grouped into one IANA day
        assert_eq!(rows[0].event_count, 2);
    }
}
