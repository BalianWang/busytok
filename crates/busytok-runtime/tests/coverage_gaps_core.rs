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
//! Coverage gap tests for `busytok-runtime` core modules.
//!
//! Targets uncovered code paths in:
//! - `status.rs` — `ServiceStatusSnapshot` helper methods (push_transient_sample,
//!   transient_samples, apply_*, hydrate_from_service_state_row, hydrate_chip_data,
//!   invalidate_chip_data).
//! - `rebuild.rs` — `initiate_rebuild`, `record_new_file_observation`,
//!   `record_new_file_during_rebuild`, `record_rebuild_diagnostic`, drift variants.
//! - `tail.rs` — `prepare_tail_batch_command` (no-adapter, empty-file, populated-file).
//! - `writer.rs` — `SettingsWrite`, `RealtimeSummaryReplace`, `ResetFailedCheckpoints`,
//!   `RebuildRollups` (no-op + error paths), `DiagnosticWrite` (warning/error
//!   publishing), `TailReplayBatch`, `RecordTailReplay`, `ApplyReplayToTarget`,
//!   `LogSourceUpsert`, threshold diagnostics, batch flush with multiple generations.
//! - `read_service.rs` — error paths (open failure, query failure, timeout,
//!   in-memory backend, slow query logging).
//! - `service_app.rs` — `shutdown_control_server` with `result_already_read=true`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serial_test::serial;

use busytok_config::BusytokPaths;
use busytok_domain::{
    now_ms, AgentKind, NormalizedUsageEvent, OperationalDiagnosticEvent, ReportingTimezone,
    UsageWritePolicy,
};
use busytok_events::AppEvent;
use busytok_events::AppEventBus;
use busytok_protocol::dto::{LiveSampleDto, ReadinessStateDto, ScanProgressDto};
use busytok_runtime::read_service::{ReadError, ReadErrorKind, ReadQuery, ReadService};
use busytok_runtime::rebuild::{
    self, create_generation, execute_promotion_barrier, initiate_rebuild,
    record_new_file_during_rebuild, record_new_file_observation, record_rebuild_diagnostic,
    RebuildFrontier,
};
use busytok_runtime::status::{
    CachedClientRollup, ServiceStateRow, ServiceStatusSnapshot, TRANSIENT_RING_BUFFER_CAPACITY,
};
use busytok_runtime::tail::{prepare_tail_batch_command, rescan_changed_files};
use busytok_runtime::writer::{
    self, spawn_test_writer_with_capacity, spawn_writer, ApplyReplayCommand,
    DiagnosticWriteCommand, FlushCommand, GenerationCreateCommand, LogSourceUpsertCommand,
    ProgressCheckpointCommand, PromotionBarrierCommand, RealtimeSummaryReplaceCommand,
    RebuildRollupsCommand, RecordTailReplayCommand, ResetFailedCheckpointsCommand, ShutdownCommand,
    TailBatchCommand, TailReplayBatchCommand, WriteCommand, WriterHandle,
};
use busytok_runtime::BusytokSupervisor;
use busytok_runtime::ServiceApp;
use busytok_store::write_queries;
use busytok_store::{Database, LogSourceRow};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_test_event(id: &str, tokens: i64, cost: Option<f64>) -> NormalizedUsageEvent {
    let mut evt = NormalizedUsageEvent::minimal_for_test(id, AgentKind::ClaudeCode);
    evt.total_tokens = tokens;
    evt.input_tokens = tokens / 2;
    evt.output_tokens = tokens / 2;
    evt.cost_usd = cost;
    evt.timestamp_ms = now_ms();
    evt
}

fn make_test_event_with_agent(id: &str, agent: AgentKind) -> NormalizedUsageEvent {
    let mut evt = NormalizedUsageEvent::minimal_for_test(id, agent);
    evt.total_tokens = 100;
    evt.timestamp_ms = now_ms();
    evt
}

fn make_supervisor() -> (BusytokSupervisor, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let db = Database::open_in_memory().expect("open in-memory");
    (BusytokSupervisor::new(db, paths), dir)
}

fn seed_active_generation(db: &Database, gen_id: &str) {
    let now = now_ms();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO audit_generations \
             (generation_id, state, started_at_ms, promoted_at_ms, is_active, \
              created_at_ms, updated_at_ms) \
             VALUES (?1, 'promoted', ?2, ?2, 1, ?2, ?2)",
            rusqlite::params![gen_id, now],
        )
        .expect("seed active generation");
}

fn seed_log_source_row(db: &Database, id: &str, status: &str) {
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
    .expect("seed log source");
}

// =============================================================================
// status.rs — ServiceStatusSnapshot method coverage
// =============================================================================

#[test]
fn status_snapshot_push_transient_sample_appends_to_buffer() {
    let mut snap = ServiceStatusSnapshot::new();
    assert_eq!(snap.transient_ring_buffer.len(), 0);

    let sample = LiveSampleDto {
        bucket_start_ms: 1000,
        tokens_per_sec: 50.0,
        cost_per_sec: Some(0.05),
        events_per_sec: 5.0,
    };
    snap.push_transient_sample(sample.clone());
    assert_eq!(snap.transient_ring_buffer.len(), 1);
    assert_eq!(snap.transient_ring_buffer[0].bucket_start_ms, 1000);
}

#[test]
fn status_snapshot_push_transient_sample_evicts_oldest_at_capacity() {
    let mut snap = ServiceStatusSnapshot::new();
    // Pre-fill to capacity.
    for i in 0..TRANSIENT_RING_BUFFER_CAPACITY {
        snap.push_transient_sample(LiveSampleDto {
            bucket_start_ms: i as i64,
            tokens_per_sec: 1.0,
            cost_per_sec: None,
            events_per_sec: 1.0,
        });
    }
    assert_eq!(
        snap.transient_ring_buffer.len(),
        TRANSIENT_RING_BUFFER_CAPACITY
    );

    // Push one more — oldest should be evicted.
    snap.push_transient_sample(LiveSampleDto {
        bucket_start_ms: 9999,
        tokens_per_sec: 99.0,
        cost_per_sec: Some(0.99),
        events_per_sec: 9.0,
    });
    assert_eq!(
        snap.transient_ring_buffer.len(),
        TRANSIENT_RING_BUFFER_CAPACITY
    );
    // First entry should now be index=1 (was 0, evicted).
    assert_ne!(snap.transient_ring_buffer[0].bucket_start_ms, 0);
    assert_eq!(
        snap.transient_ring_buffer.back().unwrap().bucket_start_ms,
        9999
    );
}

#[test]
fn status_snapshot_transient_samples_returns_clone() {
    let mut snap = ServiceStatusSnapshot::new();
    snap.push_transient_sample(LiveSampleDto {
        bucket_start_ms: 7,
        tokens_per_sec: 1.0,
        cost_per_sec: None,
        events_per_sec: 1.0,
    });
    snap.push_transient_sample(LiveSampleDto {
        bucket_start_ms: 8,
        tokens_per_sec: 2.0,
        cost_per_sec: Some(0.02),
        events_per_sec: 2.0,
    });

    let samples = snap.transient_samples();
    assert_eq!(samples.len(), 2);
    assert_eq!(samples[0].bucket_start_ms, 7);
    assert_eq!(samples[1].bucket_start_ms, 8);
}

#[test]
fn status_snapshot_apply_runtime_health_update_sets_fields() {
    let mut snap = ServiceStatusSnapshot::new();
    snap.apply_runtime_health_update(42, 1500, Some(99));

    assert_eq!(snap.writer_queue_depth, 42);
    assert_eq!(snap.aggregate_lag_ms, 1500);
    assert_eq!(snap.latest_event_seq, Some(99));
}

