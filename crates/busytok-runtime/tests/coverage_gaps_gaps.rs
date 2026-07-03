#![allow(
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    unused_imports,
    unused_variables,
    dead_code
)]
//! Coverage gap tests targeting uncovered lines in:
//!
//! - `source_registry.rs` — codex manual roots (line 123), codex DB-configured
//!   roots (lines 168, 195, 209-211), unknown agent type path (lines 170-184).
//! - `generation_manager.rs` — `ensure_active_generation_for_existing_events`
//!   (line 107), `transition_after_initial_scan` ReadyDegraded→ReadyExact
//!   (lines 142, 171-172), `readiness_value` match arms (lines 148-150),
//!   DB transition returning false (line 181), `can_transition` false
//!   (lines 143-144), `hydrate_from_db` unexpected readiness (lines 230-237).
//! - `read_service.rs` — `map_open_error` Internal fallback (line 369) via
//!   symlink-loop path that yields a non-standard SQLite error.

use std::sync::Arc;

use busytok_config::{BusytokPaths, BusytokSettings, ManualRootConfig};
use busytok_domain::now_ms;
use busytok_protocol::dto::ReadinessStateDto;
use busytok_runtime::BusytokSupervisor;
use busytok_store::Database;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_supervisor() -> (BusytokSupervisor, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let db = Database::open_in_memory().expect("open in-memory");
    (BusytokSupervisor::new(db, paths), dir)
}

fn make_supervisor_with_settings(
    settings: BusytokSettings,
) -> (BusytokSupervisor, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let db = Database::open_in_memory().expect("open in-memory");
    (
        BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings),
        dir,
    )
}

fn make_supervisor_with_file_db() -> (BusytokSupervisor, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let db_path = paths.db_path();
    let db = Database::open(&db_path).expect("open file-backed db");
    (
        BusytokSupervisor::with_adapters_and_settings(
            db,
            paths,
            vec![],
            settings_with_defaults_disabled(),
        ),
        dir,
    )
}

fn settings_with_defaults_disabled() -> BusytokSettings {
    let mut settings = BusytokSettings::default();
    settings.discovery.claude_code_default_paths = false;
    settings.discovery.codex_default_paths = false;
    settings
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

fn seed_service_state(db: &Database, readiness: &str, gen_id: Option<&str>) {
    let now = now_ms();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO service_state \
             (id, writer_queue_depth, aggregate_lag_ms, readiness, \
              active_generation_id, updated_at_ms) \
             VALUES (1, 0, 0, ?1, ?2, ?3)",
            rusqlite::params![readiness, gen_id, now],
        )
        .expect("seed service_state");
}

fn seed_user_configured_root(db: &Database, id: &str, agent: &str, root_path: &str) {
    let now = now_ms();
    db.conn()
        .execute(
            "INSERT INTO log_sources \
             (id, agent, source_type, root_path, status, configured_by_user, \
              first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, 'jsonl', ?3, 'active', 1, ?4, ?4, ?4, ?4)",
            rusqlite::params![id, agent, root_path, now],
        )
        .expect("seed user-configured root");
}

// =============================================================================
// source_registry.rs — codex manual root exercises codex discovery path
// =============================================================================

#[tokio::test]
async fn source_registry_codex_manual_root_exercises_codex_discovery_path() {
    // Covers source_registry.rs line 123 (codex_roots non-empty branch) and
    // lines 209-211 (AgentKind::Codex branch in discover_roots_for_agent).
    //
    // We configure a codex manual root pointing to an empty temp directory.
    // Discovery will return zero sources (no log files), but the codex code
    // path (including discover_roots_for_agent's Codex arm) is exercised.
    let dir = tempfile::tempdir().expect("tempdir");
    let codex_root = dir.path().join("codex-logs");
    std::fs::create_dir_all(&codex_root).expect("create codex dir");

    let mut settings = settings_with_defaults_disabled();
    settings.discovery.manual_roots = vec![ManualRootConfig {
        id: "codex-manual-1".to_string(),
        client_id: "codex".to_string(),
        root_path: codex_root.display().to_string(),
    }];

    let (supervisor, _dir) = make_supervisor_with_settings(settings);
    let stats = supervisor
        .register_new_install_sources()
        .await
        .expect("register should succeed");
    // No log files in the empty dir → zero sources discovered.
    assert_eq!(stats.sources, 0);
}

// =============================================================================
// source_registry.rs — unknown agent type in DB publishes error event
// =============================================================================

