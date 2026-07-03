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
//! Integration tests for BusytokSupervisor RuntimeControl implementation.
//!
//! Tests seed data directly into the in-memory database and then call each
//! control method, asserting the shape and content of the returned DTOs.

use futures::FutureExt;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use busytok_adapters::AgentLogAdapter;
use busytok_config::{
    BusytokPaths, BusytokSettings, DiscoverySettings, ManualRootConfig, ProviderConfig,
    ProviderKind,
};
use busytok_control::dispatch::RuntimeControl;
use busytok_discovery::DiscoveredLogSource;
use busytok_domain::{
    AgentKind, LogSourceType, NormalizedUsageEvent, OperationalDiagnosticEvent, ParseContext,
    ParseError, ParsedLogEvent, ReportingTimezone, UsageWritePolicy,
};
use busytok_protocol::dto::*;
use busytok_runtime::status::CachedClientRollup;
use busytok_runtime::subagent_usage::normalize_task_usage;
use busytok_runtime::BusytokSupervisor;
use busytok_store::repository::LogSourceRow;
use busytok_store::Database;
use busytok_subagent::models::TaskUsage;
use busytok_subagent::sidecar::SidecarConfig;
use serial_test::serial;
use time::{Duration as TimeDuration, Month, OffsetDateTime, Time, UtcOffset};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedLogBuffer {
    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }

    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl<'a> MakeWriter<'a> for SharedLogBuffer {
    type Writer = SharedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedLogWriter(self.0.clone())
    }
}

struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

impl io::Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn test_logs() -> SharedLogBuffer {
    static LOGS: OnceLock<SharedLogBuffer> = OnceLock::new();
    LOGS.get_or_init(SharedLogBuffer::default).clone()
}

fn init_test_logging() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(test_logs())
            .with_ansi(false)
            .without_time()
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}

fn log_capture_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

async fn wait_for_log_line<F>(logs: &SharedLogBuffer, timeout: Duration, predicate: F) -> bool
where
    F: Fn(&str) -> bool,
{
    let started = Instant::now();
    loop {
        if logs.text().lines().any(&predicate) {
            return true;
        }
        if started.elapsed() >= timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_supervisor(db: Database, tmp: &tempfile::TempDir) -> BusytokSupervisor {
    let paths = BusytokPaths::for_test(tmp.path());
    // Save default settings to prevent the supervisor from loading stale ones.
    let settings = busytok_config::BusytokSettings::default();
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .ok();
    BusytokSupervisor::new(db, paths)
}

fn make_file_backed_db(tmp: &tempfile::TempDir) -> Database {
    let db_path = tmp.path().join("busytok.sqlite");
    Database::open(&db_path).unwrap()
}

fn make_supervisor_with_settings(
    db: Database,
    tmp: &tempfile::TempDir,
    settings: BusytokSettings,
) -> BusytokSupervisor {
    let paths = BusytokPaths::for_test(tmp.path());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .ok();
    BusytokSupervisor::new(db, paths)
}

// ---------------------------------------------------------------------------
// Sidecar test helpers — used by the `workers: [one row]` stopped-sidecar
// path (`subagent_runtime_status_workers_one_row_when_sidecar_stopped`).
// Mirrors the helpers in `subagent_e2e_sidecar.rs`; duplicated because the
// `busytok-subagent` test `support` module is test-private to that crate.
// ---------------------------------------------------------------------------

fn sidecar_shell_path() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            return PathBuf::from(program_files)
                .join("Git")
                .join("bin")
                .join("bash.exe");
        }
        PathBuf::from(r"C:\Program Files\Git\bin\bash.exe")
    }

    #[cfg(not(windows))]
    {
        PathBuf::from("/bin/bash")
    }
}

fn mock_sidecar_path() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(format!(
        "{manifest}/../busytok-subagent/tests/fixtures/mock-sidecar.sh"
    ))
}

fn mock_sidecar_bundle_path() -> PathBuf {
    let path = mock_sidecar_path();
    #[cfg(windows)]
    {
        let raw = path.to_string_lossy().replace('\\', "/");
        if let Some((drive, rest)) = raw.split_once(":/") {
            let drive = drive.to_ascii_lowercase();
            return PathBuf::from(format!("/{drive}/{rest}"));
        }
        PathBuf::from(raw)
    }

    #[cfg(not(windows))]
    {
        path
    }
}

/// `SidecarConfig` pointing at the mock-sidecar.sh fixture. Mirrors the fields
/// `resolve_sidecar_config` would produce, with `bundle_path` set to the mock
/// fixture (no env var needed).
fn make_sidecar_config() -> SidecarConfig {
    SidecarConfig {
        node_binary: sidecar_shell_path(),
        bundle_path: mock_sidecar_bundle_path(),
        env: HashMap::new(),
        idle_exit_seconds: 300,
        health_interval: Duration::from_secs(30),
        task_timeout: Duration::from_secs(30),
        max_restart_attempts: 3,
        restart_backoff_base: Duration::from_secs(1),
        harness_name: "pi".to_string(),
        max_hot_sessions: 3,
        memory_soft_limit_mb: 800,
        memory_hard_limit_mb: 1200,
        provider_id: String::new(),
        api_key_env_name: String::new(),
        base_url_env_name: String::new(),
    }
}

/// Settings with `pi_sidecar.enabled = true` so `new_with_sidecar_config`
/// takes the sidecar wiring path. The injected `SidecarConfig` means
/// `resolve_sidecar_config` is skipped, so `node_runtime`/`system_node_path`
/// are irrelevant here.
fn make_sidecar_settings() -> BusytokSettings {
    let mut settings = BusytokSettings::default();
    settings.subagent.pi_sidecar.enabled = true;

    // Add a test provider + bind profiles so the WorkerPool can route.
    settings.providers.push(ProviderConfig {
        id: "test-provider".to_string(),
        name: "Test Provider".to_string(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://api.test-provider.example.com/v1".to_string(),
        api_key_env_name: "TEST_API_KEY".to_string(),
        base_url_env_name: None,
        models: vec!["test-model".to_string()],
        enabled: true,
    });
    for profile in settings.subagent.profiles.values_mut() {
        profile.provider_id = Some("test-provider".to_string());
    }
    std::env::set_var("BUSYTOK_TEST_API_KEY", "test-key-for-e2e");

    settings
}

/// Construct a supervisor with `pi_sidecar.enabled = true` and the mock sidecar
/// bundle injected via `new_with_sidecar_config`. The sidecar child is NOT
/// started (`ensure_started` is never called), so `worker_snapshot()` reports
/// `Stopped` — the "configured-but-stopped" posture under test.
fn make_sidecar_stopped_supervisor(db: Database, tmp: &tempfile::TempDir) -> BusytokSupervisor {
    let paths = BusytokPaths::for_test(tmp.path());
    make_sidecar_settings()
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");
    BusytokSupervisor::new_with_sidecar_config(db, paths, make_sidecar_config())
}

fn seed_event(
    db: &Database,
    id: &str,
    timestamp_ms: i64,
    tokens: i64,
    cost: Option<f64>,
    client_kind: Option<&str>,
    model: Option<&str>,
    project_hash: Option<&str>,
    session_id: Option<&str>,
) {
    let mut event = NormalizedUsageEvent::minimal_for_test(id, AgentKind::ClaudeCode);
    event.timestamp_ms = timestamp_ms;
    event.total_tokens = tokens;
    event.cost_usd = cost;
    event.client_kind = client_kind.map(|s| s.to_string());
    event.model = model.map(|s| s.to_string());
    event.project_hash = project_hash.map(|s| s.to_string());
    event.session_id = session_id.unwrap_or("session-default").to_string();
    db.write_usage_event(&event, busytok_domain::UsageWritePolicy::InsertOnce)
        .unwrap();
}

fn set_active_generation(db: &Database, generation_id: &str, updated_at_ms: i64) {
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO service_state
             (id, writer_queue_depth, aggregate_lag_ms, readiness, active_generation_id, updated_at_ms)
             VALUES (1, 0, 0, 'ready_exact', ?1, ?2)",
            rusqlite::params![generation_id, updated_at_ms],
        )
        .unwrap();
}

fn assign_event_generation(db: &Database, event_id: &str, generation_id: &str) {
    db.conn()
        .execute(
            "UPDATE usage_events SET generation_id = ?1, dedupe_key = ?2 WHERE id = ?3",
            rusqlite::params![generation_id, event_id, event_id],
        )
        .unwrap();
}

/// Populate `daily_usage` for a generation the same way the writer's flush
/// path does. Required by tests that read overview via the IANA /
/// non-whole-hour-fixed path (`+05:30`, `Asia/Shanghai`, …), because those
/// paths skip the `usage_buckets_hour` fast path entirely.
fn seed_daily_usage_for_generation(db: &Database, timezone: &str, generation_id: &str) {
    let rtz = ReportingTimezone::parse(timezone).unwrap();
    let events = db.usage_events_for_generation(generation_id).unwrap();
    busytok_store::write_queries::upsert_daily_usage_for_events(
        db.conn(),
        &events,
        &rtz,
        generation_id,
    )
    .unwrap();
}

fn set_latest_event_seq(db: &Database, latest_event_seq: i64) {
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO event_sequence_state
             (id, latest_event_seq, latest_event_timestamp_ms, updated_at_ms)
             VALUES (1, ?1, NULL, 1000)",
            rusqlite::params![latest_event_seq],
        )
        .unwrap();
}

fn seed_diagnostic(db: &Database, id: &str, source_id: &str, severity: &str) {
    let event = OperationalDiagnosticEvent {
        id: id.to_string(),
        agent: None,
        source_id: Some(source_id.to_string()),
        source_file_id: None,
        source_path: None,
        source_line: None,
        category: "test".to_string(),
        severity: severity.to_string(),
        message: format!("test {}", severity),
        detail_json: None,
        happened_at_ms: 1000,
        created_at_ms: 1000,
    };
    db.record_diagnostic_event(&event).unwrap();
}

fn seed_source(
    db: &Database,
    id: &str,
    agent: &str,
    root_path: &str,
    status: &str,
    source_type: &str,
) {
    let row = LogSourceRow {
        id: id.to_string(),
        agent: agent.to_string(),
        source_type: source_type.to_string(),
        root_path: root_path.to_string(),
        configured_by_user: 1,
        default_discovery_enabled: 0,
        status: status.to_string(),
        last_scan_started_at_ms: None,
        last_scan_completed_at_ms: None,
        last_error: None,
        first_seen_at_ms: 1000,
        last_seen_at_ms: 1000,
        created_at_ms: 1000,
        updated_at_ms: 1000,
    };
    db.upsert_log_source(&row).unwrap();
}

fn set_source_scan_timestamps(
    db: &Database,
    source_id: &str,
    started_at_ms: Option<i64>,
    completed_at_ms: Option<i64>,
) {
    db.conn()
        .execute(
            "UPDATE log_sources \
             SET last_scan_started_at_ms = ?2, last_scan_completed_at_ms = ?3 \
             WHERE id = ?1",
            rusqlite::params![source_id, started_at_ms, completed_at_ms],
        )
        .unwrap();
}

fn seed_log_file(db: &Database, id: &str, source_id: &str, agent: &str, path: &str) {
    // We need to use the ingest mechanism to properly link files and events.
    // For simpler tests, this is a direct SQL insert.
    let conn = db.conn();
    conn.execute(
        "INSERT OR IGNORE INTO log_files (id, source_id, agent, path, \
         inode, offset_bytes, state, first_seen_at_ms, last_seen_at_ms, \
         created_at_ms, updated_at_ms) \
         VALUES (?1, ?2, ?3, ?4, NULL, 0, 'active', 1000, 1000, 1000, 1000)",
        rusqlite::params![id, source_id, agent, path],
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// shell.status
// ---------------------------------------------------------------------------

#[test]
fn shell_status_returns_chips() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let result = sup.shell_status().now_or_never().unwrap().unwrap();
    assert!(result.generated_at_ms > 0);
    assert!(!result.status_chips.is_empty(), "should have status chips");

    // When there's no data, we should still get sensible chip labels.
    let capture = result.status_chips.iter().find(|c| c.id == "capture");
    assert!(capture.is_some(), "should have a capture chip");
    assert_eq!(capture.unwrap().label, "No data yet");

    // Scan chip should be first.
    assert_eq!(result.status_chips[0].id, "scan");
}

#[test]
fn shell_status_shows_event_count() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    seed_event(
        &db,
        "evt-1",
        1000,
        100,
        Some(1.0),
        Some("claude_code"),
        Some("claude-sonnet-4"),
        Some("hash-1"),
        None,
    );

    let sup = make_supervisor(db, &tmp);
    let result = sup.shell_status().now_or_never().unwrap().unwrap();

    let capture = result
        .status_chips
        .iter()
        .find(|c| c.id == "capture")
        .unwrap();
    assert!(capture.label.contains("1 events"));
}

#[test]
fn shell_status_has_no_risk_chip_even_with_errors() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();

    let mut event = NormalizedUsageEvent::minimal_for_test("evt-err", AgentKind::ClaudeCode);
    event.timestamp_ms = 1000;
    event.total_tokens = 100;
    event.is_error = true;
    event.session_id = "session-1".to_string();
    db.write_usage_event(&event, busytok_domain::UsageWritePolicy::InsertOnce)
        .unwrap();

    let sup = make_supervisor(db, &tmp);
    let result = sup.shell_status().now_or_never().unwrap().unwrap();

    assert!(
        result.status_chips.iter().all(|c| c.id != "risk"),
        "risk chip should not exist"
    );
}

#[test]
fn shell_status_has_no_local_audit_chip() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    seed_diagnostic(&db, "diag-1", "src-1", "warning");
    seed_diagnostic(&db, "diag-2", "src-1", "error");

    let sup = make_supervisor(db, &tmp);
    let result = sup.shell_status().now_or_never().unwrap().unwrap();

    assert!(
        result.status_chips.iter().all(|c| c.id != "local_audit"),
        "local_audit chip should not exist"
    );
}

#[test]
fn shell_status_shows_scan_state() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let result = sup.shell_status().now_or_never().unwrap().unwrap();
    let scan = result.status_chips.iter().find(|c| c.id == "scan").unwrap();
    assert_eq!(scan.label, "Service offline");
    assert_eq!(scan.tone, ToneDto::Danger);
}

#[test]
fn shell_status_keeps_scan_chip_when_client_rollups_exist() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    sup.apply_service_status_snapshot(|snap| {
        snap.chip_data_hydrated = true;
        snap.cached_client_rollups = vec![CachedClientRollup {
            client_kind: "claude_code".to_string(),
            active_source_count: 2,
            event_count: 42,
        }];
    })
    .unwrap();

    let result = sup.shell_status().now_or_never().unwrap().unwrap();
    assert_eq!(result.status_chips[0].id, "scan");
    assert_eq!(result.status_chips[0].label, "Service offline");

    let client_chip = result
        .status_chips
        .iter()
        .find(|c| c.id == "client:claude_code")
        .expect("expected per-client chip alongside scan chip");
    assert_eq!(client_chip.label, "Claude Code");
    assert_eq!(client_chip.tone, ToneDto::Success);
    assert_eq!(client_chip.detail.as_deref(), Some("2 sources, 42 events"));
}

#[test]
fn shell_status_shows_live_capture_when_service_running_and_scan_completed() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    seed_source(
        &db,
        "src-1",
        "claude_code",
        "/logs",
        "active",
        "session_root",
    );
    set_source_scan_timestamps(&db, "src-1", Some(1_000), Some(2_000));

    let sup = make_supervisor(db, &tmp);
    busytok_config::service_marker::write(paths.data_dir()).unwrap();

    let result = sup.shell_status().now_or_never().unwrap().unwrap();
    let scan = result.status_chips.iter().find(|c| c.id == "scan").unwrap();
    assert_eq!(scan.label, "Live capture active");
    assert_eq!(scan.tone, ToneDto::Success);
}

#[test]
fn shell_status_ignores_stale_unfinished_scan_timestamps() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let stale_started_at = busytok_domain::now_ms() - (11 * 60 * 1000);

    seed_source(
        &db,
        "src-stale",
        "claude_code",
        "/logs/claude",
        "active",
        "session_root",
    );
    set_source_scan_timestamps(&db, "src-stale", Some(stale_started_at), None);

    seed_source(
        &db,
        "src-live",
        "codex",
        "/logs/codex",
        "active",
        "session_root",
    );
    set_source_scan_timestamps(&db, "src-live", Some(1_000), Some(2_000));

    let sup = make_supervisor(db, &tmp);
    busytok_config::service_marker::write(paths.data_dir()).unwrap();

    let result = sup.shell_status().now_or_never().unwrap().unwrap();
    let scan = result.status_chips.iter().find(|c| c.id == "scan").unwrap();
    assert_eq!(scan.label, "Live capture active");
    assert_eq!(scan.tone, ToneDto::Success);
}

#[derive(Clone)]
struct BlockingClaudeAdapter {
    started_tx: mpsc::Sender<()>,
    sleep_for: Duration,
}

impl AgentLogAdapter for BlockingClaudeAdapter {
    fn agent(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }

    fn can_parse_path(&self, _path: &Path) -> bool {
        true
    }

    fn parse_line(
        &self,
        _context: &ParseContext,
        _line: &str,
    ) -> Result<Vec<ParsedLogEvent>, ParseError> {
        let _ = self.started_tx.send(());
        std::thread::sleep(self.sleep_for);
        Ok(Vec::new())
    }

    fn write_policy(&self) -> UsageWritePolicy {
        UsageWritePolicy::InsertOnce
    }

    fn clone_boxed(&self) -> Box<dyn AgentLogAdapter + Send + Sync> {
        Box::new(self.clone())
    }
}

#[test]
fn shell_status_returns_while_file_backed_scan_is_in_progress() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("busytok.db");
    let db = Database::open(&db_path).unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    let settings = busytok_config::BusytokSettings::default();
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();

    let root = tmp.path().join("logs");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("session.jsonl"), "{\"event\":\"slow\"}\n").unwrap();

    let (started_tx, started_rx) = mpsc::channel();
    let supervisor = Arc::new(BusytokSupervisor::with_adapters(
        db,
        paths,
        vec![Box::new(BlockingClaudeAdapter {
            started_tx,
            sleep_for: Duration::from_millis(400),
        })],
    ));

    let scan_supervisor = Arc::clone(&supervisor);
    let source = DiscoveredLogSource {
        agent: AgentKind::ClaudeCode,
        source_id: "test-source".to_string(),
        root_path: root.clone(),
        files: vec![root.join("session.jsonl")],
        source_type: LogSourceType::Jsonl,
        configured_by_user: true,
    };
    let scan_thread = std::thread::spawn(move || {
        scan_supervisor
            .run_initial_scan_with_sources(vec![source])
            .unwrap();
    });

    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("scan should start parsing a file");

    let response_supervisor = Arc::clone(&supervisor);
    let (result_tx, result_rx) = mpsc::channel();
    let response_thread = std::thread::spawn(move || {
        let started_at = Instant::now();
        let result = response_supervisor
            .shell_status()
            .now_or_never()
            .unwrap()
            .unwrap();
        result_tx.send((started_at.elapsed(), result)).unwrap();
    });

    let maybe_result = result_rx.recv_timeout(Duration::from_millis(200)).ok();
    let returned_within_budget = maybe_result.is_some();
    scan_thread.join().unwrap();
    let (elapsed, result) =
        maybe_result.unwrap_or_else(|| result_rx.recv_timeout(Duration::from_secs(2)).unwrap());
    response_thread.join().unwrap();

    assert!(
        returned_within_budget && elapsed < Duration::from_millis(200),
        "shell.status should stay responsive during initial scan, took {elapsed:?}"
    );
    assert!(
        !result.status_chips.is_empty(),
        "shell.status should still return a normal payload"
    );
}

#[test]
fn legacy_audit_rebuild_recommended_does_not_delete_existing_events() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    {
        let db = sup.db_handle().lock().unwrap();

        let mut codex = NormalizedUsageEvent::minimal_for_test("codex-bad", AgentKind::Codex);
        codex.session_id = "codex-session".to_string();
        codex.total_tokens = 42;
        codex.model = Some(String::new());
        db.write_usage_event(&codex, UsageWritePolicy::InsertOnce)
            .unwrap();

        let mut claude =
            NormalizedUsageEvent::minimal_for_test("claude-bad", AgentKind::ClaudeCode);
        claude.session_id = "claude-session".to_string();
        claude.input_tokens = 10;
        claude.output_tokens = 5;
        claude.cache_read_tokens = 3;
        claude.total_tokens = 1;
        db.write_usage_event(&claude, UsageWritePolicy::InsertOnce)
            .unwrap();
    }

    let before_count = sup.db_handle().lock().unwrap().usage_event_count().unwrap();
    assert_eq!(before_count, 2);

    let recommended = sup.legacy_audit_rebuild_recommended().unwrap();
    let after_count = sup.db_handle().lock().unwrap().usage_event_count().unwrap();

    assert!(
        recommended,
        "legacy rows should trigger rebuild recommendation"
    );
    assert_eq!(
        after_count, before_count,
        "detection must not delete persisted audit data"
    );
}

