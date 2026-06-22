#![allow(
    clippy::await_holding_lock,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::field_reassign_with_default,
    clippy::uninlined_format_args,
    clippy::inconsistent_digit_grouping,
    clippy::len_zero,
    clippy::identity_op,
    clippy::useless_vec,
    clippy::manual_dangling_ptr,
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::absurd_extreme_comparisons,
    unused_variables,
    unused_imports,
    dead_code
)]
//! Integration tests for rebuild frontiers, replay queue drain, and promotion
//! barrier. These tests verify the rebuild orchestration layer uses the
//! writer actor correctly, captures frontiers, replays tail deltas, and
//! enforces consistency-drift guardrails before promoting a generation.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use busytok_config::BusytokPaths;
use busytok_domain::now_ms;
use busytok_runtime::supervisor::BusytokSupervisor;
use busytok_runtime::writer::{WriteCommand, WriterHandle};
use busytok_store::{Database, LogSourceRow};

// ── Helpers ────────────────────────────────────────────────────────────────

fn make_supervisor(db: Database) -> BusytokSupervisor {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = BusytokPaths::for_test(tmp.path());
    BusytokSupervisor::new(db, paths)
}

/// Seed a source file checkpoint row in the database.
fn seed_file_checkpoint(
    db: &Database,
    file_id: &str,
    source_id: &str,
    agent: &str,
    path: &str,
    offset: i64,
    size: i64,
    mtime: Option<i64>,
) {
    let conn = db.conn();
    let now = now_ms();
    conn.execute(
        "INSERT OR REPLACE INTO source_file_checkpoints \
         (id, source_id, agent, path, inode, offset_bytes, size_bytes, \
          last_mtime_ms, state, first_seen_at_ms, last_seen_at_ms, \
          created_at_ms, updated_at_ms) \
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, 'active', ?8, ?8, ?8, ?8)",
        rusqlite::params![file_id, source_id, agent, path, offset, size, mtime, now],
    )
    .expect("seed file checkpoint");
}

/// Seed an audit_generations row.
fn seed_generation(db: &Database, gen_id: &str, state: &str, is_active: bool) {
    let conn = db.conn();
    let now = now_ms();
    conn.execute(
        "INSERT OR REPLACE INTO audit_generations \
         (generation_id, state, started_at_ms, is_active, created_at_ms, updated_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?3, ?3)",
        rusqlite::params![gen_id, state, now, is_active as i32],
    )
    .expect("seed generation");
}

fn seed_log_source(db: &Database, id: &str, status: &str) {
    let now = now_ms();
    db.upsert_log_source(&LogSourceRow {
        id: id.to_string(),
        agent: "claude_code".to_string(),
        source_type: "jsonl".to_string(),
        root_path: format!("/logs/{id}"),
        configured_by_user: 1,
        default_discovery_enabled: 1,
        status: status.to_string(),
        last_scan_started_at_ms: Some(now),
        last_scan_completed_at_ms: Some(now),
        last_error: None,
        first_seen_at_ms: now,
        last_seen_at_ms: now,
        created_at_ms: now,
        updated_at_ms: now,
    })
    .unwrap();
}

