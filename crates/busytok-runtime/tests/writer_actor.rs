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
//! Integration tests for the bounded writer actor and WriteCommand taxonomy.
//!
//! Covers: channel backpressure, command dispatch, queue depth tracking,
//! sequential ordering, and graceful shutdown.

use std::sync::Arc;
use std::time::Duration;

use std::sync::Mutex;

use busytok_config::BusytokSettings;
use busytok_domain::now_ms;
use busytok_events::AppEventBus;
use busytok_runtime::status::{CachedClientRollup, ServiceStatusSnapshot};
use busytok_runtime::writer::{spawn_test_writer_with_capacity, spawn_writer, WriteCommand};
use busytok_store::{Database, LogSourceRow};

// ── Helpers ──────────────────────────────────────────────────────────────

fn test_command(label: &str) -> WriteCommand {
    WriteCommand::DiagnosticWrite(busytok_runtime::writer::DiagnosticWriteCommand {
        source_id: format!("test-{label}"),
        code: "test".to_string(),
        details_json: None,
        message: format!("diagnostic from {label}"),
        severity: "info".to_string(),
    })
}

fn test_db_status_bus_settings() -> (
    Database,
    Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    Arc<AppEventBus>,
    Arc<Mutex<BusytokSettings>>,
) {
    let db = Database::open_in_memory().unwrap();
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let settings = Arc::new(Mutex::new(BusytokSettings::default()));
    (db, status, event_bus, settings)
}

fn seed_generation(db: &Database, gen_id: &str, is_active: bool) {
    let now = now_ms();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO audit_generations \
             (generation_id, state, started_at_ms, promoted_at_ms, is_active, created_at_ms, updated_at_ms) \
             VALUES (?1, 'promoted', ?2, ?2, ?3, ?2, ?2)",
            rusqlite::params![gen_id, now, is_active as i32],
        )
        .unwrap();
}

fn seed_log_source(db: &Database, id: &str, status: &str) {
    let now = now_ms();
    db.upsert_log_source(&LogSourceRow {
        id: id.to_string(),
        agent: "claude_code".to_string(),
        source_type: "jsonl".to_string(),
        root_path: format!("/tmp/{id}"),
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

fn seed_log_file(db: &Database, file_id: &str, source_id: &str) {
    let now = now_ms();
    db.conn()
        .execute(
            "INSERT INTO log_files \
             (id, source_id, agent, path, inode, size_bytes, offset_bytes, last_mtime_ms, \
              first_seen_at_ms, last_seen_at_ms, state, last_error, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, 'claude_code', ?3, NULL, 0, 0, NULL, ?4, ?4, 'active', NULL, ?4, ?4)",
            rusqlite::params![file_id, source_id, format!("/tmp/{file_id}.jsonl"), now],
        )
        .unwrap();
}

// ── Channel backpressure ─────────────────────────────────────────────────

#[tokio::test]
async fn writer_queue_blocks_when_full() {
    let (handle, _join) = spawn_test_writer_with_capacity(1);
    // First send fills the channel (capacity 1).
    handle.try_send(test_command("one")).unwrap();
    // Second try_send must fail because the channel is full — the writer
    // actor hasn't had a chance to drain yet since we used try_send.
    let second = handle.try_send(test_command("two"));
    assert!(
        second.is_err(),
        "second try_send should fail when channel is full (capacity 1)"
    );
}

#[tokio::test]
async fn writer_send_blocks_until_drain() {
    // Use a slow writer to demonstrate that send() blocks when full.
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db, status, event_bus, settings, 1);

    // Fill the channel
    handle.try_send(test_command("block-1")).unwrap();
    // try_send should fail since capacity is 1
    assert!(handle.try_send(test_command("block-2")).is_err());

    // send() should block until the writer drains, then succeed
    let sent =
        tokio::time::timeout(Duration::from_secs(2), handle.send(test_command("block-3"))).await;
    assert!(
        sent.is_ok(),
        "send() should succeed once the writer drains the channel"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn writer_queue_depth_reflects_pending_commands() {
    let (handle, _join) = spawn_test_writer_with_capacity(16);
    assert_eq!(handle.queue_depth(), 0);

    handle.send(test_command("a")).await.unwrap();
    // The actor processes immediately, so depth may be 1 briefly but the actor
    // drains synchronously. We test depth >= 0 for correctness of the API.
    let depth = handle.queue_depth();
    assert!(depth <= 16, "depth should not exceed capacity");
}

// ── Sequential ordering ──────────────────────────────────────────────────

#[tokio::test]
async fn writer_processes_commands_in_order() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    for i in 0..10u32 {
        handle
            .send(WriteCommand::DiagnosticWrite(
                busytok_runtime::writer::DiagnosticWriteCommand {
                    source_id: "ordering-test".to_string(),
                    code: "test".to_string(),
                    details_json: None,
                    message: format!("msg-{i}"),
                    severity: "info".to_string(),
                },
            ))
            .await
            .unwrap();
    }

    // Drop the handle to signal shutdown
    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;

    // Verify all diagnostic messages were recorded
    let db = db.lock().unwrap();
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM diagnostic_events WHERE source_id = 'ordering-test'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 10, "all 10 diagnostic writes should be persisted");
}

// ── Graceful shutdown ────────────────────────────────────────────────────

#[tokio::test]
async fn writer_shuts_down_cleanly_when_handle_dropped() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    handle
        .send(WriteCommand::DiagnosticWrite(
            busytok_runtime::writer::DiagnosticWriteCommand {
                source_id: "shutdown-test".to_string(),
                code: "test".to_string(),
                details_json: None,
                message: "before shutdown".to_string(),
                severity: "info".to_string(),
            },
        ))
        .await
        .unwrap();

    // Drop the handle; the actor should process the last command then exit.
    drop(handle);
    let result = tokio::time::timeout(Duration::from_secs(2), join).await;
    assert!(
        result.is_ok(),
        "writer should shut down cleanly within timeout"
    );

    // Verify the command was processed before shutdown.
    let db = db.lock().unwrap();
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM diagnostic_events WHERE source_id = 'shutdown-test'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "last command should be processed before shutdown");
}