#[tokio::test]
async fn source_registry_unknown_agent_type_in_db_publishes_error_event() {
    // Covers source_registry.rs lines 170-184: the `_` arm in the
    // AgentKind parse match, which publishes an ephemeral Error event and
    // logs a warning with event_code "source_registry.unknown_agent".
    //
    // We seed the DB with a user-configured root whose agent string is not
    // a valid AgentKind. discover_all() will encounter it, publish the
    // error event, and continue without erroring.
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let db = Database::open_in_memory().expect("db");

    seed_user_configured_root(&db, "unknown-1", "unknown_agent_type", "/tmp/unknown");

    let settings = settings_with_defaults_disabled();
    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    let stats = supervisor
        .register_new_install_sources()
        .await
        .expect("register should not error despite unknown agent");
    // Unknown agent type → no sources discovered from that root.
    assert_eq!(stats.sources, 0);
}

// =============================================================================
// source_registry.rs — codex DB-configured root exercises codex discovery
// =============================================================================

#[tokio::test]
async fn source_registry_codex_db_configured_root_exercises_codex_discovery() {
    // Covers source_registry.rs line 168 (Ok(AgentKind::Codex) arm) and
    // line 195 (codex_roots non-empty → discover_roots_for_agent for Codex).
    //
    // We seed the DB with a user-configured root whose agent is 'codex'.
    // discover_db_configured_roots will parse it as AgentKind::Codex and
    // attempt discovery on the root path.
    let dir = tempfile::tempdir().expect("tempdir");
    let codex_root = dir.path().join("codex-data");
    std::fs::create_dir_all(&codex_root).expect("create codex dir");

    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let db = Database::open_in_memory().expect("db");

    seed_user_configured_root(
        &db,
        "codex-db-1",
        "codex",
        &codex_root.display().to_string(),
    );

    let settings = settings_with_defaults_disabled();
    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    let stats = supervisor
        .register_new_install_sources()
        .await
        .expect("register should succeed");
    // Empty dir → zero sources, but codex DB-configured path was exercised.
    assert_eq!(stats.sources, 0);
}

// =============================================================================
// source_registry.rs — claude_code DB-configured root exercises claude path
// =============================================================================

#[tokio::test]
async fn source_registry_claude_db_configured_root_exercises_claude_discovery() {
    // Covers source_registry.rs line 167 (Ok(AgentKind::ClaudeCode) arm) and
    // line 192 (claude_roots non-empty → discover_roots_for_agent for Claude).
    //
    // We seed the DB with a user-configured root whose agent is 'claude_code'.
    let dir = tempfile::tempdir().expect("tempdir");
    let claude_root = dir.path().join("claude-data");
    std::fs::create_dir_all(&claude_root).expect("create claude dir");

    let paths = BusytokPaths::for_test(dir.path());
    paths.ensure_dirs_exist().expect("ensure dirs");
    let db = Database::open_in_memory().expect("db");

    seed_user_configured_root(
        &db,
        "claude-db-1",
        "claude_code",
        &claude_root.display().to_string(),
    );

    let settings = settings_with_defaults_disabled();
    let supervisor = BusytokSupervisor::with_adapters_and_settings(db, paths, vec![], settings);

    let stats = supervisor
        .register_new_install_sources()
        .await
        .expect("register should succeed");
    assert_eq!(stats.sources, 0);
}

// =============================================================================
// generation_manager.rs — transition from ReadyDegraded to ReadyExact
// =============================================================================

#[tokio::test]
async fn generation_manager_transition_from_ready_degraded_to_ready_exact() {
    // Covers generation_manager.rs:
    // - Line 142 (ReadyDegraded && target == ReadyExact → can_transition = true)
    // - Lines 148-150 (readiness_value match arms for "ready_exact")
    // - Lines 171-172 (ReadyDegraded check in the write().await path)
    //
    // We seed a promoted+active generation in the DB and set the snapshot
    // to ReadyDegraded. transition_after_initial_scan(ReadyExact) should
    // succeed because:
    //   1. can_transition is true (ReadyDegraded + ReadyExact target)
    //   2. transition_readiness returns true (promoted gen exists)
    //   3. In the write block, snap.readiness is still ReadyDegraded → can = true
    let (supervisor, _dir) = make_supervisor();
    let db = supervisor.db_handle().clone();
    {
        let db = db.lock().unwrap();
        seed_active_generation(&db, "gen-degraded-1");
        seed_service_state(&db, "ready_degraded", Some("gen-degraded-1"));
    }

    supervisor
        .apply_service_status_snapshot(|snap| {
            snap.readiness = ReadinessStateDto::ReadyDegraded;
            snap.active_generation_id = Some("gen-degraded-1".to_string());
        })
        .expect("apply snapshot");

    let transitioned = supervisor
        .transition_after_initial_scan(ReadinessStateDto::ReadyExact)
        .await
        .expect("transition");
    assert!(
        transitioned,
        "ReadyDegraded → ReadyExact should transition successfully"
    );

    let snap = supervisor.read_status_snapshot().await;
    assert_eq!(snap.readiness, ReadinessStateDto::ReadyExact);
}