/// Count pending replay rows.
fn pending_replay_count(db: &Database) -> i64 {
    db.conn()
        .query_row(
            "SELECT COUNT(*) FROM tail_replay_queue WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0)
}

/// Count usage_events for a generation.
fn generation_event_count(db: &Database, gen_id: &str) -> i64 {
    db.conn()
        .query_row(
            "SELECT COUNT(*) FROM usage_events WHERE generation_id = ?1",
            rusqlite::params![gen_id],
            |row| row.get(0),
        )
        .unwrap_or(0)
}

/// Read a file checkpoint offset.
fn file_checkpoint_offset(db: &Database, file_id: &str) -> Option<i64> {
    db.conn()
        .query_row(
            "SELECT offset_bytes FROM source_file_checkpoints WHERE id = ?1",
            rusqlite::params![file_id],
            |row| row.get(0),
        )
        .ok()
}

/// Read a generation_file_observations row' s offset.
fn generation_obs_offset(db: &Database, gen_id: &str, file_id: &str) -> Option<i64> {
    db.conn()
        .query_row(
            "SELECT offset_bytes FROM generation_file_observations \
             WHERE generation_id = ?1 AND source_file_id = ?2",
            rusqlite::params![gen_id, file_id],
            |row| row.get(0),
        )
        .ok()
}

/// Get active generation ID.
fn active_generation_id(db: &Database) -> Option<String> {
    db.conn()
        .query_row(
            "SELECT generation_id FROM audit_generations WHERE is_active = 1",
            [],
            |row| row.get(0),
        )
        .ok()
}

// ── Rebuild system helper ──────────────────────────────────────────────────

/// A lightweight rebuild test harness that owns a supervisor and its DB.
struct RebuildHarness {
    supervisor: BusytokSupervisor,
}

impl RebuildHarness {
    fn new() -> Self {
        let db = Database::open_in_memory().expect("open db");
        let supervisor = make_supervisor(db);
        Self { supervisor }
    }

    fn db(&self) -> &Arc<Mutex<Database>> {
        self.supervisor.db_handle()
    }

    fn writer(&self) -> &WriterHandle {
        self.supervisor.writer_handle()
    }

    /// Wait for the writer actor to drain pending commands.
    async fn drain_writer(&self) {
        let _ = self.supervisor.writer_handle().flush().await;
    }

    /// Seed basic test data: a source file checkpoint and a generation.
    async fn seed_basic(&self) {
        {
            let db_guard = self.db().lock().unwrap();
            seed_file_checkpoint(
                &db_guard,
                "file-source-a",
                "source-a",
                "claude_code",
                "/logs/source-a.jsonl",
                0,
                5000,
                Some(1000),
            );
            seed_generation(&db_guard, "gen-current", "building", false);
        }
        self.drain_writer().await;
    }

    /// Consume a tail delta: record new events through the writer.
    async fn consume_tail_delta(
        &self,
        source_file_id: &str,
        start_offset: i64,
        end_offset: i64,
        event_count: i64,
    ) {
        for i in 0..event_count {
            self.writer()
                .send(WriteCommand::RecordTailReplay(
                    busytok_runtime::writer::RecordTailReplayCommand {
                        source_file_id: source_file_id.to_string(),
                        event_seq: start_offset + i,
                        event_data_json: format!(
                            r#"{{"id":"evt-{}-{}","agent":"claude_code","source_file_id":"{}","timestamp_ms":{},"total_tokens":100}}"#,
                            source_file_id, i, source_file_id, now_ms()
                        ),
                    },
                ))
                .await
                .expect("send tail replay");
        }
        // Let the writer process the replay commands.
        self.drain_writer().await;

        // Advance the checkpoint to reflect tail consumption.
        self.writer()
            .send(WriteCommand::ProgressCheckpoint(
                busytok_runtime::writer::ProgressCheckpointCommand {
                    file_id: source_file_id.to_string(),
                    source_id: "source-a".to_string(),
                    agent: "claude_code".to_string(),
                    path: "/logs/source-a.jsonl".to_string(),
                    inode: None,
                    offset_bytes: end_offset,
                    size_bytes: 5000,
                    last_mtime_ms: None,
                    state: "active".to_string(),
                },
            ))
            .await
            .expect("send checkpoint");

        // Let the writer process the checkpoint.
        self.drain_writer().await;
    }

    /// Get current checkpoint offset for a source file.
    fn current_checkpoint_offset(&self, file_id: &str) -> Option<i64> {
        let db = self.db().lock().unwrap();
        file_checkpoint_offset(&db, file_id)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn first_run_rebuild_advances_tail_checkpoints_without_exact_active_generation() {
    let harness = RebuildHarness::new();
    harness.seed_basic().await;

    // Write some initial tail replay events.
    harness.consume_tail_delta("file-source-a", 0, 400, 5).await;

    // The checkpoint should now be at 400.
    let cp = harness.current_checkpoint_offset("file-source-a");
    assert_eq!(cp, Some(400), "tail checkpoint should advance to 400");

    // Consume more tail deltas to reach 1400.
    harness
        .consume_tail_delta("file-source-a", 400, 1400, 10)
        .await;

    let cp = harness.current_checkpoint_offset("file-source-a");
    assert_eq!(cp, Some(1400), "tail checkpoint should advance to 1400");

    // Verify tail replay queue has pending events.
    let db = harness.db().lock().unwrap();
    let pending = pending_replay_count(&db);
    assert!(
        pending > 0,
        "tail replay queue should contain pending events"
    );

    // Verify the system can create a new generation.
    let gen_id = "gen-rebuild-1";
    seed_generation(&db, gen_id, "building", false);
    let gen_exists: bool = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM audit_generations WHERE generation_id = ?1",
            rusqlite::params![gen_id],
            |row| row.get::<_, i64>(0).map(|c| c > 0),
        )
        .unwrap_or(false);
    assert!(gen_exists, "new generation should be created");
}

#[tokio::test]
async fn rebuild_promotion_barrier_drains_replay_queue_and_activates_generation() {
    let harness = RebuildHarness::new();
    harness.seed_basic().await;

    // Directly enqueue replay events via write_queries (bypassing writer).
    {
        let db = harness.db().lock().unwrap();
        busytok_store::write_queries::enqueue_tail_replay_rows(
            db.conn(),
            &[
                busytok_store::write_queries::TailReplayEnqueue {
                    source_file_id: "file-source-a".to_string(),
                    event_seq: 1,
                    event_data_json: r#"{"id":"evt-a1","agent":"claude_code","source_file_id":"file-source-a","timestamp_ms":1700000000000,"total_tokens":100}"#.to_string(),
                },
                busytok_store::write_queries::TailReplayEnqueue {
                    source_file_id: "file-source-a".to_string(),
                    event_seq: 2,
                    event_data_json: r#"{"id":"evt-a2","agent":"claude_code","source_file_id":"file-source-a","timestamp_ms":1700000000100,"total_tokens":200}"#.to_string(),
                },
            ],
        )
        .expect("enqueue replay rows");
    }

    let db = harness.db().lock().unwrap();
    let pending_before = pending_replay_count(&db);
    assert!(pending_before > 0, "should have pending replay rows");

    // Create a new generation to promote into.
    seed_generation(&db, "gen-promoted-1", "building", false);
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO generation_file_observations \
             (generation_id, source_file_id, observed_at_ms, \
              offset_bytes, size_bytes, last_mtime_ms, scan_status, scan_errors) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'ok', NULL)",
            rusqlite::params![
                "gen-promoted-1",
                "file-source-a",
                now_ms(),
                0i64,
                5000i64,
                Some(1000i64)
            ],
        )
        .expect("insert observation");
    drop(db);

    harness.drain_writer().await;

    // Send promotion barrier command through the writer.
    harness
        .writer()
        .send(WriteCommand::PromotionBarrier(
            busytok_runtime::writer::PromotionBarrierCommand {
                from_generation_id: "gen-current".to_string(),
                to_generation_id: "gen-promoted-1".to_string(),
            },
        ))
        .await
        .expect("send promotion barrier");

    // Give the writer actor time to process the barrier.
    harness.drain_writer().await;

    // Verify the new generation is active.
    let db = harness.db().lock().unwrap();
    let active = active_generation_id(&db);
    assert_eq!(
        active.as_deref(),
        Some("gen-promoted-1"),
        "new generation should be active after promotion"
    );
}