// ---------------------------------------------------------------------------
// overview.summary
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_summary_uses_active_generation_read_models() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    let bucket_start_ms = utc_midday_ms_for_test();
    set_active_generation(&db, "gen-active", 1000);
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_hour
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_usd,
              cost_status, event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-active', ?1, 'claude_code', 'sonnet', 500, 0.1, 'exact', 2, 1000, 1000)",
            rusqlite::params![bucket_start_ms],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_hour
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_usd,
              cost_status, event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-shadow', ?1, 'claude_code', 'sonnet', 9999, 9.9, 'exact', 99, 1000, 1000)",
            rusqlite::params![bucket_start_ms],
        )
        .unwrap();

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .overview_summary(OverviewSummaryRequestDto {
            range: RangePresetDto::Day,
        })
        .await
        .unwrap();

    assert!(
        result
            .data
            .metrics
            .iter()
            .any(|card| card.id == "tokens" && card.value == "500"),
        "overview summary should use the active generation's read-model rows"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_summary_counts_non_hour_offset_boundary_usage_in_year_range() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+05:30".to_string(),
        ..BusytokSettings::default()
    };
    let offset = UtcOffset::from_hms(5, 30, 0).unwrap();
    let local_now = OffsetDateTime::now_utc().to_offset(offset);
    let local_year_start = time::Date::from_calendar_date(local_now.year(), Month::January, 1)
        .unwrap()
        .with_time(Time::MIDNIGHT)
        .assume_offset(offset);
    let event_ms = (local_year_start + TimeDuration::minutes(15)).unix_timestamp() * 1_000;

    set_active_generation(&db, "gen-summary-half-hour", today_ms_for_test());
    seed_event(
        &db,
        "summary-half-hour",
        event_ms,
        321,
        Some(0.4),
        Some("claude_code"),
        Some("sonnet"),
        None,
        Some("session-summary-half-hour"),
    );
    assign_event_generation(&db, "summary-half-hour", "gen-summary-half-hour");
    // +05:30 routes through daily_usage (non-whole-hour fixed offset), so seed
    // it the same way the writer's flush path would.
    seed_daily_usage_for_generation(&db, "+05:30", "gen-summary-half-hour");

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .overview_summary(OverviewSummaryRequestDto {
            range: RangePresetDto::Year,
        })
        .await
        .unwrap();

    assert!(
        result
            .data
            .metrics
            .iter()
            .any(|card| card.id == "tokens" && card.value == "321"),
        "overview summary should include boundary usage for non-hour offsets"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn overview_summary_logs_snapshot_read_metadata() {
    let _lock = log_capture_lock().lock().unwrap();
    init_test_logging();
    let logs = test_logs();
    logs.clear();
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    let bucket_start_ms = utc_midday_ms_for_test();
    set_active_generation(&db, "gen-log", 1000);
    set_latest_event_seq(&db, 777);
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_hour
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_usd,
              cost_status, event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-log', ?1, 'claude_code', 'sonnet', 500, 0.1, 'exact', 2, 1000, 1000)",
            rusqlite::params![bucket_start_ms],
        )
        .unwrap();

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    let _ = sup
        .overview_summary(OverviewSummaryRequestDto {
            range: RangePresetDto::Day,
        })
        .await
        .unwrap();

    let lines: Vec<String> = logs.text().lines().map(str::to_owned).collect();
    assert!(
        lines.iter().any(|line| {
            line.contains("read.query.completed")
                && line.contains("method=\"overview.summary\"")
                && line.contains("generation_id=\"gen-log\"")
                && line.contains("readiness=\"ready_exact\"")
                && line.contains("watermark_ms=777")
                && line.contains("used_read_model=true")
        }),
        "overview.summary should log snapshot read metadata, rendered={}",
        logs.text()
    );
}

// ---------------------------------------------------------------------------
// activity.list
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_list_returns_items() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    let today = utc_midday_ms_for_test();
    set_active_generation(&db, "gen-activity", today);

    seed_event(
        &db,
        "evt-a",
        today - 3600000,
        100,
        Some(0.50),
        Some("claude_code"),
        Some("claude-sonnet-4"),
        Some("hash-proj"),
        Some("session-1"),
    );
    assign_event_generation(&db, "evt-a", "gen-activity");
    seed_event(
        &db,
        "evt-b",
        today - 7200000,
        50,
        None,
        Some("claude_code"),
        Some("claude-haiku"),
        None,
        None,
    );
    assign_event_generation(&db, "evt-b", "gen-activity");
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_hour
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_usd,
              cost_status, event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-activity', ?1, 'claude_code', 'claude-sonnet-4', 150, 0.5, 'partial', 2, 1000, 1000)",
            rusqlite::params![today - 7_200_000],
        )
        .unwrap();

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();
    let req = ActivityListRequestDto {
        range: RangePresetDto::Day,
        cursor: None,
        limit: Some(10),
        client_id: None,
        source_id: None,
        project_hash: None,
        model_id: None,
    };
    let result = sup.activity_list(req).await.unwrap().data;

    assert_eq!(result.items.len(), 2);
    assert!(result.generated_at_ms > 0);
    assert_eq!(result.summary.item_count, 2);
    assert_eq!(result.summary.total_tokens, 150);

    // Items should have labels populated
    assert_eq!(result.items[0].client_label, "Claude Code");
    assert!(result.items[0].model_label.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_list_pagination() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    let today = utc_midday_ms_for_test();
    set_active_generation(&db, "gen-activity", today);

    for i in 0..5 {
        seed_event(
            &db,
            &format!("evt-{}", i),
            today - (i as i64 * 3600000),
            10,
            None,
            Some("claude_code"),
            None,
            None,
            None,
        );
        assign_event_generation(&db, &format!("evt-{}", i), "gen-activity");
    }

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();
    let req = ActivityListRequestDto {
        range: RangePresetDto::Day,
        cursor: None,
        limit: Some(2),
        client_id: None,
        source_id: None,
        project_hash: None,
        model_id: None,
    };
    let result = sup.activity_list(req).await.unwrap().data;
    assert_eq!(result.items.len(), 2);
    assert!(result.next_cursor.is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn activity_list_logs_result_row_count() {
    let _lock = log_capture_lock().lock().unwrap();
    init_test_logging();
    let logs = test_logs();
    logs.clear();
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    let today = utc_midday_ms_for_test();
    set_active_generation(&db, "gen-activity-log", today);
    set_latest_event_seq(&db, 999);

    for i in 0..2 {
        seed_event(
            &db,
            &format!("evt-log-{}", i),
            today - (i as i64 * 1_000),
            10,
            None,
            Some("claude_code"),
            Some("claude-sonnet-4"),
            None,
            None,
        );
        assign_event_generation(&db, &format!("evt-log-{}", i), "gen-activity-log");
    }
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_hour
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_usd,
              cost_status, event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-activity-log', ?1, 'claude_code', 'claude-sonnet-4', 20, NULL, 'unavailable', 2, 1000, 1000)",
            rusqlite::params![today],
        )
        .unwrap();

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    let _ = sup
        .activity_list(ActivityListRequestDto {
            range: RangePresetDto::Day,
            cursor: None,
            limit: Some(10),
            client_id: None,
            source_id: None,
            project_hash: None,
            model_id: None,
        })
        .await
        .unwrap();

    assert!(
        wait_for_log_line(&logs, Duration::from_secs(2), |line| {
            line.contains("read.query.completed")
                && line.contains("method=\"activity.list\"")
                && line.contains("row_count=2")
        })
        .await,
        "activity.list should log actual row_count, rendered={}",
        logs.text()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_recent_returns_active_generation_items() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let today = busytok_domain::now_ms();
    set_active_generation(&db, "gen-recent", today);

    seed_event(
        &db,
        "recent-visible",
        today - 1_000,
        42,
        Some(0.02),
        Some("claude_code"),
        Some("claude-sonnet-4"),
        None,
        Some("session-visible"),
    );
    assign_event_generation(&db, "recent-visible", "gen-recent");
    seed_event(
        &db,
        "recent-hidden",
        today - 500,
        999,
        Some(1.0),
        Some("claude_code"),
        Some("claude-sonnet-4"),
        None,
        Some("session-hidden"),
    );
    assign_event_generation(&db, "recent-hidden", "gen-shadow");

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .activity_recent(ActivityRecentRequestDto {
            range: RangePresetDto::Day,
            limit: Some(10),
        })
        .await
        .unwrap()
        .data;

    assert_eq!(result.recent_activity.len(), 1);
    assert_eq!(result.recent_activity[0].id, "recent-visible");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_recent_clamps_oversized_limit_to_500() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let today = busytok_domain::now_ms();
    set_active_generation(&db, "gen-recent-limit", today);

    for i in 0..501 {
        let event_id = format!("recent-limit-{i:03}");
        seed_event(
            &db,
            &event_id,
            today - i as i64,
            1,
            None,
            Some("claude_code"),
            Some("claude-sonnet-4"),
            None,
            Some("session-limit"),
        );
        assign_event_generation(&db, &event_id, "gen-recent-limit");
    }

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .activity_recent(ActivityRecentRequestDto {
            range: RangePresetDto::Day,
            limit: Some(5_000),
        })
        .await
        .unwrap()
        .data;

    assert_eq!(result.recent_activity.len(), 500);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_heatmap_assigns_local_midnight_usage_to_local_day() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+08:00".to_string(),
        ..BusytokSettings::default()
    };
    let offset = UtcOffset::from_hms(8, 0, 0).unwrap();
    let local_now = OffsetDateTime::now_utc().to_offset(offset);
    let target_date = local_now.date() - TimeDuration::DAY;
    let previous_date = target_date - TimeDuration::DAY;
    let local_midnight = target_date.with_time(Time::MIDNIGHT).assume_offset(offset);
    let event_ms = (local_midnight + TimeDuration::minutes(30)).unix_timestamp() * 1_000;
    let hour_bucket_start_ms = event_ms - event_ms.rem_euclid(3_600_000);
    let day_bucket_start_ms = event_ms - event_ms.rem_euclid(86_400_000);

    set_active_generation(&db, "gen-heatmap", today_ms_for_test());
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_hour
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_usd,
              cost_status, event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-heatmap', ?1, 'claude_code', 'sonnet', 321, 0.4, 'exact', 1, 1000, 1000)",
            rusqlite::params![hour_bucket_start_ms],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_day
             (generation_id, bucket_start_ms, agent, model, total_tokens, cost_usd,
              cost_status, event_count, created_at_ms, updated_at_ms)
             VALUES ('gen-heatmap', ?1, 'claude_code', 'sonnet', 321, 0.4, 'exact', 1, 1000, 1000)",
            rusqlite::params![day_bucket_start_ms],
        )
        .unwrap();

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .overview_heatmap(OverviewHeatmapRequestDto {
            range: RangePresetDto::Year,
        })
        .await
        .unwrap()
        .data;

    let target_day = result
        .heatmap
        .days
        .iter()
        .find(|day| day.date == target_date.to_string())
        .expect("target local day should be present");
    let previous_day = result
        .heatmap
        .days
        .iter()
        .find(|day| day.date == previous_date.to_string())
        .expect("previous local day should be present");

    assert_eq!(target_day.tokens, 321);
    assert_eq!(previous_day.tokens, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_trend_counts_non_hour_offset_boundary_usage_in_month_bucket() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+05:30".to_string(),
        ..BusytokSettings::default()
    };
    let offset = UtcOffset::from_hms(5, 30, 0).unwrap();
    let local_now = OffsetDateTime::now_utc().to_offset(offset);
    let local_year_start = time::Date::from_calendar_date(local_now.year(), Month::January, 1)
        .unwrap()
        .with_time(Time::MIDNIGHT)
        .assume_offset(offset);
    let event_ms = (local_year_start + TimeDuration::minutes(15)).unix_timestamp() * 1_000;

    set_active_generation(&db, "gen-trend-half-hour", today_ms_for_test());
    seed_event(
        &db,
        "trend-half-hour",
        event_ms,
        321,
        Some(0.4),
        Some("claude_code"),
        Some("sonnet"),
        None,
        Some("session-trend-half-hour"),
    );
    assign_event_generation(&db, "trend-half-hour", "gen-trend-half-hour");
    // +05:30 routes through daily_usage (non-whole-hour fixed offset), so seed
    // it the same way the writer's flush path would.
    seed_daily_usage_for_generation(&db, "+05:30", "gen-trend-half-hour");

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .overview_trend(OverviewTrendRequestDto {
            range: RangePresetDto::Year,
            granularity: None,
        })
        .await
        .unwrap()
        .data;

    let january_bucket = result
        .trend
        .buckets
        .iter()
        .find(|bucket| bucket.key == format!("{:04}-01", local_now.year()))
        .expect("january bucket should be present");

    assert_eq!(january_bucket.tokens, 321);
    assert_eq!(january_bucket.cost_usd, Some(0.4));
    assert_eq!(january_bucket.event_count, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_heatmap_assigns_non_hour_offset_boundary_usage_to_local_day() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+05:30".to_string(),
        ..BusytokSettings::default()
    };
    let offset = UtcOffset::from_hms(5, 30, 0).unwrap();
    let local_now = OffsetDateTime::now_utc().to_offset(offset);
    let target_date = local_now.date() - TimeDuration::DAY;
    let previous_date = target_date - TimeDuration::DAY;
    let local_midnight = target_date.with_time(Time::MIDNIGHT).assume_offset(offset);
    let event_ms = (local_midnight + TimeDuration::minutes(15)).unix_timestamp() * 1_000;

    set_active_generation(&db, "gen-heatmap-half-hour", today_ms_for_test());
    seed_event(
        &db,
        "heatmap-half-hour",
        event_ms,
        321,
        Some(0.4),
        Some("claude_code"),
        Some("sonnet"),
        None,
        Some("session-heatmap-half-hour"),
    );
    assign_event_generation(&db, "heatmap-half-hour", "gen-heatmap-half-hour");
    // +05:30 routes through daily_usage (non-whole-hour fixed offset), so seed
    // it the same way the writer's flush path would.
    seed_daily_usage_for_generation(&db, "+05:30", "gen-heatmap-half-hour");

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .overview_heatmap(OverviewHeatmapRequestDto {
            range: RangePresetDto::Year,
        })
        .await
        .unwrap()
        .data;

    let target_day = result
        .heatmap
        .days
        .iter()
        .find(|day| day.date == target_date.to_string())
        .expect("target local day should be present");
    let previous_day = result
        .heatmap
        .days
        .iter()
        .find(|day| day.date == previous_date.to_string())
        .expect("previous local day should be present");

    assert_eq!(target_day.tokens, 321);
    assert_eq!(previous_day.tokens, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_rankings_counts_local_midnight_usage_in_local_day_range() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+08:00".to_string(),
        ..BusytokSettings::default()
    };
    let offset = UtcOffset::from_hms(8, 0, 0).unwrap();
    let local_now = OffsetDateTime::now_utc().to_offset(offset);
    let local_midnight = local_now
        .date()
        .with_time(Time::MIDNIGHT)
        .assume_offset(offset);
    let edge_event_ms = local_midnight.unix_timestamp() * 1_000;
    let midday_event_ms = (local_midnight + TimeDuration::hours(12)).unix_timestamp() * 1_000;
    let edge_utc_date = OffsetDateTime::from_unix_timestamp(edge_event_ms / 1_000)
        .unwrap()
        .date()
        .to_string();
    let midday_utc_date = OffsetDateTime::from_unix_timestamp(midday_event_ms / 1_000)
        .unwrap()
        .date()
        .to_string();

    set_active_generation(&db, "gen-rankings", today_ms_for_test());

    seed_event(
        &db,
        "rank-edge",
        edge_event_ms,
        900,
        Some(0.9),
        Some("claude_code"),
        Some("claude-sonnet-4"),
        Some("proj-edge"),
        Some("session-edge"),
    );
    assign_event_generation(&db, "rank-edge", "gen-rankings");
    db.conn()
        .execute(
            "UPDATE usage_events SET project_path = '/repo/edge' WHERE id = 'rank-edge'",
            [],
        )
        .unwrap();

    seed_event(
        &db,
        "rank-midday",
        midday_event_ms,
        300,
        Some(0.3),
        Some("claude_code"),
        Some("claude-haiku-4"),
        Some("proj-midday"),
        Some("session-midday"),
    );
    assign_event_generation(&db, "rank-midday", "gen-rankings");
    db.conn()
        .execute(
            "UPDATE usage_events SET project_path = '/repo/midday' WHERE id = 'rank-midday'",
            [],
        )
        .unwrap();

    db.conn()
        .execute(
            "INSERT INTO usage_by_project_day
             (generation_id, date, project_id, project_path, agent, model,
              total_tokens, cost_usd, event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES
             ('gen-rankings', ?1, 'proj-edge', '/repo/edge', 'claude_code', 'claude-sonnet-4', 900, 0.9, 1, ?2, 1000, 1000),
             ('gen-rankings', ?3, 'proj-midday', '/repo/midday', 'claude_code', 'claude-haiku-4', 300, 0.3, 1, ?4, 1000, 1000)",
            rusqlite::params![edge_utc_date, edge_event_ms, midday_utc_date, midday_event_ms],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_by_model_day
             (generation_id, date, agent, model,
              total_tokens, cost_usd, event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES
             ('gen-rankings', ?1, 'claude_code', 'claude-sonnet-4', 900, 0.9, 1, ?2, 1000, 1000),
             ('gen-rankings', ?3, 'claude_code', 'claude-haiku-4', 300, 0.3, 1, ?4, 1000, 1000)",
            rusqlite::params![
                edge_utc_date,
                edge_event_ms,
                midday_utc_date,
                midday_event_ms
            ],
        )
        .unwrap();

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .overview_rankings(OverviewRankingsRequestDto {
            range: RangePresetDto::Day,
        })
        .await
        .unwrap()
        .data;

    let model_section = result
        .rankings
        .iter()
        .find(|section| section.id == "models")
        .expect("models section should be present");
    assert_eq!(model_section.items[0].id, "claude-sonnet-4");
}

#[tokio::test]
async fn overview_rankings_returns_costs_section_before_models() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);

    let now_ms = busytok_domain::now_ms();
    let today_date = OffsetDateTime::from_unix_timestamp(now_ms / 1000)
        .unwrap()
        .date()
        .to_string();

    set_active_generation(&db, "gen-costs", now_ms);

    // Model A: high tokens, low cost. Model B: low tokens, high cost.
    seed_event(
        &db,
        "evt-cheap",
        now_ms - 3600_000,
        1000,
        Some(0.1),
        Some("claude_code"),
        Some("high-tok-low-cost"),
        Some("proj-a"),
        Some("session-a"),
    );
    assign_event_generation(&db, "evt-cheap", "gen-costs");

    seed_event(
        &db,
        "evt-pricey",
        now_ms - 1800_000,
        100,
        Some(1.0),
        Some("claude_code"),
        Some("low-tok-high-cost"),
        Some("proj-b"),
        Some("session-b"),
    );
    assign_event_generation(&db, "evt-pricey", "gen-costs");

    db.conn()
        .execute(
            "INSERT INTO usage_by_model_day
             (generation_id, date, agent, model,
              total_tokens, cost_usd, cost_status, event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES
             ('gen-costs', ?1, 'claude_code', 'high-tok-low-cost',
              1000, 0.1, 'exact', 1, ?2, 1000, 1000),
             ('gen-costs', ?1, 'claude_code', 'low-tok-high-cost',
              100, 1.0, 'exact', 1, ?2, 1000, 1000)",
            rusqlite::params![today_date, now_ms],
        )
        .unwrap();

    let sup = make_supervisor_with_settings(
        db,
        &tmp,
        BusytokSettings {
            timezone: "UTC".to_string(),
            ..BusytokSettings::default()
        },
    );
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .overview_rankings(OverviewRankingsRequestDto {
            range: RangePresetDto::Day,
        })
        .await
        .unwrap()
        .data;

    // First section should be costs, not projects.
    assert_eq!(result.rankings[0].id, "costs");
    assert_eq!(result.rankings[0].title, "Top Costs");
    // Highest-cost model should be first.
    assert_eq!(result.rankings[0].items[0].id, "low-tok-high-cost");
    // Bar value should be 100.0 (it is the max).
    assert!((result.rankings[0].items[0].bar_value - 100.0).abs() < 0.1);

    // Models section should be second.
    assert_eq!(result.rankings[1].id, "models");
}

fn today_ms_for_test() -> i64 {
    busytok_domain::now_ms()
}

fn utc_midday_ms_for_test() -> i64 {
    let today = OffsetDateTime::now_utc().date();
    today
        .with_time(Time::MIDNIGHT)
        .assume_utc()
        .unix_timestamp()
        * 1_000
        + 12 * 3_600_000
}

fn utc_today_date_for_test() -> String {
    OffsetDateTime::now_utc().date().to_string()
}

fn seed_usage_by_project_day_row(
    db: &Database,
    generation_id: &str,
    date: &str,
    project_id: &str,
    project_path: &str,
    agent: &str,
    model: &str,
    total_tokens: i64,
    cost_usd: Option<f64>,
    event_count: i64,
    last_active_at_ms: i64,
) {
    db.conn()
        .execute(
            "INSERT INTO usage_by_project_day
             (generation_id, date, project_id, project_path, agent, model,
              input_tokens, output_tokens, total_tokens, cost_usd, cost_status,
              event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, ?7, ?8,
                     CASE WHEN ?8 IS NOT NULL THEN 'exact' ELSE 'unavailable' END,
                     ?9, ?10, 1000, 1000)",
            rusqlite::params![
                generation_id,
                date,
                project_id,
                project_path,
                agent,
                model,
                total_tokens,
                cost_usd,
                event_count,
                last_active_at_ms
            ],
        )
        .unwrap();
}

fn seed_usage_by_model_day_row(
    db: &Database,
    generation_id: &str,
    date: &str,
    agent: &str,
    model: &str,
    total_tokens: i64,
    cost_usd: Option<f64>,
    event_count: i64,
    last_active_at_ms: i64,
) {
    db.conn()
        .execute(
            "INSERT INTO usage_by_model_day
             (generation_id, date, agent, model, input_tokens, output_tokens,
              total_tokens, cost_usd, cost_status, event_count, last_active_at_ms,
              created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, 0, 0, ?5, ?6,
                     CASE WHEN ?6 IS NOT NULL THEN 'exact' ELSE 'unavailable' END,
                     ?7, ?8, 1000, 1000)",
            rusqlite::params![
                generation_id,
                date,
                agent,
                model,
                total_tokens,
                cost_usd,
                event_count,
                last_active_at_ms
            ],
        )
        .unwrap();
}

fn seed_usage_by_session_day_row(
    db: &Database,
    generation_id: &str,
    date: &str,
    session_id: &str,
    agent: &str,
    client_kind: Option<&str>,
    project_path: Option<&str>,
    project_hash: Option<&str>,
    model: Option<&str>,
    total_tokens: i64,
    cost_usd: Option<f64>,
    event_count: i64,
    last_active_at_ms: i64,
) {
    db.conn()
        .execute(
            "INSERT INTO usage_by_session_day
             (generation_id, date, session_id, agent, client_kind, project_path,
              project_hash, model, input_tokens, output_tokens, total_tokens, cost_usd,
              cost_status, event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 0, ?9, ?10,
                     CASE WHEN ?10 IS NOT NULL THEN 'exact' ELSE 'unavailable' END,
                     ?11, ?12, 1000, 1000)",
            rusqlite::params![
                generation_id,
                date,
                session_id,
                agent,
                client_kind,
                project_path,
                project_hash,
                model,
                total_tokens,
                cost_usd,
                event_count,
                last_active_at_ms
            ],
        )
        .unwrap();
}

fn seed_source_health_summary_row(
    db: &Database,
    generation_id: &str,
    source_id: &str,
    agent: &str,
    root_path: &str,
    source_type: &str,
    status: &str,
    configured_by_user: bool,
    last_scan_at_ms: i64,
    file_count: i64,
    parsed_file_count: i64,
    event_count: i64,
    last_error: Option<&str>,
    latest_activity_at_ms: i64,
) {
    db.conn()
        .execute(
            "INSERT INTO source_health_summary
             (generation_id, source_id, agent, root_path, source_type, status,
              configured_by_user, last_scan_at_ms, file_count, parsed_file_count,
              event_count, last_error, latest_activity_at_ms, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 1000, 1000)",
            rusqlite::params![
                generation_id,
                source_id,
                agent,
                root_path,
                source_type,
                status,
                configured_by_user as i32,
                last_scan_at_ms,
                file_count,
                parsed_file_count,
                event_count,
                last_error,
                latest_activity_at_ms
            ],
        )
        .unwrap();
}

// ---------------------------------------------------------------------------
// activity.detail
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_detail_returns_full_event() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    set_active_generation(&db, "gen-detail", 1000);

    let mut event = NormalizedUsageEvent::minimal_for_test("detail-evt-1", AgentKind::ClaudeCode);
    event.timestamp_ms = 1000;
    event.total_tokens = 500;
    event.cost_usd = Some(2.50);
    event.client_kind = Some("claude_code".to_string());
    event.model = Some("claude-sonnet-4-20250514".to_string());
    event.project_hash = Some("hash-xyz".to_string());
    event.project_path = Some("/home/user/project".to_string());
    event.session_id = "session-42".to_string();
    event.input_tokens = 200;
    event.output_tokens = 300;
    event.is_error = false;
    db.write_usage_event(&event, busytok_domain::UsageWritePolicy::InsertOnce)
        .unwrap();
    assign_event_generation(&db, "detail-evt-1", "gen-detail");

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();
    let req = ActivityDetailRequestDto {
        id: "detail-evt-1".to_string(),
    };
    let result = sup.activity_detail(req).await.unwrap().data;

    assert_eq!(result.id, "detail-evt-1");
    assert_eq!(result.tokens, 500);
    assert_eq!(result.client_id, "claude_code");
    assert_eq!(result.client_label, "Claude Code");
    assert!(result.token_breakdown.is_some());
    let tb = result.token_breakdown.unwrap();
    assert_eq!(tb.total_tokens, 500);
    assert_eq!(tb.input_tokens, Some(200));
    assert_eq!(tb.output_tokens, Some(300));
    assert_eq!(result.status, ActivityStatusDto::Ok);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_detail_not_found() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    set_active_generation(&db, "gen-detail", 1000);
    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();
    let req = ActivityDetailRequestDto {
        id: "nonexistent".to_string(),
    };
    let result = sup.activity_detail(req).await;
    assert!(result.is_err(), "should error for missing event");
}