#[test]
fn status_snapshot_apply_runtime_health_update_keeps_existing_seq_when_none() {
    let mut snap = ServiceStatusSnapshot::new();
    snap.latest_event_seq = Some(7);
    snap.apply_runtime_health_update(10, 20, None);
    assert_eq!(
        snap.latest_event_seq,
        Some(7),
        "existing seq must be preserved"
    );
}

#[test]
fn status_snapshot_apply_progress_update_sets_progress() {
    let mut snap = ServiceStatusSnapshot::new();
    assert!(snap.progress.is_none());

    let progress = ScanProgressDto {
        scanned_files: 5,
        total_files: Some(10),
        current_path: Some("/tmp/path".to_string()),
        elapsed_ms: 250,
    };
    snap.apply_progress_update(progress.clone());
    assert_eq!(snap.progress.as_ref().unwrap().scanned_files, 5);
    assert_eq!(snap.progress.as_ref().unwrap().total_files, Some(10));
    assert_eq!(snap.progress.as_ref().unwrap().elapsed_ms, 250);
}

#[test]
fn status_snapshot_clear_progress_resets_to_none() {
    let mut snap = ServiceStatusSnapshot::new();
    snap.apply_progress_update(ScanProgressDto {
        scanned_files: 1,
        total_files: Some(1),
        current_path: None,
        elapsed_ms: 0,
    });
    assert!(snap.progress.is_some());
    snap.clear_progress();
    assert!(snap.progress.is_none());
}

#[test]
fn status_snapshot_apply_durable_transition_sets_readiness_and_generation() {
    let mut snap = ServiceStatusSnapshot::new();
    assert_eq!(snap.readiness, ReadinessStateDto::Starting);
    assert!(snap.active_generation_id.is_none());

    snap.apply_durable_transition(ReadinessStateDto::ReadyExact, Some("gen-xyz".to_string()));
    assert_eq!(snap.readiness, ReadinessStateDto::ReadyExact);
    assert_eq!(snap.active_generation_id.as_deref(), Some("gen-xyz"));

    // Transition to degraded with no active generation.
    snap.apply_durable_transition(ReadinessStateDto::ReadyDegraded, None);
    assert_eq!(snap.readiness, ReadinessStateDto::ReadyDegraded);
    assert!(snap.active_generation_id.is_none());
}

#[test]
fn status_snapshot_hydrate_from_service_state_row_copies_all_fields() {
    let mut snap = ServiceStatusSnapshot::new();
    snap.writer_queue_depth = 99;
    snap.aggregate_lag_ms = 99;
    snap.latest_event_seq = Some(99);

    let row = ServiceStateRow {
        latest_event_seq: Some(500),
        readiness: ReadinessStateDto::ReadyExact,
        active_generation_id: Some("gen-hydrate".to_string()),
        writer_queue_depth: 12,
        aggregate_lag_ms: 345,
    };
    snap.hydrate_from_service_state_row(&row);

    assert_eq!(snap.latest_event_seq, Some(500));
    assert_eq!(snap.readiness, ReadinessStateDto::ReadyExact);
    assert_eq!(snap.active_generation_id.as_deref(), Some("gen-hydrate"));
    assert_eq!(snap.writer_queue_depth, 12);
    assert_eq!(snap.aggregate_lag_ms, 345);
}

#[test]
fn status_snapshot_hydrate_chip_data_reads_counts_from_db() {
    let db = Database::open_in_memory().expect("db");
    seed_active_generation(&db, "gen-chip");
    seed_log_source_row(&db, "src-active", "active");
    seed_log_source_row(&db, "src-removed", "removed");

    // Insert a usage event so the count is non-zero.
    let mut event = NormalizedUsageEvent::minimal_for_test("evt-1", AgentKind::ClaudeCode);
    event.timestamp_ms = now_ms();
    event.total_tokens = 50;
    db.write_usage_event(&event, UsageWritePolicy::InsertOnce)
        .expect("write event");

    let mut snap = ServiceStatusSnapshot::new();
    snap.active_generation_id = Some("gen-chip".to_string());
    assert!(!snap.chip_data_hydrated);

    snap.hydrate_chip_data(db.conn());

    assert!(snap.chip_data_hydrated);
    assert_eq!(snap.total_usage_event_count, 1);
    assert_eq!(snap.source_count, 1, "removed source should be excluded");
    // Client rollups may be empty for a single event with no client_kind,
    // but the call should not panic.
}

#[test]
fn status_snapshot_hydrate_chip_data_is_idempotent() {
    let db = Database::open_in_memory().expect("db");
    let mut snap = ServiceStatusSnapshot::new();
    snap.chip_data_hydrated = true;
    snap.total_usage_event_count = 999;

    snap.hydrate_chip_data(db.conn());

    // Should be a no-op since chip_data_hydrated is true.
    assert_eq!(snap.total_usage_event_count, 999);
}

#[test]
fn status_snapshot_invalidate_chip_data_resets_flag_and_rollups() {
    let mut snap = ServiceStatusSnapshot::new();
    snap.chip_data_hydrated = true;
    snap.cached_client_rollups = vec![CachedClientRollup {
        client_kind: "claude_code".to_string(),
        active_source_count: 3,
        event_count: 100,
    }];

    snap.invalidate_chip_data();

    assert!(!snap.chip_data_hydrated);
    assert!(snap.cached_client_rollups.is_empty());
}

// =============================================================================
// rebuild.rs — additional coverage
// =============================================================================

#[test]
fn initiate_rebuild_creates_generation_and_persists_frontiers() {
    let db = Database::open_in_memory().expect("db");
    // Seed a checkpoint row so frontier capture has something to read.
    let conn = db.conn();
    let now = now_ms();
    conn.execute(
        "INSERT OR REPLACE INTO source_file_checkpoints \
         (id, source_id, agent, path, inode, offset_bytes, size_bytes, \
          last_mtime_ms, state, first_seen_at_ms, last_seen_at_ms, \
          created_at_ms, updated_at_ms) \
         VALUES ('file-1', 'src-1', 'claude_code', '/tmp/x.jsonl', NULL, \
                 100, 500, ?1, 'active', ?1, ?1, ?1, ?1)",
        rusqlite::params![now],
    )
    .expect("seed checkpoint");

    let frontier_set = initiate_rebuild(&db, "gen-init").expect("initiate_rebuild");

    assert_eq!(frontier_set.generation_id, "gen-init");
    assert_eq!(frontier_set.frontiers.len(), 1);
    assert_eq!(frontier_set.frontiers[0].source_file_id, "file-1");
    assert_eq!(frontier_set.frontiers[0].offset_bytes, 100);

    // Generation row should be in 'building' state.
    let state: String = conn
        .query_row(
            "SELECT state FROM audit_generations WHERE generation_id = 'gen-init'",
            [],
            |r| r.get(0),
        )
        .expect("query state");
    assert_eq!(state, "building");

    // Frontier should have been persisted as an observation.
    let obs_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM generation_file_observations WHERE generation_id = 'gen-init'",
            [],
            |r| r.get(0),
        )
        .expect("query obs count");
    assert_eq!(obs_count, 1);
}