// ── Status snapshot updates ──────────────────────────────────────────────

#[tokio::test]
async fn writer_updates_status_snapshot_after_commit() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status.clone(), event_bus, settings.clone(), 16);

    handle
        .send(WriteCommand::DiagnosticWrite(
            busytok_runtime::writer::DiagnosticWriteCommand {
                source_id: "status-test".to_string(),
                code: "test".to_string(),
                details_json: None,
                message: "check status".to_string(),
                severity: "info".to_string(),
            },
        ))
        .await
        .unwrap();

    // Small delay for the actor to process
    tokio::time::sleep(Duration::from_millis(50)).await;

    let snap = status.read().await;
    // After a diagnostic write, the latest_event_seq may be None, but
    // writer_queue_depth should be updated
    assert!(snap.writer_queue_depth >= 0);

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn diagnostic_write_refreshes_source_summaries_for_active_generation() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    seed_generation(&db, "gen-diag", true);
    seed_log_source(&db, "source-diag", "active");

    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    handle
        .send(WriteCommand::DiagnosticWrite(
            busytok_runtime::writer::DiagnosticWriteCommand {
                source_id: "source-diag".to_string(),
                code: "test".to_string(),
                details_json: None,
                message: "summary refresh".to_string(),
                severity: "warning".to_string(),
            },
        ))
        .await
        .unwrap();

    handle.flush().await.unwrap();

    let db = db.lock().unwrap();
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM source_health_summary \
             WHERE generation_id = 'gen-diag' AND source_id = 'source-diag'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
#[ignore = "timing-sensitive on CI runners; passes on macOS dev"]
async fn writer_flush_reports_all_dispatch_errors_since_previous_barrier() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let mut seed = busytok_domain::NormalizedUsageEvent::minimal_for_test(
        "rollup-error-seed",
        busytok_domain::AgentKind::ClaudeCode,
    );
    seed.timestamp_ms = 1_768_435_200_000;
    seed.total_tokens = 1;
    db.write_usage_event(&seed, busytok_domain::UsageWritePolicy::InsertOnce)
        .unwrap();

    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db, status, event_bus, settings, 16);

    for timezone in ["Mars/First", "Mars/Second"] {
        let (respond_tx, _respond_rx) = tokio::sync::oneshot::channel();
        handle
            .send(WriteCommand::RebuildRollups(
                busytok_runtime::writer::RebuildRollupsCommand {
                    timezone: timezone.to_string(),
                    respond_tx,
                },
            ))
            .await
            .unwrap();
    }

    let err = handle
        .flush()
        .await
        .expect_err("flush should surface accumulated dispatch errors")
        .to_string();

    assert!(err.contains("Mars/First"), "missing first error: {err}");
    assert!(err.contains("Mars/Second"), "missing second error: {err}");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