// ---------------------------------------------------------------------------
// breakdown.list
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn breakdown_list_project() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    let date = utc_today_date_for_test();

    set_active_generation(&db, "gen-breakdown-project", today_ms_for_test());
    seed_usage_by_project_day_row(
        &db,
        "gen-breakdown-project",
        &date,
        "hash-a",
        "/repo/a",
        "claude_code",
        "model-a",
        300,
        None,
        2,
        2_000,
    );
    seed_usage_by_project_day_row(
        &db,
        "gen-breakdown-project",
        &date,
        "hash-b",
        "/repo/b",
        "codex",
        "model-b",
        300,
        None,
        1,
        3_000,
    );

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();
    let req = BreakdownListRequestDto {
        kind: BreakdownKindDto::Project,
        range: RangePresetDto::Day,
        cursor: None,
        limit: Some(10),
    };
    let result = sup.breakdown_list(req).await.unwrap().data;

    assert_eq!(result.items.len(), 2);
    assert_eq!(result.summary.total_tokens, 600);
    assert_eq!(result.summary.item_count, 2);
    assert_eq!(result.kind, BreakdownKindDto::Project);
    // Items should be sorted by total_tokens DESC
    let tokens: Vec<i64> = result
        .items
        .iter()
        .map(|item| match item {
            BreakdownListItemDto::Project(p) => p.tokens,
            _ => panic!("expected Project variant"),
        })
        .collect();
    assert_eq!(tokens[0], 300, "hash-b has 300 tokens, should be first");
    assert_eq!(tokens[1], 300, "hash-a has 300 tokens as well");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn breakdown_list_model() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    let date = utc_today_date_for_test();

    set_active_generation(&db, "gen-breakdown-model", today_ms_for_test());
    seed_usage_by_model_day_row(
        &db,
        "gen-breakdown-model",
        &date,
        "claude_code",
        "model-a",
        100,
        None,
        1,
        2_000,
    );
    seed_usage_by_model_day_row(
        &db,
        "gen-breakdown-model",
        &date,
        "codex",
        "model-b",
        200,
        None,
        1,
        3_000,
    );
    seed_usage_by_project_day_row(
        &db,
        "gen-breakdown-model",
        &date,
        "hash-a",
        "/repo/a",
        "claude_code",
        "model-a",
        100,
        None,
        1,
        2_000,
    );
    seed_usage_by_project_day_row(
        &db,
        "gen-breakdown-model",
        &date,
        "hash-b",
        "/repo/b",
        "codex",
        "model-b",
        200,
        None,
        1,
        3_000,
    );

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();
    let req = BreakdownListRequestDto {
        kind: BreakdownKindDto::Model,
        range: RangePresetDto::Day,
        cursor: None,
        limit: Some(10),
    };
    let result = sup.breakdown_list(req).await.unwrap().data;

    assert_eq!(result.items.len(), 2);
    assert_eq!(result.kind, BreakdownKindDto::Model);
    // Model items should have client_labels when used by known clients
    for item in &result.items {
        match item {
            BreakdownListItemDto::Model(m) => {
                assert!(
                    !m.client_labels.is_empty(),
                    "model '{}' should have non-empty client_labels",
                    m.id
                );
            }
            _ => panic!("expected Model variant"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn breakdown_list_session() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    let date = utc_today_date_for_test();

    set_active_generation(&db, "gen-breakdown-session", today_ms_for_test());
    seed_usage_by_session_day_row(
        &db,
        "gen-breakdown-session",
        &date,
        "session-1",
        "claude_code",
        Some("claude_code"),
        None,
        None,
        None,
        100,
        None,
        1,
        2_000,
    );
    seed_usage_by_session_day_row(
        &db,
        "gen-breakdown-session",
        &date,
        "session-2",
        "codex",
        Some("codex"),
        None,
        None,
        None,
        200,
        None,
        1,
        3_000,
    );

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();
    let req = BreakdownListRequestDto {
        kind: BreakdownKindDto::Session,
        range: RangePresetDto::Day,
        cursor: None,
        limit: Some(10),
    };
    let result = sup.breakdown_list(req).await.unwrap().data;

    assert_eq!(result.items.len(), 2);
    assert_eq!(result.kind, BreakdownKindDto::Session);
    // Session items should have client_label and project_label populated
    for item in &result.items {
        match item {
            BreakdownListItemDto::Session(s) => {
                assert!(
                    !s.client_label.is_empty(),
                    "session '{}' should have non-empty client_label",
                    s.id
                );
                // project_label can be None if no project_hash on the event
                assert!(
                    s.project_hash.is_none(),
                    "session '{}' should have None project_hash (no project set)",
                    s.id
                );
            }
            _ => panic!("expected Session variant"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn breakdown_list_reads_materialized_project_rows_without_usage_events() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    let today = utc_today_date_for_test();

    set_active_generation(&db, "gen-breakdown-read-model", today_ms_for_test());
    db.conn()
        .execute(
            "INSERT INTO usage_by_project_day
             (generation_id, date, project_id, project_path, total_tokens, cost_usd,
              cost_status, event_count, last_active_at_ms, created_at_ms, updated_at_ms)
             VALUES ('gen-breakdown-read-model', ?1, 'hash-materialized', '/repo/materialized',
                     321, 0.4, 'exact', 2, 2000, 1000, 1000)",
            rusqlite::params![today],
        )
        .unwrap();

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .breakdown_list(BreakdownListRequestDto {
            kind: BreakdownKindDto::Project,
            range: RangePresetDto::Day,
            cursor: None,
            limit: Some(10),
        })
        .await
        .unwrap()
        .data;

    assert_eq!(result.items.len(), 1);
    match &result.items[0] {
        BreakdownListItemDto::Project(project) => {
            assert_eq!(project.id, "hash-materialized");
            assert_eq!(project.tokens, 321);
        }
        _ => panic!("expected Project variant"),
    }
}

// ---------------------------------------------------------------------------
// breakdown.detail
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn breakdown_detail_project() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    let today = utc_midday_ms_for_test();
    let date = utc_today_date_for_test();
    set_active_generation(&db, "gen-breakdown-detail-project", today_ms_for_test());

    seed_event(
        &db,
        "e1",
        today - 3600000,
        100,
        Some(1.0),
        Some("claude_code"),
        Some("sonnet-4"),
        Some("hash-proj"),
        None,
    );
    assign_event_generation(&db, "e1", "gen-breakdown-detail-project");
    seed_event(
        &db,
        "e2",
        today - 1800000,
        200,
        Some(2.0),
        Some("claude_code"),
        Some("haiku"),
        Some("hash-proj"),
        None,
    );
    assign_event_generation(&db, "e2", "gen-breakdown-detail-project");
    seed_usage_by_session_day_row(
        &db,
        "gen-breakdown-detail-project",
        &date,
        "session-default",
        "claude_code",
        Some("claude_code"),
        None,
        Some("hash-proj"),
        Some("sonnet-4"),
        300,
        Some(3.0),
        2,
        today,
    );

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();
    let req = BreakdownDetailRequestDto {
        kind: BreakdownKindDto::Project,
        id: "hash-proj".to_string(),
        range: RangePresetDto::Day,
    };
    let result = sup.breakdown_detail(req).await.unwrap().data;

    match result {
        BreakdownDetailDto::Project(p) => {
            assert_eq!(p.id, "hash-proj");
            assert_eq!(p.metrics.len(), 3);
            assert!(p.model_mix.len() >= 1);
            assert!(
                !p.sessions.is_empty(),
                "project detail should have non-empty sessions when events exist in the project"
            );
        }
        _ => panic!("expected Project breakdown detail"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn breakdown_detail_model() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    set_active_generation(&db, "gen-breakdown-detail-model", today_ms_for_test());

    seed_event(
        &db,
        "e1",
        1000,
        100,
        Some(1.0),
        Some("claude_code"),
        Some("sonnet-4"),
        Some("hash-a"),
        None,
    );
    assign_event_generation(&db, "e1", "gen-breakdown-detail-model");
    seed_event(
        &db,
        "e2",
        2000,
        200,
        Some(2.0),
        Some("codex"),
        Some("sonnet-4"),
        Some("hash-b"),
        None,
    );
    assign_event_generation(&db, "e2", "gen-breakdown-detail-model");

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();
    let req = BreakdownDetailRequestDto {
        kind: BreakdownKindDto::Model,
        id: "sonnet-4".to_string(),
        range: RangePresetDto::Day,
    };
    let result = sup.breakdown_detail(req).await.unwrap().data;

    match result {
        BreakdownDetailDto::Model(m) => {
            assert_eq!(m.id, "sonnet-4");
            assert_eq!(m.metrics.len(), 3);
        }
        _ => panic!("expected Model breakdown detail"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn breakdown_detail_session() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    let now = utc_midday_ms_for_test();
    set_active_generation(&db, "gen-breakdown-detail-session", today_ms_for_test());

    seed_event(
        &db,
        "e1",
        now - 3600000,
        100,
        Some(1.0),
        Some("claude_code"),
        Some("sonnet-4"),
        Some("hash-a"),
        Some("session-xyz"),
    );
    assign_event_generation(&db, "e1", "gen-breakdown-detail-session");
    seed_event(
        &db,
        "e2",
        now - 7200000,
        200,
        Some(2.0),
        Some("claude_code"),
        Some("sonnet-4"),
        Some("hash-a"),
        Some("session-xyz"),
    );
    assign_event_generation(&db, "e2", "gen-breakdown-detail-session");
    seed_event(
        &db,
        "e3",
        now - 1800000,
        300,
        Some(3.0),
        Some("claude_code"),
        Some("haiku"),
        Some("hash-a"),
        Some("session-xyz"),
    );
    assign_event_generation(&db, "e3", "gen-breakdown-detail-session");

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();
    let req = BreakdownDetailRequestDto {
        kind: BreakdownKindDto::Session,
        id: "session-xyz".to_string(),
        range: RangePresetDto::Day,
    };
    let result = sup.breakdown_detail(req).await.unwrap().data;

    match result {
        BreakdownDetailDto::Session(s) => {
            assert_eq!(s.id, "session-xyz");
            assert_eq!(s.metrics.len(), 3);
            assert_eq!(s.client_label, "Claude Code");
            // project_label comes from project_path, which was not set in seed
            assert_eq!(s.project_label, None);
            // Timeline should be in chronological order (ASC by happened_at_ms)
            assert_eq!(s.timeline.len(), 3, "should have 3 timeline items");
            for i in 1..s.timeline.len() {
                assert!(
                    s.timeline[i - 1].happened_at_ms <= s.timeline[i].happened_at_ms,
                    "timeline items should be in chronological ASC order: item {} ({}ms) before item {} ({}ms)",
                    i - 1, s.timeline[i - 1].happened_at_ms,
                    i, s.timeline[i].happened_at_ms,
                );
            }
        }
        _ => panic!("expected Session breakdown detail"),
    }
}

// ---------------------------------------------------------------------------
// clients.snapshot
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clients_snapshot_returns_full_response() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);

    set_active_generation(&db, "gen-clients-full", today_ms_for_test());
    seed_source_health_summary_row(
        &db,
        "gen-clients-full",
        "src-1",
        "claude_code",
        "/tmp/cc",
        "default",
        "active",
        true,
        2_000,
        3,
        2,
        11,
        None,
        3_000,
    );
    seed_source_health_summary_row(
        &db,
        "gen-clients-full",
        "src-2",
        "codex",
        "/tmp/cx",
        "custom",
        "active",
        true,
        4_000,
        5,
        5,
        13,
        Some("boom"),
        5_000,
    );

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();
    let req = ClientsSnapshotRequestDto {
        cursor: None,
        limit: None,
        client_id: None,
        scan_state: None,
    };
    let result = sup.clients_snapshot(req).await.unwrap().data;

    assert!(result.generated_at_ms > 0);
    assert_eq!(result.client_cards.len(), 2, "should have two client cards");
    assert_eq!(result.sources.len(), 2);
    assert_eq!(result.summary.source_count, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clients_snapshot_filters_by_client() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);

    set_active_generation(&db, "gen-clients-filter", today_ms_for_test());
    seed_source_health_summary_row(
        &db,
        "gen-clients-filter",
        "src-1",
        "claude_code",
        "/tmp/cc",
        "default",
        "active",
        true,
        2_000,
        3,
        2,
        11,
        None,
        3_000,
    );
    seed_source_health_summary_row(
        &db,
        "gen-clients-filter",
        "src-2",
        "codex",
        "/tmp/cx",
        "custom",
        "active",
        true,
        4_000,
        5,
        5,
        13,
        None,
        5_000,
    );

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();
    let req = ClientsSnapshotRequestDto {
        cursor: None,
        limit: None,
        client_id: Some("claude_code".to_string()),
        scan_state: None,
    };
    let result = sup.clients_snapshot(req).await.unwrap().data;

    assert_eq!(result.sources.len(), 1);
    assert_eq!(result.sources[0].client_id, "claude_code");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clients_snapshot_reads_summary_tables_without_log_sources() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);

    set_active_generation(&db, "gen-clients-read-model", today_ms_for_test());
    db.conn()
        .execute(
            "INSERT INTO source_health_summary
             (generation_id, source_id, agent, root_path, source_type, status,
              configured_by_user, last_scan_at_ms, file_count, parsed_file_count,
              event_count, last_error, latest_activity_at_ms, created_at_ms, updated_at_ms)
             VALUES ('gen-clients-read-model', 'src-summary-1', 'claude_code', '/repo/claude', 'default',
                     'active', 1, 2000, 3, 2, 11, NULL, 3000, 1000, 1000),
                    ('gen-clients-read-model', 'src-summary-2', 'codex', '/repo/codex', 'custom',
                     'active', 0, 4000, 5, 5, 13, 'boom', 5000, 1000, 1000)",
            [],
        )
        .unwrap();

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    let result = sup
        .clients_snapshot(ClientsSnapshotRequestDto {
            cursor: None,
            limit: Some(10),
            client_id: None,
            scan_state: None,
        })
        .await
        .unwrap()
        .data;

    assert_eq!(result.sources.len(), 2);
    assert_eq!(result.client_cards.len(), 2);
    assert_eq!(result.summary.source_count, 2);
}

// ---------------------------------------------------------------------------
// clients.detail
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clients_detail_returns_source() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);

    seed_source(
        &db,
        "src-detail-1",
        "claude_code",
        "/tmp/cc",
        "active",
        "jsonl",
    );
    seed_log_file(
        &db,
        "lf-1",
        "src-detail-1",
        "claude_code",
        "/tmp/cc/log.jsonl",
    );

    let sup = make_supervisor(db, &tmp);
    let req = ClientSourceDetailRequestDto {
        source_id: "src-detail-1".to_string(),
    };
    let result = sup.clients_detail(req).await.unwrap().data;

    assert_eq!(result.source.id, "src-detail-1");
    assert_eq!(result.source.client_id, "claude_code");
    assert_eq!(result.source.client_label, "Claude Code");
    assert_eq!(result.source.root_path, "/tmp/cc");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clients_detail_not_found() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = make_file_backed_db(&tmp);
    let sup = make_supervisor(db, &tmp);
    let req = ClientSourceDetailRequestDto {
        source_id: "nonexistent".to_string(),
    };
    let result = sup.clients_detail(req).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// settings.snapshot
// ---------------------------------------------------------------------------

#[test]
fn settings_snapshot_returns_defaults() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let result = sup
        .settings_snapshot()
        .now_or_never()
        .unwrap()
        .unwrap()
        .data;
    assert!(!result.timezone.is_empty());
    assert_eq!(result.week_starts_on, WeekdayIndexDto::MONDAY);
    assert!(result.discovery.claude_code_default_paths);
    assert!(result.discovery.codex_default_paths);
    assert_eq!(
        result.recovery_actions.len(),
        3,
        "should have 3 recovery actions"
    );
}

// ---------------------------------------------------------------------------
// settings.diagnostics
// ---------------------------------------------------------------------------

#[test]
fn settings_diagnostics_returns_real_data() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();

    // Seed events and diagnostics so the health check returns non-zero values.
    seed_event(&db, "e1", 1000, 100, None, None, None, None, None);
    seed_diagnostic(&db, "d1", "src-1", "warning");

    let sup = make_supervisor(db, &tmp);
    let result = sup
        .settings_diagnostics()
        .now_or_never()
        .unwrap()
        .unwrap()
        .data;

    assert!(result.db_healthy);
    assert_eq!(result.usage_event_count, 1);
}

// ---------------------------------------------------------------------------
// prompts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn supervisor_prompt_palette_crud_search_and_use() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let supervisor = make_supervisor(db, &tmp);

    let created = supervisor
        .prompts_create(PromptCreateRequestDto {
            alias: Some(";;review".to_string()),
            content: "Review this diff for bugs.".to_string(),
            tags: vec!["Review".to_string()],
        })
        .await
        .unwrap();

    assert_eq!(created.data.alias.as_deref(), Some(";;review"));
    let prompt_id = created.data.id.clone();

    let fetched = supervisor
        .prompts_get(PromptGetRequestDto {
            id: prompt_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(fetched.data.alias.as_deref(), Some(";;review"));
    assert_eq!(fetched.data.content, "Review this diff for bugs.");
    assert_eq!(fetched.data.tags, vec!["Review".to_string()]);
    assert!(!fetched.data.is_pinned);

    let updated = supervisor
        .prompts_update(PromptUpdateRequestDto {
            id: prompt_id.clone(),
            alias: Some(";;patch-review".to_string()),
            content: "Review this patch for correctness and regressions.".to_string(),
            tags: vec!["Updated".to_string(), "Patch".to_string()],
            is_pinned: false,
        })
        .await
        .unwrap();
    assert_eq!(updated.data.id, prompt_id);
    assert_eq!(updated.data.alias.as_deref(), Some(";;patch-review"));
    assert_eq!(
        updated.data.content,
        "Review this patch for correctness and regressions."
    );
    assert_eq!(
        updated.data.tags,
        vec!["Updated".to_string(), "Patch".to_string()]
    );
    assert!(!updated.data.is_pinned);

    let fetched_updated = supervisor
        .prompts_get(PromptGetRequestDto {
            id: prompt_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        fetched_updated.data.alias.as_deref(),
        Some(";;patch-review")
    );
    assert_eq!(
        fetched_updated.data.content,
        "Review this patch for correctness and regressions."
    );
    assert_eq!(
        fetched_updated.data.tags,
        vec!["Updated".to_string(), "Patch".to_string()]
    );
    assert!(!fetched_updated.data.is_pinned);

    let listed = supervisor
        .prompts_list(PromptListQueryDto {
            query: Some(";;patch-review".to_string()),
            tag: None,
            sort: Some(PromptSortDto::Smart),
            limit: Some(50),
        })
        .await
        .unwrap();
    assert_eq!(listed.data.total_count, 1);
    assert_eq!(listed.data.entries.len(), 1);
    assert_eq!(listed.data.entries[0].id, prompt_id);
    assert_eq!(
        listed.data.entries[0].alias.as_deref(),
        Some(";;patch-review")
    );

    let used = supervisor
        .prompts_use(PromptUseRequestDto {
            id: prompt_id.clone(),
            action: PromptActionDto::CopyAndPaste,
            surface: PromptUseSurfaceDto::Overlay,
            outcome: PromptUseOutcomeDto::PasteAttempted,
            failure_reason: None,
        })
        .await
        .unwrap();
    assert_eq!(used.usage_count, 1);

    let deleted = supervisor
        .prompts_delete(PromptDeleteRequestDto {
            id: prompt_id.clone(),
        })
        .await
        .unwrap();
    assert!(deleted.deleted);

    let listed_after_delete = supervisor
        .prompts_list(PromptListQueryDto {
            query: Some(";;patch-review".to_string()),
            tag: None,
            sort: Some(PromptSortDto::Smart),
            limit: Some(50),
        })
        .await
        .unwrap();
    assert_eq!(listed_after_delete.data.total_count, 0);

    let get_after_delete = supervisor
        .prompts_get(PromptGetRequestDto {
            id: prompt_id.clone(),
        })
        .await;
    assert!(
        get_after_delete
            .unwrap_err()
            .to_string()
            .contains(&prompt_id),
        "deleted prompt get should fail with prompt id in error"
    );

    supervisor.shutdown_writer().await.unwrap();
}

#[test]
fn prompts_list_logs_lengths_without_query_contents() {
    let db = Database::open_in_memory().unwrap();
    db.create_prompt_entry(busytok_store::NewPromptEntryRow {
        alias: Some(";;review".to_string()),
        content: "Summarize the attached diff".to_string(),
        tags: vec!["engineering".to_string()],
    })
    .unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let supervisor = make_supervisor(db, &tmp);
    let logs = SharedLogBuffer::default();

    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .without_time()
        .with_writer(logs.clone())
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        supervisor
            .prompts_list(PromptListQueryDto {
                query: Some("secret needle".to_string()),
                tag: Some("private-tag".to_string()),
                sort: Some(PromptSortDto::Smart),
                limit: Some(10),
            })
            .now_or_never()
            .unwrap()
            .unwrap();
    });

    let rendered = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(
        rendered.contains("has_query=true"),
        "logs should record query presence without the raw query, rendered={rendered}"
    );
    assert!(rendered.contains("query_len=13"));
    assert!(rendered.contains("has_tag=true"));
    assert!(rendered.contains("tag_len=11"));
    assert!(!rendered.contains("secret needle"));
    assert!(!rendered.contains("private-tag"));
}

// ---------------------------------------------------------------------------
// settings.recovery_action
// ---------------------------------------------------------------------------

#[test]
fn settings_recovery_action_rescan_all() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let req = SettingsRecoveryActionRequestDto {
        id: SettingsRecoveryActionIdDto::RescanAll,
    };
    let result = sup
        .settings_recovery_action(req)
        .now_or_never()
        .unwrap()
        .unwrap()
        .data;
    assert_eq!(result.id, SettingsRecoveryActionIdDto::RescanAll);
    assert!(!result.accepted);
    assert!(result.message.contains("writer-backed background job"));
}

#[tokio::test]
async fn settings_recovery_action_rebuild_rollups() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let req = SettingsRecoveryActionRequestDto {
        id: SettingsRecoveryActionIdDto::RebuildRollups,
    };
    let result = sup.settings_recovery_action(req).await.unwrap().data;
    assert_eq!(result.id, SettingsRecoveryActionIdDto::RebuildRollups);
    assert!(result.accepted);
    assert!(result.message.contains("writer actor"));
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_recovery_action_rebuild_rollups_with_events_but_no_active_generation_returns_structured_error(
) {
    // Regression: previously, when usage_events existed but service_state had no
    // active_generation_id, handle_rebuild_rollups hit an early `?` return that
    // dropped cmd.respond_tx, so callers saw "writer rollup rebuild response
    // channel dropped" — a misleading message that hid the real reason.
    // Contract: the writer must always respond, and the surfaced message must
    // name the actual cause.
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();

    // Seed an event without setting any active generation.
    seed_event(
        &db,
        "evt-orphan",
        1_000,
        100,
        None,
        Some("claude_code"),
        Some("sonnet"),
        None,
        None,
    );

    let sup = make_supervisor(db, &tmp);
    let req = SettingsRecoveryActionRequestDto {
        id: SettingsRecoveryActionIdDto::RebuildRollups,
    };
    let result = sup.settings_recovery_action(req).await.unwrap().data;
    assert_eq!(result.id, SettingsRecoveryActionIdDto::RebuildRollups);
    assert!(
        !result.accepted,
        "rebuild with events but no active generation should not be accepted"
    );
    assert!(
        !result.message.contains("channel dropped"),
        "error message must not mask the cause with 'channel dropped'; got: {}",
        result.message
    );
    assert!(
        result.message.to_lowercase().contains("active generation"),
        "error message should name the missing active generation; got: {}",
        result.message
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn settings_recovery_action_reset_failed_checkpoints() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();

    // Seed a log file in error state.
    let conn = db.conn();
    conn.execute(
        "INSERT INTO log_files (id, source_id, agent, path, state, \
         first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms) \
         VALUES ('lf-err', 'src-1', 'claude_code', '/tmp/test', 'error', \
         1000, 1000, 1000, 1000)",
        [],
    )
    .unwrap();

    let sup = make_supervisor(db, &tmp);
    let req = SettingsRecoveryActionRequestDto {
        id: SettingsRecoveryActionIdDto::ResetFailedCheckpoints,
    };
    let result = sup.settings_recovery_action(req).await.unwrap().data;
    assert_eq!(
        result.id,
        SettingsRecoveryActionIdDto::ResetFailedCheckpoints
    );
    assert!(result.accepted);
    assert!(result.message.contains("1"));
    sup.shutdown_writer().await.unwrap();
}

// ---------------------------------------------------------------------------
// settings.update validation
// ---------------------------------------------------------------------------

#[test]
fn settings_update_empty_timezone_rejected() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let req = SettingsUpdateRequestDto {
        timezone: Some(String::new()),
        week_starts_on: None,
        discovery: None,
        privacy: None,
        prompt_palette_default_action: None,
    };
    let result = sup.settings_update(req).now_or_never().unwrap();
    assert!(result.is_err(), "empty timezone should be rejected");
    let err = result.unwrap_err().to_string();
    assert!(
        err.starts_with("SETTINGS_VALIDATION_FAILED:"),
        "should start with SETTINGS_VALIDATION_FAILED: got: {}",
        err
    );
    // Extract and parse the JSON payload after the prefix.
    let payload_str = err
        .strip_prefix("SETTINGS_VALIDATION_FAILED: ")
        .unwrap_or(&err);
    let payload: serde_json::Value = serde_json::from_str(payload_str)
        .expect("validation error should carry parseable JSON payload");
    let errors = payload
        .get("errors")
        .and_then(|v| v.as_array())
        .expect("payload should have an 'errors' array");
    assert_eq!(errors.len(), 1, "should have exactly one validation error");
    let first = &errors[0];
    assert_eq!(
        first.get("code").and_then(|v| v.as_str()),
        Some("invalid_timezone"),
        "error code should be invalid_timezone"
    );
    assert_eq!(
        first.get("field_path").and_then(|v| v.as_str()),
        Some("timezone"),
        "field_path should be timezone"
    );
}

#[test]
fn settings_update_invalid_timezone_rejected() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let req = SettingsUpdateRequestDto {
        timezone: Some("NotATimezone".to_string()),
        week_starts_on: None,
        discovery: None,
        privacy: None,
        prompt_palette_default_action: None,
    };
    let result = sup.settings_update(req).now_or_never().unwrap();
    assert!(result.is_err(), "invalid timezone should be rejected");
}

