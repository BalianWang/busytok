//! Durable event sequence helpers.
//!
//! The `event_sequence_state` table holds a singleton row (id=1) with the
//! latest allocated event sequence number. The writer actor uses
//! `allocate_event_sequence_batch` to claim a contiguous range before
//! writing events, guaranteeing gap-free monotonic sequencing.

use anyhow::{Context, Result};
use busytok_domain::now_ms;
use rusqlite::{params, Connection};

/// Ensure the singleton `event_sequence_state` row exists.
pub fn ensure_event_sequence_state(conn: &Connection) -> Result<()> {
    let now_ms = now_ms();
    conn.execute(
        "INSERT OR IGNORE INTO event_sequence_state (id, latest_event_seq, latest_event_timestamp_ms, updated_at_ms) \
         VALUES (1, 0, NULL, ?1)",
        params![now_ms],
    )
    .context("failed to ensure event_sequence_state singleton")?;
    Ok(())
}

/// Allocate a contiguous batch of event sequence numbers.
///
/// Atomically increments `latest_event_seq` by `count` and returns the
/// half-open range `(start, end]`, i.e. the first allocated sequence
/// number and the last allocated sequence number.
///
/// For example, if `latest_event_seq` is currently 0 and `count` is 5,
/// this returns `(1, 5)`, meaning sequences 1 through 5 are now allocated
/// to the caller, and the counter is at 5.
pub fn allocate_event_sequence_batch(conn: &Connection, count: i64) -> Result<(i64, i64)> {
    ensure_event_sequence_state(conn)?;

    let old_seq: i64 = conn
        .query_row(
            "SELECT latest_event_seq FROM event_sequence_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .context("failed to read latest_event_seq")?;

    let new_seq = old_seq + count;
    let now_ms = now_ms();

    conn.execute(
        "UPDATE event_sequence_state SET latest_event_seq = ?1, updated_at_ms = ?2 WHERE id = 1",
        params![new_seq, now_ms],
    )
    .context("failed to update latest_event_seq")?;

    Ok((old_seq + 1, new_seq))
}

/// Read the latest allocated event sequence number.
///
/// Returns 0 if the singleton row does not exist yet.
pub fn read_latest_event_seq(conn: &Connection) -> Result<i64> {
    ensure_event_sequence_state(conn)?;
    let seq: i64 = conn
        .query_row(
            "SELECT latest_event_seq FROM event_sequence_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .context("failed to read latest event sequence")?;
    Ok(seq)
}

/// Record a timestamp against the latest known event sequence.
pub fn update_latest_event_timestamp(conn: &Connection, timestamp_ms: i64) -> Result<()> {
    let now_ms = now_ms();
    conn.execute(
        "UPDATE event_sequence_state \
         SET latest_event_timestamp_ms = MAX(COALESCE(latest_event_timestamp_ms, 0), ?1), \
             updated_at_ms = ?2 \
         WHERE id = 1",
        params![timestamp_ms, now_ms],
    )
    .context("failed to update latest event timestamp")?;
    Ok(())
}

/// Checkpoint current writer metrics into the `service_state` singleton.
///
/// Persists `writer_queue_depth` and `aggregate_lag_ms` so they survive
/// restarts. Called by the writer actor when values materially change
/// (threshold crossings) or at a coarse interval (~30 s).
///
/// Note: `latest_event_seq` is tracked in `event_sequence_state` (updated
/// by `allocate_event_sequence_batch`), not in `service_state`.
pub fn checkpoint_writer_metrics(
    conn: &Connection,
    queue_depth: i64,
    lag_ms: i64,
    _latest_event_seq: Option<i64>,
) -> Result<()> {
    let now_ms = now_ms();

    conn.execute(
        "INSERT INTO service_state \
         (id, writer_queue_depth, aggregate_lag_ms, updated_at_ms) \
         VALUES (1, ?1, ?2, ?3) \
         ON CONFLICT(id) DO UPDATE SET \
           writer_queue_depth = excluded.writer_queue_depth, \
           aggregate_lag_ms = excluded.aggregate_lag_ms, \
           updated_at_ms = excluded.updated_at_ms",
        rusqlite::params![queue_depth, lag_ms, now_ms],
    )
    .context("failed to checkpoint writer metrics to service_state")?;
    Ok(())
}

/// Append durable outbox envelope events to the persisted outbox_log.
///
/// Each JSON-encoded `PublishedEvent` envelope is persisted so event replays
/// survive crashes. The caller provides an `event_seq` per entry so that
/// catch-up can key off the durable sequence number rather than the row id.
pub fn append_durable_outbox_events(conn: &Connection, entries: &[(i64, String)]) -> Result<()> {
    let now_ms = now_ms();
    for (event_seq, json) in entries {
        conn.execute(
            "INSERT INTO outbox_log (event_seq, envelope_json, created_at_ms) VALUES (?1, ?2, ?3)",
            rusqlite::params![event_seq, json, now_ms],
        )
        .context("failed to append durable outbox event")?;
    }
    Ok(())
}

/// Read outbox log entries whose `event_seq` is strictly greater than
/// `after_seq`, ordered by `event_seq` ascending. `limit` caps the batch size.
///
/// Returns rows with `(event_seq, envelope_json)`.
pub fn read_outbox_since(
    conn: &Connection,
    after_seq: i64,
    limit: i64,
) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT event_seq, envelope_json FROM outbox_log \
         WHERE event_seq > ?1 ORDER BY event_seq ASC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![after_seq, limit], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[test]
    fn read_latest_event_seq_returns_zero_when_row_absent() {
        let db = Database::open_in_memory().unwrap();
        let seq = read_latest_event_seq(db.conn()).unwrap();
        assert_eq!(seq, 0);
    }

    #[test]
    fn allocate_event_sequence_batch_allocates_contiguous_range() {
        let db = Database::open_in_memory().unwrap();
        let (start, end) = allocate_event_sequence_batch(db.conn(), 5).unwrap();
        assert_eq!(start, 1);
        assert_eq!(end, 5);
    }

    #[test]
    fn allocate_event_sequence_batch_accumulates_across_calls() {
        let db = Database::open_in_memory().unwrap();
        // First batch
        let (s1, e1) = allocate_event_sequence_batch(db.conn(), 3).unwrap();
        assert_eq!(s1, 1);
        assert_eq!(e1, 3);
        // Second batch
        let (s2, e2) = allocate_event_sequence_batch(db.conn(), 4).unwrap();
        assert_eq!(s2, 4);
        assert_eq!(e2, 7);
        // Verify counter
        let seq = read_latest_event_seq(db.conn()).unwrap();
        assert_eq!(seq, 7);
    }

    #[test]
    fn allocate_event_sequence_batch_zero_count_no_op() {
        let db = Database::open_in_memory().unwrap();
        let (start, end) = allocate_event_sequence_batch(db.conn(), 0).unwrap();
        assert_eq!(start, 1);
        assert_eq!(end, 0);
        let seq = read_latest_event_seq(db.conn()).unwrap();
        assert_eq!(seq, 0);
    }

    #[test]
    fn checkpoint_writer_metrics_persists_to_service_state() {
        let db = Database::open_in_memory().unwrap();

        checkpoint_writer_metrics(db.conn(), 64, 2500, Some(42)).unwrap();

        let (qd, lag): (i64, i64) = db
            .conn()
            .query_row(
                "SELECT writer_queue_depth, aggregate_lag_ms FROM service_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(qd, 64);
        assert_eq!(lag, 2500);
    }

    #[test]
    fn checkpoint_writer_metrics_overwrites_previous_values() {
        let db = Database::open_in_memory().unwrap();
        checkpoint_writer_metrics(db.conn(), 10, 100, None).unwrap();
        checkpoint_writer_metrics(db.conn(), 20, 200, None).unwrap();

        let (qd, lag): (i64, i64) = db
            .conn()
            .query_row(
                "SELECT writer_queue_depth, aggregate_lag_ms FROM service_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(qd, 20);
        assert_eq!(lag, 200);
    }

    #[test]
    fn append_durable_outbox_events_persists_and_reads_back() {
        let db = Database::open_in_memory().unwrap();
        let entries = vec![
            (
                1i64,
                r#"{"event_id":"evt-1","agent":"cc","event_seq":1}"#.to_string(),
            ),
            (
                2i64,
                r#"{"event_id":"evt-2","agent":"cc","event_seq":2}"#.to_string(),
            ),
        ];
        append_durable_outbox_events(db.conn(), &entries).unwrap();

        // Read back by event_seq.
        let rows = read_outbox_since(db.conn(), 0, 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, 1);
        assert_eq!(rows[1].0, 2);
        assert!(rows[0].1.contains("evt-1"));
        assert!(rows[1].1.contains("evt-2"));
    }

    #[test]
    fn read_outbox_since_respects_after_seq_and_limit() {
        let db = Database::open_in_memory().unwrap();
        let entries: Vec<(i64, String)> = (1..=10)
            .map(|i| (i as i64, format!(r#"{{"event_seq":{i}}}"#)))
            .collect();
        append_durable_outbox_events(db.conn(), &entries).unwrap();

        // Read from after seq 5, limit 3.
        let rows = read_outbox_since(db.conn(), 5, 3).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].0, 6);
        assert_eq!(rows[2].0, 8);
    }
}