// =============================================================================
// generation_manager.rs — transition returns false when cannot transition
// =============================================================================

#[tokio::test]
async fn generation_manager_transition_returns_false_when_cannot_transition() {
    // Covers generation_manager.rs lines 143-144: the `if !can_transition`
    // early return Ok(false) path.
    //
    // We set the snapshot to ReadyExact (not Starting, not ReadyDegraded)
    // with a non-empty active_generation_id. The first guard (empty gen_id)
    // passes, but can_transition is false.
    let (supervisor, _dir) = make_supervisor();
    let db = supervisor.db_handle().clone();
    {
        let db = db.lock().unwrap();
        seed_active_generation(&db, "gen-cannot-1");
    }

    supervisor
        .apply_service_status_snapshot(|snap| {
            snap.readiness = ReadinessStateDto::ReadyExact;
            snap.active_generation_id = Some("gen-cannot-1".to_string());
        })
        .expect("apply snapshot");

    let transitioned = supervisor
        .transition_after_initial_scan(ReadinessStateDto::ReadyExact)
        .await
        .expect("transition");
    assert!(
        !transitioned,
        "ReadyExact → ReadyExact should not transition (can_transition is false)"
    );
}

// =============================================================================
// generation_manager.rs — transition returns false when DB transition fails
// =============================================================================

#[tokio::test]
async fn generation_manager_transition_returns_false_when_db_transition_fails() {
    // Covers generation_manager.rs line 181: `Ok(false)` when
    // transition_readiness returns false (no promoted generation in DB).
    //
    // We set the snapshot to Starting with a gen_id that does NOT exist as
    // promoted+active in audit_generations. can_transition is true (Starting),
    // but transition_readiness returns false because the promoted gen check fails.
    let (supervisor, _dir) = make_supervisor();

    supervisor
        .apply_service_status_snapshot(|snap| {
            snap.readiness = ReadinessStateDto::Starting;
            snap.active_generation_id = Some("gen-not-in-db".to_string());
        })
        .expect("apply snapshot");

    let transitioned = supervisor
        .transition_after_initial_scan(ReadinessStateDto::ReadyExact)
        .await
        .expect("transition");
    assert!(
        !transitioned,
        "non-promoted generation → DB transition should return false"
    );
}

// =============================================================================
// generation_manager.rs — transition to Starting covers readiness_value arm
// =============================================================================

#[tokio::test]
async fn generation_manager_transition_to_starting_covers_readiness_value_arm() {
    // Covers generation_manager.rs line 148: `ReadinessStateDto::Starting => "starting"`
    // in the readiness_value match.
    let (supervisor, _dir) = make_supervisor();
    let db = supervisor.db_handle().clone();
    {
        let db = db.lock().unwrap();
        seed_active_generation(&db, "gen-target-starting");
    }

    supervisor
        .apply_service_status_snapshot(|snap| {
            snap.readiness = ReadinessStateDto::Starting;
            snap.active_generation_id = Some("gen-target-starting".to_string());
        })
        .expect("apply snapshot");

    let transitioned = supervisor
        .transition_after_initial_scan(ReadinessStateDto::Starting)
        .await
        .expect("transition");
    assert!(transitioned, "Starting → Starting should succeed");
}

// =============================================================================
// generation_manager.rs — transition to Rebuilding covers readiness_value arm
// =============================================================================

#[tokio::test]
async fn generation_manager_transition_to_rebuilding_covers_readiness_value_arm() {
    // Covers generation_manager.rs line 149: `ReadinessStateDto::Rebuilding => "rebuilding"`
    // in the readiness_value match.
    let (supervisor, _dir) = make_supervisor();
    let db = supervisor.db_handle().clone();
    {
        let db = db.lock().unwrap();
        seed_active_generation(&db, "gen-target-rebuilding");
    }

    supervisor
        .apply_service_status_snapshot(|snap| {
            snap.readiness = ReadinessStateDto::Starting;
            snap.active_generation_id = Some("gen-target-rebuilding".to_string());
        })
        .expect("apply snapshot");

    let transitioned = supervisor
        .transition_after_initial_scan(ReadinessStateDto::Rebuilding)
        .await
        .expect("transition");
    assert!(transitioned, "Starting → Rebuilding should succeed");
}