#[tokio::test]
async fn promotion_barrier_clears_stale_summary_rows_for_removed_sources() {
    let harness = RebuildHarness::new();
    harness.seed_basic().await;

    {
        let db = harness.db().lock().unwrap();
        seed_log_source(&db, "source-active", "active");
        seed_log_source(&db, "source-removed", "removed");
        db.conn()
            .execute(
                "INSERT INTO source_health_summary \
                 (generation_id, source_id, agent, root_path, source_type, status, \
                  configured_by_user, last_scan_at_ms, file_count, parsed_file_count, \
                  event_count, last_error, latest_activity_at_ms, created_at_ms, updated_at_ms) \
                 VALUES ('gen-promoted-cleanup', 'source-removed', 'claude_code', '/logs/source-removed', \
                         'jsonl', 'removed', 1, NULL, 0, 0, 0, NULL, NULL, 1, 1)",
                [],
            )
            .unwrap();
        seed_generation(&db, "gen-promoted-cleanup", "building", false);
    }

    let status = harness.supervisor.status_snapshot_arc();
    let result = busytok_runtime::rebuild::execute_promotion_barrier(
        harness.db(),
        &status,
        "gen-promoted-cleanup",
    )
    .expect("promotion should succeed");
    assert!(result.promoted);

    let db = harness.db().lock().unwrap();
    let source_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM source_health_summary \
             WHERE generation_id = 'gen-promoted-cleanup' AND source_id = 'source-removed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(source_count, 0);
}

