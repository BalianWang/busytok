//! Live throughput queries used by the sampler and backfill handler.

use anyhow::Result;
use busytok_domain::now_ms;
use busytok_protocol::dto::LiveSampleDto;
use rusqlite::Connection;

/// Query the most recently completed 2s bucket so the sampler and backfill
/// produce the same kind of discrete-bucket data. Shared `bucket_start_ms`
/// alignment means the splice replace semantics are valid.
pub fn query_sample_window(conn: &Connection) -> Result<LiveSampleDto> {
    let now = now_ms();
    // Most recently completed 2s-aligned bucket: [bucket_start_ms, bucket_start_ms + 2000)
    let bucket_start_ms = ((now - 2000) / 2000) * 2000;
    let bucket_end_ms = bucket_start_ms + 2000;

    let total_tokens: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(total_tokens), 0) FROM usage_events \
             WHERE timestamp_ms >= ?1 AND timestamp_ms < ?2",
            rusqlite::params![bucket_start_ms, bucket_end_ms],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let total_cost: Option<f64> = conn
        .query_row(
            "SELECT COALESCE(SUM(cost_usd), 0) FROM usage_events \
             WHERE timestamp_ms >= ?1 AND timestamp_ms < ?2 AND cost_usd IS NOT NULL",
            rusqlite::params![bucket_start_ms, bucket_end_ms],
            |row| row.get(0),
        )
        .ok()
        .filter(|v: &f64| *v > 0.0);

    let event_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM usage_events \
             WHERE timestamp_ms >= ?1 AND timestamp_ms < ?2",
            rusqlite::params![bucket_start_ms, bucket_end_ms],
            |row| row.get(0),
        )
        .unwrap_or(0);

    // Per-second rate = total over 2s bucket ÷ 2.
    Ok(LiveSampleDto {
        bucket_start_ms,
        tokens_per_sec: total_tokens as f64 / 2.0,
        cost_per_sec: total_cost.map(|c| c / 2.0),
        events_per_sec: event_count as f64 / 2.0,
    })
}

/// Query backfill buckets for an explicit time range.
pub fn query_backfill_buckets_range(
    conn: &Connection,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<LiveSampleDto>> {
    let mut stmt = conn.prepare(
        "SELECT \
            (timestamp_ms / 2000) * 2000 AS bucket_start_ms, \
            COALESCE(SUM(total_tokens), 0) AS total_tokens, \
            COALESCE(SUM(cost_usd), 0) AS total_cost, \
            COUNT(*) AS event_count \
         FROM usage_events \
         WHERE timestamp_ms >= ?1 AND timestamp_ms < ?2 \
         GROUP BY bucket_start_ms \
         ORDER BY bucket_start_ms ASC",
    )?;

    let rows = stmt.query_map(rusqlite::params![start_ms, end_ms], |row| {
        let bucket_ms: i64 = row.get(0)?;
        let tokens: i64 = row.get(1)?;
        let cost: f64 = row.get(2)?;
        let count: i64 = row.get(3)?;
        Ok(LiveSampleDto {
            bucket_start_ms: bucket_ms,
            tokens_per_sec: tokens as f64 / 2.0,
            cost_per_sec: if cost > 0.0 { Some(cost / 2.0) } else { None },
            events_per_sec: count as f64 / 2.0,
        })
    })?;

    let mut samples = Vec::new();
    for row in rows {
        samples.push(row?);
    }
    Ok(samples)
}

/// Query exact 2s buckets from the promoted materialized table for a
/// generation, grouped only by bucket so the live chart receives one point per
/// timestamp.
pub fn query_exact_buckets_range(
    conn: &Connection,
    generation_id: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<LiveSampleDto>> {
    if !usage_buckets_2s_has_rows(conn)? {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        "SELECT \
            bucket_start_ms, \
            COALESCE(SUM(total_tokens), 0) AS total_tokens, \
            COALESCE(SUM(cost_usd), 0) AS total_cost, \
            COALESCE(SUM(event_count), 0) AS event_count \
         FROM usage_buckets_2s \
         WHERE generation_id = ?1 \
           AND bucket_start_ms >= ?2 AND bucket_start_ms < ?3 \
         GROUP BY bucket_start_ms \
         ORDER BY bucket_start_ms ASC",
    )?;

    let rows = stmt.query_map(rusqlite::params![generation_id, start_ms, end_ms], |row| {
        let bucket_ms: i64 = row.get(0)?;
        let tokens: i64 = row.get(1)?;
        let cost: f64 = row.get(2)?;
        let count: i64 = row.get(3)?;
        Ok(LiveSampleDto {
            bucket_start_ms: bucket_ms,
            tokens_per_sec: tokens as f64 / 2.0,
            cost_per_sec: if cost > 0.0 { Some(cost / 2.0) } else { None },
            events_per_sec: count as f64 / 2.0,
        })
    })?;

    let mut samples = Vec::new();
    for row in rows {
        samples.push(row?);
    }
    Ok(samples)
}

/// Query the most recently completed 2s bucket from `usage_buckets_2s`
/// for a specific promoted generation. Returns `None` when no data exists
/// for the given generation in the current 2s window.
pub fn query_exact_sample_window(
    conn: &Connection,
    generation_id: &str,
) -> Result<Option<LiveSampleDto>> {
    if !usage_buckets_2s_has_rows(conn)? {
        return Ok(None);
    }

    let now = now_ms();
    let bucket_start_ms = ((now - 2000) / 2000) * 2000;
    let bucket_end_ms = bucket_start_ms + 2000;

    let row = conn.query_row(
        "SELECT \
            COALESCE(SUM(total_tokens), 0), \
            COALESCE(SUM(cost_usd), 0), \
            COALESCE(SUM(event_count), 0) \
         FROM usage_buckets_2s \
         WHERE generation_id = ?1 \
           AND bucket_start_ms >= ?2 AND bucket_start_ms < ?3",
        rusqlite::params![generation_id, bucket_start_ms, bucket_end_ms],
        |row| {
            let tokens: i64 = row.get(0)?;
            let cost: f64 = row.get(1)?;
            let events: i64 = row.get(2)?;
            Ok((tokens, cost, events))
        },
    );

    match row {
        Ok((tokens, cost, events)) if tokens > 0 || events > 0 => Ok(Some(LiveSampleDto {
            bucket_start_ms,
            tokens_per_sec: tokens as f64 / 2.0,
            cost_per_sec: if cost > 0.0 { Some(cost / 2.0) } else { None },
            events_per_sec: events as f64 / 2.0,
        })),
        Ok(_) => Ok(None),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("query_exact_sample_window failed: {e}")),
    }
}

fn usage_buckets_2s_has_rows(conn: &Connection) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM usage_buckets_2s LIMIT 1)",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|exists| exists != 0)
    .map_err(|e| anyhow::anyhow!("usage_buckets_2s row check failed: {e}"))
}