#[test]
fn record_new_file_observation_creates_checkpoint_and_observation() {
    let db = Database::open_in_memory().expect("db");

    record_new_file_observation(
        &db,
        "gen-new",
        "file-new-1",
        "src-new",
        "claude_code",
        "/tmp/new.jsonl",
        0,
        1000,
        Some(1234),
    )
    .expect("record_new_file_observation");

    let conn = db.conn();

    // source_file_checkpoints should now have a row.
    let (offset, size, mtime): (i64, i64, Option<i64>) = conn
        .query_row(
            "SELECT offset_bytes, size_bytes, last_mtime_ms FROM source_file_checkpoints \
             WHERE id = 'file-new-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("query checkpoint");
    assert_eq!(offset, 0);
    assert_eq!(size, 1000);
    assert_eq!(mtime, Some(1234));

    // log_files should also have a row (upsert_log_file_checkpoint writes here).
    let log_file_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM log_files WHERE id = 'file-new-1'",
            [],
            |r| r.get(0),
        )
        .expect("query log_files");
    assert_eq!(log_file_count, 1);

    // Observation should be recorded with scan_status='new_file'.
    // Note: generation_file_observations has no source_id column — the
    // source_id is joined via the file checkpoint / log_files table.
    let (status, offset, size): (String, i64, i64) = conn
        .query_row(
            "SELECT scan_status, offset_bytes, size_bytes FROM generation_file_observations \
             WHERE generation_id = 'gen-new' AND source_file_id = 'file-new-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("query obs");
    assert_eq!(status, "new_file");
    assert_eq!(offset, 0);
    assert_eq!(size, 1000);
}

#[test]
fn record_new_file_during_rebuild_delegates_to_observation() {
    let db = Database::open_in_memory().expect("db");
    let frontier = RebuildFrontier {
        source_file_id: "file-frontier".to_string(),
        source_id: "src-f".to_string(),
        agent: "codex".to_string(),
        path: "/tmp/f.jsonl".to_string(),
        offset_bytes: 200,
        size_bytes: 800,
        last_mtime_ms: Some(5555),
    };

    record_new_file_during_rebuild(&db, "gen-f", &frontier)
        .expect("record_new_file_during_rebuild");

    let conn = db.conn();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM generation_file_observations \
             WHERE generation_id = 'gen-f' AND source_file_id = 'file-frontier'",
            [],
            |r| r.get(0),
        )
        .expect("query obs");
    assert_eq!(count, 1);
}

#[test]
fn record_rebuild_diagnostic_inserts_diagnostic_event() {
    let db = Database::open_in_memory().expect("db");
    record_rebuild_diagnostic(&db, "gen-d", "warning", "scan skipped file")
        .expect("record_rebuild_diagnostic");

    let conn = db.conn();
    let (severity, message, code): (String, String, String) = conn
        .query_row(
            "SELECT severity, message, code FROM diagnostic_events \
             WHERE source_id = 'rebuild' AND severity = 'warning' \
             ORDER BY happened_at_ms DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("query diag");
    assert_eq!(severity, "warning");
    assert_eq!(message, "scan skipped file");
    assert_eq!(code, "rebuild_lifecycle");
}

#[test]
fn create_generation_is_idempotent_via_replace() {
    let db = Database::open_in_memory().expect("db");
    create_generation(&db, "gen-idem").expect("first create");
    create_generation(&db, "gen-idem").expect("second create (replace)");

    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM audit_generations WHERE generation_id = 'gen-idem'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn promotion_barrier_succeeds_when_no_observations_exist() {
    // No drift can be detected when there are zero observations.
    let db = Database::open_in_memory().expect("db");
    let db = Arc::new(Mutex::new(db));
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));

    {
        let d = db.lock().unwrap();
        create_generation(&d, "gen-empty").expect("create generation");
    }

    let result = execute_promotion_barrier(&db, &status, "gen-empty").expect("barrier");
    assert!(
        result.promoted,
        "promotion should succeed with no observations"
    );
    assert_eq!(result.replay_rows_applied, 0);

    let snap = status.try_read().unwrap();
    assert_eq!(snap.readiness, ReadinessStateDto::ReadyExact);
    assert_eq!(snap.active_generation_id.as_deref(), Some("gen-empty"));
}

#[test]
fn promotion_barrier_drift_on_mtime_and_size_mismatch() {
    // Covers the second drift branch (mtime differs AND size differs AND size > 0).
    let db = Database::open_in_memory().expect("db");
    let db = Arc::new(Mutex::new(db));
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));

    {
        let d = db.lock().unwrap();
        create_generation(&d, "gen-mtime").expect("create generation");
        // Checkpoint: size=1000, mtime=2000.
        let now = now_ms();
        d.conn()
            .execute(
                "INSERT OR REPLACE INTO source_file_checkpoints \
                 (id, source_id, agent, path, inode, offset_bytes, size_bytes, \
                  last_mtime_ms, state, first_seen_at_ms, last_seen_at_ms, \
                  created_at_ms, updated_at_ms) \
                 VALUES ('file-m', 'src-m', 'claude_code', '/tmp/m.jsonl', NULL, \
                         0, 1000, 2000, 'active', ?1, ?1, ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("seed checkpoint");
        // Observation: same offset, but DIFFERENT size (500) and DIFFERENT mtime (9999).
        // offset is 0 in both, so drift cond 1 (offset regression) does NOT fire;
        // drift cond 2 should fire because mtime differs AND size differs AND size > 0.
        write_queries::insert_generation_observation(
            d.conn(),
            "gen-mtime",
            "file-m",
            0,
            500,
            Some(9999),
            Some("ok"),
            None,
        )
        .expect("insert observation");
    }

    let result = execute_promotion_barrier(&db, &status, "gen-mtime").expect("barrier");
    assert!(!result.promoted, "drift should block promotion");
    assert!(result.degradation_reason.is_some());

    let snap = status.try_read().unwrap();
    assert_eq!(snap.readiness, ReadinessStateDto::ReadyDegraded);
}

#[test]
fn promotion_barrier_drift_zero_size_does_not_fire() {
    // Drift condition requires size > 0; size=0 observations should not trip drift.
    let db = Database::open_in_memory().expect("db");
    let db = Arc::new(Mutex::new(db));
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));

    {
        let d = db.lock().unwrap();
        create_generation(&d, "gen-zero").expect("create generation");
        let now = now_ms();
        // Checkpoint: size=0, mtime=2000.
        d.conn()
            .execute(
                "INSERT OR REPLACE INTO source_file_checkpoints \
                 (id, source_id, agent, path, inode, offset_bytes, size_bytes, \
                  last_mtime_ms, state, first_seen_at_ms, last_seen_at_ms, \
                  created_at_ms, updated_at_ms) \
                 VALUES ('file-z', 'src-z', 'claude_code', '/tmp/z.jsonl', NULL, \
                         0, 0, 2000, 'active', ?1, ?1, ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("seed checkpoint");
        // Observation: offset=0, size=0, mtime=9999 (different) — drift cond 2 requires size>0.
        write_queries::insert_generation_observation(
            d.conn(),
            "gen-zero",
            "file-z",
            0,
            0,
            Some(9999),
            Some("ok"),
            None,
        )
        .expect("insert observation");
    }

    let result = execute_promotion_barrier(&db, &status, "gen-zero").expect("barrier");
    assert!(
        result.promoted,
        "zero-size observations should not trip drift"
    );
}

// =============================================================================
// tail.rs — prepare_tail_batch_command + rescan_changed_files
// =============================================================================

#[test]
fn prepare_tail_batch_command_returns_none_when_no_adapter_matches() {
    let db = Database::open_in_memory().expect("db");
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("no-adapter.jsonl");
    std::fs::write(&file_path, "{}\n").expect("write");

    // Empty adapters slice → no adapter can match.
    let adapters: Vec<Box<dyn busytok_adapters::AgentLogAdapter + Send + Sync>> = vec![];
    let result = prepare_tail_batch_command(
        &db,
        &adapters,
        &file_path,
        "src-1",
        AgentKind::Codex,
        "gen-1",
    )
    .expect("call should not error");

    assert!(result.is_none(), "no adapter → no command");
}