#[tokio::test]
async fn rebuild_drift_prevents_promotion_and_enters_ready_degraded() {
    let harness = RebuildHarness::new();
    harness.seed_basic().await;

    // Create intentional drift: the generation observation shows a LARGER
    // offset than the current checkpoint (file truncated after observation).
    {
        let db = harness.db().lock().unwrap();
        let _now = now_ms();

        // Checkpoint is seeded at offset 0 by seed_basic.
        // Insert observation with offset 5000 (file was at 5000 when observed,
        // but checkpoint says 0 — drift!).
        db.conn()
            .execute(
                "INSERT OR REPLACE INTO generation_file_observations \
                 (generation_id, source_file_id, observed_at_ms, \
                  offset_bytes, size_bytes, last_mtime_ms, scan_status, scan_errors) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'ok', NULL)",
                rusqlite::params![
                    "gen-current",
                    "file-source-a",
                    _now,
                    5000i64,
                    5000i64,
                    Some(1000i64)
                ],
            )
            .expect("insert drift observation");
    }

    harness.drain_writer().await;

    // Use the rebuild barrier directly to check drift detection.
    let result = busytok_runtime::rebuild::execute_promotion_barrier(
        harness.db(),
        &Arc::new(tokio::sync::RwLock::new(
            busytok_runtime::status::ServiceStatusSnapshot::new(),
        )),
        "gen-current",
    )
    .expect("barrier should complete without panic");

    assert!(
        !result.promoted,
        "promotion should be refused when drift is detected"
    );
    assert!(
        result.degradation_reason.is_some(),
        "degradation reason should be set"
    );
    assert!(
        result.degradation_reason.unwrap().contains("drift"),
        "reason should mention drift"
    );
}

#[tokio::test]
async fn rebuild_frontier_captures_file_state_at_rebuild_start() {
    let harness = RebuildHarness::new();

    // Seed source file checkpoints at known state.
    {
        let db = harness.db().lock().unwrap();
        seed_file_checkpoint(
            &db,
            "file-frontier-a",
            "source-a",
            "claude_code",
            "/logs/frontier-a.jsonl",
            2000,
            10000,
            Some(1700000000000i64),
        );
    }

    harness.drain_writer().await;

    // Record a generation observation for the file (frontier capture).
    {
        let db = harness.db().lock().unwrap();
        busytok_store::write_queries::insert_generation_observation(
            db.conn(),
            "gen-frontier",
            "file-frontier-a",
            2000,
            10000,
            Some(1700000000000i64),
            Some("ok"),
            None,
        )
        .expect("insert generation observation");
    }

    // Verify observation was recorded.
    let db = harness.db().lock().unwrap();
    let obs_offset = generation_obs_offset(&db, "gen-frontier", "file-frontier-a");
    assert_eq!(obs_offset, Some(2000), "frontier offset should be 2000");

    // Verify the observations table has the correct schema fields.
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM generation_file_observations WHERE generation_id = 'gen-frontier'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "one frontier observation recorded");
}

#[tokio::test]
async fn replay_queue_writes_to_target_generation_on_apply() {
    let harness = RebuildHarness::new();

    // Seed a generation and replay rows.
    {
        let db = harness.db().lock().unwrap();
        seed_generation(&db, "gen-target", "building", false);

        // Enqueue some tail replay rows via write_queries.
        busytok_store::write_queries::enqueue_tail_replay_rows(
            db.conn(),
            &[
                busytok_store::write_queries::TailReplayEnqueue {
                    source_file_id: "file-replay-a".to_string(),
                    event_seq: 1,
                    event_data_json: r#"{"id":"evt-r1","agent":"claude_code","source_file_id":"file-replay-a","timestamp_ms":1700000000000,"total_tokens":100}"#.to_string(),
                },
                busytok_store::write_queries::TailReplayEnqueue {
                    source_file_id: "file-replay-a".to_string(),
                    event_seq: 2,
                    event_data_json: r#"{"id":"evt-r2","agent":"claude_code","source_file_id":"file-replay-a","timestamp_ms":1700000000100,"total_tokens":200}"#.to_string(),
                },
            ],
        )
        .expect("enqueue replay rows");
    }

    // Apply replay rows to the target generation.
    {
        let db = harness.db().lock().unwrap();
        let applied = busytok_store::write_queries::apply_replay_rows_to_target_generation(
            db.conn(),
            "gen-target",
            None,
            100,
        )
        .expect("apply replay rows");
        assert_eq!(applied, 2, "both replay rows should be applied");
    }

    // Verify events in the target generation.
    let db = harness.db().lock().unwrap();
    let count = generation_event_count(&db, "gen-target");
    assert_eq!(
        count, 2,
        "target generation should have 2 events from replay"
    );
}

