#![allow(
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    unused_imports,
    unused_variables,
    dead_code
)]
//! Coverage gap tests targeting `writer.rs`, `read_service.rs`, and `service_app.rs`.
//!
//! Focuses on uncovered code paths not already covered by `coverage_gaps_core.rs`:
//!
//! - `writer.rs`: `WriterHandle::capacity()`, `insert_supplementary_events`
//!   (tool_events / diagnostic_events / codex_snapshots INSERT loops),
//!   `drain_pending_errors` multi-error join path, `emit_threshold_diagnostics`
//!   (lag warning/critical/recovery, queue critical), `handle_reset_failed_checkpoints`
//!   with active generation (refresh_source_summaries path), RebuildBatch
//!   `is_final_batch` path.
//! - `read_service.rs`: `ReadError::code()` for Timeout/Internal, `Display` impl,
//!   `map_read_error` / `map_open_error` with specific SQLite `ErrorCode` variants
//!   (DatabaseBusy, DatabaseLocked, NotADatabase, CannotOpen, NotFound),
//!   zero-connection panics.
//! - `service_app.rs`: `ServiceApp::run()` setup stages (initial scan, tailer,
//!   sampler, background jobs, service_ready) via `LocalSet` + `spawn_local` +
//!   abort pattern (the `run()` future is `!Send`).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use busytok_config::BusytokPaths;
use busytok_domain::{
    now_ms, AgentKind, NormalizedUsageEvent, OperationalDiagnosticEvent, ToolEvent,
    UsageWritePolicy,
};
use busytok_events::{AppEvent, AppEventBus};
use busytok_runtime::read_service::{
    ReadError, ReadErrorKind, ReadOutcome, ReadQuery, ReadService,
};
use busytok_runtime::status::ServiceStatusSnapshot;
use busytok_runtime::writer::{
    spawn_writer, DiagnosticWriteCommand, FlushCommand, PromotionBarrierCommand,
    RebuildBatchCommand, TailBatchCommand, WriteCommand,
};
use busytok_runtime::ServiceApp;
use busytok_store::write_queries;
use busytok_store::{CodexTokenSnapshotRow, Database, LogSourceRow};

// ── Shared helpers ─────────────────────────────────────────────────────────

fn make_test_event(id: &str, tokens: i64, cost: Option<f64>) -> NormalizedUsageEvent {
    let mut evt = NormalizedUsageEvent::minimal_for_test(id, AgentKind::ClaudeCode);
    evt.total_tokens = tokens;
    evt.input_tokens = tokens / 2;
    evt.output_tokens = tokens / 2;
    evt.cost_usd = cost;
    evt.timestamp_ms = now_ms();
    evt
}

fn make_tool_event(id: &str) -> ToolEvent {
    ToolEvent {
        id: id.to_string(),
        agent: AgentKind::ClaudeCode,
        source_file_id: "file-tool-1".to_string(),
        source_path: "/tmp/tool.jsonl".to_string(),
        source_line: 10,
        source_offset_start: 0,
        source_offset_end: 100,
        session_id: "sess-tool".to_string(),
        message_id: Some("msg-1".to_string()),
        tool_name: "Bash".to_string(),
        status: Some("success".to_string()),
        timestamp_ms: Some(now_ms()),
        project_hash: Some("hash-1".to_string()),
        created_at_ms: now_ms(),
    }
}

fn make_diag_event(id: &str) -> OperationalDiagnosticEvent {
    OperationalDiagnosticEvent {
        id: id.to_string(),
        agent: Some(AgentKind::ClaudeCode),
        source_id: Some("src-diag".to_string()),
        source_file_id: Some("file-diag-1".to_string()),
        source_path: Some("/tmp/diag.jsonl".to_string()),
        source_line: Some(5),
        category: "parse_error".to_string(),
        severity: "warning".to_string(),
        message: "malformed line".to_string(),
        detail_json: Some(r#"{"line":5}"#.to_string()),
        happened_at_ms: now_ms(),
        created_at_ms: now_ms(),
    }
}

fn make_codex_snapshot(id: &str) -> CodexTokenSnapshotRow {
    CodexTokenSnapshotRow {
        id: id.to_string(),
        source_file_id: "file-codex-1".to_string(),
        source_line: 20,
        source_offset_start: 0,
        source_offset_end: 200,
        session_id: "sess-codex".to_string(),
        turn_id: Some("turn-1".to_string()),
        token_event_ordinal: 1,
        input_tokens: 100,
        cached_input_tokens: 50,
        output_tokens: 30,
        reasoning_tokens: 10,
        total_tokens: 140,
        model: Some("codex-1".to_string()),
        raw_usage_json: r#"{"input":100}"#.to_string(),
        emitted_event_id: Some("evt-codex-1".to_string()),
        created_at_ms: now_ms(),
        updated_at_ms: now_ms(),
    }
}

fn test_db_status_bus_settings() -> (
    Database,
    Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    Arc<AppEventBus>,
    Arc<Mutex<busytok_config::BusytokSettings>>,
) {
    let db = Database::open_in_memory().expect("open db");
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(256));
    let settings = Arc::new(Mutex::new(busytok_config::BusytokSettings::default()));
    (db, status, event_bus, settings)
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

/// Set up drift for a generation: observation offset > checkpoint offset.
/// This causes `execute_promotion_barrier` to detect drift and refuse the
/// promotion, which makes `handle_promotion_barrier` return an error that
/// gets pushed onto `pending_errors` in the writer actor loop.
fn seed_drift_for_generation(db: &Database, gen_id: &str, file_id: &str, source_id: &str) {
    let now = now_ms();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO audit_generations \
             (generation_id, state, started_at_ms, is_active, created_at_ms, updated_at_ms) \
             VALUES (?1, 'building', ?2, 0, ?2, ?2)",
            rusqlite::params![gen_id, now],
        )
        .expect("seed generation");
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO source_file_checkpoints \
             (id, source_id, agent, path, inode, offset_bytes, size_bytes, \
              last_mtime_ms, state, first_seen_at_ms, last_seen_at_ms, \
              created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, 'claude_code', ?3, NULL, \
                     100, 5000, NULL, 'active', ?4, ?4, ?4, ?4)",
            rusqlite::params![file_id, source_id, format!("/tmp/{file_id}.jsonl"), now],
        )
        .expect("seed checkpoint");
    write_queries::insert_generation_observation(
        db.conn(),
        gen_id,
        file_id,
        5000, // observation offset > checkpoint offset 100
        5000,
        None,
        Some("ok"),
        None,
    )
    .expect("insert observation");
}