// =============================================================================
// generation_manager.rs — transition to ReadyDegraded covers readiness_value arm
// =============================================================================

#[tokio::test]
async fn generation_manager_transition_to_ready_degraded_covers_readiness_value_arm() {
    // Covers generation_manager.rs line 150: `ReadinessStateDto::ReadyDegraded => "ready_degraded"`
    // in the readiness_value match.
    let (supervisor, _dir) = make_supervisor();
    let db = supervisor.db_handle().clone();
    {
        let db = db.lock().unwrap();
        seed_active_generation(&db, "gen-target-degraded");
    }

    supervisor
        .apply_service_status_snapshot(|snap| {
            snap.readiness = ReadinessStateDto::Starting;
            snap.active_generation_id = Some("gen-target-degraded".to_string());
        })
        .expect("apply snapshot");

    let transitioned = supervisor
        .transition_after_initial_scan(ReadinessStateDto::ReadyDegraded)
        .await
        .expect("transition");
    assert!(transitioned, "Starting → ReadyDegraded should succeed");
}

// =============================================================================
// generation_manager.rs — hydrate_from_db with unexpected readiness value
// =============================================================================

#[test]
fn generation_manager_hydrate_from_db_with_unexpected_readiness_falls_back_to_starting() {
    // Covers generation_manager.rs lines 230-237: the `Some(other)` arm in
    // the readiness match, which logs a warning and falls back to Starting.
    //
    // We seed service_state with a bogus readiness string that doesn't match
    // any known value. hydrate_from_db should fall back to Starting and
    // log the unexpected_readiness warning.
    let (supervisor, _dir) = make_supervisor();
    let db = supervisor.db_handle().clone();
    {
        let db = db.lock().unwrap();
        seed_service_state(&db, "bogus_unrecognized_readiness", None);
    }

    supervisor.hydrate_status_from_db().expect("hydrate");

    let snap = supervisor.status_snapshot_arc();
    let snap = snap.try_read().unwrap();
    assert_eq!(
        snap.readiness,
        ReadinessStateDto::Starting,
        "unexpected readiness should fall back to Starting"
    );
}

// =============================================================================
// generation_manager.rs — ensure_active_generation_for_existing_events via run_initial_scan
// =============================================================================

#[tokio::test]
async fn generation_manager_ensure_active_generation_for_existing_events_loads_from_db() {
    // Covers generation_manager.rs line 107: the
    // `*self.active_generation_id.lock().unwrap() = Some(gen_id.clone())`
    // assignment in ensure_active_generation_for_existing_events when the
    // DB has an active generation.
    //
    // We use a file-backed DB so run_initial_scan() can call db.reopen().
    // We seed a promoted+active generation and disable all discovery so
    // the scan returns zero stats but the existing-generation path is taken.
    let (supervisor, _dir) = make_supervisor_with_file_db();
    let db = supervisor.db_handle().clone();
    {
        let db = db.lock().unwrap();
        seed_active_generation(&db, "gen-existing-1");
    }

    let stats = supervisor.run_initial_scan().await.expect("initial scan");
    assert_eq!(stats.sources, 0, "empty discovery → zero sources");

    // The active generation should have been loaded from the DB.
    let snap = supervisor.read_status_snapshot().await;
    assert_eq!(
        snap.active_generation_id.as_deref(),
        Some("gen-existing-1"),
        "run_initial_scan should load existing active generation from DB"
    );
    assert_eq!(
        snap.readiness,
        ReadinessStateDto::ReadyExact,
        "existing promoted generation should be marked ready_exact"
    );
}

// =============================================================================
// generation_manager.rs — hydrate_from_db with ready_exact readiness
// =============================================================================

#[test]
fn generation_manager_hydrate_from_db_with_ready_exact_readiness() {
    // Covers generation_manager.rs line 226: the `Some("ready_exact")` arm
    // in the readiness match in hydrate_from_db.
    let (supervisor, _dir) = make_supervisor();
    let db = supervisor.db_handle().clone();
    {
        let db = db.lock().unwrap();
        seed_active_generation(&db, "gen-hydrate-exact");
        seed_service_state(&db, "ready_exact", Some("gen-hydrate-exact"));
    }

    supervisor.hydrate_status_from_db().expect("hydrate");

    let snap = supervisor.status_snapshot_arc();
    let snap = snap.try_read().unwrap();
    assert_eq!(snap.readiness, ReadinessStateDto::ReadyExact);
    assert_eq!(
        snap.active_generation_id.as_deref(),
        Some("gen-hydrate-exact")
    );
}