#[tokio::test]
async fn settings_update_timezone_change_rebuilds_rollups_through_writer() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let req = SettingsUpdateRequestDto {
        timezone: Some("+05:30".to_string()),
        week_starts_on: None,
        discovery: None,
        privacy: None,
        prompt_palette_default_action: None,
    };
    let result = sup.settings_update(req).await.unwrap().data;
    assert_eq!(result.timezone, "+05:30");
    sup.shutdown_writer().await.unwrap();
}

// ---------------------------------------------------------------------------
// service.health and service.status
// ---------------------------------------------------------------------------

#[test]
fn service_health_returns_ok() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let result = sup.service_health().now_or_never().unwrap().unwrap();
    assert!(!result.ready);
    assert!(result.db_healthy);
    assert_eq!(result.scan_state, "offline");
}

#[test]
fn service_status_returns_version() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let result = sup.service_status().now_or_never().unwrap().unwrap();
    assert!(!result.version.is_empty());
    assert!(!result.db_path.is_empty());
    assert_eq!(result.state, "offline");
}

// ---------------------------------------------------------------------------
// shell.status snapshot fast path
// ---------------------------------------------------------------------------

#[test]
fn shell_status_uses_in_memory_snapshot_when_available() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let supervisor = make_supervisor(db, &tmp);

    // Seed the snapshot with a known latest_event_seq
    supervisor
        .apply_service_status_snapshot(|snap| {
            snap.latest_event_seq = Some(7);
        })
        .unwrap();

    let dto = supervisor.shell_status().now_or_never().unwrap().unwrap();
    assert_eq!(dto.latest_event_seq, Some(7));
}

#[test]
fn shell_status_falls_back_to_db_when_snapshot_uninitialized() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();

    // Seed a usage event so latest_event_seq is computed from the DB.
    seed_event(
        &db,
        "evt-1",
        1000,
        100,
        Some(1.0),
        Some("claude_code"),
        Some("claude-sonnet-4"),
        Some("hash-1"),
        None,
    );

    let sup = make_supervisor(db, &tmp);

    let result = sup.shell_status().now_or_never().unwrap().unwrap();
    // No snapshot seeded, so fallback to DB: chips should reflect the event.
    let capture = result
        .status_chips
        .iter()
        .find(|c| c.id == "capture")
        .unwrap();
    assert!(capture.label.contains("1 events"));
}

// ---------------------------------------------------------------------------
// Service bootstrap: status snapshot hydration
// ---------------------------------------------------------------------------

#[test]
fn service_bootstrap_initializes_shell_status_before_scan_completes() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());

    // Seed a service_state row simulating a previous run that was in
    // "starting" state — the snapshot should carry this forward.
    let conn = db.conn();
    conn.execute(
        "INSERT OR REPLACE INTO service_state \
         (id, writer_queue_depth, aggregate_lag_ms, readiness, \
          active_generation_id, last_exact_rebuild_at_ms, updated_at_ms) \
         VALUES (1, 0, 0, 'starting', NULL, NULL, 1000)",
        [],
    )
    .unwrap();

    let sup = BusytokSupervisor::new(db, paths);
    sup.hydrate_status_from_db()
        .expect("hydrate_status_from_db should succeed");

    let result = sup.shell_status().now_or_never().unwrap().unwrap();
    assert!(
        matches!(
            result.readiness,
            ReadinessStateDto::Starting | ReadinessStateDto::Rebuilding
        ),
        "shell.status readiness should be Starting or Rebuilding before scan completes, got {:?}",
        result.readiness
    );
}

#[test]
fn service_bootstrap_recovers_legacy_events_without_generation_metadata() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();

    seed_event(
        &db,
        "legacy-evt-1",
        1000,
        100,
        Some(0.01),
        Some("claude_code"),
        Some("claude-sonnet-4"),
        Some("hash-legacy"),
        Some("legacy-session"),
    );

    db.conn()
        .execute(
            "INSERT OR REPLACE INTO service_state \
             (id, writer_queue_depth, aggregate_lag_ms, readiness, \
              active_generation_id, last_exact_rebuild_at_ms, updated_at_ms) \
             VALUES (1, 0, 0, 'ready_degraded', NULL, NULL, 1000)",
            [],
        )
        .unwrap();

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db()
        .expect("hydrate_status_from_db should recover legacy generation state");

    let result = sup.shell_status().now_or_never().unwrap().unwrap();
    assert_eq!(result.readiness, ReadinessStateDto::ReadyExact);

    let db_guard = sup.db_handle().lock().unwrap();
    let active_generation_id: String = db_guard
        .conn()
        .query_row(
            "SELECT active_generation_id FROM service_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .expect("recovery should persist an active generation id");

    let active_count: i64 = db_guard
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM audit_generations \
             WHERE generation_id = ?1 AND state = 'promoted' AND is_active = 1",
            rusqlite::params![&active_generation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active_count, 1);

    let recovered_events: i64 = db_guard
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM usage_events WHERE generation_id = ?1",
            rusqlite::params![&active_generation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(recovered_events, 1);

    let materialized: (i64, i64) = db_guard
        .conn()
        .query_row(
            "SELECT COALESCE(SUM(total_tokens), 0), COALESCE(SUM(event_count), 0) \
             FROM usage_buckets_day WHERE generation_id = ?1",
            rusqlite::params![&active_generation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(materialized, (100, 1));
}

#[test]
fn service_bootstrap_repairs_degraded_state_with_promoted_generation() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();

    db.conn()
        .execute(
            "INSERT INTO audit_generations \
             (generation_id, state, started_at_ms, promoted_at_ms, is_active, \
              created_at_ms, updated_at_ms) \
             VALUES ('gen-existing', 'promoted', 1000, 1000, 1, 1000, 1000)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO service_state \
             (id, writer_queue_depth, aggregate_lag_ms, readiness, \
              active_generation_id, last_exact_rebuild_at_ms, updated_at_ms) \
             VALUES (1, 0, 0, 'ready_degraded', 'gen-existing', NULL, 1000)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO usage_buckets_day \
             (generation_id, bucket_start_ms, agent, model, total_tokens, event_count, \
              cost_status, created_at_ms, updated_at_ms) \
             VALUES ('gen-existing', 0, 'claude_code', 'sonnet', 100, 1, \
                     'unavailable', 1000, 1000)",
            [],
        )
        .unwrap();

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db()
        .expect("hydrate_status_from_db should repair sticky degraded state");

    let result = sup.shell_status().now_or_never().unwrap().unwrap();
    assert_eq!(result.readiness, ReadinessStateDto::ReadyExact);

    let db_guard = sup.db_handle().lock().unwrap();
    let persisted: (String, String) = db_guard
        .conn()
        .query_row(
            "SELECT readiness, active_generation_id FROM service_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        persisted,
        ("ready_exact".to_string(), "gen-existing".to_string())
    );
}

#[test]
fn service_bootstrap_does_not_mark_promoted_generation_exact_without_materialized_rows() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();

    db.conn()
        .execute(
            "INSERT INTO audit_generations \
             (generation_id, state, started_at_ms, promoted_at_ms, is_active, \
              created_at_ms, updated_at_ms) \
             VALUES ('gen-existing', 'promoted', 1000, 1000, 1, 1000, 1000)",
            [],
        )
        .unwrap();

    let mut event =
        NormalizedUsageEvent::minimal_for_test("event-needs-rematerialize", AgentKind::ClaudeCode);
    event.timestamp_ms = 1000;
    event.total_tokens = 100;
    busytok_store::write_queries::insert_usage_events_batch(db.conn(), &[event], "gen-existing")
        .unwrap();

    db.conn()
        .execute(
            "INSERT OR REPLACE INTO service_state \
             (id, writer_queue_depth, aggregate_lag_ms, readiness, \
              active_generation_id, last_exact_rebuild_at_ms, updated_at_ms) \
             VALUES (1, 0, 0, 'ready_degraded', 'gen-existing', NULL, 1000)",
            [],
        )
        .unwrap();

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db()
        .expect("hydrate_status_from_db should not repair empty materialized state");

    let result = sup.shell_status().now_or_never().unwrap().unwrap();
    assert_eq!(result.readiness, ReadinessStateDto::ReadyDegraded);

    let db_guard = sup.db_handle().lock().unwrap();
    let persisted: (String, Option<i64>) = db_guard
        .conn()
        .query_row(
            "SELECT readiness, last_exact_rebuild_at_ms FROM service_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(persisted, ("ready_degraded".to_string(), None));
}

#[test]
fn service_bootstrap_preserves_degraded_state_after_generation_drift() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();

    db.conn()
        .execute(
            "INSERT INTO audit_generations \
             (generation_id, state, started_at_ms, promoted_at_ms, is_active, \
              created_at_ms, updated_at_ms) \
             VALUES ('gen-existing', 'promoted', 1000, 1000, 1, 1000, 1000)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO service_state \
             (id, writer_queue_depth, aggregate_lag_ms, readiness, \
              active_generation_id, last_exact_rebuild_at_ms, updated_at_ms) \
             VALUES (1, 0, 0, 'ready_degraded', 'gen-existing', NULL, 1000)",
            [],
        )
        .unwrap();
    let drift = OperationalDiagnosticEvent {
        id: "diag-generation-drift".to_string(),
        agent: None,
        source_id: Some("rebuild".to_string()),
        source_file_id: None,
        source_path: None,
        source_line: None,
        category: "generation_drift".to_string(),
        severity: "error".to_string(),
        message: "promotion refused due to drift".to_string(),
        detail_json: None,
        happened_at_ms: 1000,
        created_at_ms: 1000,
    };
    db.record_diagnostic_event(&drift).unwrap();

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db()
        .expect("hydrate_status_from_db should preserve real degraded state");

    let result = sup.shell_status().now_or_never().unwrap().unwrap();
    assert_eq!(result.readiness, ReadinessStateDto::ReadyDegraded);
}

#[tokio::test]
async fn initial_scan_does_not_clear_drift_degraded_without_new_sources() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();

    db.conn()
        .execute(
            "INSERT INTO audit_generations \
             (generation_id, state, started_at_ms, promoted_at_ms, is_active, \
              created_at_ms, updated_at_ms) \
             VALUES ('gen-existing', 'promoted', 1000, 1000, 1, 1000, 1000)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO service_state \
             (id, writer_queue_depth, aggregate_lag_ms, readiness, \
              active_generation_id, last_exact_rebuild_at_ms, updated_at_ms) \
             VALUES (1, 0, 0, 'ready_degraded', 'gen-existing', NULL, 1000)",
            [],
        )
        .unwrap();
    let drift = OperationalDiagnosticEvent {
        id: "diag-generation-drift-scan".to_string(),
        agent: None,
        source_id: Some("rebuild".to_string()),
        source_file_id: None,
        source_path: None,
        source_line: None,
        category: "generation_drift".to_string(),
        severity: "error".to_string(),
        message: "promotion refused due to drift".to_string(),
        detail_json: None,
        happened_at_ms: 1000,
        created_at_ms: 1000,
    };
    db.record_diagnostic_event(&drift).unwrap();

    let mut settings = BusytokSettings::default();
    settings.discovery = DiscoverySettings {
        claude_code_default_paths: false,
        codex_default_paths: false,
        manual_roots: vec![],
    };

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    sup.hydrate_status_from_db()
        .expect("hydrate should preserve drift degraded state");
    let stats = sup
        .run_initial_scan()
        .await
        .expect("empty initial scan should not clear real degraded state");
    assert_eq!(stats.sources, 0);

    let status = sup.shell_status().await.unwrap();
    assert_eq!(status.readiness, ReadinessStateDto::ReadyDegraded);
}

#[test]
fn service_bootstrap_does_not_promote_partial_building_generation() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();

    seed_event(
        &db,
        "partial-building-evt-1",
        1000,
        100,
        Some(0.01),
        Some("claude_code"),
        Some("claude-sonnet-4"),
        Some("hash-partial"),
        Some("partial-session"),
    );
    db.conn()
        .execute(
            "UPDATE usage_events SET generation_id = 'gen-building', dedupe_key = id",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO audit_generations \
             (generation_id, state, started_at_ms, promoted_at_ms, is_active, \
              created_at_ms, updated_at_ms) \
             VALUES ('gen-building', 'building', 1000, NULL, 0, 1000, 1000)",
            [],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO service_state \
             (id, writer_queue_depth, aggregate_lag_ms, readiness, \
              active_generation_id, last_exact_rebuild_at_ms, updated_at_ms) \
             VALUES (1, 0, 0, 'ready_degraded', NULL, NULL, 1000)",
            [],
        )
        .unwrap();

    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db()
        .expect("hydrate_status_from_db should not promote building generation");

    let result = sup.shell_status().now_or_never().unwrap().unwrap();
    assert_eq!(result.readiness, ReadinessStateDto::ReadyDegraded);
}

#[tokio::test]
async fn initial_scan_promotes_active_generation_metadata() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = Database::open(&tmp.path().join("busytok.sqlite")).unwrap();
    let log_root = tmp.path().join("claude-root");
    let project_dir = log_root.join("projects").join("example");
    std::fs::create_dir_all(&project_dir).unwrap();
    let transcript = project_dir.join("session.jsonl");
    let line = serde_json::json!({
        "type": "assistant",
        "message": {
            "id": "msg_initial_scan_generation",
            "model": "claude-sonnet-4-20250514",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50
            }
        },
        "sessionId": "sess-initial-scan-generation",
        "timestamp": "2026-05-15T10:00:00Z"
    })
    .to_string();
    std::fs::write(&transcript, format!("{line}\n")).unwrap();

    let mut settings = BusytokSettings::default();
    settings.discovery = DiscoverySettings {
        claude_code_default_paths: false,
        codex_default_paths: false,
        manual_roots: vec![ManualRootConfig {
            id: "manual-claude-root".to_string(),
            client_id: "claude_code".to_string(),
            root_path: log_root.display().to_string(),
        }],
    };

    let sup = make_supervisor_with_settings(db, &tmp, settings);
    let stats = sup
        .run_initial_scan()
        .await
        .expect("initial scan should succeed");
    assert_eq!(stats.sources, 1);
    assert!(stats.events_found >= 1);

    let status = sup.shell_status().await.unwrap();
    assert_eq!(status.readiness, ReadinessStateDto::ReadyExact);
    let snapshot = sup.read_status_snapshot().await;
    let active_generation_id = snapshot
        .active_generation_id
        .expect("initial scan should persist active generation id");

    {
        let db_guard = sup.db_handle().lock().unwrap();
        let row: (String, i64) = db_guard
            .conn()
            .query_row(
                "SELECT state, is_active FROM audit_generations WHERE generation_id = ?1",
                rusqlite::params![&active_generation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, ("promoted".to_string(), 1));

        let service_state: (String, String) = db_guard
            .conn()
            .query_row(
                "SELECT readiness, active_generation_id FROM service_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(service_state.0, "ready_exact");
        assert_eq!(service_state.1, active_generation_id);
    }

    sup.shutdown_writer()
        .await
        .expect("writer should shut down cleanly");
}

// ---------------------------------------------------------------------------
// subagent.* (end-to-end via supervisor RuntimeControl impl)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subagent_delegate_list_show_hibernate_delete_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let supervisor = make_supervisor(db, &tmp);

    // delegate (mock execution)
    let delegate_resp = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "reviewer".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "find the bug".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .unwrap();
    // SubagentDelegateResponseDto serializes with task_id / subagent_id / status.
    let sub_id = delegate_resp.subagent_id.clone();
    assert_eq!(delegate_resp.status, "completed");

    // list (no filters → all active; the just-created subagent must appear).
    // Response is SubagentListResponseDto { subagents: [...] }.
    let list = supervisor
        .subagent_list(SubagentListRequestDto {
            status: None,
            project: None,
            include_deleted: Some(false),
        })
        .await
        .unwrap();
    assert!(list.subagents.iter().any(|s| s.id == sub_id));

    // show by UUID
    let shown = supervisor
        .subagent_show(SubagentResolveRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
        })
        .await
        .unwrap();
    assert_eq!(shown.name, "reviewer");

    // hibernate then still resolvable by name (memory written → warm)
    supervisor
        .subagent_hibernate(SubagentResolveRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
        })
        .await
        .unwrap();
    assert!(supervisor
        .subagent_show(SubagentResolveRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
        })
        .await
        .is_ok());

    // soft delete
    supervisor
        .subagent_delete(SubagentDeleteRequestDto {
            name: None,
            id: Some(sub_id.clone()),
            cwd: None,
            hard: Some(false),
        })
        .await
        .unwrap();
    // soft-deleted rows drop out of the active list
    let after_list = supervisor
        .subagent_list(SubagentListRequestDto {
            status: None,
            project: None,
            include_deleted: Some(false),
        })
        .await
        .unwrap();
    assert!(after_list.subagents.iter().all(|s| s.id != sub_id));
}