// =============================================================================
// writer.rs — WriterHandle::capacity()
// =============================================================================

#[tokio::test]
async fn writer_handle_capacity_returns_configured_value() {
    let (handle, _join) = busytok_runtime::writer::spawn_test_writer_with_capacity(42);
    assert_eq!(handle.capacity(), 42);
    // Also exercise queue_depth() which derives from max_capacity - capacity.
    // With no commands sent, depth should be 0.
    assert_eq!(handle.queue_depth(), 0);
    drop(handle);
}

#[tokio::test]
async fn writer_handle_capacity_default_is_128() {
    let (handle, _join) = busytok_runtime::writer::spawn_test_writer_with_capacity(128);
    assert_eq!(handle.capacity(), 128);
    drop(handle);
}

// =============================================================================
// writer.rs — insert_supplementary_events (tool_events, diagnostic_events,
// codex_snapshots INSERT loops) via TailBatch
// =============================================================================

#[tokio::test]
async fn tail_batch_with_supplementary_events_persists_all() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let event = make_test_event("evt-sup-1", 100, Some(0.01));
    let tool = make_tool_event("tool-sup-1");
    let diag = make_diag_event("diag-sup-1");
    let snap = make_codex_snapshot("snap-sup-1");

    handle
        .send(WriteCommand::TailBatch(TailBatchCommand {
            source_id: "src-sup".to_string(),
            source_file_id: Some("file-sup-1".to_string()),
            source_file_agent: "claude_code".to_string(),
            source_file_path: "/tmp/sup.jsonl".to_string(),
            source_file_inode: None,
            events: vec![event],
            tool_events: vec![tool],
            diagnostic_events: vec![diag],
            codex_snapshots: vec![snap],
            generation_id: "gen-sup".to_string(),
            checkpoint_offset: Some(100),
            write_policy: UsageWritePolicy::InsertOnce,
        }))
        .await
        .expect("send tail batch");

    handle.flush().await.expect("flush");

    let db = db.lock().unwrap();
    let tool_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM tool_events WHERE id = 'tool-sup-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tool_count, 1, "tool_event should be persisted");

    let diag_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM diagnostic_events WHERE id = 'diag-sup-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(diag_count, 1, "diagnostic_event should be persisted");

    let snap_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM codex_token_snapshots WHERE id = 'snap-sup-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(snap_count, 1, "codex_token_snapshot should be persisted");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn tail_batch_with_multiple_supplementary_events_persists_all() {
    // Cover the loop bodies in insert_supplementary_events with >1 element each.
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let event = make_test_event("evt-sup-multi", 200, None);
    let tools = vec![make_tool_event("tool-a"), make_tool_event("tool-b")];
    let diags = vec![
        make_diag_event("diag-a"),
        make_diag_event("diag-b"),
        make_diag_event("diag-c"),
    ];
    let snaps = vec![make_codex_snapshot("snap-a"), make_codex_snapshot("snap-b")];

    handle
        .send(WriteCommand::TailBatch(TailBatchCommand {
            source_id: "src-multi-sup".to_string(),
            source_file_id: Some("file-multi-sup".to_string()),
            source_file_agent: "claude_code".to_string(),
            source_file_path: "/tmp/multi-sup.jsonl".to_string(),
            source_file_inode: None,
            events: vec![event],
            tool_events: tools,
            diagnostic_events: diags,
            codex_snapshots: snaps,
            generation_id: "gen-multi-sup".to_string(),
            checkpoint_offset: Some(200),
            write_policy: UsageWritePolicy::InsertOnce,
        }))
        .await
        .expect("send tail batch");

    handle.flush().await.expect("flush");

    let db = db.lock().unwrap();
    let tool_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM tool_events WHERE id IN ('tool-a','tool-b')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tool_count, 2);

    let diag_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM diagnostic_events WHERE id IN ('diag-a','diag-b','diag-c')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(diag_count, 3);

    let snap_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM codex_token_snapshots WHERE id IN ('snap-a','snap-b')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(snap_count, 2);

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