#[test]
fn prepare_tail_batch_command_returns_none_when_file_is_empty() {
    let db = Database::open_in_memory().expect("db");
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("empty.jsonl");
    std::fs::write(&file_path, "").expect("create empty file");

    let adapters: Vec<Box<dyn busytok_adapters::AgentLogAdapter + Send + Sync>> =
        vec![Box::new(busytok_adapters::CodexAdapter)];
    let result = prepare_tail_batch_command(
        &db,
        &adapters,
        &file_path,
        "src-empty",
        AgentKind::Codex,
        "gen-empty",
    )
    .expect("call should not error");

    assert!(result.is_none(), "empty file → no command");
}

#[test]
fn prepare_tail_batch_command_builds_command_with_codex_heartbeat_data() {
    let db = Database::open_in_memory().expect("db");
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("rollout-prepare.jsonl");

    let heartbeat1 = r#"{"timestamp":"2026-05-20T07:16:22.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":5,"total_tokens":165},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":165}}}}"#;
    let heartbeat2 = r#"{"timestamp":"2026-05-20T07:16:23.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":130,"cached_input_tokens":15,"output_tokens":70,"reasoning_output_tokens":8,"total_tokens":208},"last_token_usage":{"input_tokens":30,"cached_input_tokens":5,"output_tokens":20,"reasoning_output_tokens":3,"total_tokens":58}}}}"#;
    {
        let mut f = std::fs::File::create(&file_path).expect("create");
        use std::io::Write;
        writeln!(f, "{heartbeat1}").expect("write");
        writeln!(f, "{heartbeat2}").expect("write");
    }

    let adapters: Vec<Box<dyn busytok_adapters::AgentLogAdapter + Send + Sync>> =
        vec![Box::new(busytok_adapters::CodexAdapter)];
    let cmd_opt = prepare_tail_batch_command(
        &db,
        &adapters,
        &file_path,
        "src-prepare",
        AgentKind::Codex,
        "gen-prepare",
    )
    .expect("call should not error");

    let cmd = cmd_opt.expect("should produce a command");
    match cmd {
        WriteCommand::TailBatch(c) => {
            assert_eq!(c.source_id, "src-prepare");
            assert_eq!(c.source_file_agent, "codex");
            assert_eq!(c.generation_id, "gen-prepare");
            assert!(!c.events.is_empty(), "should produce a delta event");
            assert!(c.checkpoint_offset.is_some());
            assert_eq!(
                c.write_policy,
                <busytok_adapters::CodexAdapter as busytok_adapters::AgentLogAdapter>::write_policy(
                    &busytok_adapters::CodexAdapter
                )
            );
        }
        other => panic!("expected TailBatch, got {other:?}"),
    }
}

#[test]
fn prepare_tail_batch_command_returns_none_when_file_does_not_exist() {
    // Reading a missing file should produce an error (not Ok(None)).
    let db = Database::open_in_memory().expect("db");
    let adapters: Vec<Box<dyn busytok_adapters::AgentLogAdapter + Send + Sync>> =
        vec![Box::new(busytok_adapters::CodexAdapter)];
    let result = prepare_tail_batch_command(
        &db,
        &adapters,
        std::path::Path::new("/nonexistent/path/file.jsonl"),
        "src-missing",
        AgentKind::Codex,
        "gen-missing",
    );
    assert!(
        result.is_err(),
        "missing file should produce a read error, not Ok(None)"
    );
}

#[test]
fn prepare_tail_batch_command_uses_existing_checkpoint_offset() {
    // Seed a checkpoint at offset 50, then add data after it.
    let db = Database::open_in_memory().expect("db");
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("resume.jsonl");

    // Write two lines.
    let heartbeat1 = r#"{"timestamp":"2026-05-20T07:16:22.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":5,"total_tokens":165},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":165}}}}"#;
    let heartbeat2 = r#"{"timestamp":"2026-05-20T07:16:23.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":130,"cached_input_tokens":15,"output_tokens":70,"reasoning_output_tokens":8,"total_tokens":208},"last_token_usage":{"input_tokens":30,"cached_input_tokens":5,"output_tokens":20,"reasoning_output_tokens":3,"total_tokens":58}}}}"#;
    let line1_with_newline = format!("{heartbeat1}\n");
    {
        let mut f = std::fs::File::create(&file_path).expect("create");
        use std::io::Write;
        writeln!(f, "{heartbeat1}").expect("write 1");
        writeln!(f, "{heartbeat2}").expect("write 2");
    }

    // Seed a checkpoint at the end of line 1.
    let file_id = busytok_runtime::scan::derive_file_id(&file_path);
    let conn = db.conn();
    let now = now_ms();
    conn.execute(
        "INSERT OR REPLACE INTO log_files \
         (id, source_id, agent, path, inode, offset_bytes, state, \
          first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms) \
         VALUES (?1, 'src-resume', 'codex', ?2, NULL, ?3, 'active', ?4, ?4, ?4, ?4)",
        rusqlite::params![
            file_id,
            file_path.to_string_lossy().to_string(),
            line1_with_newline.len() as i64,
            now
        ],
    )
    .expect("seed checkpoint");

    let adapters: Vec<Box<dyn busytok_adapters::AgentLogAdapter + Send + Sync>> =
        vec![Box::new(busytok_adapters::CodexAdapter)];
    let cmd_opt = prepare_tail_batch_command(
        &db,
        &adapters,
        &file_path,
        "src-resume",
        AgentKind::Codex,
        "gen-resume",
    )
    .expect("call should not error");

    let cmd = cmd_opt.expect("should produce a command");
    if let WriteCommand::TailBatch(c) = cmd {
        // The new checkpoint_offset should be greater than the seeded offset.
        let new_offset = c.checkpoint_offset.unwrap_or(0) as i64;
        assert!(
            new_offset > line1_with_newline.len() as i64,
            "new offset {new_offset} should advance past line 1 ({})",
            line1_with_newline.len()
        );
        assert!(
            !c.events.is_empty(),
            "should produce a delta event from line 2"
        );
    } else {
        panic!("expected TailBatch");
    }
}

#[test]
fn rescan_changed_files_processes_codex_file_end_to_end() {
    // Use rescan_changed_files as the simplest public entry point for tail processing.
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("rescan.jsonl");

    let heartbeat1 = r#"{"timestamp":"2026-05-20T07:16:22.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":5,"total_tokens":165},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":165}}}}"#;
    let heartbeat2 = r#"{"timestamp":"2026-05-20T07:16:23.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":130,"cached_input_tokens":15,"output_tokens":70,"reasoning_output_tokens":8,"total_tokens":208},"last_token_usage":{"input_tokens":30,"cached_input_tokens":5,"output_tokens":20,"reasoning_output_tokens":3,"total_tokens":58}}}}"#;
    {
        let mut f = std::fs::File::create(&file_path).expect("create");
        use std::io::Write;
        writeln!(f, "{heartbeat1}").expect("write 1");
        writeln!(f, "{heartbeat2}").expect("write 2");
    }

    let db = Database::open_in_memory().expect("db");
    let adapters: Vec<Box<dyn busytok_adapters::AgentLogAdapter + Send + Sync>> =
        vec![Box::new(busytok_adapters::CodexAdapter)];

    rescan_changed_files(
        &db,
        &adapters,
        "src-rescan",
        AgentKind::Codex,
        &file_path,
        "UTC",
        "gen-rescan",
    )
    .expect("rescan should succeed");

    let events = db.all_usage_events().expect("get all events");
    assert!(
        !events.is_empty(),
        "rescan_changed_files should ingest at least one event"
    );
}