// ---------------------------------------------------------------------------
// providers.* (Phase 1: Credential Foundation)
// ---------------------------------------------------------------------------
//
// The settings-CRUD code paths (create without api_key, list, update without
// api_key, delete) never touch the OS keychain and are safe to run in CI on
// every platform. `provider_to_dto` calls `ProviderCredentialStore::has_key`,
// which is a read that returns `false` on `NoEntry` — also safe for CI.
//
// Tests that exercise the api_key flow (create/update with a non-empty key,
// or delete of a provider that has a key) touch the real macOS Keychain and
// are gated behind `#[cfg(target_os = "macos")] #[ignore]`.

fn provider_create_request(name: &str) -> ProviderCreateRequestDto {
    ProviderCreateRequestDto {
        name: name.to_string(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://api.example.com/v1".to_string(),
        api_key: None,
    }
}

/// Seed a provider row directly into SQL with a SPECIFIC id (bypassing the
/// store's UUID v4 generation). Used by sidecar-coupled tests where the
/// worker-pool provider-lookup closure still reads from `settings.providers`
/// (which uses hardcoded ids like "test-provider"). Task 7 will unify the
/// sidecar to read from SQL, at which point this helper can be removed.
fn seed_provider_to_sql(
    sup: &BusytokSupervisor,
    id: &str,
    name: &str,
    base_url: &str,
    api_key: Option<&str>,
    enabled: bool,
) {
    use rusqlite::params;
    let db = sup.db_handle().lock().unwrap();
    let now = busytok_domain::now_ms();
    db.conn()
        .execute(
            "INSERT INTO providers (id, name, provider_kind, base_url, enabled, api_key, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                id,
                name,
                serde_json::to_string(&ProviderKind::OpenAiCompatible).unwrap(),
                base_url,
                enabled as i64,
                api_key,
                now,
            ],
        )
        .expect("seed provider to SQL");
}

/// Seed a model row directly into SQL with a specific provider_id + model_id.
/// Used by profile tests that need model whitelist validation (the whitelist
/// check now queries SQL instead of `provider.models.contains(...)`).
fn seed_model_to_sql(sup: &BusytokSupervisor, provider_id: &str, model_id: &str, enabled: bool) {
    use rusqlite::params;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let db = sup.db_handle().lock().unwrap();
    let now = busytok_domain::now_ms();
    let id = format!(
        "seed-model-{}-{}",
        now,
        COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    db.conn()
        .execute(
            "INSERT INTO models (id, provider_id, model_id, enabled, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![id, provider_id, model_id, enabled as i64, now],
        )
        .expect("seed model to SQL");
}

/// Toggle a model's `enabled` flag in SQL. Used by profile tests that need
/// to simulate "shrinking the whitelist" (previously done via
/// `provider_update(models: Some(vec![...]))`).
fn set_model_enabled_in_sql(
    sup: &BusytokSupervisor,
    provider_id: &str,
    model_id: &str,
    enabled: bool,
) {
    use rusqlite::params;
    let db = sup.db_handle().lock().unwrap();
    let now = busytok_domain::now_ms();
    db.conn()
        .execute(
            "UPDATE models SET enabled = ?1, updated_at_ms = ?2
             WHERE provider_id = ?3 AND model_id = ?4",
            params![enabled as i64, now, provider_id, model_id],
        )
        .expect("set model enabled in SQL");
}

#[tokio::test]
async fn provider_crud_round_trips_without_api_key() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create → list shows it. id is now system-generated (UUID v4).
    let created = sup
        .provider_create(provider_create_request("Acme"))
        .await
        .unwrap();
    assert_eq!(created.name, "Acme");
    assert!(created.enabled);
    assert!(!created.has_api_key, "no api_key was supplied");
    let pid = created.id.clone();

    let list = sup.provider_list().await.unwrap();
    assert_eq!(list.providers.len(), 1);
    assert_eq!(list.providers[0].id, pid);

    // Update name + enabled, no api_key → keychain untouched.
    let updated = sup
        .provider_update(ProviderUpdateRequestDto {
            id: pid.clone(),
            name: Some("Acme Renamed".to_string()),
            base_url: None,
            enabled: Some(false),
            api_key: None,
        })
        .await
        .unwrap();
    assert_eq!(updated.name, "Acme Renamed");
    assert!(!updated.enabled);
    assert!(!updated.has_api_key, "still no api_key after update");

    // Delete → list empty.
    sup.provider_delete(ProviderDeleteRequestDto {
        id: pid.clone(),
    })
    .await
    .unwrap();
    let list_after = sup.provider_list().await.unwrap();
    assert!(
        list_after.providers.is_empty(),
        "provider list should be empty after delete"
    );
}

#[tokio::test]
async fn provider_update_with_none_api_key_is_a_noop_on_keychain() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let created = sup
        .provider_create(provider_create_request("Noop"))
        .await
        .unwrap();
    let pid = created.id.clone();

    // Update with api_key: None must succeed (three-state: None = unchanged).
    let updated = sup
        .provider_update(ProviderUpdateRequestDto {
            id: pid,
            name: Some("Noop Renamed".to_string()),
            base_url: None,
            enabled: None,
            api_key: None,
        })
        .await
        .unwrap();
    assert_eq!(updated.name, "Noop Renamed");
    assert!(!updated.has_api_key, "no api_key was ever set");
}

#[tokio::test]
async fn provider_update_returns_error_for_unknown_id() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .provider_update(ProviderUpdateRequestDto {
            id: "ghost".to_string(),
            name: Some("Ghost".to_string()),
            base_url: None,
            enabled: None,
            api_key: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("provider not found"),
        "expected not-found error, got: {err}"
    );
}

#[tokio::test]
async fn provider_delete_returns_error_for_unknown_id() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .provider_delete(ProviderDeleteRequestDto {
            id: "ghost".to_string(),
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("provider not found"),
        "delete of unknown provider should error early; got: {err}"
    );
}

#[tokio::test]
async fn provider_test_connection_errors_when_provider_missing() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .provider_test_connection(ProviderTestConnectionRequestDto {
            id: "ghost".to_string(),
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("provider not found"),
        "expected not-found error, got: {err}"
    );
}