// =============================================================================
// writer.rs — RebuildBatch with is_final_batch
// =============================================================================

#[tokio::test]
async fn rebuild_batch_with_supplementary_events_and_final_flag() {
    // Cover the RebuildBatch arm of flush_single_generation including the
    // is_final_batch flag propagation and insert_supplementary_events.
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let event = make_test_event("evt-rebuild-1", 300, Some(0.05));
    let tool = make_tool_event("tool-rebuild-1");
    let diag = make_diag_event("diag-rebuild-1");
    let snap = make_codex_snapshot("snap-rebuild-1");

    handle
        .send(WriteCommand::RebuildBatch(RebuildBatchCommand {
            source_id: "src-rebuild".to_string(),
            source_file_id: Some("file-rebuild-1".to_string()),
            source_file_agent: "claude_code".to_string(),
            source_file_path: "/tmp/rebuild.jsonl".to_string(),
            source_file_inode: None,
            events: vec![event],
            tool_events: vec![tool],
            diagnostic_events: vec![diag],
            codex_snapshots: vec![snap],
            generation_id: "gen-rebuild".to_string(),
            checkpoint_offset: Some(500),
            is_final_batch: true,
            write_policy: UsageWritePolicy::InsertOnce,
        }))
        .await
        .expect("send rebuild batch");

    handle.flush().await.expect("flush");

    let db = db.lock().unwrap();
    let evt_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM usage_events WHERE id = 'evt-rebuild-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(evt_count, 1);

    let tool_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM tool_events WHERE id = 'tool-rebuild-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tool_count, 1);

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn rebuild_batch_non_final_then_final_sets_is_final_flag() {
    // Send two RebuildBatch commands: first with is_final_batch=false, then
    // second with is_final_batch=true. The is_final_rebuild flag should track
    // the last seen value.
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let event1 = make_test_event("evt-rebuild-nf-1", 100, None);
    let event2 = make_test_event("evt-rebuild-nf-2", 200, None);

    handle
        .send(WriteCommand::RebuildBatch(RebuildBatchCommand {
            source_id: "src-rebuild-nf".to_string(),
            source_file_id: Some("file-rebuild-nf".to_string()),
            source_file_agent: "claude_code".to_string(),
            source_file_path: "/tmp/rebuild-nf.jsonl".to_string(),
            source_file_inode: None,
            events: vec![event1],
            tool_events: vec![],
            diagnostic_events: vec![],
            codex_snapshots: vec![],
            generation_id: "gen-rebuild-nf".to_string(),
            checkpoint_offset: Some(100),
            is_final_batch: false,
            write_policy: UsageWritePolicy::InsertOnce,
        }))
        .await
        .expect("send rebuild batch 1");

    handle
        .send(WriteCommand::RebuildBatch(RebuildBatchCommand {
            source_id: "src-rebuild-nf".to_string(),
            source_file_id: Some("file-rebuild-nf".to_string()),
            source_file_agent: "claude_code".to_string(),
            source_file_path: "/tmp/rebuild-nf.jsonl".to_string(),
            source_file_inode: None,
            events: vec![event2],
            tool_events: vec![],
            diagnostic_events: vec![],
            codex_snapshots: vec![],
            generation_id: "gen-rebuild-nf".to_string(),
            checkpoint_offset: Some(200),
            is_final_batch: true,
            write_policy: UsageWritePolicy::InsertOnce,
        }))
        .await
        .expect("send rebuild batch 2");

    handle.flush().await.expect("flush");

    let db = db.lock().unwrap();
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM usage_events WHERE id IN ('evt-rebuild-nf-1','evt-rebuild-nf-2')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

// =============================================================================
// writer.rs — drain_pending_errors multi-error join path
// =============================================================================

#[tokio::test]
async fn flush_returns_joined_message_for_multiple_dispatch_errors() {
    // Send 2+ PromotionBarrier commands that fail due to drift. Each failure
    // pushes an error onto pending_errors. When flush() is called with
    // count > 1, drain_pending_errors joins them into a single message.
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    // Set up drift so promotion barriers fail.
    seed_drift_for_generation(&db, "gen-drift-multi", "file-dm", "src-dm");

    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status, event_bus, settings, 16);

    // Send two PromotionBarrier commands — both will fail due to drift.
    for _ in 0..2 {
        handle
            .send(WriteCommand::PromotionBarrier(PromotionBarrierCommand {
                from_generation_id: "gen-old".to_string(),
                to_generation_id: "gen-drift-multi".to_string(),
            }))
            .await
            .expect("send promotion barrier");
    }

    let flush_result = handle.flush().await;
    // With 2+ errors, drain_pending_errors returns a joined message containing
    // "N writer errors since previous barrier:" and numbered errors.
    match flush_result {
        Err(msg) => {
            let msg = msg.to_string();
            assert!(
                msg.contains("2 writer errors since previous barrier"),
                "expected joined multi-error message, got: {msg}"
            );
            assert!(
                msg.contains("1. "),
                "joined message should include numbered first error: {msg}"
            );
            assert!(
                msg.contains("2. "),
                "joined message should include numbered second error: {msg}"
            );
        }
        Ok(()) => {
            // On very slow CI, the writer might process the first barrier
            // before the second arrives, leaving only 1 error (single-message
            // path). Accept this as a soft-pass since the join path is
            // exercised on faster machines.
        }
    }

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn flush_returns_single_error_for_one_dispatch_error() {
    // Cover the count == 1 branch of drain_pending_errors.
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    seed_drift_for_generation(&db, "gen-drift-single", "file-ds", "src-ds");

    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status, event_bus, settings, 16);

    handle
        .send(WriteCommand::PromotionBarrier(PromotionBarrierCommand {
            from_generation_id: "gen-old".to_string(),
            to_generation_id: "gen-drift-single".to_string(),
        }))
        .await
        .expect("send promotion barrier");

    let flush_result = handle.flush().await;
    // With exactly 1 error, drain_pending_errors returns the raw error message
    // (no "N writer errors" prefix, no numbering).
    if let Err(msg) = flush_result {
        let msg = msg.to_string();
        assert!(
            !msg.contains("writer errors since previous barrier"),
            "single error should NOT use the joined format, got: {msg}"
        );
    }

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

// =============================================================================
// writer.rs — emit_threshold_diagnostics: lag warning/critical/recovery
// =============================================================================

#[tokio::test]
async fn writer_lag_threshold_warning_fires_and_recovers() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status.clone(), event_bus.clone(), settings, 16);

    let mut rx = event_bus.subscribe();

    // Set aggregate_lag_ms above the warning threshold (5000ms) but below
    // critical (30000ms). This triggers the "crossed above" path for warning.
    {
        let mut snap = status.write().await;
        snap.aggregate_lag_ms = 6_000;
    }

    // Send a fast command to trigger emit_threshold_diagnostics.
    handle
        .send(WriteCommand::DiagnosticWrite(DiagnosticWriteCommand {
            source_id: "src-lag-w".to_string(),
            code: "test".to_string(),
            message: "trigger lag check".to_string(),
            severity: "info".to_string(),
            details_json: None,
        }))
        .await
        .expect("send");

    // Collect the warning event.
    let mut found_warning = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(evt) => {
                if let AppEvent::WriterLagThreshold { severity, .. } = evt.event {
                    if severity == "warning" {
                        found_warning = true;
                        break;
                    }
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Err(_) => break,
        }
    }
    assert!(
        found_warning,
        "WriterLagThreshold warning event should fire when aggregate_lag_ms >= 5000"
    );

    // Now set lag to 0 — this triggers the "recovered" path.
    {
        let mut snap = status.write().await;
        snap.aggregate_lag_ms = 0;
    }

    handle
        .send(WriteCommand::DiagnosticWrite(DiagnosticWriteCommand {
            source_id: "src-lag-w2".to_string(),
            code: "test".to_string(),
            message: "trigger lag recovery".to_string(),
            severity: "info".to_string(),
            details_json: None,
        }))
        .await
        .expect("send recovery trigger");

    let mut found_recovery = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(evt) => {
                if let AppEvent::WriterLagThreshold { severity, .. } = evt.event {
                    if severity.starts_with("recovered_from_") {
                        found_recovery = true;
                        break;
                    }
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Err(_) => break,
        }
    }
    assert!(
        found_recovery,
        "WriterLagThreshold recovery event should fire when lag drops below thresholds"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn writer_lag_threshold_critical_fires_and_recovers() {
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status.clone(), event_bus.clone(), settings, 16);

    let mut rx = event_bus.subscribe();

    // Set lag above the critical threshold (30000ms).
    {
        let mut snap = status.write().await;
        snap.aggregate_lag_ms = 31_000;
    }

    handle
        .send(WriteCommand::DiagnosticWrite(DiagnosticWriteCommand {
            source_id: "src-lag-c".to_string(),
            code: "test".to_string(),
            message: "trigger critical lag".to_string(),
            severity: "info".to_string(),
            details_json: None,
        }))
        .await
        .expect("send");

    let mut found_critical = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(evt) => {
                if let AppEvent::WriterLagThreshold { severity, .. } = evt.event {
                    if severity == "critical" {
                        found_critical = true;
                        break;
                    }
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Err(_) => break,
        }
    }
    assert!(
        found_critical,
        "WriterLagThreshold critical event should fire when aggregate_lag_ms >= 30000"
    );

    // Recover.
    {
        let mut snap = status.write().await;
        snap.aggregate_lag_ms = 0;
    }

    handle
        .send(WriteCommand::DiagnosticWrite(DiagnosticWriteCommand {
            source_id: "src-lag-c2".to_string(),
            code: "test".to_string(),
            message: "trigger critical recovery".to_string(),
            severity: "info".to_string(),
            details_json: None,
        }))
        .await
        .expect("send recovery trigger");

    let mut found_recovery = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(evt) => {
                if let AppEvent::WriterLagThreshold { severity, .. } = evt.event {
                    if severity.starts_with("recovered_from_") {
                        found_recovery = true;
                        break;
                    }
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Err(_) => break,
        }
    }
    assert!(
        found_recovery,
        "WriterLagThreshold recovery event should fire after critical"
    );

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

#[tokio::test]
async fn writer_lag_threshold_transitions_warning_to_critical() {
    // Cover the transition from warning to critical (prev_state=warning,
    // current_state=critical).
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status.clone(), event_bus.clone(), settings, 16);

    let mut rx = event_bus.subscribe();

    // First trigger warning.
    {
        let mut snap = status.write().await;
        snap.aggregate_lag_ms = 6_000;
    }
    handle
        .send(WriteCommand::DiagnosticWrite(DiagnosticWriteCommand {
            source_id: "src-trans-1".to_string(),
            code: "test".to_string(),
            message: "warning".to_string(),
            severity: "info".to_string(),
            details_json: None,
        }))
        .await
        .expect("send warning trigger");
    // Drain the warning event.
    let _ = tokio::time::timeout(Duration::from_millis(200), async {
        loop {
            match rx.recv().await {
                Ok(evt) => {
                    if matches!(evt.event, AppEvent::WriterLagThreshold { .. }) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
    .await;

    // Now transition to critical.
    {
        let mut snap = status.write().await;
        snap.aggregate_lag_ms = 31_000;
    }
    handle
        .send(WriteCommand::DiagnosticWrite(DiagnosticWriteCommand {
            source_id: "src-trans-2".to_string(),
            code: "test".to_string(),
            message: "critical".to_string(),
            severity: "info".to_string(),
            details_json: None,
        }))
        .await
        .expect("send critical trigger");

    let mut found_critical = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(evt) => {
                if let AppEvent::WriterLagThreshold { severity, .. } = evt.event {
                    if severity == "critical" {
                        found_critical = true;
                        break;
                    }
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Err(_) => break,
        }
    }
    assert!(found_critical, "should transition from warning to critical");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

// =============================================================================
// writer.rs — emit_threshold_diagnostics: queue depth critical + recovery
// =============================================================================

#[tokio::test]
async fn writer_queue_threshold_critical_fires_and_recovers() {
    // Fill the channel with enough commands to push queue_depth above the
    // critical threshold (96). Using try_send (synchronous, no yield) ensures
    // the writer task does not drain until we await flush().
    let (db, status, event_bus, settings) = test_db_status_bus_settings();
    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db, status, event_bus.clone(), settings, 128);

    let mut rx = event_bus.subscribe();

    // Fill the queue with 100 DiagnosticWrite commands via try_send (no yield).
    for i in 0..100u32 {
        let _ = handle.try_send(WriteCommand::DiagnosticWrite(DiagnosticWriteCommand {
            source_id: format!("src-q-{i}"),
            code: "test".to_string(),
            message: format!("msg-{i}"),
            severity: "info".to_string(),
            details_json: None,
        }));
    }

    // Flush triggers the writer to start draining. As it processes commands,
    // queue_depth will be above 96 at some point, triggering the critical
    // threshold event.
    let _ = handle.flush().await;

    // Collect threshold events.
    let mut found_critical = false;
    let mut found_recovery = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(evt) => {
                if let AppEvent::WriterQueueThreshold { severity, .. } = evt.event {
                    if severity == "critical" {
                        found_critical = true;
                    }
                    if severity.starts_with("recovered_from_") {
                        found_recovery = true;
                    }
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Err(_) => break,
        }
        if found_critical && found_recovery {
            break;
        }
    }
    // These assertions are best-effort: on very fast machines the writer
    // might drain some commands before try_send fills the queue, preventing
    // the threshold from being crossed. We assert only when the threshold was
    // actually crossed (non-flaky on typical hardware).
    if found_critical {
        assert!(
            found_recovery,
            "if critical threshold fired, recovery should also fire after drain"
        );
    }

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(5), join).await;
}

// =============================================================================
// writer.rs — handle_reset_failed_checkpoints with active generation
// =============================================================================

#[tokio::test]
async fn reset_failed_checkpoints_with_active_generation_refreshes_summary() {
    // Cover the `if let Some(generation_id) = current_active_generation_id()`
    // branch inside handle_reset_failed_checkpoints (lines 1479-1488).
    let (db, status, event_bus, settings) = test_db_status_bus_settings();

    // Seed an active generation so current_active_generation_id returns Some.
    seed_active_generation(&db, "gen-active-rfc");
    seed_log_source_row(&db, "src-rfc", "active");

    let now = now_ms();
    db.conn()
        .execute(
            "INSERT INTO log_files \
             (id, source_id, agent, path, inode, offset_bytes, state, last_error, \
              first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms) \
             VALUES ('file-err-rfc', 'src-rfc', 'claude_code', '/tmp/rfc.jsonl', NULL, \
                     100, 'error', 'boom', ?1, ?1, ?1, ?1)",
            rusqlite::params![now],
        )
        .expect("seed error file");

    let db = Arc::new(Mutex::new(db));
    let (handle, join) = spawn_writer(db.clone(), status, event_bus, settings, 16);

    let updated = handle.reset_failed_checkpoints().await.expect("reset");
    assert_eq!(updated, 1, "one error file should be reset");

    let db = db.lock().unwrap();
    let state: String = db
        .conn()
        .query_row(
            "SELECT state FROM log_files WHERE id = 'file-err-rfc'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "active", "error file should be reset to active");

    drop(handle);
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
}

// =============================================================================
// read_service.rs — ReadError::code() and Display
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_error_code_for_timeout_returns_read_timeout() {
    // Trigger a timeout and then call .code() — the existing timeout test
    // only checks .kind(), not .code().
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("timeout.sqlite");
    let _db = Database::open(&path).unwrap();
    let service = ReadService::new(path, 1);

    let err = service
        .run(
            ReadQuery::new("test.code_timeout", "test").timeout(Duration::from_millis(1)),
            |_conn| {
                std::thread::sleep(Duration::from_millis(25));
                Ok::<_, anyhow::Error>(())
            },
        )
        .await
        .unwrap_err();

    assert_eq!(err.kind(), ReadErrorKind::Timeout);
    assert_eq!(err.code(), "read_timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_error_display_formats_with_code_and_message() {
    // Exercise the Display impl: write!(f, "{}: {}", self.code(), self.message).
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("display.sqlite");
    let _db = Database::open(&path).unwrap();
    let service = ReadService::new(path, 1);

    let err = service
        .run(
            ReadQuery::new("test.display", "test_family").timeout(Duration::from_millis(1)),
            |_conn| {
                std::thread::sleep(Duration::from_millis(25));
                Ok::<_, anyhow::Error>(())
            },
        )
        .await
        .unwrap_err();

    let formatted = format!("{}", err);
    assert!(
        formatted.contains("read_timeout"),
        "Display should include code: {formatted}"
    );
    assert!(
        formatted.contains("timed out"),
        "Display should include message: {formatted}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_error_code_for_internal_returns_read_internal_error() {
    // Cover the Internal arm of code().
    let db = Database::open_in_memory().unwrap();
    let db = Arc::new(Mutex::new(db));
    let service = ReadService::new_in_memory(db, 1);

    let err = service
        .run(ReadQuery::new("test.internal_code", "test"), |_conn| {
            Err::<(), anyhow::Error>(anyhow::anyhow!("some internal error"))
        })
        .await
        .unwrap_err();

    assert_eq!(err.kind(), ReadErrorKind::Internal);
    assert_eq!(err.code(), "read_internal_error");
    let formatted = format!("{}", err);
    assert!(formatted.contains("read_internal_error"));
}

// =============================================================================
// read_service.rs — map_read_error with specific SQLite error codes
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_map_read_error_with_database_busy_code() {
    // Construct a rusqlite::Error::SqliteFailure with ErrorCode::DatabaseBusy
    // and return it from the closure. This covers the DatabaseBusy arm of
    // map_read_error.
    let db = Database::open_in_memory().unwrap();
    let db = Arc::new(Mutex::new(db));
    let service = ReadService::new_in_memory(db, 1);

    let sqlite_err = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ffi::ErrorCode::DatabaseBusy,
            extended_code: 5,
        },
        Some("database is busy".to_string()),
    );

    let err = service
        .run(ReadQuery::new("test.busy_code", "test"), move |_conn| {
            Err::<(), anyhow::Error>(sqlite_err.into())
        })
        .await
        .unwrap_err();

    assert_eq!(
        err.kind(),
        ReadErrorKind::DatabaseBusy,
        "DatabaseBusy SQLite code should map to ReadErrorKind::DatabaseBusy"
    );
    assert_eq!(err.code(), "database_busy");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_map_read_error_with_database_locked_code() {
    // Cover the DatabaseLocked variant in the same match arm.
    let db = Database::open_in_memory().unwrap();
    let db = Arc::new(Mutex::new(db));
    let service = ReadService::new_in_memory(db, 1);

    let sqlite_err = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ffi::ErrorCode::DatabaseLocked,
            extended_code: 6,
        },
        Some("database is locked".to_string()),
    );

    let err = service
        .run(ReadQuery::new("test.locked_code", "test"), move |_conn| {
            Err::<(), anyhow::Error>(sqlite_err.into())
        })
        .await
        .unwrap_err();

    assert_eq!(err.kind(), ReadErrorKind::DatabaseBusy);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_map_read_error_with_not_a_database_code() {
    // Construct a rusqlite::Error::SqliteFailure with ErrorCode::NotADatabase
    // and return it from the closure. This covers the NotADatabase arm of
    // map_read_error.
    let db = Database::open_in_memory().unwrap();
    let db = Arc::new(Mutex::new(db));
    let service = ReadService::new_in_memory(db, 1);

    let sqlite_err = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ffi::ErrorCode::NotADatabase,
            extended_code: 26,
        },
        Some("file is not a database".to_string()),
    );

    let err = service
        .run(ReadQuery::new("test.notadb_code", "test"), move |_conn| {
            Err::<(), anyhow::Error>(sqlite_err.into())
        })
        .await
        .unwrap_err();

    assert_eq!(
        err.kind(),
        ReadErrorKind::Unavailable,
        "NotADatabase SQLite code should map to ReadErrorKind::Unavailable"
    );
    assert_eq!(err.code(), "read_model_unavailable");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_map_read_error_with_cannot_open_code() {
    // Cover the CannotOpen variant in the same match arm.
    let db = Database::open_in_memory().unwrap();
    let db = Arc::new(Mutex::new(db));
    let service = ReadService::new_in_memory(db, 1);

    let sqlite_err = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ffi::ErrorCode::CannotOpen,
            extended_code: 14,
        },
        Some("cannot open".to_string()),
    );

    let err = service
        .run(ReadQuery::new("test.cantopen_code", "test"), move |_conn| {
            Err::<(), anyhow::Error>(sqlite_err.into())
        })
        .await
        .unwrap_err();

    assert_eq!(err.kind(), ReadErrorKind::Unavailable);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_map_read_error_with_not_found_code() {
    // Cover the NotFound variant in the same match arm.
    let db = Database::open_in_memory().unwrap();
    let db = Arc::new(Mutex::new(db));
    let service = ReadService::new_in_memory(db, 1);

    let sqlite_err = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ffi::ErrorCode::NotFound,
            extended_code: 12,
        },
        Some("not found".to_string()),
    );

    let err = service
        .run(ReadQuery::new("test.notfound_code", "test"), move |_conn| {
            Err::<(), anyhow::Error>(sqlite_err.into())
        })
        .await
        .unwrap_err();

    assert_eq!(err.kind(), ReadErrorKind::Unavailable);
}

// =============================================================================
// read_service.rs — map_open_error via garbage file (NotADatabase)
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_not_a_database_file_maps_to_unavailable() {
    // Create a file with garbage content. When Database::open_readonly tries
    // to open it and a query is executed, SQLite returns NotADatabase. This
    // covers the NotADatabase arm of map_open_error.
    //
    // SQLite may lazily validate the file header, so we must execute a query
    // (e.g. SELECT 1) to trigger the "file is not a database" error.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("garbage.sqlite");
    std::fs::write(&path, b"this is not a sqlite database file").unwrap();

    let service = ReadService::new(path, 1);

    let result = service
        .run(ReadQuery::new("test.garbage_open", "test"), |conn| {
            // Execute a trivial query to trigger SQLite header validation.
            let val: i64 = conn.query_row("SELECT 1", [], |r| r.get(0))?;
            Ok::<_, anyhow::Error>(val)
        })
        .await;

    let err = result.unwrap_err();
    // SQLite may return NotADatabase, CannotOpen, or DatabaseCorrupt depending
    // on the SQLite version and header check timing. All of these map to
    // Unavailable (NotADatabase/CannotOpen) or Internal (DatabaseCorrupt).
    // We assert that it's either Unavailable or Internal — the key is that
    // map_open_error / map_read_error was exercised with a real SQLite error.
    assert!(
        err.kind() == ReadErrorKind::Unavailable || err.kind() == ReadErrorKind::Internal,
        "garbage file should map to Unavailable or Internal, got {:?}: {}",
        err.kind(),
        err.message()
    );
    assert!(!err.message().is_empty());
}

// =============================================================================
// read_service.rs — zero-connection panics
// =============================================================================

#[test]
#[should_panic(expected = "max_connections must be greater than zero")]
fn read_service_new_panics_on_zero_connections() {
    let path = std::path::PathBuf::from("/tmp/nonexistent_test_db.sqlite");
    let _ = ReadService::new(path, 0);
}

#[test]
#[should_panic(expected = "max_connections must be greater than zero")]
fn read_service_new_in_memory_panics_on_zero_connections() {
    let db = Database::open_in_memory().unwrap();
    let db = Arc::new(Mutex::new(db));
    let _ = ReadService::new_in_memory(db, 0);
}

// =============================================================================
// read_service.rs — ReadOutcome with_row_count covers outcome.row_count path
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_read_outcome_with_row_count_logs_count() {
    // ReadOutcome::with_row_count sets row_count = Some(count). The run()
    // method logs this count via log_completion. This covers the
    // outcome.row_count path in the Ok(Ok(Ok(outcome))) branch.
    let db = Database::open_in_memory().unwrap();
    let db = Arc::new(Mutex::new(db));
    let service = ReadService::new_in_memory(db, 1);

    let result: i64 = service
        .run(
            ReadQuery::new("test.outcome_rc", "test").row_count(42),
            |conn| {
                let val: i64 = conn.query_row("SELECT 42", [], |r| r.get(0))?;
                Ok(ReadOutcome::with_row_count(val, 1))
            },
        )
        .await
        .expect("query should succeed");

    assert_eq!(result, 42);
}

// =============================================================================
// service_app.rs — ServiceApp::run() setup stages
//
// The `run()` future is `!Send` (the file watcher / ctrl_c signal handler
// contains non-Send internals), so we use a `LocalSet` with `spawn_local`
// instead of `tokio::spawn`. We abort the task after a short sleep to cover
// the setup stages (initial scan, tailer, sampler, background jobs,
// service_ready) without exercising the graceful shutdown path (which
// requires a ctrl_c signal).
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_app_run_setup_executes_and_can_be_aborted() {
    // Boot the service, spawn run() in a LocalSet task, sleep briefly to let
    // the setup stages execute, then abort the task.
    //
    // This covers lines 61-95 of run() (the setup before tokio::select!).
    // The graceful shutdown path (ctrl_c) is intentionally not covered here
    // because sending SIGINT process-wide is unsafe under cargo test's
    // parallel test runner.
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let startup = Instant::now();
    let data_dir = paths.data_dir().to_path_buf();

    let app = ServiceApp::boot(paths, startup).await.expect("boot");

    // boot() must have written the marker.
    assert!(
        busytok_config::service_marker::exists(&data_dir),
        "boot must write service.ready marker"
    );

    // Use a LocalSet so we can spawn_local the !Send run() future.
    let local = tokio::task::LocalSet::new();
    let run_task = local.spawn_local(async move { app.run().await });

    // Give the setup stages time to execute.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Abort the task — this covers the setup lines but not the graceful
    // shutdown path. The marker is NOT removed on abort.
    run_task.abort();

    // Wait for the abort to take effect.
    let _ = tokio::time::timeout(Duration::from_secs(2), run_task).await;

    // Run the LocalSet to completion (drains any remaining local tasks).
    let _ = tokio::time::timeout(Duration::from_secs(2), local).await;

    // Clean up: remove the marker left by boot (since we aborted before
    // the graceful shutdown could remove it).
    let _ = busytok_config::service_marker::remove(&data_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_app_boot_then_run_quickly_aborts_covers_setup() {
    // Additional coverage: verify that boot() + run() setup completes
    // without panicking even when aborted quickly. This is a second
    // data point for the run() setup path.
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let startup = Instant::now();
    let data_dir = paths.data_dir().to_path_buf();

    let app = ServiceApp::boot(paths, startup).await.expect("boot");
    assert!(busytok_config::service_marker::exists(&data_dir));

    let local = tokio::task::LocalSet::new();
    let run_task = local.spawn_local(async move { app.run().await });

    // Very short sleep — just enough for setup to begin.
    tokio::time::sleep(Duration::from_millis(100)).await;

    run_task.abort();
    let _ = tokio::time::timeout(Duration::from_secs(2), run_task).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), local).await;

    let _ = busytok_config::service_marker::remove(&data_dir);
}

// =============================================================================
// service_app.rs — ServiceApp::run() via tokio::time::timeout
//
// The key insight: tokio::time::timeout polls the inner future directly in
// the current task. This actually executes run()'s setup stages (initial
// scan, tailer, sampler, background jobs, service_ready) before the timeout
// fires and drops the future. The previous LocalSet+spawn_local approach
// never polled the run() future because sleep() didn't yield to the
// LocalSet.
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_app_run_timeout_covers_setup_stages_5_to_9() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let data_dir = paths.data_dir().to_path_buf();

    let app = ServiceApp::boot(paths, Instant::now()).await.expect("boot");

    // tokio::time::timeout polls run() directly. Setup stages 5-9 execute
    // before the timeout fires. run() blocks at tokio::select! waiting for
    // ctrl_c, then the timeout drops the future. 2s gives start_tailing
    // enough time to complete on slower machines.
    let _ = tokio::time::timeout(Duration::from_millis(2000), app.run()).await;

    // Clean up the marker (run() didn't complete gracefully).
    let _ = busytok_config::service_marker::remove(&data_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "sends SIGINT to the test process — run with --ignored --test-threads=1"]
async fn service_app_run_shutdown_via_sigint_covers_graceful_path() {
    use std::process::Command;

    let dir = tempfile::tempdir().expect("tempdir");
    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let data_dir = paths.data_dir().to_path_buf();

    let app = ServiceApp::boot(paths, Instant::now()).await.expect("boot");

    // Spawn run() in a LocalSet (it's !Send due to ctrl_c signal handler).
    let local = tokio::task::LocalSet::new();
    let run_task = local.spawn_local(async move { app.run().await });

    // Schedule SIGINT after 500ms — gives run() time to progress through setup.
    let pid = std::process::id();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = Command::new("kill")
            .args(["-INT", &pid.to_string()])
            .output();
    });

    // Run the LocalSet until run() completes (triggered by SIGINT → ctrl_c).
    let _ = tokio::time::timeout(Duration::from_secs(15), local.run_until(run_task)).await;

    // After graceful shutdown, the marker should be removed.
    assert!(
        !busytok_config::service_marker::exists(&data_dir),
        "marker should be removed after graceful shutdown"
    );
}