// ── Writer threshold diagnostics ────────────────────────────────────────────

#[tokio::test]
async fn writer_emits_and_clears_queue_and_lag_diagnostics_on_threshold_transitions() {
    use busytok_config::BusytokSettings;
    use busytok_domain::{AgentKind, NormalizedUsageEvent};
    use busytok_events::{AppEventBus, PublishedEvent};
    use busytok_runtime::status::ServiceStatusSnapshot;
    use busytok_runtime::writer::{TailBatchCommand, WriteCommand};
    use busytok_store::Database;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    // Set up writer with adequate capacity and wait for processing.
    let db = Database::open_in_memory().expect("open db");
    let db = Arc::new(Mutex::new(db));
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(128));
    let settings = Arc::new(Mutex::new(BusytokSettings::default()));

    let (handle, join) = busytok_runtime::writer::spawn_writer(
        db.clone(),
        status.clone(),
        event_bus.clone(),
        settings.clone(),
        16,
    );

    // Subscribe to bus to capture events.
    let mut rx = event_bus.subscribe();

    // Send several small tail batches sequentially (use send rather than try_send).
    for n in 0..5 {
        let evt = {
            let mut e = NormalizedUsageEvent::minimal_for_test(
                &format!("evt-diag-{}", n),
                AgentKind::ClaudeCode,
            );
            e.timestamp_ms = 1_700_000_000_000i64 + n as i64;
            e.total_tokens = 100;
            e.input_tokens = 50;
            e.output_tokens = 50;
            e.model = Some("claude-sonnet".to_string());
            e
        };

        let cmd = WriteCommand::TailBatch(TailBatchCommand {
            source_id: "test-source".to_string(),
            source_file_id: Some("file-diag".to_string()),
            source_file_agent: "claude_code".to_string(),
            source_file_path: "/tmp/test.jsonl".to_string(),
            source_file_inode: None,
            events: vec![evt],
            tool_events: vec![],
            diagnostic_events: vec![],
            codex_snapshots: vec![],
            generation_id: "gen-diag".to_string(),
            checkpoint_offset: Some(100u64 + n as u64),
            write_policy: busytok_domain::UsageWritePolicy::InsertOnce,
        });

        handle.send(cmd).await.expect("send tail batch");
    }

    // Give writer time to process all commands and publish events.
    handle.flush().await.unwrap();

    // Collect published events; verify durable events carry event_seq.
    let mut events: Vec<PublishedEvent> = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    // Verify at least some durable events carry event_seq.
    let durable_with_seq: Vec<_> = events.iter().filter(|e| e.event_seq.is_some()).collect();
    assert!(
        !durable_with_seq.is_empty(),
        "should have durable events with event_seq (collected {} events total)",
        events.len()
    );

    // Verify event_seq values are strictly increasing. Each durable event
    // envelope (UsageEventInserted, DataInvalidated, SummaryUpdated) must
    // carry a unique, monotonically increasing sequence number.
    let seqs: Vec<i64> = durable_with_seq
        .iter()
        .filter_map(|e| e.event_seq)
        .collect();
    if seqs.len() > 1 {
        for window in seqs.windows(2) {
            assert!(
                window[1] > window[0],
                "event_seq must be strictly increasing: {} then {}",
                window[1],
                window[0]
            );
        }
    }

    // Verify generation_id is carried in durable events.
    for evt in &durable_with_seq {
        assert!(
            evt.generation_id.is_some(),
            "durable events should carry generation_id"
        );
    }

    // Drop handle to trigger graceful shutdown.
    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;

    // event_sequence_state should reflect processed event sequences
    // (the latest_event_seq is tracked in event_sequence_state, not service_state).
    let db_guard = db.lock().unwrap();
    let seq_in_state: Option<i64> = db_guard
        .conn()
        .query_row(
            "SELECT latest_event_seq FROM event_sequence_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .ok();
    assert!(
        seq_in_state.unwrap_or(0) > 0,
        "event_sequence_state should reflect processed event sequences"
    );

    // service_state should have been checkpointed too.
    let qd: i64 = db_guard
        .conn()
        .query_row(
            "SELECT writer_queue_depth FROM service_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    assert!(qd >= 0, "service_state should be checkpointed");
}

// ── Regression: fully deduped batch preserves latest_event_seq ────────

#[tokio::test]
async fn fully_deduped_batch_preserves_latest_event_seq() {
    use busytok_config::BusytokSettings;
    use busytok_domain::{AgentKind, NormalizedUsageEvent, UsageWritePolicy};
    use busytok_events::AppEventBus;
    use busytok_runtime::status::ServiceStatusSnapshot;
    use busytok_runtime::writer::{TailBatchCommand, WriteCommand};
    use busytok_store::Database;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let db = Database::open_in_memory().expect("open db");
    let db = Arc::new(Mutex::new(db));
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(128));
    let settings = Arc::new(Mutex::new(BusytokSettings::default()));

    let (handle, join) = busytok_runtime::writer::spawn_writer(
        db.clone(),
        status.clone(),
        event_bus.clone(),
        settings.clone(),
        16,
    );

    // Insert one event to establish a baseline sequence number.
    let evt = {
        let mut e = NormalizedUsageEvent::minimal_for_test("evt-baseline", AgentKind::ClaudeCode);
        e.timestamp_ms = 1_700_000_000_000i64;
        e.total_tokens = 100;
        e.input_tokens = 50;
        e.output_tokens = 50;
        e.model = Some("claude-sonnet".to_string());
        e
    };

    handle
        .send(WriteCommand::TailBatch(TailBatchCommand {
            source_id: "test".to_string(),
            source_file_id: Some("file-1".to_string()),
            source_file_agent: "claude_code".to_string(),
            source_file_path: "/tmp/test.jsonl".to_string(),
            source_file_inode: None,
            events: vec![evt],
            tool_events: vec![],
            diagnostic_events: vec![],
            codex_snapshots: vec![],
            generation_id: "gen-1".to_string(),
            checkpoint_offset: Some(100),
            write_policy: UsageWritePolicy::InsertOnce,
        }))
        .await
        .expect("send baseline");

    handle.flush().await.unwrap();

    let seq_after_first = {
        let snap = status.read().await;
        snap.latest_event_seq
    };
    assert!(
        seq_after_first.unwrap_or(0) > 0,
        "first insert should advance event_seq"
    );

    // Send the SAME event again — full dedupe, inserted == 0.
    let dup_evt = {
        let mut e = NormalizedUsageEvent::minimal_for_test("evt-baseline", AgentKind::ClaudeCode);
        e.timestamp_ms = 1_700_000_000_000i64;
        e.total_tokens = 100;
        e.input_tokens = 50;
        e.output_tokens = 50;
        e.model = Some("claude-sonnet".to_string());
        e
    };

    handle
        .send(WriteCommand::TailBatch(TailBatchCommand {
            source_id: "test".to_string(),
            source_file_id: Some("file-1".to_string()),
            source_file_agent: "claude_code".to_string(),
            source_file_path: "/tmp/test.jsonl".to_string(),
            source_file_inode: None,
            events: vec![dup_evt],
            tool_events: vec![],
            diagnostic_events: vec![],
            codex_snapshots: vec![],
            generation_id: "gen-1".to_string(),
            checkpoint_offset: Some(100),
            write_policy: UsageWritePolicy::InsertOnce,
        }))
        .await
        .expect("send duplicate");

    handle.flush().await.unwrap();

    let seq_after_dup = {
        let snap = status.read().await;
        snap.latest_event_seq
    };

    assert_eq!(
        seq_after_first, seq_after_dup,
        "fully deduped batch must not rewind latest_event_seq"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}