#[tokio::test]
async fn provider_test_connection_errors_when_no_api_key() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let created = sup
        .provider_create(provider_create_request("No Key"))
        .await
        .unwrap();
    let pid = created.id.clone();

    let err = sup
        .provider_test_connection(ProviderTestConnectionRequestDto {
            id: pid.clone(),
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("provider has no api key"),
        "expected no-api-key error, got: {err}"
    );

    // Clean up.
    sup.provider_delete(ProviderDeleteRequestDto {
        id: pid,
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn provider_update_three_state_api_key_semantics() {
    // SQL store replaces the macOS Keychain. This test verifies the three-state
    // api_key patch semantics directly against the SQL store:
    //   - None            = unchanged
    //   - Some(None)      = clear
    //   - Some(Some(k))   = update
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create with api_key → has_api_key true.
    let created = sup
        .provider_create(ProviderCreateRequestDto {
            name: "Three State".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.example.com/v1".to_string(),
            api_key: Some("sk-test-123".to_string()),
        })
        .await
        .unwrap();
    let pid = created.id.clone();
    assert!(created.has_api_key, "api_key was supplied");

    // Update with api_key: None → key unchanged.
    let updated = sup
        .provider_update(ProviderUpdateRequestDto {
            id: pid.clone(),
            name: Some("Three State Renamed".to_string()),
            base_url: None,
            enabled: None,
            api_key: None,
        })
        .await
        .unwrap();
    assert!(
        updated.has_api_key,
        "api_key must still be present after update with None"
    );
    assert_eq!(updated.name, "Three State Renamed");

    // Update with api_key: Some(None) → clear.
    let cleared = sup
        .provider_update(ProviderUpdateRequestDto {
            id: pid.clone(),
            name: None,
            base_url: None,
            enabled: None,
            api_key: Some(None),
        })
        .await
        .unwrap();
    assert!(
        !cleared.has_api_key,
        "api_key must be cleared after Some(None)"
    );

    // Update with api_key: Some(Some(k)) → update.
    let restored = sup
        .provider_update(ProviderUpdateRequestDto {
            id: pid.clone(),
            name: None,
            base_url: None,
            enabled: None,
            api_key: Some(Some("sk-new-456".to_string())),
        })
        .await
        .unwrap();
    assert!(
        restored.has_api_key,
        "api_key must be present after Some(Some(k))"
    );

    // Delete cleans up the SQL row.
    sup.provider_delete(ProviderDeleteRequestDto {
        id: pid,
    })
    .await
    .unwrap();
    let list = sup.provider_list().await.unwrap();
    assert!(list.providers.is_empty());
}

// ---------------------------------------------------------------------------
// subagent.runtime_status (Phase 2: Subagent Monitoring Page)
// ---------------------------------------------------------------------------
//
// Tests exercise the BusytokSupervisor handler end-to-end via the RuntimeControl
// trait. Data is seeded directly via the shared DB handle (`sup.db_handle()`)
// using `SubagentLogicalSubagentRow` / `SubagentTaskRow` so we can control
// `created_at_ms` and `status` precisely — the mock executor path
// (`subagent_delegate`) timestamps tasks at `now_ms()` and only produces
// `completed` rows, which doesn't let us verify ordering or last_task_status.

use busytok_store::repository::{SubagentLogicalSubagentRow, SubagentTaskRow};

/// Helper: insert a logical subagent row with the given name + status.
/// Returns the row id (same as `name` for tests).
fn seed_subagent_row(sup: &BusytokSupervisor, name: &str, status: &str) -> String {
    let mut row = SubagentLogicalSubagentRow::for_test(name, name);
    row.status = status.to_string();
    let db = sup.db_handle().lock().unwrap();
    db.subagent_upsert_logical(&row).expect("upsert logical");
    row.id
}

/// Helper: insert a task row with the given id, subagent_id, status, and
/// created_at_ms. `SubagentTaskRow::for_test` defaults to `status="queued"`;
/// we override it (and the timestamp) after construction.
fn seed_task_row(
    sup: &BusytokSupervisor,
    id: &str,
    subagent_id: &str,
    status: &str,
    created_at_ms: i64,
) {
    let mut row = SubagentTaskRow::for_test(id, subagent_id, "pi/search-cheap", "do work");
    row.status = status.to_string();
    row.created_at_ms = created_at_ms;
    if status == "completed" || status == "failed" || status == "cancelled" {
        row.completed_at_ms = Some(created_at_ms + 1000);
    }
    if status == "failed" {
        row.error = Some("boom".to_string());
    }
    let db = sup.db_handle().lock().unwrap();
    db.subagent_insert_task(&row).expect("insert task");
}

#[tokio::test]
async fn subagent_runtime_status_returns_empty_when_no_data() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let sup = make_supervisor(db, &tmp);

    let envelope = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
        .await
        .expect("runtime_status should succeed on empty DB");

    // Envelope-level assertions — `build_read_envelope` populates these from
    // `ServiceStatusSnapshot`. `generated_at_ms` is `now_ms()` so must be > 0.
    assert!(
        envelope.generated_at_ms > 0,
        "generated_at_ms must be populated"
    );

    // Inner data — empty state.
    assert_eq!(envelope.data.pressure_gate.level, "normal");
    assert_eq!(envelope.data.pressure_gate.memory_used_pct, 0);
    assert_eq!(envelope.data.pressure_gate.hot_sessions_total, 0);
    // Default `max_hot_sessions` from `SubagentSettings::default()` is 3.
    assert_eq!(envelope.data.pressure_gate.hot_sessions_limit, 3);
    // No sidecar supervisor in `make_supervisor` → `worker_sampled_at_ms` is None.
    assert_eq!(envelope.data.pressure_gate.worker_sampled_at_ms, None);
    assert!(envelope.data.subagents.is_empty());
    assert!(envelope.data.tasks_recent.is_empty());
    // `make_supervisor` doesn't configure a sidecar → `workers: []`.
    assert!(envelope.data.workers.is_empty());

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn subagent_runtime_status_includes_subagents_with_task_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let sup = make_supervisor(db, &tmp);

    seed_subagent_row(&sup, "sub-a", "warm");
    seed_subagent_row(&sup, "sub-b", "warm");
    // sub-a: 2 tasks (one completed, one failed — failed is the latest by ts)
    seed_task_row(&sup, "t1", "sub-a", "completed", 1_000);
    seed_task_row(&sup, "t2", "sub-a", "failed", 2_000);
    // sub-b: 1 task
    seed_task_row(&sup, "t3", "sub-b", "completed", 3_000);

    let envelope = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
        .await
        .unwrap();

    assert_eq!(envelope.data.subagents.len(), 2);
    let sub_a = envelope
        .data
        .subagents
        .iter()
        .find(|s| s.name == "sub-a")
        .expect("sub-a present");
    assert_eq!(sub_a.task_count, 2);
    assert_eq!(sub_a.last_task_at_ms, Some(2_000));
    assert_eq!(sub_a.last_task_status.as_deref(), Some("failed"));

    let sub_b = envelope
        .data
        .subagents
        .iter()
        .find(|s| s.name == "sub-b")
        .expect("sub-b present");
    assert_eq!(sub_b.task_count, 1);
    assert_eq!(sub_b.last_task_at_ms, Some(3_000));
    assert_eq!(sub_b.last_task_status.as_deref(), Some("completed"));

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn subagent_runtime_status_tasks_recent_ordered_desc_by_created_at() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let sup = make_supervisor(db, &tmp);

    seed_subagent_row(&sup, "sub-a", "hot");
    // Insert out of order — handler must return newest-first.
    seed_task_row(&sup, "t1", "sub-a", "completed", 1_000);
    seed_task_row(&sup, "t2", "sub-a", "completed", 3_000);
    seed_task_row(&sup, "t3", "sub-a", "completed", 2_000);

    let envelope = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
        .await
        .unwrap();

    assert_eq!(envelope.data.tasks_recent.len(), 3);
    // Descending by created_at_ms.
    assert_eq!(envelope.data.tasks_recent[0].task_id, "t2"); // 3000ms
    assert_eq!(envelope.data.tasks_recent[1].task_id, "t3"); // 2000ms
    assert_eq!(envelope.data.tasks_recent[2].task_id, "t1"); // 1000ms

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn subagent_runtime_status_excludes_deleted_subagents() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let sup = make_supervisor(db, &tmp);

    seed_subagent_row(&sup, "sub-active", "warm");
    seed_subagent_row(&sup, "sub-gone", "deleted");

    let envelope = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
        .await
        .unwrap();

    // `subagent_list_filtered(None, None, false)` excludes `deleted` rows.
    assert_eq!(envelope.data.subagents.len(), 1);
    assert_eq!(envelope.data.subagents[0].name, "sub-active");

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn subagent_runtime_status_tasks_recent_resolves_subagent_name() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let sup = make_supervisor(db, &tmp);

    seed_subagent_row(&sup, "my-agent", "hot");
    seed_task_row(&sup, "t1", "my-agent", "completed", 1_000);

    let envelope = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
        .await
        .unwrap();

    assert_eq!(envelope.data.tasks_recent.len(), 1);
    assert_eq!(envelope.data.tasks_recent[0].task_id, "t1");
    assert_eq!(envelope.data.tasks_recent[0].subagent_name, "my-agent");
    assert_eq!(envelope.data.tasks_recent[0].status, "completed");

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn subagent_runtime_status_tasks_recent_includes_error_for_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let sup = make_supervisor(db, &tmp);

    seed_subagent_row(&sup, "sub-a", "warm");
    seed_task_row(&sup, "t1", "sub-a", "failed", 1_000);
    seed_task_row(&sup, "t2", "sub-a", "completed", 2_000);

    let envelope = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
        .await
        .unwrap();

    let failed = envelope
        .data
        .tasks_recent
        .iter()
        .find(|t| t.task_id == "t1")
        .expect("failed task present");
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.error.as_deref(), Some("boom"));

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn subagent_runtime_status_envelope_populated_from_status_snapshot() {
    // The handler must wrap the inner DTO via `build_read_envelope`, which
    // populates `readiness` / `is_exact` / `is_stale` / `degraded_reason`
    // from `ServiceStatusSnapshot`. On a fresh supervisor the snapshot is
    // `Starting` (no scan has run), so `is_stale=true` and `is_exact=false`.
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let sup = make_supervisor(db, &tmp);

    let envelope = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
        .await
        .unwrap();

    assert!(envelope.generated_at_ms > 0);
    // Fresh supervisor → readiness is `Starting`.
    assert_eq!(envelope.readiness, ReadinessStateDto::Starting);
    assert!(!envelope.is_exact);
    assert!(envelope.is_stale);

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn subagent_runtime_status_pressure_gate_no_sidecar_defaults() {
    // When no sidecar supervisor is configured (`make_supervisor` path), the
    // pressure gate must report `level=normal`, `memory_used_pct=0`,
    // `hot_sessions_total=0`, `worker_sampled_at_ms=None`, and
    // `hot_sessions_limit` from settings (default 3). `workers` must be `[]`.
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let sup = make_supervisor(db, &tmp);

    let envelope = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
        .await
        .unwrap();

    assert_eq!(envelope.data.pressure_gate.level, "normal");
    assert_eq!(envelope.data.pressure_gate.memory_used_pct, 0);
    assert_eq!(envelope.data.pressure_gate.hot_sessions_total, 0);
    assert_eq!(envelope.data.pressure_gate.hot_sessions_limit, 3);
    assert_eq!(envelope.data.pressure_gate.worker_sampled_at_ms, None);
    assert!(envelope.data.workers.is_empty(), "no sidecar → workers: []");

    sup.shutdown_writer().await.expect("writer shutdown");
}

#[tokio::test]
async fn subagent_runtime_status_subagent_status_string_mapped() {
    // Verify `LogicalSubagent.status` (enum) → DTO `status` (String) mapping
    // via `as_str()`. Seeds one subagent per non-deleted status variant.
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let sup = make_supervisor(db, &tmp);

    seed_subagent_row(&sup, "hot-sub", "hot");
    seed_subagent_row(&sup, "warm-sub", "warm");
    seed_subagent_row(&sup, "cold-sub", "cold");
    // `deleted` excluded from the list — not asserted here (covered by
    // `subagent_runtime_status_excludes_deleted_subagents`).

    let envelope = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
        .await
        .unwrap();

    // Ordering of subagents is by `last_active_at_ms DESC NULLS LAST`; all
    // seeded rows have `last_active_at_ms = None`, so the order among them is
    // unspecified. Assert presence + status mapping instead of order.
    assert_eq!(envelope.data.subagents.len(), 3);
    let hot = envelope
        .data
        .subagents
        .iter()
        .find(|s| s.name == "hot-sub")
        .unwrap();
    assert_eq!(hot.status, "hot");
    let warm = envelope
        .data
        .subagents
        .iter()
        .find(|s| s.name == "warm-sub")
        .unwrap();
    assert_eq!(warm.status, "warm");
    let cold = envelope
        .data
        .subagents
        .iter()
        .find(|s| s.name == "cold-sub")
        .unwrap();
    assert_eq!(cold.status, "cold");

    sup.shutdown_writer().await.expect("writer shutdown");
}
/// `workers: [one row]` path: when a sidecar supervisor is configured but the
/// child has never been started, `subagent_runtime_status` must return exactly
/// one worker row with `state="stopped"`, `pid=None`, `uptime_seconds=None`,
/// and a `pressure_gate` reporting `level="normal"` with
/// `worker_sampled_at_ms=None` (pre-first-sample). Covers the
/// `if let Some(ref snap) = worker_opt` branches (PressureLevel/WorkerState
/// string mapping; `worker_sampled_at_ms = Some(..)` is the NOT-taken branch
/// here) that `..._pressure_gate_no_sidecar_defaults` does not exercise.
#[tokio::test]
async fn subagent_runtime_status_workers_one_row_when_sidecar_stopped() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let sup = make_sidecar_stopped_supervisor(db, &tmp);

    let envelope = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
        .await
        .unwrap();

    // Envelope is always populated.
    assert!(
        envelope.generated_at_ms > 0,
        "generated_at_ms must be positive"
    );

    // workers: [one row] — the configured-but-stopped sidecar.
    assert_eq!(envelope.data.workers.len(), 1, "stopped sidecar -> one row");
    let worker = &envelope.data.workers[0];
    assert_eq!(worker.state, "stopped");
    assert_eq!(worker.pid, None, "pid must be None when stopped");
    assert_eq!(
        worker.uptime_seconds, None,
        "uptime must be None when stopped"
    );
    assert_eq!(worker.hot_sessions, 0, "hot_sessions starts at 0");

    // pressure_gate: normal, no sample yet, hot_sessions_total=0.
    assert_eq!(envelope.data.pressure_gate.level, "normal");
    assert_eq!(envelope.data.pressure_gate.hot_sessions_total, 0);
    assert_eq!(envelope.data.pressure_gate.hot_sessions_limit, 3);
    assert_eq!(
        envelope.data.pressure_gate.worker_sampled_at_ms, None,
        "worker_sampled_at_ms is None before first sample"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `workers: [one row, state="running"]` path: when the sidecar supervisor is
/// configured AND the child has been started via `ensure_started`,
/// `subagent_runtime_status` must return exactly one worker row with
/// `state="running"`, `pid=Some(..)`, `uptime_seconds=Some(..)`. Covers the
/// `WorkerState::Running => "running"` branch (supervisor.rs) that the
/// stopped-sidecar test does not exercise.
#[tokio::test]
async fn subagent_runtime_status_workers_running_when_sidecar_started() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let sup = make_sidecar_stopped_supervisor(db, &tmp);

    // Start the sidecar child so `worker_snapshot()` reports `Running`.
    let sidecar = sup
        .sidecar_supervisor()
        .expect("sidecar supervisor must be configured");
    let _handle = sidecar.ensure_started().await.expect("ensure_started");

    let envelope = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
        .await
        .unwrap();

    assert_eq!(envelope.data.workers.len(), 1, "one worker row");
    let worker = &envelope.data.workers[0];
    assert_eq!(worker.state, "running");
    assert!(worker.pid.is_some(), "pid must be Some when running");
    assert!(
        worker.uptime_seconds.is_some(),
        "uptime must be Some when running"
    );

    // Clean up: stop the child, then shut down the writer thread.
    sidecar.shutdown().await.expect("sidecar shutdown");
    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `tasks_recent[].subagent_name` resolution for deleted subagents: when a
/// task references a subagent that has been deleted (excluded from
/// `subagents[]` by the non-deleted filter), the handler must STILL show the
/// display name (not the raw `subagent_id`). The `name_lookup` in
/// `runtime_status_snapshot` includes ALL subagents (including deleted),
/// decoupling display name from delete filtering (reviewer P1-2).
#[tokio::test]
async fn subagent_runtime_status_tasks_recent_shows_display_name_for_deleted_subagent() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Seed a subagent with id != name, then mark it deleted so it's excluded
    // from the `subagents[]` list but its task still appears in `tasks_recent`.
    let mut row = SubagentLogicalSubagentRow::for_test("ghost-id", "Ghost Agent");
    row.status = "deleted".to_string();
    {
        let db = sup.db_handle().lock().unwrap();
        db.subagent_upsert_logical(&row).expect("upsert logical");
    }
    seed_task_row(&sup, "t1", "ghost-id", "completed", 1_000);

    let envelope = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
        .await
        .unwrap();

    // The deleted subagent must NOT appear in `subagents[]`.
    assert!(
        envelope.data.subagents.is_empty(),
        "deleted subagent must be excluded from subagents[]"
    );

    // The task must appear in `tasks_recent` with `subagent_name` showing
    // the display name "Ghost Agent" (NOT the raw id "ghost-id").
    assert_eq!(envelope.data.tasks_recent.len(), 1);
    let task = &envelope.data.tasks_recent[0];
    assert_eq!(task.task_id, "t1");
    assert_eq!(
        task.subagent_name, "Ghost Agent",
        "subagent_name must show display name even for deleted subagent (reviewer P1-2)"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

// ---------------------------------------------------------------------------
// Phase 3 Task 4: BusytokSupervisor wiring — delegate validation,
// runtime_status aggregation, provider_changed/deleted worker lifecycle.
// ---------------------------------------------------------------------------
//
// These tests cover the Task 4 brief requirements:
// - `subagent_delegate` validates profile.provider_id (unbound → error),
//   provider existence (unknown → error), provider enabled state (disabled
//   → error), and model whitelist (model not in provider.models → error).
// - `subagent_runtime_status` aggregates `pool.worker_snapshots()` across
//   all providers (not just the first).
// - `provider_update` / `provider_delete` kill + remove the affected
//   provider's worker so the next delegate re-spawns with fresh state.

/// Build sidecar-enabled settings with a single test provider whose model
/// whitelist is `["test-model"]`. Profiles are NOT bound by default (caller
/// binds per-test). Used by the delegate validation tests.
fn make_unbound_sidecar_settings() -> BusytokSettings {
    let mut settings = BusytokSettings::default();
    settings.subagent.pi_sidecar.enabled = true;
    settings.providers.push(ProviderConfig {
        id: "test-provider".to_string(),
        name: "Test Provider".to_string(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://api.test-provider.example.com/v1".to_string(),
        api_key_env_name: "TEST_API_KEY".to_string(),
        base_url_env_name: None,
        models: vec!["test-model".to_string()],
        enabled: true,
    });
    std::env::set_var("BUSYTOK_TEST_API_KEY", "test-key-for-e2e");
    settings
}

/// Build a sidecar-stopped supervisor from arbitrary settings. Mirrors
/// `make_sidecar_stopped_supervisor` but accepts custom settings so the
/// delegate validation tests can configure provider/profile bindings.
fn make_sidecar_stopped_supervisor_with_settings(
    db: Database,
    tmp: &tempfile::TempDir,
    settings: BusytokSettings,
) -> BusytokSupervisor {
    let paths = BusytokPaths::for_test(tmp.path());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");
    BusytokSupervisor::new_with_sidecar_config(db, paths, make_sidecar_config())
}

/// `delegate_fails_for_unbound_profile`: when the sidecar is enabled AND the
/// profile has `provider_id: None`, the runtime handler must reject the
/// delegate with "profile not bound to a provider" BEFORE the manager inserts
/// a task row. Covers spec §3.4 + Phase 3 Task 4 Step 3.
#[tokio::test]
async fn delegate_fails_for_unbound_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    // Built-in profiles ship unbound (`provider_id: None`).
    let settings = make_unbound_sidecar_settings();
    let sup = make_sidecar_stopped_supervisor_with_settings(db, &tmp, settings);

    let err = sup
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "reviewer".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "find the bug".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .expect_err("unbound profile must fail validation");

    assert!(
        err.to_string().contains("profile not bound to a provider"),
        "expected 'profile not bound to a provider' error, got: {err}"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `delegate_fails_for_unknown_provider`: profile bound to a provider_id that
/// doesn't exist in `settings.providers` → "provider not found: ...".
#[tokio::test]
async fn delegate_fails_for_unknown_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let mut settings = make_unbound_sidecar_settings();
    // Bind the profile to a provider that doesn't exist.
    settings
        .subagent
        .profiles
        .get_mut("pi/search-cheap")
        .unwrap()
        .provider_id = Some("nonexistent".to_string());
    let sup = make_sidecar_stopped_supervisor_with_settings(db, &tmp, settings);

    let err = sup
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "reviewer".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "find the bug".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .expect_err("unknown provider must fail validation");

    assert!(
        err.to_string().contains("provider not found: nonexistent"),
        "expected 'provider not found: nonexistent' error, got: {err}"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `delegate_fails_for_disabled_provider`: provider exists but `enabled:
/// false` → "provider disabled: ...".
#[tokio::test]
async fn delegate_fails_for_disabled_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let mut settings = make_unbound_sidecar_settings();
    // Add a disabled provider + bind the profile to it.
    settings.providers.push(ProviderConfig {
        id: "disabled-prov".to_string(),
        name: "Disabled Provider".to_string(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://api.disabled.example.com/v1".to_string(),
        api_key_env_name: "DISABLED_API_KEY".to_string(),
        base_url_env_name: None,
        models: vec!["test-model".to_string()],
        enabled: false,
    });
    settings
        .subagent
        .profiles
        .get_mut("pi/search-cheap")
        .unwrap()
        .provider_id = Some("disabled-prov".to_string());
    let sup = make_sidecar_stopped_supervisor_with_settings(db, &tmp, settings);

    // Seed SQL with the disabled provider so the SQL-backed delegate handler
    // can find it. The delegate validates provider existence + enabled flag
    // against SQL (not settings).
    seed_provider_to_sql(
        &sup,
        "disabled-prov",
        "Disabled Provider",
        "https://api.disabled.example.com/v1",
        None,
        false,
    );

    let err = sup
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "reviewer".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "find the bug".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .expect_err("disabled provider must fail validation");

    assert!(
        err.to_string().contains("provider disabled: disabled-prov"),
        "expected 'provider disabled: disabled-prov' error, got: {err}"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `delegate_fails_for_model_not_in_whitelist` (M2 fix, spec §3.4): profile
/// bound to an enabled provider, but `profile.model` is NOT in
/// `provider.models` → "model '...' not in provider '...' whitelist". Also
/// covers the empty-model edge case.
#[tokio::test]
async fn delegate_fails_for_model_not_in_whitelist() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let mut settings = make_unbound_sidecar_settings();
    // Bind the profile to the valid test-provider, but set its model to
    // something NOT in the provider's `["test-model"]` whitelist.
    {
        let profile = settings
            .subagent
            .profiles
            .get_mut("pi/search-cheap")
            .unwrap();
        profile.provider_id = Some("test-provider".to_string());
        profile.model = "wrong-model".to_string();
    }
    let sup = make_sidecar_stopped_supervisor_with_settings(db, &tmp, settings);

    // Seed SQL with test-provider (enabled) + test-model (enabled) so the
    // delegate's SQL-backed validation finds the provider but rejects the
    // model ("wrong-model" is not in the SQL whitelist).
    seed_provider_to_sql(
        &sup,
        "test-provider",
        "Test Provider",
        "https://api.test-provider.example.com/v1",
        None,
        true,
    );
    seed_model_to_sql(&sup, "test-provider", "test-model", true);

    let err = sup
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "reviewer".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "find the bug".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .expect_err("model not in whitelist must fail validation");

    assert!(
        err.to_string()
            .contains("model 'wrong-model' not in provider 'test-provider' whitelist"),
        "expected whitelist violation error, got: {err}"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `runtime_status_aggregates_multiple_workers`: when the pool has workers
/// for MORE than one provider, `subagent_runtime_status` must return one
/// worker row per provider (not just the first). Covers Phase 3 Task 4
/// Step 4 multi-provider aggregation.
#[tokio::test]
async fn runtime_status_aggregates_multiple_workers() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let mut settings = make_unbound_sidecar_settings();
    // Add a second enabled provider so the pool can spawn two workers.
    settings.providers.push(ProviderConfig {
        id: "test-provider-2".to_string(),
        name: "Test Provider 2".to_string(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://api.test-provider-2.example.com/v1".to_string(),
        api_key_env_name: "TEST_API_KEY_2".to_string(),
        base_url_env_name: None,
        models: vec!["test-model".to_string()],
        enabled: true,
    });
    let sup = make_sidecar_stopped_supervisor_with_settings(db, &tmp, settings);

    // construct_sidecar auto-spawns the FIRST enabled provider's worker.
    // Spawn the SECOND provider's worker via the pool to exercise the
    // multi-provider aggregation path.
    let pool = sup
        .worker_pool()
        .expect("worker_pool must be Some when sidecar is enabled");
    pool.ensure_worker("test-provider-2")
        .expect("ensure_worker for second provider must succeed");

    let envelope = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
        .await
        .expect("runtime_status must succeed");

    // Two workers — one per provider. Both stopped (no ensure_started call).
    assert_eq!(
        envelope.data.workers.len(),
        2,
        "multi-provider pool must aggregate to two worker rows"
    );

    // Both provider_ids must be present (order is not guaranteed by HashMap).
    let provider_ids: Vec<Option<String>> = envelope
        .data
        .workers
        .iter()
        .map(|w| w.provider_id.clone())
        .collect();
    assert!(
        provider_ids.contains(&Some("test-provider".to_string())),
        "workers must include test-provider, got: {provider_ids:?}"
    );
    assert!(
        provider_ids.contains(&Some("test-provider-2".to_string())),
        "workers must include test-provider-2, got: {provider_ids:?}"
    );

    // Both workers are stopped (child never started).
    for worker in &envelope.data.workers {
        assert_eq!(worker.state, "stopped");
        assert_eq!(worker.pid, None);
    }

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `runtime_status_workers_empty_when_pool_none`: when the sidecar is
/// disabled (`worker_pool` is `None`), `subagent_runtime_status` must
/// return `workers: []` and a default pressure_gate. Covers Phase 3 Task 4
/// Step 4 "no pool" branch.
#[tokio::test]
async fn runtime_status_workers_empty_when_pool_none() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    // `make_supervisor` uses default settings → sidecar disabled → pool None.
    let sup = make_supervisor(db, &tmp);

    let envelope = sup
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
        .await
        .expect("runtime_status must succeed");

    assert!(
        envelope.data.workers.is_empty(),
        "no pool → workers must be empty"
    );
    assert_eq!(envelope.data.pressure_gate.level, "normal");
    assert_eq!(envelope.data.pressure_gate.hot_sessions_total, 0);
    assert_eq!(envelope.data.pressure_gate.worker_sampled_at_ms, None);

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `provider_changed_removes_worker_then_respawns` (P1b fix): calling
/// `provider_update` must kill + remove the affected provider's worker from
/// the pool. The next `ensure_worker` call re-spawns a fresh worker (with
/// updated credentials/config). Covers Phase 3 Task 4 Step 5.
#[tokio::test]
async fn provider_changed_removes_worker_then_respawns() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let settings = make_sidecar_settings();
    let sup = make_sidecar_stopped_supervisor_with_settings(db, &tmp, settings);

    let pool = sup
        .worker_pool()
        .expect("worker_pool must be Some when sidecar is enabled");

    // Initially, construct_sidecar auto-spawned ONE worker for test-provider.
    let snaps = pool.worker_snapshots().await;
    assert_eq!(snaps.len(), 1, "initial state: one worker");
    assert_eq!(snaps[0].0, "test-provider");

    // Seed SQL with the same provider id so the SQL-backed provider_update
    // handler can find it. (Sidecar worker pool still reads from settings; Task
    // 7 will unify this.)
    seed_provider_to_sql(
        &sup,
        "test-provider",
        "Test Provider",
        "https://api.test-provider.example.com/v1",
        None,
        true,
    );

    // Trigger provider_changed via provider_update (metadata-only change:
    // update the display name). This must remove the worker.
    sup.provider_update(ProviderUpdateRequestDto {
        id: "test-provider".to_string(),
        name: Some("Updated Name".to_string()),
        base_url: None,
        enabled: None,
        api_key: None,
    })
    .await
    .expect("provider_update must succeed");

    // Worker must be gone from the pool.
    let snaps_after = pool.worker_snapshots().await;
    assert!(
        snaps_after.is_empty(),
        "provider_update must remove the worker from the pool, got: {snaps_after:?}"
    );

    // Re-spawn: ensure_worker creates a NEW worker with the updated config.
    let re_spawned = pool
        .ensure_worker("test-provider")
        .expect("ensure_worker must re-spawn the worker");
    assert!(
        re_spawned.worker_snapshot().await.is_some(),
        "re-spawned worker must produce a snapshot"
    );
    let snaps_final = pool.worker_snapshots().await;
    assert_eq!(
        snaps_final.len(),
        1,
        "re-spawned worker must be in the pool"
    );
    assert_eq!(snaps_final[0].0, "test-provider");

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `provider_update_respawn_picks_up_live_config` (P1 fix): the
/// `WorkerPool`'s provider-lookup closure must read LIVE settings (not a
/// startup snapshot), so when `provider_update` changes `base_url` /
/// `base_url_env_name`, the respawned supervisor's config reflects the new
/// values. Without the live-settings fix, the closure would return the stale
/// provider snapshot captured at `construct_sidecar` time, and `ensure_worker`
/// would build the config with the OLD `base_url` / `base_url_env_name` —
/// silently running the sidecar against the wrong endpoint.
///
/// **DISABLED (Task 5):** provider CRUD now writes to SQL, not `settings.providers`.
/// The sidecar worker-pool closure still reads from `settings.providers` (TOML),
/// so `provider_update` no longer mutates the data the closure reads. This test
/// will be rewritten in Task 7 when the sidecar provider-lookup migrates to SQL.
#[tokio::test]
#[ignore = "disabled under SQL-backed provider CRUD; rewritable in Task 7 (sidecar → SQL)"]
async fn provider_update_respawn_picks_up_live_config() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let settings = make_sidecar_settings();
    let sup = make_sidecar_stopped_supervisor_with_settings(db, &tmp, settings);

    let pool = sup
        .worker_pool()
        .expect("worker_pool must be Some when sidecar is enabled");

    // Initial worker: base_url_env_name defaults to OPENAI_BASE_URL (provider
    // has base_url_env_name: None in make_sidecar_settings).
    let initial = pool
        .ensure_worker("test-provider")
        .expect("ensure_worker (initial)");
    let initial_cfg = initial.config();
    assert_eq!(initial_cfg.base_url_env_name, "OPENAI_BASE_URL");
    assert_eq!(
        initial_cfg.env.get("OPENAI_BASE_URL"),
        Some(&"https://api.test-provider.example.com/v1".to_string()),
        "initial worker env should have the original base_url"
    );

    // Update BOTH base_url and base_url_env_name via provider_update. This
    // mutates `self.settings` (the shared `Arc<Mutex<BusytokSettings>>`) and
    // calls `provider_changed` to kill + remove the worker.
    sup.provider_update(ProviderUpdateRequestDto {
        id: "test-provider".to_string(),
        name: None,
        base_url: Some("https://api.updated.example.com/v1".to_string()),
        enabled: None,
        api_key: None,
    })
    .await
    .expect("provider_update must succeed");

    // Worker must be gone (provider_changed killed + removed it).
    assert!(
        pool.worker_snapshots().await.is_empty(),
        "provider_update must remove the worker from the pool"
    );

    // Re-spawn: ensure_worker builds a NEW supervisor. The pool's
    // provider-lookup closure must read the LIVE (post-update) settings, so
    // the new supervisor's config has the updated base_url / env name.
    let respawned = pool
        .ensure_worker("test-provider")
        .expect("ensure_worker must re-spawn the worker");
    let respawned_cfg = respawned.config();

    // The fix: base_url_env_name MUST be the updated value, not the original
    // "OPENAI_BASE_URL" (which would indicate the closure read a stale snapshot).
    assert_eq!(
        respawned_cfg.base_url_env_name, "UPDATED_BASE_URL",
        "respawned worker must use the updated base_url_env_name \
         (live settings), not the startup snapshot"
    );
    // OPENAI_BASE_URL is always set (canonical name) and must reflect the new base_url.
    assert_eq!(
        respawned_cfg.env.get("OPENAI_BASE_URL"),
        Some(&"https://api.updated.example.com/v1".to_string()),
        "respawned worker env[OPENAI_BASE_URL] must be the updated base_url"
    );
    // The provider-specific alias (UPDATED_BASE_URL) must also be set to the new base_url.
    assert_eq!(
        respawned_cfg.env.get("UPDATED_BASE_URL"),
        Some(&"https://api.updated.example.com/v1".to_string()),
        "respawned worker env[UPDATED_BASE_URL] must be the updated base_url"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `provider_deleted_removes_worker` (P1b fix): calling `provider_delete`
/// must kill + remove the deleted provider's worker from the pool. Other
/// providers' workers are unaffected. Covers Phase 3 Task 4 Step 5.
#[tokio::test]
async fn provider_deleted_removes_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let mut settings = make_unbound_sidecar_settings();
    // Add a second provider that NO profile references — `provider_delete`
    // rejects if any profile still references the provider.
    settings.providers.push(ProviderConfig {
        id: "test-provider-2".to_string(),
        name: "Test Provider 2".to_string(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://api.test-provider-2.example.com/v1".to_string(),
        api_key_env_name: "TEST_API_KEY_2".to_string(),
        base_url_env_name: None,
        models: vec!["test-model".to_string()],
        enabled: true,
    });
    let sup = make_sidecar_stopped_supervisor_with_settings(db, &tmp, settings);

    let pool = sup
        .worker_pool()
        .expect("worker_pool must be Some when sidecar is enabled");

    // Seed SQL with the second provider so the SQL-backed provider_delete
    // handler can find it. No profile references it, so delete won't be blocked.
    seed_provider_to_sql(
        &sup,
        "test-provider-2",
        "Test Provider 2",
        "https://api.test-provider-2.example.com/v1",
        None,
        true,
    );

    // Spawn a worker for the second provider (no profile references it, so
    // `provider_delete` won't reject on profile-reference check).
    pool.ensure_worker("test-provider-2")
        .expect("ensure_worker for second provider must succeed");
    assert_eq!(
        pool.worker_snapshots().await.len(),
        2,
        "two workers before delete"
    );

    // Delete the second provider — must remove ONLY its worker.
    sup.provider_delete(ProviderDeleteRequestDto {
        id: "test-provider-2".to_string(),
    })
    .await
    .expect("provider_delete must succeed");

    let snaps_after = pool.worker_snapshots().await;
    assert_eq!(
        snaps_after.len(),
        1,
        "only the deleted provider's worker must be removed, got: {snaps_after:?}"
    );
    assert_eq!(
        snaps_after[0].0, "test-provider",
        "the first provider's worker must still be alive"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `provider_update_with_api_key_removes_worker_then_respawns` (Phase 3 Task 7):
/// key rotation — when `provider_update` is called with
/// `req.api_key = Some(Some(k))`, the runtime must persist the new key to the
/// SQL store AND remove the worker so the next `ensure_worker` respawns with
/// the rotated key. Complements the metadata-only
/// `provider_changed_removes_worker_then_respawns` test by exercising the
/// credential-bearing code path. Covers Phase 3 Task 7.
#[tokio::test]
async fn provider_update_with_api_key_removes_worker_then_respawns() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let settings = make_sidecar_settings();
    let sup = make_sidecar_stopped_supervisor_with_settings(db, &tmp, settings);

    let pool = sup
        .worker_pool()
        .expect("worker_pool must be Some when sidecar is enabled");

    // Initially, construct_sidecar auto-spawned ONE worker for test-provider.
    let snaps = pool.worker_snapshots().await;
    assert_eq!(snaps.len(), 1, "initial state: one worker");
    assert_eq!(snaps[0].0, "test-provider");

    // Seed SQL with the same provider id so the SQL-backed provider_update
    // handler can find it.
    seed_provider_to_sql(
        &sup,
        "test-provider",
        "Test Provider",
        "https://api.test-provider.example.com/v1",
        None,
        true,
    );

    // Key rotation: api_key = Some(Some(k)) (three-state: update). This writes
    // the new key to the SQL providers table AND triggers provider_changed ->
    // remove_worker_and_kill.
    sup.provider_update(ProviderUpdateRequestDto {
        id: "test-provider".to_string(),
        name: None,
        base_url: None,
        enabled: None,
        api_key: Some(Some("rotated-new-key".to_string())),
    })
    .await
    .expect("provider_update with api_key rotation must succeed");

    // Worker must be gone from the pool (remove_worker_and_kill ran).
    let snaps_after = pool.worker_snapshots().await;
    assert!(
        snaps_after.is_empty(),
        "provider_update with api_key rotation must remove the worker from the pool, got: {snaps_after:?}"
    );

    // Re-spawn: ensure_worker creates a NEW worker (with the rotated key
    // injected into its env map).
    let re_spawned = pool
        .ensure_worker("test-provider")
        .expect("ensure_worker must re-spawn the worker after key rotation");
    assert!(
        re_spawned.worker_snapshot().await.is_some(),
        "re-spawned worker must produce a snapshot"
    );
    let snaps_final = pool.worker_snapshots().await;
    assert_eq!(
        snaps_final.len(),
        1,
        "re-spawned worker must be in the pool"
    );
    assert_eq!(snaps_final[0].0, "test-provider");

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// Check whether a process with the given PID is currently alive. Used by
/// `provider_update_kills_old_process` (Phase 3 Task 7) to verify the P1b
/// guarantee: `remove_worker_and_kill` actually kills the sidecar child,
/// rather than just dropping the worker from the pool map (which would
/// orphan the bash child). Uses sysinfo's targeted refresh to avoid a full
/// process scan.
fn is_process_alive(pid: u32) -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
    let mut sys = sysinfo::System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
        false,
        ProcessRefreshKind::everything(),
    );
    sys.process(sysinfo::Pid::from_u32(pid)).is_some()
}

/// `provider_update_kills_old_process` (Phase 3 Task 7, P1b fix): the
/// existing `provider_changed_removes_worker_then_respawns` test only checks
/// that the worker is removed from the pool map — it does NOT verify the
/// underlying sidecar child process is actually dead. This test captures the
/// child PID (after `ensure_started` spawns it) and asserts it is NO LONGER
/// alive after `provider_update`, proving `remove_worker_and_kill` actually
/// killed the process (not just dropped the entry — a plain `remove_worker`
/// without `force_kill` would orphan the bash child).
#[tokio::test]
async fn provider_update_kills_old_process() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let settings = make_sidecar_settings();
    let sup = make_sidecar_stopped_supervisor_with_settings(db, &tmp, settings);

    let pool = sup
        .worker_pool()
        .expect("worker_pool must be Some when sidecar is enabled");

    // The auto-spawned worker is Stopped (no child). Start the sidecar child
    // so we have a real OS PID to verify kill.
    let sidecar = sup
        .sidecar_supervisor()
        .expect("sidecar supervisor must be configured for the first enabled provider");
    let _handle = sidecar
        .ensure_started()
        .await
        .expect("ensure_started must spawn the child");

    // Capture the initial sidecar child PID.
    let snaps = pool.worker_snapshots().await;
    assert_eq!(snaps.len(), 1, "one worker before provider_update");
    let old_pid = snaps[0]
        .1
        .pid
        .expect("worker must have a PID after ensure_started");
    assert!(
        is_process_alive(old_pid),
        "old sidecar child (pid={old_pid}) must be alive before provider_update"
    );

    // Seed SQL with the same provider id so the SQL-backed provider_update
    // handler can find it.
    seed_provider_to_sql(
        &sup,
        "test-provider",
        "Test Provider",
        "https://api.test-provider.example.com/v1",
        None,
        true,
    );

    // Trigger provider_changed via provider_update (metadata-only change).
    // This calls remove_worker_and_kill -> force_kill -> child.start_kill() +
    // child.wait().await (SIGKILL on Unix).
    sup.provider_update(ProviderUpdateRequestDto {
        id: "test-provider".to_string(),
        name: Some("Updated".to_string()),
        base_url: None,
        enabled: None,
        api_key: None,
    })
    .await
    .expect("provider_update must succeed");

    // Give the kill (SIGKILL via child.start_kill()) time to take effect.
    // `force_kill` already awaits `child.wait()` (reaps the process), but
    // sysinfo's process table may lag by a tick. Poll up to 2s to avoid
    // flakiness on slow CI runners.
    let mut killed = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if !is_process_alive(old_pid) {
            killed = true;
            break;
        }
    }
    assert!(
        killed,
        "old sidecar child (pid={old_pid}) must be DEAD after provider_update — \
         remove_worker_and_kill must have force-killed it"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `provider_changed_no_op_when_pool_none` (Phase 3 Task 7): when the sidecar
/// is disabled (`worker_pool == None`), `provider_changed` and
/// `provider_deleted` must be safe no-ops — no panic, no error. The
/// `if let Some(pool) = &self.worker_pool` guard in both methods routes to a
/// `debug!` log + return on the None arm. Covers Phase 3 Task 7.
#[tokio::test]
async fn provider_changed_no_op_when_pool_none() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    // make_supervisor uses BusytokSupervisor::new (sidecar disabled) —
    // worker_pool is None.
    let sup = make_supervisor(db, &tmp);
    assert!(
        sup.worker_pool().is_none(),
        "worker_pool must be None when sidecar is disabled"
    );

    // Both must be safe no-ops regardless of whether the provider_id refers
    // to a real provider (the None arm doesn't look it up).
    sup.provider_changed("test-provider").await;
    sup.provider_deleted("test-provider").await;

    sup.shutdown_writer().await.expect("writer shutdown");
}

// ---------------------------------------------------------------------------
// Phase 3 Task 5: subagent usage bridge (usage_events + rollups)
// ---------------------------------------------------------------------------

/// Helper: count rows in `usage_events` matching the given predicate.
fn count_usage_events(db: &Database, where_clause: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM usage_events WHERE {where_clause}");
    db.conn().query_row(&sql, [], |row| row.get(0)).unwrap_or(0)
}

/// Helper: count rows in a table matching the given predicate.
fn count_rows(db: &Database, table: &str, where_clause: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE {where_clause}");
    db.conn().query_row(&sql, [], |row| row.get(0)).unwrap_or(0)
}

/// Helper: read the `generation_id` column of the first subagent usage event.
fn subagent_event_generation_id(db: &Database) -> Option<String> {
    db.conn()
        .query_row(
            "SELECT generation_id FROM usage_events WHERE client_kind = 'subagent' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok()
}

/// `normalize_usage_populates_required_fields` — the pure-function contract:
/// `client_kind = "subagent"`, `model`, `input_tokens`, `output_tokens`,
/// `total_tokens = input + output`. Also verifies `dedupe_key` is stable.
#[test]
fn normalize_usage_populates_required_fields() {
    let usage = TaskUsage {
        model: Some("gpt-5".to_string()),
        provider: Some("test".to_string()),
        input_tokens: Some(100),
        output_tokens: Some(200),
        cache_read_tokens: None,
        cache_write_tokens: None,
        cost_usd: None,
    };
    let event = normalize_task_usage("task-1", "sub-1", "/repo", &usage, None);
    assert_eq!(event.client_kind.as_deref(), Some("subagent"));
    assert_eq!(event.model.as_deref(), Some("gpt-5"));
    assert_eq!(event.input_tokens, 100);
    assert_eq!(event.output_tokens, 200);
    assert_eq!(event.total_tokens, 300);
    assert_eq!(event.cwd.as_deref(), Some("/repo"));
    assert_eq!(event.session_id, "sub-1");
    assert_eq!(event.agent, AgentKind::Codex);
}

/// `normalize_usage_handles_missing_tokens` — `input_tokens: None` → 0.
#[test]
fn normalize_usage_handles_missing_tokens() {
    let usage = TaskUsage {
        model: Some("gpt-5".to_string()),
        provider: None,
        input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_write_tokens: None,
        cost_usd: None,
    };
    let event = normalize_task_usage("task-2", "sub-1", "/repo", &usage, None);
    assert_eq!(event.input_tokens, 0);
    assert_eq!(event.output_tokens, 0);
    assert_eq!(event.total_tokens, 0);
}

/// `normalize_usage_cost_none_when_no_catalog` — `catalog: None` →
/// `cost_usd: None`, `cost_source: None`.
#[test]
fn normalize_usage_cost_none_when_no_catalog() {
    let usage = TaskUsage {
        model: Some("gpt-5".to_string()),
        provider: None,
        input_tokens: Some(100),
        output_tokens: Some(200),
        cache_read_tokens: None,
        cache_write_tokens: None,
        cost_usd: None,
    };
    let event = normalize_task_usage("task-3", "sub-1", "/repo", &usage, None);
    assert!(event.cost_usd.is_none());
    assert!(event.cost_source.is_none());
}

/// `write_usage_event_inserts_into_usage_events` — after the runtime
/// handler writes, `SELECT count(*) FROM usage_events WHERE
/// client_kind='subagent'` = 1.
#[tokio::test]
async fn write_usage_event_inserts_into_usage_events() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    set_active_generation(&db, "gen-write-1", 1000);
    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    let resp = sup
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "writer".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "do work".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .unwrap();
    assert_eq!(resp.status, "completed");

    // The mock executor produces non-zero token usage.
    let count = count_usage_events(&sup.db_handle().lock().unwrap(), "client_kind = 'subagent'");
    assert_eq!(
        count, 1,
        "exactly one subagent usage event should be in usage_events"
    );
}

/// `write_usage_event_idempotent_on_same_task_id` — write twice with the
/// same `task_id` → only 1 row (the `dedupe_key` provides idempotency).
#[tokio::test]
async fn write_usage_event_idempotent_on_same_task_id() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    set_active_generation(&db, "gen-write-2", 1000);
    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    let cwd = tmp.path().join("repo").to_string_lossy().to_string();

    // First delegate — writes one usage event via `write_subagent_usage_event`
    // (called internally by `subagent_delegate`).
    let resp1 = sup
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "idem".to_string(),
            subagent_id: None,
            cwd: cwd.clone(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "first run".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .unwrap();
    assert_eq!(resp1.status, "completed");

    let count = count_usage_events(&sup.db_handle().lock().unwrap(), "client_kind = 'subagent'");
    assert_eq!(
        count, 1,
        "exactly one subagent usage event should be in usage_events after first delegate"
    );

    // Re-issue the SAME task_id via the internal API to verify dedupe.
    // The `UsageWritePolicy::InsertOnce` dedupe_key (per-task_id) must
    // suppress the second insert, so the row count stays at 1.
    let result = busytok_subagent::models::DelegateResult {
        task_id: resp1.task_id.clone(),
        subagent_id: resp1.subagent_id.clone(),
        subagent_name: "idem".to_string(),
        adapter: "pi".to_string(),
        adapter_session_id: None,
        session_reused: false,
        status: busytok_subagent::models::TaskStatus::Completed,
        profile: "pi/search-cheap".to_string(),
        model: None,
        summary: Some("re-run".to_string()),
        usage: TaskUsage {
            model: Some("gpt-5".to_string()),
            provider: Some("mock".to_string()),
            input_tokens: Some(50),
            output_tokens: Some(50),
            cache_read_tokens: None,
            cache_write_tokens: None,
            cost_usd: None,
        },
    };
    sup.write_subagent_usage_event(&result, &cwd).unwrap();

    let final_count =
        count_usage_events(&sup.db_handle().lock().unwrap(), "client_kind = 'subagent'");
    assert_eq!(
        final_count, 1,
        "duplicate write with same task_id should be deduped by InsertOnce"
    );
}

/// `write_usage_event_uses_active_generation_id` (P0) — the event row's
/// `generation_id` equals `generation_manager.active_generation_id()`,
/// NOT a synthetic `subagent_{task_id}` string.
#[tokio::test]
async fn write_usage_event_uses_active_generation_id() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    set_active_generation(&db, "gen-active-for-subagent", 1000);
    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    let resp = sup
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "gen-check".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "verify generation".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .unwrap();
    assert_eq!(resp.status, "completed");

    let event_gen = subagent_event_generation_id(&sup.db_handle().lock().unwrap());
    assert_eq!(
        event_gen.as_deref(),
        Some("gen-active-for-subagent"),
        "subagent usage event must use the active generation_id, not a synthetic one"
    );
}

/// `write_usage_event_produces_real_rollup_rows` (P1a) — after the handler
/// writes, `SELECT count(*) FROM daily_usage WHERE agent='codex'` >= 1
/// AND `SELECT count(*) FROM model_summary` >= 1. This proves
/// `build_scan_mutations` ran (NOT `RollupRows::default()`).
#[tokio::test]
async fn write_usage_event_produces_real_rollup_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    set_active_generation(&db, "gen-rollup", 1000);
    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    let resp = sup
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "rollup".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "produce rollups".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: Some("gpt-5".to_string()),
            source_harness: None,
            source_session_id: None,
        })
        .await
        .unwrap();
    assert_eq!(resp.status, "completed");

    let db_ref = sup.db_handle().lock().unwrap();
    let daily_count = count_rows(
        &db_ref,
        "daily_usage",
        "agent = 'codex' AND generation_id = 'gen-rollup'",
    );
    assert!(
        daily_count >= 1,
        "daily_usage should have >=1 row for agent='codex', got {daily_count}"
    );

    let model_count = count_rows(&db_ref, "model_summary", "model = 'gpt-5'");
    assert!(
        model_count >= 1,
        "model_summary should have >=1 row for model='gpt-5', got {model_count}"
    );
}

/// `write_usage_event_visible_in_overview_read_path` (P1a end-to-end) —
/// after the handler writes, `read_overview_summary_from_daily_usage(...)`
/// returns totals that include the subagent tokens.
#[tokio::test]
async fn write_usage_event_visible_in_overview_read_path() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    // Use a fixed-offset timezone so the Overview read path uses
    // `read_overview_summary_from_daily_usage` (the IANA path).
    let settings = BusytokSettings {
        timezone: "+00:00".to_string(),
        ..BusytokSettings::default()
    };
    settings
        .save_to_file(&tmp.path().join("config").join("settings.toml"))
        .ok();
    let paths = BusytokPaths::for_test(tmp.path());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .ok();
    let db2 = busytok_store::Database::open_in_memory().unwrap();
    set_active_generation(&db2, "gen-overview", 1000);
    let sup = make_supervisor_with_settings(db2, &tmp, settings);
    sup.hydrate_status_from_db().unwrap();

    let resp = sup
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "overview".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "be visible in overview".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .unwrap();
    assert_eq!(resp.status, "completed");

    // The mock executor produces input_tokens = prompt.len() and
    // output_tokens = summary.len(), both non-zero. Read the overview
    // summary from daily_usage and verify total_tokens > 0.
    let db_ref = sup.db_handle().lock().unwrap();
    let rtz = ReportingTimezone::parse("+00:00").unwrap();
    let (year, month, day) = rtz.today_civil_ymd().unwrap_or((2026, 1, 1));
    // Use a wide date range so today's event is always included.
    let start_date = format!("{year:04}-01-01");
    let end_date = format!("{year:04}-12-31");
    let summary = busytok_store::read_queries::read_overview_summary_from_daily_usage(
        db_ref.conn(),
        rtz.canonical_name(),
        &start_date,
        &end_date,
        "gen-overview",
    )
    .unwrap();
    assert!(
        summary.total_tokens > 0,
        "overview summary should include subagent tokens, got {}",
        summary.total_tokens
    );
    assert!(
        summary.event_count >= 1,
        "overview summary should count the subagent event, got {}",
        summary.event_count
    );
}

// ---------------------------------------------------------------------------
// Phase 3 Task 8: end-to-end integration tests (acceptance criteria)
// ---------------------------------------------------------------------------

/// `e2e_multi_provider_creates_separate_workers` (Phase 3 Task 8): two
/// configured providers → two delegates → two separate worker entries in the
/// pool. Verifies the per-provider supervisor map routing (C7 fix).
#[tokio::test]
async fn e2e_multi_provider_creates_separate_workers() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let mut settings = make_sidecar_settings();
    // Add a second provider.
    settings.providers.push(ProviderConfig {
        id: "test-provider-2".to_string(),
        name: "Test Provider 2".to_string(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://api.test-provider-2.example.com/v1".to_string(),
        api_key_env_name: "TEST_API_KEY_2".to_string(),
        base_url_env_name: None,
        models: vec!["test-model".to_string()],
        enabled: true,
    });
    let sup = make_sidecar_stopped_supervisor_with_settings(db, &tmp, settings);

    let pool = sup.worker_pool().expect("worker_pool must be Some");

    // Auto-spawn only created ONE worker (for the first enabled provider).
    // Ensure the second provider's worker too.
    pool.ensure_worker("test-provider-2")
        .expect("ensure_worker for provider-2");
    let snaps = pool.worker_snapshots().await;
    assert_eq!(snaps.len(), 2, "two providers → two workers");
    let ids: Vec<&str> = snaps.iter().map(|(id, _)| id.as_str()).collect();
    assert!(ids.contains(&"test-provider"), "provider 1 worker present");
    assert!(
        ids.contains(&"test-provider-2"),
        "provider 2 worker present"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `e2e_auth_failure_kills_worker` (Phase 3 Task 8): mock sidecar returns 401
/// (AUTH_FAILURE -32010) → task fails with `error_kind: "auth"` → worker
/// killed+removed from pool → next ensure_worker creates a new worker.
/// Verifies the auth-fail kill path end-to-end through the delegate handler
/// (not just the executor level — `sidecar_executor.rs` already covers that).
///
/// NOTE: This test spawns a real sidecar child (mock-sidecar.sh) so it is
/// `#[serial]` to prevent parallel contamination.
#[tokio::test]
#[serial]
async fn e2e_auth_failure_kills_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    set_active_generation(&db, "gen-auth-fail", 1000);
    let mut settings = make_sidecar_settings();
    // The default profile model ("deepseek-chat") is NOT in the test
    // provider's whitelist (["test-model"]). Set it to "test-model" so the
    // delegate passes whitelist validation and reaches the executor (where
    // the auth-fail mock returns -32010).
    settings
        .subagent
        .profiles
        .get_mut("pi/search-cheap")
        .unwrap()
        .model = "test-model".to_string();
    let paths = BusytokPaths::for_test(tmp.path());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();
    let mut sidecar_cfg = make_sidecar_config();
    sidecar_cfg
        .env
        .insert("BUSYTOK_MOCK_AUTH_FAIL".into(), "1".into());
    let sup = BusytokSupervisor::new_with_sidecar_config(db, paths, sidecar_cfg);

    // The sidecar worker-pool closure reads provider config from TOML
    // (`settings.providers`), but the SQL-backed delegate validation queries
    // the `providers` / `models` tables. Seed both so the delegate passes
    // validation and reaches the executor (where the auth-fail mock fires).
    seed_provider_to_sql(
        &sup,
        "test-provider",
        "Test Provider",
        "https://api.test-provider.example.com/v1",
        None,
        true,
    );
    seed_model_to_sql(&sup, "test-provider", "test-model", true);

    let pool = sup.worker_pool().expect("worker_pool must be Some");
    // Auto-spawn created one worker for test-provider (Stopped — no child).
    assert_eq!(
        pool.worker_snapshots().await.len(),
        1,
        "one worker before delegate"
    );

    // Start the sidecar child so we have a real OS PID to verify kill.
    // The auto-spawned worker is Stopped; `ensure_started` spawns the bash
    // child running mock-sidecar.sh.
    let sidecar = sup
        .sidecar_supervisor()
        .expect("sidecar supervisor must be configured for the first enabled provider");
    let _handle = sidecar
        .ensure_started()
        .await
        .expect("ensure_started must spawn the child");

    // Capture the sidecar child PID before the delegate triggers auth-fail kill.
    let old_pid = {
        let snaps = pool.worker_snapshots().await;
        snaps[0]
            .1
            .pid
            .expect("worker must have a PID after ensure_started")
    };
    assert!(
        is_process_alive(old_pid),
        "sidecar child (pid={old_pid}) must be alive before delegate"
    );

    // Delegate — will hit the auth-fail path. The executor's `execute()`
    // returns Err (SidecarRpc -32010), which propagates through
    // `execute_task` → `delegate` → `subagent_delegate`. The task row was
    // already inserted (status="running") before execute() ran.
    let resp = sup
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "reviewer".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "do work".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await;

    // The delegate returns Err because execute() returned Err (the `?` in
    // `execute_task` propagates it before the success-path result
    // persistence block runs).
    assert!(
        resp.is_err(),
        "delegate must return Err on auth failure, got: {resp:?}"
    );

    // Verify the task row has error_kind = "auth". The task row was inserted
    // (status="running") before execute() ran. The executor classifies the
    // -32010 error as TaskErrorKind::Auth and calls remove_worker_and_kill,
    // then returns Err. Spec §3.4 / Task 5 require the error_kind to be
    // persisted on the task row.
    let db_handle = sup.db_handle();
    let error_kind: String = db_handle
        .lock()
        .unwrap()
        .conn()
        .query_row(
            "SELECT error_kind FROM subagent_tasks ORDER BY created_at_ms DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or_default();
    assert_eq!(
        error_kind, "auth",
        "task error_kind must be 'auth' after 401 (AUTH_FAILURE -32010)"
    );

    // Worker must be removed from the pool (auth-fail kill).
    assert!(
        pool.worker_snapshots().await.is_empty(),
        "pool must be empty after auth-fail kill"
    );

    // The sidecar child PROCESS must actually be dead (P1b guarantee).
    // `remove_worker_and_kill` calls `force_kill` which SIGKILLs the child;
    // a mere `remove_worker` (without kill) would orphan the bash child.
    // Poll up to 2s — sysinfo's process table may lag by a tick.
    let mut killed = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if !is_process_alive(old_pid) {
            killed = true;
            break;
        }
    }
    assert!(
        killed,
        "sidecar child (pid={old_pid}) must be DEAD after auth-fail — \
         remove_worker_and_kill must have force-killed it"
    );

    // Next ensure_worker creates a new worker (slot was freed).
    pool.ensure_worker("test-provider")
        .expect("ensure_worker re-spawns");
    assert_eq!(pool.worker_snapshots().await.len(), 1);

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `e2e_usage_events_agent_kind_is_codex` (Phase 3 Task 8, I3 fix): after
/// delegate, the `agent` column in `usage_events` for the subagent row must
/// be `'codex'` — verifying the `AgentKind::Codex` discriminator round-trips
/// through the normalize → insert pipeline. (The pi-sidecar wraps a
/// Codex-family SDK, so `AgentKind::Codex` is the correct agent kind for
/// subagent events.)
#[tokio::test]
async fn e2e_usage_events_agent_kind_is_codex() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    set_active_generation(&db, "gen-codex", 1000);
    let sup = make_supervisor(db, &tmp);
    sup.hydrate_status_from_db().unwrap();

    sup.subagent_delegate(SubagentDelegateRequestDto {
        subagent_name: "writer".to_string(),
        subagent_id: None,
        cwd: tmp.path().join("repo").to_string_lossy().to_string(),
        profile: "pi/search-cheap".to_string(),
        intent: None,
        prompt: "do work".to_string(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
    })
    .await
    .unwrap();

    let agent: String = sup
        .db_handle()
        .lock()
        .unwrap()
        .conn()
        .query_row(
            "SELECT agent FROM usage_events WHERE client_kind = 'subagent' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("must have a subagent usage event");
    assert_eq!(
        agent, "codex",
        "subagent usage event agent must be 'codex' (AgentKind::Codex.as_str())"
    );

    sup.shutdown_writer().await.expect("writer shutdown");
}

/// `queued_task_bridges_usage_events_through_dispatcher_hook` (P1 #2 fix):
/// when a task is queued (because the pressure gate is paused) and later
/// executed by the background `TaskDispatcher` after the gate clears, the
/// post-task completion hook MUST bridge the queued task's usage into the
/// unified `usage_events` + rollup tables — the SAME seam synchronous
/// `delegate()` uses. Without the hook, queued tasks' usage is invisible in
/// Overview / Activity / receipt reads.
///
/// Sequence:
/// 1. Pause the gate → delegate returns `Queued`.
/// 2. Assert NO usage_events / daily_usage / model_summary rows exist yet
///    (proves the queued task hasn't been bridged — guards against tests
///    that pass simply because the sync seam wrote events before queueing).
/// 3. Resume the gate → dispatcher picks up + executes the queued task.
/// 4. Assert usage_events has a row tagged with the active generation_id,
///    and daily_usage / model_summary have rollup rows.
///
/// NOTE: spawns a real sidecar child (mock-sidecar.sh) — `#[serial]`.
#[tokio::test]
#[serial]
async fn queued_task_bridges_usage_events_through_dispatcher_hook() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    set_active_generation(&db, "gen-queued-bridge", 1000);
    let mut settings = make_sidecar_settings();
    // The default profile model ("deepseek-chat") is NOT in the test
    // provider's whitelist (["test-model"]). Set it to "test-model" so the
    // delegate passes whitelist validation and the dispatcher can execute it.
    settings
        .subagent
        .profiles
        .get_mut("pi/search-cheap")
        .unwrap()
        .model = "test-model".to_string();
    let paths = BusytokPaths::for_test(tmp.path());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .unwrap();
    let sup = BusytokSupervisor::new_with_sidecar_config(db, paths, make_sidecar_config());
    sup.hydrate_status_from_db().unwrap();

    // The sidecar worker-pool closure reads provider config from TOML
    // (`settings.providers`), but the SQL-backed delegate validation queries
    // the `providers` / `models` tables. Seed both so the delegate (and the
    // dispatcher's later execution of the queued task) pass validation.
    seed_provider_to_sql(
        &sup,
        "test-provider",
        "Test Provider",
        "https://api.test-provider.example.com/v1",
        None,
        true,
    );
    seed_model_to_sql(&sup, "test-provider", "test-model", true);

    let gate = sup
        .pressure_gate()
        .expect("pressure_gate must be Some when sidecar is enabled")
        .clone();

    // 1. Pause the gate → next delegate must queue.
    gate.set_action(busytok_subagent::PressureAction::PauseNewTasks);
    assert!(gate.is_paused(), "gate must be paused before delegate");

    let resp = sup
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "queued-bridge".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "queued bridge work".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .unwrap();
    assert_eq!(
        resp.status, "queued",
        "delegate must return Queued when gate is paused, got: {:?}",
        resp.status
    );
    let sub_id = resp.subagent_id.clone();
    let task_id = resp.task_id.clone();

    // 2. Pre-condition: NO usage_events / rollups for this task yet.
    //    This proves the queued task hasn't been bridged (the sync seam
    //    didn't fire — only the dispatcher hook can produce these rows).
    {
        let db_ref = sup.db_handle().lock().unwrap();
        let event_count = count_rows(
            &db_ref,
            "usage_events",
            &format!("client_kind = 'subagent' AND dedupe_key = '{task_id}'"),
        );
        assert_eq!(
            event_count, 0,
            "no usage_event row should exist for the queued task before dispatcher runs"
        );
        let daily_count = count_rows(
            &db_ref,
            "daily_usage",
            "generation_id = 'gen-queued-bridge'",
        );
        assert_eq!(
            daily_count, 0,
            "no daily_usage rollup should exist before dispatcher runs"
        );
        let model_count = count_rows(&db_ref, "model_summary", "1=1");
        assert_eq!(
            model_count, 0,
            "no model_summary rollup should exist before dispatcher runs"
        );
        // Sanity: the task row itself is queued.
        let task_status: String = db_ref
            .conn()
            .query_row(
                "SELECT status FROM subagent_tasks WHERE id = ?1",
                rusqlite::params![&task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(task_status, "queued", "task row must be in 'queued' status");
    }

    // 3. Resume the gate → dispatcher should pick up + execute the queued
    //    task. Poll for up to 10s for the task to complete (the dispatcher
    //    ticks at 200ms, and the mock sidecar needs to spawn a bash child).
    gate.set_action(busytok_subagent::PressureAction::Resume);
    assert!(!gate.is_paused(), "gate must be resumed before polling");

    let mut completed = false;
    for _ in 0..100 {
        let status_now = {
            let db_ref = sup.db_handle().lock().unwrap();
            db_ref
                .conn()
                .query_row(
                    "SELECT status FROM subagent_tasks WHERE id = ?1",
                    rusqlite::params![&task_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap_or_default()
        };
        if status_now == "completed" {
            completed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        completed,
        "queued task must be executed by the dispatcher after gate clears"
    );

    // 4. Post-condition: usage_events + rollups are NOW present (the
    //    dispatcher's post-task completion hook bridged them through the
    //    same `bridge_subagent_usage` seam as synchronous `delegate()`).
    let db_ref = sup.db_handle().lock().unwrap();

    // Diagnostic: collect all subagent usage_events so an assertion failure
    // message shows whether the hook wrote ANYTHING (and with what
    // dedupe_key / generation_id), instead of just "left: None, right: Some".
    let all_subagent_events: Vec<(String, String)> = db_ref
        .conn()
        .prepare(
            "SELECT dedupe_key, generation_id FROM usage_events WHERE client_kind = 'subagent'",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    let event_gen = db_ref
        .conn()
        .query_row(
            "SELECT generation_id FROM usage_events WHERE client_kind = 'subagent' LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok();
    assert_eq!(
        event_gen.as_deref(),
        Some("gen-queued-bridge"),
        "queued task's usage_event must use the active generation_id \
         (proves the dispatcher hook sourced it from generation_manager, \
         not a synthetic placeholder). All subagent events: {all_subagent_events:?}"
    );

    let daily_count = count_rows(
        &db_ref,
        "daily_usage",
        "agent = 'codex' AND generation_id = 'gen-queued-bridge'",
    );
    assert!(
        daily_count >= 1,
        "daily_usage should have >=1 row for agent='codex' after dispatcher runs, got {daily_count}"
    );

    // The mock sidecar always returns model="deepseek-chat" in its canned
    // response (regardless of the profile model the runtime requested).
    // Assert against the sidecar's reported model, not the profile model.
    let model_count = count_rows(&db_ref, "model_summary", "model = 'deepseek-chat'");
    assert!(
        model_count >= 1,
        "model_summary should have >=1 row for model='deepseek-chat' after dispatcher runs, got {model_count}"
    );

    // Drop the db guard before awaiting shutdown (clippy::await_holding_lock).
    drop(db_ref);

    // Graceful shutdown: stop the sidecar child + drain the dispatcher +
    // flush the writer. Order matters: shutdown_sidecar first so the child
    // is dead, then shutdown_writer so the dispatcher is drained before the
    // writer's final flush + WAL checkpoint.
    sup.shutdown_sidecar().await;
    sup.shutdown_writer().await.expect("writer shutdown");

    // Suppress unused-variable warning for `sub_id` (kept for debuggability
    // — readers can inspect the task row by subagent_id if the test fails).
    let _ = sub_id;
}

// ---------------------------------------------------------------------------
// Profile CRUD (Phase 4 Task 4: Profile/Model Configuration UI)
// ---------------------------------------------------------------------------
//
// Tests exercise the BusytokSupervisor's profile_create / profile_update /
// profile_delete handlers via the RuntimeControl trait. Built-in profiles
// (pi/search-cheap, pi/review-cheap, pi/plan-cheap) ship with default
// settings — these tests verify CRUD on user profiles + validation paths.

#[tokio::test]
async fn profile_crud_round_trips() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Built-in profiles exist from default_settings.
    let snapshot = sup.settings_snapshot().await.unwrap();
    assert_eq!(snapshot.data.subagent.profiles.len(), 3);
    assert!(snapshot
        .data
        .subagent
        .profiles
        .iter()
        .any(|p| p.id == "pi/search-cheap"));
    assert!(snapshot.data.subagent.profiles.iter().all(|p| p.is_builtin));

    // Create a user profile.
    let created = sup
        .profile_create(ProfileCreateRequestDto {
            id: "my-reviewer".to_string(),
            model: "deepseek-chat".to_string(),
            provider_id: None,
            tools: Some(vec!["read".to_string(), "grep".to_string()]),
            context_budget_tokens: Some(4000),
            timeout_seconds: Some(150),
            write_access: Some(false),
        })
        .await
        .unwrap();
    assert_eq!(created.id, "my-reviewer");
    assert!(!created.is_builtin);
    assert_eq!(created.model, "deepseek-chat");
    assert_eq!(created.context_budget_tokens, 4000);

    // Settings snapshot now shows 4 profiles.
    let snapshot = sup.settings_snapshot().await.unwrap();
    assert_eq!(snapshot.data.subagent.profiles.len(), 4);

    // Update model + provider_id (patch semantics).
    let updated = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "my-reviewer".to_string(),
            provider_id: Some("".to_string()), // unbind (empty string = None)
            model: Some("qwen-coder".to_string()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap();
    assert_eq!(updated.model, "qwen-coder");
    // provider_id was Some("") → unbound → None.
    assert_eq!(updated.provider_id, None);
    // tools/context_budget_tokens unchanged (patch semantics).
    assert_eq!(updated.tools, vec!["read", "grep"]);
    assert_eq!(updated.context_budget_tokens, 4000);

    // Delete the user profile.
    sup.profile_delete(ProfileDeleteRequestDto {
        id: "my-reviewer".to_string(),
    })
    .await
    .unwrap();
    let snapshot = sup.settings_snapshot().await.unwrap();
    assert_eq!(snapshot.data.subagent.profiles.len(), 3);

    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_create_rejects_builtin_name() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .profile_create(ProfileCreateRequestDto {
            id: "pi/search-cheap".to_string(),
            model: "deepseek-chat".to_string(),
            provider_id: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("reserved for built-in"),
        "expected reserved-name error, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_delete_rejects_builtin() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .profile_delete(ProfileDeleteRequestDto {
            id: "pi/search-cheap".to_string(),
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("cannot delete built-in"),
        "expected built-in rejection, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_rejects_disabled_provider() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create a provider, then disable it.
    let created = sup
        .provider_create(provider_create_request("Disabled"))
        .await
        .unwrap();
    let pid = created.id.clone();
    sup.provider_update(ProviderUpdateRequestDto {
        id: pid.clone(),
        name: None,
        base_url: None,
        enabled: Some(false),
        api_key: None,
    })
    .await
    .unwrap();

    // Try to bind a profile to the disabled provider → rejected.
    let err = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "pi/search-cheap".to_string(),
            provider_id: Some(pid.clone()),
            model: Some("some-model".to_string()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("disabled provider"),
        "expected disabled-provider rejection, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_rejects_stale_model_on_rebind() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create a provider and seed model "model-a" (enabled) in SQL.
    let created = sup
        .provider_create(provider_create_request("Test"))
        .await
        .unwrap();
    let pid = created.id.clone();
    seed_model_to_sql(&sup, &pid, "model-a", true);

    // Bind profile to provider with model-a.
    sup.profile_update(ProfileUpdateRequestDto {
        id: "pi/search-cheap".to_string(),
        provider_id: Some(pid.clone()),
        model: Some("model-a".to_string()),
        tools: None,
        context_budget_tokens: None,
        timeout_seconds: None,
        write_access: None,
    })
    .await
    .unwrap();

    // Disable model-a in SQL — model-a is now "stale" (no longer in the
    // enabled whitelist).
    set_model_enabled_in_sql(&sup, &pid, "model-a", false);

    // Re-bind to the same provider without changing the model → the rebind
    // path validates the effective model (model-a) against the SQL whitelist
    // and rejects because model-a is disabled.
    let err = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "pi/search-cheap".to_string(),
            provider_id: Some(pid.clone()), // re-bind same provider
            model: None,                    // not changing model
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not in provider") || err.to_string().contains("whitelist"),
        "expected stale-model rejection, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_patches_tools_without_triggering_stale_check() {
    // Patching only `tools` (neither provider_id nor model) must NOT run
    // the whitelist validation — the service trusts the existing binding
    // and only the UI surfaces stale-model warnings for already-bound profiles.
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let created = sup
        .provider_create(provider_create_request("Test"))
        .await
        .unwrap();
    let pid = created.id.clone();
    seed_model_to_sql(&sup, &pid, "model-a", true);
    sup.profile_update(ProfileUpdateRequestDto {
        id: "pi/search-cheap".to_string(),
        provider_id: Some(pid.clone()),
        model: Some("model-a".to_string()),
        tools: None,
        context_budget_tokens: None,
        timeout_seconds: None,
        write_access: None,
    })
    .await
    .unwrap();

    // Disable model-a so it's "stale" (no longer in the enabled whitelist).
    set_model_enabled_in_sql(&sup, &pid, "model-a", false);

    // Patch ONLY tools — should succeed despite the stale model, because
    // the service does not re-validate existing bindings on unrelated patches.
    let updated = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "pi/search-cheap".to_string(),
            provider_id: None, // unchanged
            model: None,       // unchanged
            tools: Some(vec!["new-tool".to_string()]),
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap();
    assert_eq!(updated.tools, vec!["new-tool".to_string()]);
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn settings_snapshot_includes_subagent_profiles() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let snapshot = sup.settings_snapshot().await.unwrap();
    assert!(snapshot.data.subagent.enabled);
    assert_eq!(snapshot.data.subagent.profiles.len(), 3);
    let search = snapshot
        .data
        .subagent
        .profiles
        .iter()
        .find(|p| p.id == "pi/search-cheap")
        .unwrap();
    assert!(search.is_builtin);
    assert_eq!(search.model, "deepseek-chat");
    assert_eq!(search.provider_id, None);
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_create_rejects_nonexistent_provider() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .profile_create(ProfileCreateRequestDto {
            id: "my-profile".to_string(),
            model: "some-model".to_string(),
            provider_id: Some("nonexistent-provider".to_string()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("provider not found"),
        "expected provider-not-found error, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_create_rejects_model_not_in_whitelist() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create a provider and seed only "model-a" in its SQL whitelist.
    let created = sup
        .provider_create(provider_create_request("Test"))
        .await
        .unwrap();
    let pid = created.id.clone();
    seed_model_to_sql(&sup, &pid, "model-a", true);

    // Try to create a profile bound to that provider with a model NOT in its whitelist.
    let err = sup
        .profile_create(ProfileCreateRequestDto {
            id: "my-profile".to_string(),
            model: "model-b".to_string(),
            provider_id: Some(pid.clone()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not in provider") || err.to_string().contains("whitelist"),
        "expected whitelist rejection, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_rejects_nonexistent_profile() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "nonexistent-profile".to_string(),
            provider_id: None,
            model: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("profile not found"),
        "expected not-found error, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_delete_rejects_nonexistent_profile() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .profile_delete(ProfileDeleteRequestDto {
            id: "nonexistent-profile".to_string(),
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("profile not found"),
        "expected not-found error, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_create_rejects_duplicate_id() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // First create succeeds.
    sup.profile_create(ProfileCreateRequestDto {
        id: "my-profile".to_string(),
        model: "some-model".to_string(),
        provider_id: None,
        tools: None,
        context_budget_tokens: None,
        timeout_seconds: None,
        write_access: None,
    })
    .await
    .unwrap();

    // Second create with the same id fails.
    let err = sup
        .profile_create(ProfileCreateRequestDto {
            id: "my-profile".to_string(),
            model: "other-model".to_string(),
            provider_id: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("already exists"),
        "expected already-exists error, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_unbinds_provider_with_empty_string() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create a provider + seed model-a + bind a user profile to it.
    let created = sup
        .provider_create(provider_create_request("Test"))
        .await
        .unwrap();
    let pid = created.id.clone();
    seed_model_to_sql(&sup, &pid, "model-a", true);
    sup.profile_create(ProfileCreateRequestDto {
        id: "my-profile".to_string(),
        model: "model-a".to_string(),
        provider_id: Some(pid.clone()),
        tools: None,
        context_budget_tokens: None,
        timeout_seconds: None,
        write_access: None,
    })
    .await
    .unwrap();

    // Unbind via Some("").
    let updated = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "my-profile".to_string(),
            provider_id: Some("".to_string()),
            model: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap();
    assert_eq!(updated.provider_id, None);
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_with_all_none_patch_is_noop() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Capture the built-in search profile's pre-state.
    let before = sup
        .settings_snapshot()
        .await
        .unwrap()
        .data
        .subagent
        .profiles
        .iter()
        .find(|p| p.id == "pi/search-cheap")
        .cloned()
        .unwrap();

    // Patch everything as None (no-op).
    let after = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "pi/search-cheap".to_string(),
            provider_id: None,
            model: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap();

    // Profile is returned unchanged.
    assert_eq!(after.model, before.model);
    assert_eq!(after.provider_id, before.provider_id);
    assert_eq!(after.tools, before.tools);
    assert_eq!(after.context_budget_tokens, before.context_budget_tokens);
    assert_eq!(after.timeout_seconds, before.timeout_seconds);
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_changes_provider_and_model_together() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create two providers, each with a different model in SQL.
    let prov_a = sup
        .provider_create(provider_create_request("Prov A"))
        .await
        .unwrap();
    let pid_a = prov_a.id.clone();
    seed_model_to_sql(&sup, &pid_a, "model-a", true);

    let prov_b = sup
        .provider_create(provider_create_request("Prov B"))
        .await
        .unwrap();
    let pid_b = prov_b.id.clone();
    seed_model_to_sql(&sup, &pid_b, "model-b", true);

    // Create a profile bound to prov-a/model-a.
    sup.profile_create(ProfileCreateRequestDto {
        id: "my-profile".to_string(),
        model: "model-a".to_string(),
        provider_id: Some(pid_a.clone()),
        tools: None,
        context_budget_tokens: None,
        timeout_seconds: None,
        write_access: None,
    })
    .await
    .unwrap();

    // Atomically switch to prov-b/model-b.
    let updated = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "my-profile".to_string(),
            provider_id: Some(pid_b.clone()),
            model: Some("model-b".to_string()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap();
    assert_eq!(updated.provider_id, Some(pid_b.clone()));
    assert_eq!(updated.model, "model-b");
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_create_rejects_invalid_id_format() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Uppercase and spaces are not allowed.
    let err = sup
        .profile_create(ProfileCreateRequestDto {
            id: "My Profile".to_string(),
            model: "some-model".to_string(),
            provider_id: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("[a-z0-9/_-]+") || err.to_string().contains("id format"),
        "expected id-format rejection, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_patches_only_model_on_bound_profile() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create provider + seed two models + profile bound to it.
    let created = sup
        .provider_create(provider_create_request("Test"))
        .await
        .unwrap();
    let pid = created.id.clone();
    seed_model_to_sql(&sup, &pid, "model-a", true);
    seed_model_to_sql(&sup, &pid, "model-b", true);
    sup.profile_create(ProfileCreateRequestDto {
        id: "my-profile".to_string(),
        model: "model-a".to_string(),
        provider_id: Some(pid.clone()),
        tools: None,
        context_budget_tokens: None,
        timeout_seconds: None,
        write_access: None,
    })
    .await
    .unwrap();

    // Patch only the model (provider_id stays None = unchanged).
    let updated = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "my-profile".to_string(),
            provider_id: None,
            model: Some("model-b".to_string()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap();
    // Provider unchanged, model updated.
    assert_eq!(updated.provider_id, Some(pid.clone()));
    assert_eq!(updated.model, "model-b");
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_create_applies_defaults_for_omitted_fields() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create a profile with tools/budget/timeout all None.
    let dto = sup
        .profile_create(ProfileCreateRequestDto {
            id: "my-profile".to_string(),
            model: "some-model".to_string(),
            provider_id: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap();

    // Defaults: write_access=false, tools=[], budget=3000, timeout=120.
    assert!(!dto.write_access);
    assert_eq!(dto.tools, Vec::<String>::new());
    assert_eq!(dto.context_budget_tokens, 3000);
    assert_eq!(dto.timeout_seconds, 120);
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_create_rejects_empty_id() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .profile_create(ProfileCreateRequestDto {
            id: "".to_string(),
            model: "some-model".to_string(),
            provider_id: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("profile id must not be empty"),
        "expected empty-id rejection, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_create_rejects_empty_model() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    let err = sup
        .profile_create(ProfileCreateRequestDto {
            id: "my-profile".to_string(),
            model: "".to_string(),
            provider_id: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("model must not be empty"),
        "expected empty-model rejection, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_rejects_model_not_in_whitelist_on_model_only_patch() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create provider and seed only "model-a" in its SQL whitelist.
    let created = sup
        .provider_create(provider_create_request("Test"))
        .await
        .unwrap();
    let pid = created.id.clone();
    seed_model_to_sql(&sup, &pid, "model-a", true);

    // Bind profile to provider with the whitelisted model.
    sup.profile_create(ProfileCreateRequestDto {
        id: "my-profile".to_string(),
        model: "model-a".to_string(),
        provider_id: Some(pid.clone()),
        tools: None,
        context_budget_tokens: None,
        timeout_seconds: None,
        write_access: None,
    })
    .await
    .unwrap();

    // Patch only the model to a value NOT in the provider whitelist.
    let err = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "my-profile".to_string(),
            provider_id: None,
            model: Some("model-b".to_string()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not in provider") || err.to_string().contains("whitelist"),
        "expected whitelist rejection on model-only patch, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_rejects_empty_model_unbound() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Built-in profile ships unbound; patch model to empty string.
    let err = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "pi/search-cheap".to_string(),
            provider_id: None,
            model: Some(String::new()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("model must not be empty"),
        "expected empty-model rejection on unbound profile, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}

#[tokio::test]
async fn profile_update_rejects_empty_model_bound() {
    let db = Database::open_in_memory().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let sup = make_supervisor(db, &tmp);

    // Create provider + seed model-a + bind profile to it.
    let created = sup
        .provider_create(provider_create_request("Test"))
        .await
        .unwrap();
    let pid = created.id.clone();
    seed_model_to_sql(&sup, &pid, "model-a", true);
    sup.profile_create(ProfileCreateRequestDto {
        id: "my-profile".to_string(),
        model: "model-a".to_string(),
        provider_id: Some(pid.clone()),
        tools: None,
        context_budget_tokens: None,
        timeout_seconds: None,
        write_access: None,
    })
    .await
    .unwrap();

    // Patch model to empty string while provider is bound.
    let err = sup
        .profile_update(ProfileUpdateRequestDto {
            id: "my-profile".to_string(),
            provider_id: None,
            model: Some(String::new()),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("model must not be empty"),
        "expected empty-model rejection on bound profile, got: {err}"
    );
    sup.shutdown_writer().await.unwrap();
}