// =============================================================================
// generation_manager.rs — hydrate_from_db with rebuilding readiness
// =============================================================================

#[test]
fn generation_manager_hydrate_from_db_with_rebuilding_readiness() {
    // Covers generation_manager.rs line 228: the `Some("rebuilding")` arm
    // in the readiness match in hydrate_from_db.
    let (supervisor, _dir) = make_supervisor();
    let db = supervisor.db_handle().clone();
    {
        let db = db.lock().unwrap();
        seed_service_state(&db, "rebuilding", None);
    }

    supervisor.hydrate_status_from_db().expect("hydrate");

    let snap = supervisor.status_snapshot_arc();
    let snap = snap.try_read().unwrap();
    assert_eq!(snap.readiness, ReadinessStateDto::Rebuilding);
}

// =============================================================================
// generation_manager.rs — hydrate_from_db with starting readiness (explicit)
// =============================================================================

#[test]
fn generation_manager_hydrate_from_db_with_explicit_starting_readiness() {
    // Covers generation_manager.rs line 229: the `Some("starting") | None` arm
    // in the readiness match in hydrate_from_db (explicit "starting" value).
    let (supervisor, _dir) = make_supervisor();
    let db = supervisor.db_handle().clone();
    {
        let db = db.lock().unwrap();
        seed_service_state(&db, "starting", None);
    }

    supervisor.hydrate_status_from_db().expect("hydrate");

    let snap = supervisor.status_snapshot_arc();
    let snap = snap.try_read().unwrap();
    assert_eq!(snap.readiness, ReadinessStateDto::Starting);
}

// =============================================================================
// read_service.rs — map_open_error Internal fallback via directory path
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_map_open_error_internal_fallback_on_directory_path() {
    // Covers read_service.rs line 369: the `_` fallback to
    // ReadErrorKind::Internal in map_open_error.
    //
    // When `Database::open_readonly` is called on a directory path,
    // `Connection::open_with_flags` fails with an error that does NOT
    // match any of the covered ErrorCode variants (DatabaseBusy,
    // DatabaseLocked, CannotOpen, NotFound, NotADatabase,
    // PermissionDenied). On macOS, opening a directory yields an
    // `SqliteFailure` with a non-standard code (or a non-SqliteFailure
    // error), causing `sqlite_error_code` to return `None` or an
    // uncovered code. This hits the `_` arm → ReadErrorKind::Internal.
    use busytok_runtime::read_service::{ReadErrorKind, ReadQuery, ReadService};

    let tmp = tempfile::tempdir().unwrap();
    let dir_path = tmp.path().join("a_directory.sqlite");
    std::fs::create_dir(&dir_path).expect("create directory");

    let service = ReadService::new(dir_path, 1);

    let result = service
        .run(ReadQuery::new("test.directory_open", "test"), |_conn| {
            Ok::<_, anyhow::Error>(())
        })
        .await;

    let err = result.unwrap_err();
    // Opening a directory yields different SQLite errors per platform:
    //   - macOS: a non-standard error that falls through to Internal
    //   - Windows: CannotOpen, which maps to Unavailable
    // Both are valid outcomes from `map_open_error`; the test's purpose is
    // to exercise the function, not to assert a specific platform's behavior.
    assert!(
        err.kind() == ReadErrorKind::Internal || err.kind() == ReadErrorKind::Unavailable,
        "directory path should map to Internal or Unavailable, got {:?}: {}",
        err.kind(),
        err.message()
    );
}

// =============================================================================
// read_service.rs — map_open_error via non-existent path (CannotOpen arm)
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_service_map_open_error_on_nonexistent_path() {
    // Exercises map_open_error with a non-existent path. SQLite returns
    // CannotOpen (or NotFound), which maps to Unavailable.
    // This is already partially covered, but we add it here to ensure
    // the map_open_error function is exercised from this test binary too.
    use std::path::PathBuf;

    use busytok_runtime::read_service::{ReadErrorKind, ReadQuery, ReadService};

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("nonexistent.sqlite");

    let service = ReadService::new(path, 1);

    let result = service
        .run(ReadQuery::new("test.nonexistent_open", "test"), |conn| {
            let val: i64 = conn.query_row("SELECT 1", [], |r| r.get(0))?;
            Ok::<_, anyhow::Error>(val)
        })
        .await;

    let err = result.unwrap_err();
    // Non-existent path → CannotOpen or NotFound → Unavailable.
    assert!(
        err.kind() == ReadErrorKind::Unavailable,
        "non-existent path should map to Unavailable, got {:?}: {}",
        err.kind(),
        err.message()
    );
}