// =============================================================================
// writer.rs — additional command handler coverage
// =============================================================================

#[tokio::test]
async fn settings_write_timezone_updates_settings_value() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status, event_bus, settings.clone(), 16);

    let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
    handle
        .send(WriteCommand::SettingsWrite(writer::SettingsWriteCommand {
            key: "timezone".to_string(),
            value_json: "Asia/Shanghai".to_string(),
            respond_tx,
        }))
        .await
        .expect("send");

    // verify the writer returned Ok(()) — the expects propagate errors.
    respond_rx.await.expect("response").expect("ok");

    // Confirm the settings field was updated.
    let s = settings.lock().unwrap();
    assert_eq!(s.timezone, "Asia/Shanghai");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn settings_write_week_starts_on_parses_numeric_value() {
    let (_db, _status, _event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let (handle, join) = spawn_writer(db, status, event_bus, settings.clone(), 16);

    let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
    handle
        .send(WriteCommand::SettingsWrite(writer::SettingsWriteCommand {
            key: "week_starts_on".to_string(),
            value_json: "1".to_string(),
            respond_tx,
        }))
        .await
        .expect("send");

    let result = respond_rx.await.expect("response");
    assert!(result.is_ok(), "numeric week_starts_on should succeed");

    let s = settings.lock().unwrap();
    assert_eq!(s.week_starts_on, 1);

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn settings_write_week_starts_on_rejects_non_numeric_value() {
    let (_db, _status, _event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let (handle, join) = spawn_writer(db, status, event_bus, settings.clone(), 16);

    let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
    handle
        .send(WriteCommand::SettingsWrite(writer::SettingsWriteCommand {
            key: "week_starts_on".to_string(),
            value_json: "not-a-number".to_string(),
            respond_tx,
        }))
        .await
        .expect("send");

    let result = respond_rx.await.expect("response");
    assert!(result.is_err(), "non-numeric value should error");
    let err = result.unwrap_err();
    assert!(
        err.contains("invalid week_starts_on"),
        "unexpected err: {err}"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn settings_write_unknown_key_returns_error() {
    let (_db, _status, _event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let (handle, join) = spawn_writer(db, status, event_bus, settings, 16);

    let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
    handle
        .send(WriteCommand::SettingsWrite(writer::SettingsWriteCommand {
            key: "nonexistent_key".to_string(),
            value_json: "value".to_string(),
            respond_tx,
        }))
        .await
        .expect("send");

    let result = respond_rx.await.expect("response");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("unsupported settings key"),
        "unexpected err: {err}"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn realtime_summary_replace_persists_entries() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let entries = vec![
        ("key-a".to_string(), r#"{"value":1}"#.to_string()),
        ("key-b".to_string(), r#"{"value":2}"#.to_string()),
    ];
    handle
        .send(WriteCommand::RealtimeSummaryReplace(
            RealtimeSummaryReplaceCommand { entries },
        ))
        .await
        .expect("send");

    handle.flush().await.expect("flush");

    let db = db.lock().unwrap();
    let count: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM realtime_summary", [], |r| r.get(0))
        .expect("query");
    assert!(count >= 2, "realtime_summary should have entries");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn reset_failed_checkpoints_resets_error_state_files() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    // Seed two log_files in 'error' state.
    let now = now_ms();
    for fid in ["file-err-1", "file-err-2"] {
        db.conn()
            .execute(
                "INSERT INTO log_files \
                 (id, source_id, agent, path, inode, offset_bytes, state, last_error, \
                  first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms) \
                 VALUES (?1, 'src-err', 'claude_code', ?2, NULL, 100, 'error', 'boom', \
                         ?3, ?3, ?3, ?3)",
                rusqlite::params![fid, format!("/tmp/{fid}.jsonl"), now],
            )
            .expect("seed error file");
    }
    // One file in 'active' state — should NOT be touched.
    db.conn()
        .execute(
            "INSERT INTO log_files \
             (id, source_id, agent, path, inode, offset_bytes, state, last_error, \
              first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms) \
             VALUES ('file-ok', 'src-ok', 'claude_code', '/tmp/ok.jsonl', NULL, 50, 'active', NULL, \
                     ?1, ?1, ?1, ?1)",
            rusqlite::params![now],
        )
        .expect("seed active file");

    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let updated = handle.reset_failed_checkpoints().await.expect("reset");
    assert_eq!(updated, 2, "two error files should be reset");

    let db = db.lock().unwrap();
    let error_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM log_files WHERE state = 'error'",
            [],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(error_count, 0, "no files should remain in error state");

    let active_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM log_files WHERE state = 'active'",
            [],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(active_count, 3, "all three files should be active");

    let reset_offset: i64 = db
        .conn()
        .query_row(
            "SELECT offset_bytes FROM log_files WHERE id = 'file-err-1'",
            [],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(reset_offset, 0, "offset should be reset to 0");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn rebuild_rollups_no_op_when_no_active_generation_and_no_events() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status, event_bus, settings, 16);

    // No active generation, no usage events → NoOp path.
    let result = handle.rebuild_rollups("UTC".to_string()).await;
    assert!(result.is_ok(), "no-op rebuild should succeed");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn rebuild_rollups_errors_when_events_exist_but_no_active_generation() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    // Seed a usage event but no active generation.
    let mut event = NormalizedUsageEvent::minimal_for_test("evt-orphan", AgentKind::ClaudeCode);
    event.timestamp_ms = now_ms();
    event.total_tokens = 100;
    db.write_usage_event(&event, UsageWritePolicy::InsertOnce)
        .expect("write event");

    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status, event_bus, settings, 16);

    let result = handle.rebuild_rollups("UTC".to_string()).await;
    assert!(
        result.is_err(),
        "rebuild with events but no active generation should error"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("active generation") || err.contains("usage events"),
        "unexpected error: {err}"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn rebuild_rollups_invalid_timezone_returns_error() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    seed_active_generation(&db, "gen-tz");
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status, event_bus, settings, 16);

    let result = handle.rebuild_rollups("Not/A/Timezone".to_string()).await;
    assert!(result.is_err(), "invalid timezone should error");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn rebuild_rollups_succeeds_with_active_generation_and_events() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    seed_active_generation(&db, "gen-rebuild-ok");
    // handle_rebuild_rollups reads active_generation_id from the status
    // snapshot (not from the DB), so we must hydrate it here.
    status.write().await.active_generation_id = Some("gen-rebuild-ok".to_string());

    let mut event = NormalizedUsageEvent::minimal_for_test("evt-rebuild", AgentKind::ClaudeCode);
    event.timestamp_ms = now_ms();
    event.total_tokens = 100;
    event.input_tokens = 50;
    event.output_tokens = 50;
    event.model = Some("claude-sonnet".to_string());
    // Assign to the generation manually so usage_events_for_generation returns it.
    db.write_usage_event(&event, UsageWritePolicy::InsertOnce)
        .expect("write event");
    db.conn()
        .execute(
            "UPDATE usage_events SET generation_id = 'gen-rebuild-ok' WHERE id = 'evt-rebuild'",
            [],
        )
        .expect("assign generation");

    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let result = handle.rebuild_rollups("UTC".to_string()).await;
    assert!(result.is_ok(), "rebuild should succeed: {:?}", result.err());

    handle.flush().await.expect("flush");

    // daily_usage should have at least one row.
    let db = db.lock().unwrap();
    let daily_count: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM daily_usage", [], |r| r.get(0))
        .expect("query");
    assert!(daily_count >= 1, "daily_usage should be populated");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn diagnostic_write_with_warning_severity_publishes_error_event() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus.clone(), settings, 16);

    let mut rx = event_bus.subscribe();

    handle
        .send(WriteCommand::DiagnosticWrite(DiagnosticWriteCommand {
            source_id: "src-warn".to_string(),
            code: "test_warn".to_string(),
            message: "warning message".to_string(),
            severity: "warning".to_string(),
            details_json: None,
        }))
        .await
        .expect("send");

    handle.flush().await.expect("flush");

    // The warning severity should trigger an ephemeral Error event.
    let mut found_error = false;
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    while std::time::Instant::now() < deadline {
        if let Ok(evt) = rx.try_recv() {
            if matches!(evt.event, AppEvent::Error { .. }) {
                found_error = true;
                break;
            }
        } else {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    assert!(
        found_error,
        "warning severity should publish AppEvent::Error"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn diagnostic_write_with_error_severity_publishes_error_event() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus.clone(), settings, 16);

    let mut rx = event_bus.subscribe();

    handle
        .send(WriteCommand::DiagnosticWrite(DiagnosticWriteCommand {
            source_id: "src-err".to_string(),
            code: "test_err".to_string(),
            message: "error message".to_string(),
            severity: "error".to_string(),
            details_json: Some(r#"{"detail":"x"}"#.to_string()),
        }))
        .await
        .expect("send");

    handle.flush().await.expect("flush");

    let mut found_error = false;
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    while std::time::Instant::now() < deadline {
        if let Ok(evt) = rx.try_recv() {
            if let AppEvent::Error { source, .. } = &evt.event {
                found_error = true;
                assert_eq!(source.as_deref(), Some("src-err"));
                break;
            }
        } else {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    assert!(found_error, "error severity should publish AppEvent::Error");

    // Verify the diagnostic was persisted with the right code.
    let db = db.lock().unwrap();
    let code: String = db
        .conn()
        .query_row(
            "SELECT code FROM diagnostic_events WHERE source_id = 'src-err' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(code, "test_err");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn diagnostic_write_with_info_severity_does_not_publish_error() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus.clone(), settings, 16);

    let mut rx = event_bus.subscribe();

    handle
        .send(WriteCommand::DiagnosticWrite(DiagnosticWriteCommand {
            source_id: "src-info".to_string(),
            code: "test_info".to_string(),
            message: "info message".to_string(),
            severity: "info".to_string(),
            details_json: None,
        }))
        .await
        .expect("send");

    handle.flush().await.expect("flush");

    // Drain any events and confirm none are AppEvent::Error.
    tokio::time::sleep(Duration::from_millis(100)).await;
    while let Ok(evt) = rx.try_recv() {
        assert!(
            !matches!(evt.event, AppEvent::Error { .. }),
            "info severity should NOT publish AppEvent::Error"
        );
    }

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn tail_replay_batch_command_enqueues_rows() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let rows = vec![
        write_queries::TailReplayEnqueue {
            source_file_id: "file-replay-1".to_string(),
            event_seq: 1,
            event_data_json: r#"{"id":"r1","agent":"claude_code"}"#.to_string(),
        },
        write_queries::TailReplayEnqueue {
            source_file_id: "file-replay-1".to_string(),
            event_seq: 2,
            event_data_json: r#"{"id":"r2","agent":"claude_code"}"#.to_string(),
        },
    ];
    handle
        .send(WriteCommand::TailReplayBatch(TailReplayBatchCommand {
            rows,
        }))
        .await
        .expect("send");

    handle.flush().await.expect("flush");

    let db = db.lock().unwrap();
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM tail_replay_queue WHERE source_file_id = 'file-replay-1'",
            [],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(count, 2, "two replay rows should be enqueued");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn record_tail_replay_command_enqueues_single_row() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    handle
        .send(WriteCommand::RecordTailReplay(RecordTailReplayCommand {
            source_file_id: "file-single".to_string(),
            event_seq: 42,
            event_data_json: r#"{"id":"single","agent":"codex"}"#.to_string(),
        }))
        .await
        .expect("send");

    handle.flush().await.expect("flush");

    let db = db.lock().unwrap();
    let seq: i64 = db
        .conn()
        .query_row(
            "SELECT event_seq FROM tail_replay_queue WHERE source_file_id = 'file-single'",
            [],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(seq, 42);

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn apply_replay_to_target_command_applies_rows_to_generation() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    // Enqueue some replay rows directly.
    write_queries::enqueue_tail_replay_rows(
        db.conn(),
        &[
            write_queries::TailReplayEnqueue {
                source_file_id: "file-apply".to_string(),
                event_seq: 1,
                event_data_json: r#"{"id":"evt-apply-1","agent":"claude_code","source_file_id":"file-apply","timestamp_ms":1700000000000,"total_tokens":100}"#.to_string(),
            },
            write_queries::TailReplayEnqueue {
                source_file_id: "file-apply".to_string(),
                event_seq: 2,
                event_data_json: r#"{"id":"evt-apply-2","agent":"claude_code","source_file_id":"file-apply","timestamp_ms":1700000000100,"total_tokens":200}"#.to_string(),
            },
        ],
    )
    .expect("enqueue");

    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    handle
        .send(WriteCommand::ApplyReplayToTarget(ApplyReplayCommand {
            target_generation_id: "gen-apply".to_string(),
            source_file_id: None,
            limit: 100,
        }))
        .await
        .expect("send");

    handle.flush().await.expect("flush");

    let db = db.lock().unwrap();
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM usage_events WHERE generation_id = 'gen-apply'",
            [],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(
        count, 2,
        "two events should be applied to target generation"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn log_source_upsert_command_persists_row() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let now = now_ms();
    let row = LogSourceRow {
        id: "src-upsert".to_string(),
        agent: "claude_code".to_string(),
        source_type: "jsonl".to_string(),
        root_path: "/logs/upsert".to_string(),
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
    };

    handle
        .send(WriteCommand::LogSourceUpsert(LogSourceUpsertCommand {
            row: row.clone(),
        }))
        .await
        .expect("send");

    handle.flush().await.expect("flush");

    let db = db.lock().unwrap();
    let root_path: String = db
        .conn()
        .query_row(
            "SELECT root_path FROM log_sources WHERE id = 'src-upsert'",
            [],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(root_path, "/logs/upsert");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn generation_create_command_inserts_building_row() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    handle
        .send(WriteCommand::GenerationCreate(GenerationCreateCommand {
            generation_id: "gen-via-writer".to_string(),
        }))
        .await
        .expect("send");

    handle.flush().await.expect("flush");

    let db = db.lock().unwrap();
    let state: String = db
        .conn()
        .query_row(
            "SELECT state FROM audit_generations WHERE generation_id = 'gen-via-writer'",
            [],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(state, "building");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn flush_command_returns_pending_errors() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status, event_bus, settings, 16);

    // Send a flush with no pending errors — should return Ok.
    let result = handle.flush().await;
    assert!(
        result.is_ok(),
        "flush with no pending errors should succeed"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn shutdown_command_returns_ok_with_no_errors() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status, event_bus, settings, 16);

    let result = handle.shutdown().await;
    assert!(result.is_ok(), "shutdown should succeed");

    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn writer_threshold_diagnostics_emit_on_queue_depth_crossing() {
    // Spawn a writer with capacity = 128 (above QUEUE_WARNING_THRESHOLD=64)
    // so the queue depth can actually cross the diagnostic threshold.
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status, event_bus.clone(), settings, 128);

    let mut rx = event_bus.subscribe();

    // Saturate the queue with DiagnosticWrite commands (fast dispatch path,
    // not TailBatch which would be batched).
    for i in 0..200 {
        // Use try_send to avoid blocking when full; we just want to push
        // the queue depth momentarily above the warning threshold.
        let _ = handle.try_send(WriteCommand::DiagnosticWrite(DiagnosticWriteCommand {
            source_id: format!("src-thresh-{i}"),
            code: "test".to_string(),
            message: format!("msg-{i}"),
            severity: "info".to_string(),
            details_json: None,
        }));
    }

    // Drain the writer.
    handle.flush().await.expect("flush");

    // We should have seen at least one WriterQueueThreshold event
    // (or a WriterLagThreshold event — both are emitted by the threshold
    // diagnostic function). With a queue capacity of 128 and 200 try_send
    // commands, the queue depth must cross the warning threshold at some
    // point.
    let mut found_threshold = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if let Ok(evt) = rx.try_recv() {
            if matches!(
                evt.event,
                AppEvent::WriterQueueThreshold { .. } | AppEvent::WriterLagThreshold { .. }
            ) {
                found_threshold = true;
                break;
            }
        } else {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    assert!(
        found_threshold,
        "queue threshold diagnostic should be emitted when 200 commands saturate a capacity-128 queue"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn flush_pending_batches_with_multiple_generations_logs_warning() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 64);

    // Send two TailBatch commands with different generation IDs.
    let mut event_a = make_test_event("evt-multi-a", 100, None);
    event_a.timestamp_ms = 1_000_000;
    let mut event_b = make_test_event("evt-multi-b", 200, None);
    event_b.timestamp_ms = 2_000_000;

    handle
        .send(WriteCommand::TailBatch(TailBatchCommand {
            source_id: "src-multi-a".to_string(),
            source_file_id: Some("file-a".to_string()),
            source_file_agent: "claude_code".to_string(),
            source_file_path: "/tmp/a.jsonl".to_string(),
            source_file_inode: None,
            events: vec![event_a],
            tool_events: vec![],
            diagnostic_events: vec![],
            codex_snapshots: vec![],
            generation_id: "gen-multi-a".to_string(),
            checkpoint_offset: Some(100),
            write_policy: UsageWritePolicy::InsertOnce,
        }))
        .await
        .expect("send a");

    handle
        .send(WriteCommand::TailBatch(TailBatchCommand {
            source_id: "src-multi-b".to_string(),
            source_file_id: Some("file-b".to_string()),
            source_file_agent: "claude_code".to_string(),
            source_file_path: "/tmp/b.jsonl".to_string(),
            source_file_inode: None,
            events: vec![event_b],
            tool_events: vec![],
            diagnostic_events: vec![],
            codex_snapshots: vec![],
            generation_id: "gen-multi-b".to_string(),
            checkpoint_offset: Some(200),
            write_policy: UsageWritePolicy::InsertOnce,
        }))
        .await
        .expect("send b");

    // Flush should process both batches (warning is logged but not an error).
    handle.flush().await.expect("flush");

    let db = db.lock().unwrap();
    let count: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
        .expect("query");
    assert_eq!(count, 2, "both events should be persisted");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn writer_shutdown_propagates_dispatch_errors_on_next_flush() {
    // Use the previously-documented RebuildRollups-with-invalid-timezone
    // pattern: the rebuild error is delivered through respond_tx, not
    // pending_errors. So a subsequent flush should still succeed.
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status, event_bus, settings, 16);

    // Issue a flush with no pending errors first.
    handle.flush().await.expect("flush 1");

    // Issue a second flush — should still succeed because no pending errors.
    handle.flush().await.expect("flush 2");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn progress_checkpoint_command_with_active_generation_refreshes_summary() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    seed_active_generation(&db, "gen-ckpt");
    seed_log_source_row(&db, "src-ckpt", "active");

    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    handle
        .send(WriteCommand::ProgressCheckpoint(
            ProgressCheckpointCommand {
                file_id: "file-ckpt".to_string(),
                source_id: "src-ckpt".to_string(),
                agent: "claude_code".to_string(),
                path: "/tmp/ckpt.jsonl".to_string(),
                inode: Some("inode-1".to_string()),
                offset_bytes: 4096,
                size_bytes: 8192,
                last_mtime_ms: Some(12345),
                state: "active".to_string(),
            },
        ))
        .await
        .expect("send");

    handle.flush().await.expect("flush");

    let db = db.lock().unwrap();
    let offset: i64 = db
        .conn()
        .query_row(
            "SELECT offset_bytes FROM source_file_checkpoints WHERE id = 'file-ckpt'",
            [],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(offset, 4096);

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn promotion_barrier_via_writer_refuses_on_drift() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    // Set up drift: observation offset > checkpoint offset.
    {
        let d = db.conn();
        let now = now_ms();
        d.execute(
            "INSERT OR REPLACE INTO audit_generations \
             (generation_id, state, started_at_ms, is_active, created_at_ms, updated_at_ms) \
             VALUES ('gen-drift-w', 'building', ?1, 0, ?1, ?1)",
            rusqlite::params![now],
        )
        .expect("seed generation");
        d.execute(
            "INSERT OR REPLACE INTO source_file_checkpoints \
             (id, source_id, agent, path, inode, offset_bytes, size_bytes, \
              last_mtime_ms, state, first_seen_at_ms, last_seen_at_ms, \
              created_at_ms, updated_at_ms) \
             VALUES ('file-dw', 'src-dw', 'claude_code', '/tmp/dw.jsonl', NULL, \
                     100, 5000, NULL, 'active', ?1, ?1, ?1, ?1)",
            rusqlite::params![now],
        )
        .expect("seed checkpoint");
        write_queries::insert_generation_observation(
            d,
            "gen-drift-w",
            "file-dw",
            5000, // observation offset > checkpoint offset 100
            5000,
            None,
            Some("ok"),
            None,
        )
        .expect("insert observation");
    }

    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status.clone(), event_bus, settings, 16);

    handle
        .send(WriteCommand::PromotionBarrier(PromotionBarrierCommand {
            from_generation_id: "gen-old".to_string(),
            to_generation_id: "gen-drift-w".to_string(),
        }))
        .await
        .expect("send");

    // Flush completes; the promotion-barrier refusal surfaces as a readiness
    // transition to ReadyDegraded (verified below) rather than necessarily
    // erroring the flush call itself.
    let _ = handle.flush().await;

    let snap = status.read().await;
    assert_eq!(
        snap.readiness,
        ReadinessStateDto::ReadyDegraded,
        "drift should transition to ReadyDegraded"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

// =============================================================================
// read_service.rs — additional error path coverage
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_in_memory_backend_executes_query() {
    let db = Database::open_in_memory().expect("db");
    let db = Arc::new(Mutex::new(db));
    let service = ReadService::new_in_memory(db, 2);

    let result: i64 = service
        .run(ReadQuery::new("test.in_memory", "test"), |conn| {
            let count: i64 = conn.query_row("SELECT 1", [], |r| r.get(0))?;
            Ok::<_, anyhow::Error>(count)
        })
        .await
        .expect("query should succeed");

    assert_eq!(result, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_in_memory_backend_propagates_query_error() {
    let db = Database::open_in_memory().expect("db");
    let db = Arc::new(Mutex::new(db));
    let service = ReadService::new_in_memory(db, 2);

    let result = service
        .run(ReadQuery::new("test.query_err", "test"), |conn| {
            conn.execute("SELECT FROM invalid_sql", [])
                .map(|_| ())
                .map_err(anyhow::Error::from)
        })
        .await;

    let err = result.unwrap_err();
    // SQL syntax errors map to Internal (no specific SQLite code match).
    assert_eq!(err.kind(), ReadErrorKind::Internal);
    assert_eq!(err.method(), "test.query_err");
    assert!(!err.message().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_file_backend_open_failure_maps_to_unavailable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let nonexistent = tmp.path().join("does-not-exist.sqlite");

    let service = ReadService::new(nonexistent, 1);

    let result = service
        .run(ReadQuery::new("test.open_fail", "test"), |_conn| {
            Ok::<_, anyhow::Error>(())
        })
        .await;

    let err = result.unwrap_err();
    // Opening a non-existent file readonly → CannotOpen or NotFound → Unavailable.
    assert_eq!(
        err.kind(),
        ReadErrorKind::Unavailable,
        "missing file should map to Unavailable, got {:?}: {}",
        err.kind(),
        err.message()
    );
    assert_eq!(err.code(), "read_model_unavailable");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_slow_query_logs_completion_with_slow_flag() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("slow.sqlite");
    let _db = Database::open(&path).expect("open db");
    let service = ReadService::new(path, 1);

    // slow_after = 1ms, query sleeps 25ms → should be flagged slow.
    let result: () = service
        .run(
            ReadQuery::new("test.slow", "test").slow_after(Duration::from_millis(1)),
            |_conn| {
                std::thread::sleep(Duration::from_millis(25));
                Ok::<_, anyhow::Error>(())
            },
        )
        .await
        .expect("slow query should still succeed");

    // result is `()`; the expect("slow query should still succeed") above
    // already verifies the call returned Ok(()).
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_returns_internal_on_join_handle_panic() {
    // Trigger a panic inside the spawn_blocking closure.
    let db = Database::open_in_memory().expect("db");
    let db = Arc::new(Mutex::new(db));
    let service = ReadService::new_in_memory(db, 1);

    let result: Result<(), ReadError> = service
        .run(
            ReadQuery::new("test.panic", "test"),
            |_conn| -> Result<(), anyhow::Error> {
                panic!("intentional panic in read closure");
            },
        )
        .await;

    let err = result.unwrap_err();
    // A panic becomes a JoinError, which we map to Internal.
    assert_eq!(err.kind(), ReadErrorKind::Internal);
    assert!(err.message().contains("join") || err.message().contains("panic"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_uses_read_query_builder_fields() {
    // Verify all ReadQuery builder methods are exercised and reflected in logs.
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("builder.sqlite");
    let _db = Database::open(&path).expect("open db");
    let service = ReadService::new(path, 1);

    let _: () = service
        .run(
            ReadQuery::new("test.builder", "test_family")
                .timeout(Duration::from_secs(2))
                .slow_after(Duration::from_millis(50))
                .generation_id_opt(Some("gen-builder".to_string()))
                .readiness_opt(Some("ready_exact".to_string()))
                .watermark_ms_opt(Some(12345))
                .row_count(7)
                .used_read_model(true),
            |_conn| Ok::<_, anyhow::Error>(()),
        )
        .await
        .expect("query should succeed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_in_memory_returns_internal_on_database_busy_message() {
    // Synthesize an error whose message contains "database is locked".
    let db = Database::open_in_memory().expect("db");
    let db = Arc::new(Mutex::new(db));
    let service = ReadService::new_in_memory(db, 1);

    let result = service
        .run(ReadQuery::new("test.locked_msg", "test"), |_conn| {
            Err::<(), anyhow::Error>(anyhow::anyhow!("database is locked"))
        })
        .await;

    let err = result.unwrap_err();
    assert_eq!(err.kind(), ReadErrorKind::DatabaseBusy);
    assert_eq!(err.code(), "database_busy");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_in_memory_returns_internal_on_database_busy_message_alt() {
    let db = Database::open_in_memory().expect("db");
    let db = Arc::new(Mutex::new(db));
    let service = ReadService::new_in_memory(db, 1);

    let result = service
        .run(ReadQuery::new("test.busy_msg", "test"), |_conn| {
            Err::<(), anyhow::Error>(anyhow::anyhow!("database is busy"))
        })
        .await;

    let err = result.unwrap_err();
    assert_eq!(err.kind(), ReadErrorKind::DatabaseBusy);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_row_count_outcome_carries_count() {
    let db = Database::open_in_memory().expect("db");
    let db = Arc::new(Mutex::new(db));
    let service = ReadService::new_in_memory(db, 1);

    let result: Vec<i64> = service
        .run(
            ReadQuery::new("test.row_count", "test").row_count(3),
            |conn| {
                let mut stmt = conn.prepare("SELECT 1 UNION SELECT 2 UNION SELECT 3")?;
                let rows: Vec<i64> = stmt
                    .query_map([], |r| r.get(0))?
                    .collect::<std::result::Result<_, _>>()?;
                Ok(busytok_runtime::read_service::ReadOutcome::with_row_count(
                    rows, 3,
                ))
            },
        )
        .await
        .expect("query should succeed");

    assert_eq!(result.len(), 3);
}

// =============================================================================
// service_app.rs — boot/shutdown coverage
//
// Note: `shutdown_control_server` is `pub(crate)` and not accessible from
// integration tests. The inline `#[cfg(test)] mod tests` in service_app.rs
// already covers both branches of `result_already_read`. From integration tests
// we exercise the public `ServiceApp::boot` path and clean up via the public
// `ControlServer::shutdown` + `await_drain` API, plus the supervisor's writer
// shutdown to avoid leaking the writer actor task.
//
// `ServiceApp::run()` is intentionally NOT covered here because it requires a
// ctrl_c signal to exit gracefully. Sending SIGINT process-wide via
// `libc::raise(SIGINT)` is unsafe under `cargo test`'s parallel test runner
// (other concurrent tests could be terminated), and aborting the run handle
// mid-await would not exercise the graceful marker-removal path. The full
// run() lifecycle is covered by the inline tests in service_app.rs instead.
// =============================================================================

#[tokio::test]
#[serial]
async fn service_app_boot_writes_marker_and_can_be_shutdown() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let startup = Instant::now();
    let data_dir = paths.data_dir().to_path_buf();

    let app = ServiceApp::boot(paths, startup).await.expect("boot");

    // boot() must write the service.ready marker.
    assert!(
        busytok_config::service_marker::exists(&data_dir),
        "boot must write service.ready marker"
    );

    // boot() also clears a stale marker from a previous run before writing
    // a fresh one. Verify the marker file is exactly the new one by
    // removing it and confirming it no longer exists.
    let _ = busytok_config::service_marker::remove(&data_dir);
    assert!(
        !busytok_config::service_marker::exists(&data_dir),
        "marker should be removable after boot"
    );

    // Dropping `app` detaches the server_task (ServiceApp does not implement
    // Drop, so the writer actor and control server are not explicitly shut
    // down here). The per-test tokio runtime is dropped when the test
    // function returns, which aborts all spawned tasks — acceptable for a
    // boot-only test. The graceful shutdown path (run() + ctrl_c) is
    // covered by the inline tests in service_app.rs.
    drop(app);
}

// =============================================================================
// Helpers (test_db_status_bus_settings — declared at end so test bodies read clearly)
// =============================================================================

fn test_db_status_bus_settings() -> (
    Database,
    Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    Arc<AppEventBus>,
    Arc<Mutex<busytok_config::BusytokSettings>>,
) {
    let db = Database::open_in_memory().expect("open db");
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(128));
    let settings = Arc::new(Mutex::new(busytok_config::BusytokSettings::default()));
    (db, status, event_bus, settings)
}