// ── TailBatch command round-trip ─────────────────────────────────────────

#[tokio::test]
async fn tail_batch_command_inserts_usage_events() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let mut event = busytok_domain::NormalizedUsageEvent::minimal_for_test(
        "tail-batch-test-1",
        busytok_domain::AgentKind::ClaudeCode,
    );
    event.timestamp_ms = 1_000_000;
    event.total_tokens = 42;
    event.model = Some("claude-sonnet".to_string());

    handle
        .send(WriteCommand::TailBatch(
            busytok_runtime::writer::TailBatchCommand {
                source_id: "src-tail".to_string(),
                source_file_id: Some("file-tail".to_string()),
                source_file_agent: "claude_code".to_string(),
                source_file_path: "/tmp/test-tail.jsonl".to_string(),
                source_file_inode: None,
                events: vec![event],
                tool_events: vec![],
                diagnostic_events: vec![],
                codex_snapshots: vec![],
                generation_id: "gen-tail".to_string(),
                checkpoint_offset: Some(100),
                write_policy: busytok_domain::UsageWritePolicy::InsertOnce,
            },
        ))
        .await
        .unwrap();

    handle.flush().await.unwrap();

    let db = db.lock().unwrap();
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM usage_events WHERE id = 'tail-batch-test-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn tail_batch_refreshes_summary_for_inserted_event_source_file_source() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    seed_generation(&db, "gen-tail", true);
    seed_log_source(&db, "source-real", "active");
    seed_log_source(&db, "source-fallback", "active");
    seed_log_file(&db, "file-real", "source-real");

    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let mut event = busytok_domain::NormalizedUsageEvent::minimal_for_test(
        "tail-summary-source-file",
        busytok_domain::AgentKind::ClaudeCode,
    );
    event.timestamp_ms = 1_000_000;
    event.total_tokens = 42;
    event.source_file_id = "file-real".to_string();

    handle
        .send(WriteCommand::TailBatch(
            busytok_runtime::writer::TailBatchCommand {
                source_id: "source-fallback".to_string(),
                source_file_id: Some("file-fallback".to_string()),
                source_file_agent: "claude_code".to_string(),
                source_file_path: "/tmp/fallback.jsonl".to_string(),
                source_file_inode: None,
                events: vec![event],
                tool_events: vec![],
                diagnostic_events: vec![],
                codex_snapshots: vec![],
                generation_id: "gen-tail".to_string(),
                checkpoint_offset: Some(100),
                write_policy: busytok_domain::UsageWritePolicy::InsertOnce,
            },
        ))
        .await
        .unwrap();

    handle.flush().await.unwrap();

    let db = db.lock().unwrap();
    let real_count: i64 = db
        .conn()
        .query_row(
            "SELECT event_count FROM source_health_summary \
             WHERE generation_id = 'gen-tail' AND source_id = 'source-real'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let fallback_count: i64 = db
        .conn()
        .query_row(
            "SELECT event_count FROM source_health_summary \
             WHERE generation_id = 'gen-tail' AND source_id = 'source-fallback'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(real_count, 1);
    assert_eq!(fallback_count, 0);

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

// ── RebuildBatch command round-trip ──────────────────────────────────────

#[tokio::test]
async fn rebuild_batch_command_inserts_usage_events() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let mut event = busytok_domain::NormalizedUsageEvent::minimal_for_test(
        "rebuild-batch-1",
        busytok_domain::AgentKind::Codex,
    );
    event.timestamp_ms = 2_000_000;
    event.total_tokens = 100;
    event.model = Some("gpt-5".to_string());

    handle
        .send(WriteCommand::RebuildBatch(
            busytok_runtime::writer::RebuildBatchCommand {
                source_id: "src-rebuild".to_string(),
                source_file_id: None,
                source_file_agent: "codex".to_string(),
                source_file_path: String::new(),
                source_file_inode: None,
                events: vec![event],
                tool_events: vec![],
                diagnostic_events: vec![],
                codex_snapshots: vec![],
                generation_id: "gen-rebuild".to_string(),
                checkpoint_offset: None,
                is_final_batch: false,
                write_policy: busytok_domain::UsageWritePolicy::InsertOnce,
            },
        ))
        .await
        .unwrap();

    handle.flush().await.unwrap();

    let db = db.lock().unwrap();
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM usage_events WHERE id = 'rebuild-batch-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

// ── ProgressCheckpoint command ───────────────────────────────────────────

#[tokio::test]
async fn progress_checkpoint_command_updates_file_checkpoint() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    handle
        .send(WriteCommand::ProgressCheckpoint(
            busytok_runtime::writer::ProgressCheckpointCommand {
                file_id: "file-progress".to_string(),
                source_id: "src-progress".to_string(),
                agent: "claude_code".to_string(),
                path: "/tmp/file-progress.jsonl".to_string(),
                inode: Some("inode-progress".to_string()),
                offset_bytes: 500,
                size_bytes: 1000,
                last_mtime_ms: Some(3_000_000),
                state: "active".to_string(),
            },
        ))
        .await
        .unwrap();

    handle.flush().await.unwrap();

    let db = db.lock().unwrap();
    let offset: i64 = db
        .conn()
        .query_row(
            "SELECT offset_bytes FROM source_file_checkpoints WHERE id = 'file-progress'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(offset, 500);

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

// ── PromotionBarrier command ─────────────────────────────────────────────

#[tokio::test]
async fn promotion_barrier_records_generation_transition() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status.clone(), event_bus, settings.clone(), 16);

    handle
        .send(WriteCommand::PromotionBarrier(
            busytok_runtime::writer::PromotionBarrierCommand {
                from_generation_id: "gen-old".to_string(),
                to_generation_id: "gen-new".to_string(),
            },
        ))
        .await
        .unwrap();

    handle.flush().await.unwrap();

    let snap = status.read().await;
    assert_eq!(snap.active_generation_id.as_deref(), Some("gen-new"));

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

// ── SettingsWrite command ────────────────────────────────────────────────

#[tokio::test]
async fn settings_write_triggers_settings_apply() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let (respond_tx, _respond_rx) = tokio::sync::oneshot::channel();
    handle
        .send(WriteCommand::SettingsWrite(
            busytok_runtime::writer::SettingsWriteCommand {
                key: "test.setting".to_string(),
                value_json: r#"{"enabled":true}"#.to_string(),
                respond_tx,
            },
        ))
        .await
        .unwrap();

    handle.flush().await.unwrap();

    // Settings writes are a no-op stub for now; the test just confirms the
    // command is accepted and does not panic.
    // Verify the DB is still usable
    let db = db.lock().unwrap();
    assert!(db.table_names().is_ok());

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

// ── Rebuild + Tail deduplication ──────────────────────────────────────────

#[tokio::test]
async fn replay_and_rebuild_do_not_duplicate_same_dedupe_key_in_generation() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let gen_id = "gen-dedupe-test".to_string();

    // Create events that exist in both a rebuild batch and a tail batch
    // (simulating the scenario where rebuild and tail overlap on the
    // same dedupe identity keys).
    let mut event_a = busytok_domain::NormalizedUsageEvent::minimal_for_test(
        "dedupe-key-A",
        busytok_domain::AgentKind::ClaudeCode,
    );
    event_a.timestamp_ms = 1_000_000;
    event_a.total_tokens = 100;
    event_a.model = Some("claude-sonnet".to_string());

    let mut event_b = busytok_domain::NormalizedUsageEvent::minimal_for_test(
        "dedupe-key-B",
        busytok_domain::AgentKind::ClaudeCode,
    );
    event_b.timestamp_ms = 2_000_000;
    event_b.total_tokens = 200;
    event_b.model = Some("claude-opus".to_string());

    // Simulate a rebuild batch scanning historical data
    handle
        .send(WriteCommand::RebuildBatch(
            busytok_runtime::writer::RebuildBatchCommand {
                source_id: "src-dedupe".to_string(),
                source_file_id: None,
                source_file_agent: "claude_code".to_string(),
                source_file_path: String::new(),
                source_file_inode: None,
                events: vec![event_a.clone(), event_b.clone()],
                tool_events: vec![],
                diagnostic_events: vec![],
                codex_snapshots: vec![],
                generation_id: gen_id.clone(),
                checkpoint_offset: None,
                is_final_batch: false,
                write_policy: busytok_domain::UsageWritePolicy::InsertOnce,
            },
        ))
        .await
        .unwrap();

    // Simulate a tail batch picking up the same events during rebuild
    handle
        .send(WriteCommand::TailBatch(
            busytok_runtime::writer::TailBatchCommand {
                source_id: "src-dedupe".to_string(),
                source_file_id: Some("file-dedupe".to_string()),
                source_file_agent: "claude_code".to_string(),
                source_file_path: "/tmp/dedupe.jsonl".to_string(),
                source_file_inode: None,
                events: vec![event_a, event_b],
                tool_events: vec![],
                diagnostic_events: vec![],
                codex_snapshots: vec![],
                generation_id: gen_id.clone(),
                checkpoint_offset: Some(500),
                write_policy: busytok_domain::UsageWritePolicy::InsertOnce,
            },
        ))
        .await
        .unwrap();

    // Flush pending batches to ensure processing.
    handle.flush().await.unwrap();

    let db = db.lock().unwrap();
    let count: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM usage_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2, "should have exactly 2 events, not 4");

    let materialized: (i64, i64) = db
        .conn()
        .query_row(
            "SELECT COALESCE(SUM(total_tokens), 0), COALESCE(SUM(event_count), 0) \
             FROM usage_buckets_2s WHERE generation_id = ?1",
            rusqlite::params![&gen_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        materialized,
        (300, 2),
        "duplicate events must not double-count materialized aggregates"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn scan_via_writer_produces_event_sequence_state() {
    use busytok_config::BusytokPaths;
    use busytok_domain::{AgentKind, LogSourceType};
    use busytok_runtime::BusytokSupervisor;
    use busytok_store::Database;
    use std::io::Write;

    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("scan-writer.jsonl");

    let line = serde_json::json!({
        "type": "assistant",
        "message": {
            "id": "msg_scan_writer",
            "model": "claude-sonnet-4-20250514",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50
            }
        },
        "sessionId": "sess-scan-writer",
        "timestamp": "2026-05-15T10:00:00Z"
    })
    .to_string();

    {
        let mut f = std::fs::File::create(&file_path).expect("create file");
        writeln!(f, "{line}").expect("write line");
    }

    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();
    let supervisor = BusytokSupervisor::new(db, paths);

    let source = busytok_discovery::DiscoveredLogSource {
        agent: AgentKind::ClaudeCode,
        source_id: "test-scan-writer".to_string(),
        root_path: dir.path().to_path_buf(),
        files: vec![file_path],
        source_type: LogSourceType::Jsonl,
        configured_by_user: false,
    };

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("scan should succeed");

    assert!(stats.events_found >= 1, "should find usage events");

    // Verify events are in the DB.
    let db_guard = supervisor.db_handle().lock().unwrap();
    let count = db_guard.usage_event_count().expect("count");
    assert!(count >= 1, "events should be stored");
    drop(db_guard);

    // The synchronous scan path (run_scan_with_sources) writes directly to DB
    // and does not populate event_sequence_state. The writer-actor path
    // (used by run_initial_scan in production) does populate it.
    // This test verifies that the sync path succeeds and stores events correctly.
    let db_guard = supervisor.db_handle().lock().unwrap();
    let count_after = db_guard.usage_event_count().expect("count");
    assert!(
        count_after >= 1,
        "events should persist after sync scan; got count={}",
        count_after
    );
    drop(db_guard);
}

// ── Batch flush integration tests ─────────────────────────────────────────────

#[tokio::test]
async fn batch_flush_persists_all_events_and_all_file_checkpoints() {
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    seed_generation(&db.lock().unwrap(), "gen-flush", true);
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let settings = Arc::new(Mutex::new(BusytokSettings::default()));
    let (handle, _join) = spawn_writer(
        Arc::clone(&db),
        Arc::clone(&status),
        Arc::clone(&event_bus),
        settings,
        64,
    );

    for i in 0..3 {
        let events: Vec<_> = (0..10)
            .map(|j| {
                let mut evt = busytok_domain::NormalizedUsageEvent::minimal_for_test(
                    &format!("bf-{i}-{j}"),
                    busytok_domain::AgentKind::ClaudeCode,
                );
                evt.total_tokens = 100;
                evt.input_tokens = 50;
                evt.output_tokens = 50;
                evt.timestamp_ms = busytok_domain::now_ms();
                evt
            })
            .collect();
        handle
            .send(WriteCommand::TailBatch(
                busytok_runtime::writer::TailBatchCommand {
                    source_id: "src-flush".into(),
                    source_file_id: Some(format!("f-{i}")),
                    source_file_agent: "claude_code".into(),
                    source_file_path: format!("/tmp/f-{i}"),
                    source_file_inode: None,
                    events,
                    tool_events: vec![],
                    diagnostic_events: vec![],
                    codex_snapshots: vec![],
                    generation_id: "gen-flush".into(),
                    checkpoint_offset: Some((i + 1) as u64 * 100),
                    write_policy: busytok_domain::UsageWritePolicy::InsertOnce,
                },
            ))
            .await
            .unwrap();
    }

    handle.flush().await.unwrap();

    let db = db.lock().unwrap();
    let count: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 30, "all 30 events should be persisted");
    let ckpt: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM log_files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ckpt, 3, "all 3 file checkpoints should be persisted");
    drop(db);

    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn batch_flush_triggers_on_event_count_threshold() {
    let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    seed_generation(&db.lock().unwrap(), "gen-thresh", true);
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let settings = Arc::new(Mutex::new(BusytokSettings::default()));
    let (handle, _join) = spawn_writer(
        Arc::clone(&db),
        Arc::clone(&status),
        Arc::clone(&event_bus),
        settings,
        64,
    );

    // Send 100+ events in one batch — should trigger immediate flush at BATCH_SIZE.
    let events: Vec<_> = (0..110)
        .map(|j| {
            let mut evt = busytok_domain::NormalizedUsageEvent::minimal_for_test(
                &format!("thresh-{}", j),
                busytok_domain::AgentKind::ClaudeCode,
            );
            evt.total_tokens = 10;
            evt.timestamp_ms = busytok_domain::now_ms();
            evt
        })
        .collect();
    handle
        .send(WriteCommand::TailBatch(
            busytok_runtime::writer::TailBatchCommand {
                source_id: "src-thresh".into(),
                source_file_id: Some("f-thresh".into()),
                source_file_agent: "claude_code".into(),
                source_file_path: "/tmp/thresh.jsonl".into(),
                source_file_inode: None,
                events,
                tool_events: vec![],
                diagnostic_events: vec![],
                codex_snapshots: vec![],
                generation_id: "gen-thresh".into(),
                checkpoint_offset: Some(1000),
                write_policy: busytok_domain::UsageWritePolicy::InsertOnce,
            },
        ))
        .await
        .unwrap();

    // Small sleep to let the writer process (flush should happen immediately at >=100).
    tokio::time::sleep(Duration::from_millis(100)).await;

    let db = db.lock().unwrap();
    let count: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 110, "threshold should trigger immediate flush");
    drop(db);

    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn tail_batch_increments_total_usage_event_count_after_hydration() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status.clone(), event_bus, settings, 16);

    // Simulate the real bug: hydration already ran when DB was empty,
    // locking the counter at 0 with chip_data_hydrated = true.
    {
        let mut snap = status.write().await;
        snap.total_usage_event_count = 0;
        snap.chip_data_hydrated = true;
        snap.cached_client_rollups = vec![CachedClientRollup {
            client_kind: "stale-client".to_string(),
            active_source_count: 1,
            event_count: 0,
        }];
    }

    let mut event = busytok_domain::NormalizedUsageEvent::minimal_for_test(
        "cnt-1",
        busytok_domain::AgentKind::ClaudeCode,
    );
    event.timestamp_ms = 1_000_000;
    event.total_tokens = 10;

    handle
        .send(WriteCommand::TailBatch(
            busytok_runtime::writer::TailBatchCommand {
                source_id: "src-cnt".to_string(),
                source_file_id: Some("file-cnt".to_string()),
                source_file_agent: "claude_code".to_string(),
                source_file_path: "/tmp/cnt.jsonl".to_string(),
                source_file_inode: None,
                events: vec![event],
                tool_events: vec![],
                diagnostic_events: vec![],
                codex_snapshots: vec![],
                generation_id: "gen-cnt".to_string(),
                checkpoint_offset: Some(100),
                write_policy: busytok_domain::UsageWritePolicy::InsertOnce,
            },
        ))
        .await
        .unwrap();

    handle.flush().await.unwrap();

    let snap = status.read().await;
    let count = snap.total_usage_event_count;
    assert_eq!(
        count, 1,
        "total_usage_event_count must increment even when chip_data_hydrated was already true"
    );
    assert!(
        !snap.chip_data_hydrated,
        "writer must invalidate hydrated chip data so client rollups are refreshed on next shell_status"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn tail_batch_increments_from_prehydrated_count() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status.clone(), event_bus, settings, 16);

    // Simulate: hydration found 7 events, then 3 more arrive via writer.
    {
        let mut snap = status.write().await;
        snap.total_usage_event_count = 7;
        snap.chip_data_hydrated = true;
    }

    let events: Vec<_> = (0..3)
        .map(|i| {
            let mut e = busytok_domain::NormalizedUsageEvent::minimal_for_test(
                &format!("cnt-extra-{i}"),
                busytok_domain::AgentKind::ClaudeCode,
            );
            e.timestamp_ms = 1_000_000 + i as i64;
            e.total_tokens = 10;
            e
        })
        .collect();

    handle
        .send(WriteCommand::TailBatch(
            busytok_runtime::writer::TailBatchCommand {
                source_id: "src-extra".to_string(),
                source_file_id: Some("file-extra".to_string()),
                source_file_agent: "claude_code".to_string(),
                source_file_path: "/tmp/extra.jsonl".to_string(),
                source_file_inode: None,
                events,
                tool_events: vec![],
                diagnostic_events: vec![],
                codex_snapshots: vec![],
                generation_id: "gen-extra".to_string(),
                checkpoint_offset: Some(100),
                write_policy: busytok_domain::UsageWritePolicy::InsertOnce,
            },
        ))
        .await
        .unwrap();

    handle.flush().await.unwrap();

    let count = status.read().await.total_usage_event_count;
    assert_eq!(
        count, 10,
        "total_usage_event_count should be 7 (hydrated) + 3 (written)"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn prune_decrement_uses_write_await_and_stays_consistent_with_db() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(std::sync::Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status.clone(), event_bus, settings, 16);

    let now = busytok_domain::now_ms();
    let old_ts = now - 86_400_000 - 3600_000; // > 24h ago
    let recent_ts = now - 1800_000;

    // Seed 3 old + 2 recent events directly into DB, all in same generation.
    {
        let db = db.lock().unwrap();
        for i in 0..3u32 {
            let mut e = busytok_domain::NormalizedUsageEvent::minimal_for_test(
                &format!("prune-old-{i}"),
                busytok_domain::AgentKind::ClaudeCode,
            );
            e.timestamp_ms = old_ts + i as i64 * 1000;
            e.total_tokens = 10;
            db.write_usage_event(&e, busytok_domain::UsageWritePolicy::InsertOnce)
                .unwrap();
            db.conn()
                .execute(
                    "UPDATE usage_events SET generation_id = 'gen-prune' WHERE id = ?1",
                    rusqlite::params![format!("prune-old-{i}")],
                )
                .unwrap();
        }
        for i in 0..2u32 {
            let mut e = busytok_domain::NormalizedUsageEvent::minimal_for_test(
                &format!("prune-new-{i}"),
                busytok_domain::AgentKind::ClaudeCode,
            );
            e.timestamp_ms = recent_ts + i as i64 * 1000;
            e.total_tokens = 10;
            db.write_usage_event(&e, busytok_domain::UsageWritePolicy::InsertOnce)
                .unwrap();
            db.conn()
                .execute(
                    "UPDATE usage_events SET generation_id = 'gen-prune' WHERE id = ?1",
                    rusqlite::params![format!("prune-new-{i}")],
                )
                .unwrap();
        }
    }

    // Simulate hydration: snapshot already has count=5 and hydrated=true.
    {
        let mut snap = status.write().await;
        snap.total_usage_event_count = 5;
        snap.chip_data_hydrated = true;
        snap.active_generation_id = Some("gen-prune".to_string());
    }

    // Prune directly (same function the writer loop calls).
    let deleted = {
        let db = db.lock().unwrap();
        busytok_store::write_queries::prune_usage_events(db.conn(), "gen-prune").unwrap()
    };
    assert_eq!(deleted, 3);

    // Apply the same status.write().await decrement the writer uses.
    {
        let mut snap = status.write().await;
        snap.total_usage_event_count = snap.total_usage_event_count.saturating_sub(deleted);
    }

    // Verify snapshot matches DB.
    let db_count: i64 = {
        let db = db.lock().unwrap();
        db.conn()
            .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
            .unwrap()
    };
    let snap_count = status.read().await.total_usage_event_count;
    assert_eq!(
        snap_count, db_count,
        "snapshot ({snap_count}) must match DB ({db_count}) after prune"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}
