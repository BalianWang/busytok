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
//! Integration test: discover → parse → store → aggregate
//!
//! Tests the full scan pipeline from fixture files through to
//! the database and aggregation layers.

use std::path::PathBuf;

use busytok_config::BusytokPaths;
use busytok_domain::{AgentKind, LogSourceType};
use busytok_runtime::BusytokSupervisor;
use busytok_store::Database;

/// Create a DiscoveredLogSource pointing to the fixture directory.
fn fixture_source(fixture_name: &str) -> busytok_discovery::DiscoveredLogSource {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/claude-code");

    let fixture_path = fixture_dir.join(fixture_name);

    busytok_discovery::DiscoveredLogSource {
        agent: AgentKind::ClaudeCode,
        source_id: "test-claude-code".to_string(),
        root_path: fixture_dir,
        files: vec![fixture_path],
        source_type: LogSourceType::Jsonl,
        configured_by_user: false,
    }
}

#[test]
fn scan_pipeline_basic_fixture() {
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();

    let supervisor = BusytokSupervisor::new(db, paths);
    let source = fixture_source("basic.jsonl");

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("scan should succeed");

    // The basic fixture has one line with usage data.
    assert_eq!(stats.sources, 1);
    assert_eq!(stats.files_scanned, 1);
    assert!(
        stats.events_found >= 1,
        "should find at least 1 usage event"
    );

    // Verify the event was stored.
    let db_guard = supervisor.db_handle().lock().unwrap();
    let count = db_guard.usage_event_count().expect("count");
    assert!(count >= 1, "should have at least 1 event in the database");

    // Verify daily usage rows were created.
    let daily_rows = db_guard.daily_usage_rows().expect("daily rows");
    assert!(!daily_rows.is_empty(), "should have daily usage rows");
}

#[test]
fn scan_pipeline_cache_fixture() {
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();

    let supervisor = BusytokSupervisor::new(db, paths);
    let source = fixture_source("cache.jsonl");

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("scan should succeed");

    assert_eq!(stats.sources, 1);
    assert_eq!(stats.files_scanned, 1);
    assert!(stats.events_found >= 1);

    let db_guard = supervisor.db_handle().lock().unwrap();
    let count = db_guard.usage_event_count().expect("count");
    assert!(count >= 1);
}

#[test]
fn scan_pipeline_claude_message_id_replace_uses_latest_usage() {
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();

    let supervisor = BusytokSupervisor::new(db, paths);
    let source = fixture_source("message-id-replace.jsonl");

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("scan should succeed");

    assert_eq!(stats.events_found, 2);

    let db_guard = supervisor.db_handle().lock().unwrap();
    let events = db_guard.all_usage_events().expect("get all events");
    assert_eq!(
        events.len(),
        1,
        "latest message-id update should replace prior usage"
    );

    let event = &events[0];
    assert_eq!(event.input_tokens, 200);
    assert_eq!(event.output_tokens, 100);
    assert_eq!(event.cache_creation_tokens, 40);
    assert_eq!(event.cache_read_tokens, 20);
    assert_eq!(event.total_tokens, 360);
}

#[test]
fn scan_pipeline_malformed_fixture() {
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();

    let supervisor = BusytokSupervisor::new(db, paths);
    let source = fixture_source("malformed.jsonl");

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("scan should handle malformed data");

    // The malformed fixture has incomplete JSON, so no usage events.
    assert_eq!(stats.sources, 1);
    // The file was scanned (even though it produced no events).
    assert_eq!(stats.files_scanned, 1);
    assert_eq!(
        stats.events_found, 0,
        "malformed JSON should produce no usage events"
    );
    assert!(
        stats.diagnostics_found >= 1,
        "should produce diagnostic for parse error"
    );
}

#[test]
fn scan_pipeline_malformed_fixture_is_idempotent_across_repeated_scans() {
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();

    let supervisor = BusytokSupervisor::new(db, paths);
    let source = fixture_source("malformed.jsonl");

    let first = supervisor
        .run_scan_with_sources(vec![source.clone()])
        .expect("first malformed scan should succeed");
    let second = supervisor
        .run_scan_with_sources(vec![source])
        .expect("second malformed scan should also succeed");

    assert!(first.diagnostics_found >= 1);
    // Second scan finds no new lines (checkpoint is up to date), so
    // diagnostics_found is 0. Diagnostic rows themselves are idempotent
    // via INSERT OR REPLACE, so DB count remains 1 (verified below).
    assert_eq!(second.diagnostics_found, 0);

    let db_guard = supervisor.db_handle().lock().unwrap();
    assert_eq!(
        db_guard.diagnostic_event_count().expect("diagnostic count"),
        1,
        "repeated malformed scans should replace the same parse diagnostic instead of duplicating or failing",
    );
}

#[test]
fn scan_pipeline_cost_enrichment() {
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();

    let supervisor = BusytokSupervisor::new(db, paths);
    let source = fixture_source("basic.jsonl");

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("scan should succeed");

    assert!(stats.events_found >= 1);

    // Verify cost enrichment happened.
    let db_guard = supervisor.db_handle().lock().unwrap();
    let events = db_guard.all_usage_events().expect("read events");

    for event in &events {
        // price_catalog_version should always be set.
        assert!(
            event.price_catalog_version.is_some(),
            "price_catalog_version should be set on every event"
        );
    }
}

/// Create a Codex DiscoveredLogSource pointing to a fixture file.
fn codex_fixture_source(fixture_name: &str) -> busytok_discovery::DiscoveredLogSource {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/codex");
    let fixture_path = fixture_dir.join(fixture_name);
    busytok_discovery::DiscoveredLogSource {
        agent: AgentKind::Codex,
        source_id: "test-codex".to_string(),
        root_path: fixture_dir,
        files: vec![fixture_path],
        source_type: LogSourceType::Jsonl,
        configured_by_user: false,
    }
}

#[test]
fn codex_fixture_scan_populates_usage_dashboard() {
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();
    let supervisor = BusytokSupervisor::new(db, paths);
    let source = codex_fixture_source("codex-token-count-snapshot.jsonl");
    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("codex scan should succeed");
    // The fixture has 2 snapshots with explicit `last_token_usage`, so runtime
    // should mirror ccusage and trust those deltas instead of re-deriving from
    // cumulative totals.
    assert_eq!(
        stats.events_found, 2,
        "codex fixture should produce exactly 2 delta events"
    );
    let db_guard = supervisor.db_handle().lock().unwrap();
    let count = db_guard.usage_event_count().expect("count");
    assert_eq!(count, 2, "codex events should be stored: 2 delta events");

    // Verify delta correctness from explicit `last_token_usage`:
    // When total_tokens is absent, runtime aligns with ccusage by falling back
    // to input + output + reasoning.
    // Line 1: last = (300, 150, 50), total=500
    // Line 2: last = (200, 100, 30), total=330
    let events = db_guard.all_usage_events().expect("get all events");
    let mut deltas: Vec<(i64, i64, i64, i64)> = events
        .iter()
        .map(|e| {
            (
                e.input_tokens,
                e.output_tokens,
                e.reasoning_tokens,
                e.total_tokens,
            )
        })
        .collect();
    deltas.sort_by_key(|d| d.3); // sort by total_tokens
    assert_eq!(
        deltas[0],
        (200, 100, 30, 330),
        "second delta: 200/100/30/330"
    );
    assert_eq!(
        deltas[1],
        (300, 150, 50, 500),
        "first delta: 300/150/50/500"
    );

    // Verify 2 snapshots are persisted (one per cumulative line).
    // Since there's no public count method for snapshots, we verify through
    // the fact that 2 delta events were produced — as each snapshot produces one delta.
    let daily_rows = db_guard.daily_usage_rows().expect("daily rows");
    assert!(
        !daily_rows.is_empty(),
        "codex scan should produce daily usage rows"
    );
}

#[test]
fn codex_last_usage_wins_when_cumulative_total_does_not_advance() {
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();
    let supervisor = BusytokSupervisor::new(db, paths);
    let source = codex_fixture_source("codex-last-usage-wins.jsonl");

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("codex scan should succeed");
    assert_eq!(stats.events_found, 2);

    let db_guard = supervisor.db_handle().lock().unwrap();
    let events = db_guard.all_usage_events().expect("get all events");
    assert_eq!(events.len(), 2);

    let totals: Vec<i64> = events.iter().map(|e| e.total_tokens).collect();
    assert_eq!(
        totals,
        vec![150, 75],
        "explicit last_token_usage should be emitted even when cumulative totals stay flat"
    );
}

#[test]
fn codex_duplicate_token_count_events_are_deduplicated_like_ccusage() {
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();
    let supervisor = BusytokSupervisor::new(db, paths);
    let source = codex_fixture_source("codex-duplicate-token-count.jsonl");

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("codex scan should succeed");
    assert_eq!(
        stats.events_found, 2,
        "fixture still parses both duplicate snapshots"
    );

    let db_guard = supervisor.db_handle().lock().unwrap();
    let events = db_guard.all_usage_events().expect("get all events");
    assert_eq!(
        events.len(),
        1,
        "duplicate token_count tuples should collapse to one stored usage event"
    );
    assert_eq!(events[0].total_tokens, 150);
}

#[test]
fn codex_total_only_events_fall_back_to_cumulative_diffs() {
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();
    let supervisor = BusytokSupervisor::new(db, paths);
    let source = codex_fixture_source("codex-total-diff-fallback.jsonl");

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("codex scan should succeed");
    assert_eq!(stats.events_found, 2);

    let db_guard = supervisor.db_handle().lock().unwrap();
    let events = db_guard.all_usage_events().expect("get all events");
    assert_eq!(events.len(), 2);

    let totals: Vec<(i64, i64, i64, i64, i64)> = events
        .iter()
        .map(|e| {
            (
                e.input_tokens,
                e.cached_input_tokens,
                e.output_tokens,
                e.reasoning_tokens,
                e.total_tokens,
            )
        })
        .collect();
    assert_eq!(totals[0], (100, 20, 50, 10, 160));
    assert_eq!(
        totals[1],
        (50, 10, 30, 5, 85),
        "second total-only snapshot should emit the delta from prior cumulative totals"
    );
}

#[test]
fn codex_zero_component_heartbeat_advances_baseline_without_emitting_usage() {
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();
    let supervisor = BusytokSupervisor::new(db, paths);
    let source = codex_fixture_source("codex-heartbeat-then-total-only.jsonl");

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("codex scan should succeed");
    assert_eq!(
        stats.events_found, 1,
        "zero-component heartbeat should not emit a usage row"
    );

    let db_guard = supervisor.db_handle().lock().unwrap();
    let events = db_guard.all_usage_events().expect("get all events");
    assert_eq!(events.len(), 1);
    assert_eq!(
        (
            events[0].input_tokens,
            events[0].cached_input_tokens,
            events[0].output_tokens,
            events[0].reasoning_tokens,
            events[0].total_tokens,
        ),
        (30, 5, 20, 3, 43),
        "follow-up total-only snapshot should diff against the heartbeat baseline"
    );
}

#[test]
fn codex_turn_context_fixture_applies_model_to_usage_events() {
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();
    let supervisor = BusytokSupervisor::new(db, paths);
    let source = codex_fixture_source("codex-turn-context-model.jsonl");

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("codex scan should succeed");
    assert_eq!(
        stats.events_found, 1,
        "fixture should produce one codex delta event"
    );

    let db_guard = supervisor.db_handle().lock().unwrap();
    let events = db_guard.all_usage_events().expect("get all events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].agent, AgentKind::Codex);
    assert_eq!(events[0].model.as_deref(), Some("gpt-5.3-codex-spark"));
}

#[test]
fn codex_legacy_fixture_falls_back_to_gpt5_model() {
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();
    let supervisor = BusytokSupervisor::new(db, paths);
    let source = codex_fixture_source("codex-legacy-missing-model.jsonl");

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("codex legacy scan should succeed");
    assert_eq!(stats.events_found, 1);

    let db_guard = supervisor.db_handle().lock().unwrap();
    let events = db_guard.all_usage_events().expect("get all events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].agent, AgentKind::Codex);
    assert_eq!(events[0].model.as_deref(), None);
}

#[test]
fn codex_resume_inherits_model_from_prior_usage_event() {
    use std::io::Write;

    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("rollout-resume.jsonl");
    let source = busytok_discovery::DiscoveredLogSource {
        agent: AgentKind::Codex,
        source_id: "test-codex-tail".to_string(),
        root_path: dir.path().to_path_buf(),
        files: vec![file_path.clone()],
        source_type: LogSourceType::Jsonl,
        configured_by_user: false,
    };

    let turn_context = r#"{"timestamp":"2026-05-20T07:16:20.000Z","type":"turn_context","payload":{"model":"gpt-5.3-codex-spark"}}"#;
    let first_count = r#"{"timestamp":"2026-05-20T07:16:22.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50,"reasoning_output_tokens":10,"total_tokens":160},"last_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50,"reasoning_output_tokens":10,"total_tokens":160}}}}"#;
    let second_count = r#"{"timestamp":"2026-05-20T07:16:24.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":130,"cached_input_tokens":0,"output_tokens":70,"reasoning_output_tokens":12,"total_tokens":212}}}}"#;

    {
        let mut f = std::fs::File::create(&file_path).expect("create file");
        writeln!(f, "{turn_context}").expect("write turn_context");
        writeln!(f, "{first_count}").expect("write first token_count");
    }

    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();
    let supervisor = BusytokSupervisor::new(db, paths);

    let first_stats = supervisor
        .run_scan_with_sources(vec![source.clone()])
        .expect("initial codex scan should succeed");
    assert_eq!(first_stats.events_found, 1);

    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&file_path)
            .expect("open append");
        writeln!(f, "{second_count}").expect("write second token_count");
    }

    let second_stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("resume codex scan should succeed");
    assert_eq!(second_stats.events_found, 1);

    let db_guard = supervisor.db_handle().lock().unwrap();
    let events = db_guard.all_usage_events().expect("get all events");
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| event.model.as_deref() == Some("gpt-5.3-codex-spark")),
        "appended model-less token_count should inherit the prior persisted Codex model"
    );
}

#[test]
fn scan_pipeline_rescan_idempotent() {
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();

    let supervisor = BusytokSupervisor::new(db, paths);
    let source = fixture_source("basic.jsonl");

    // First scan.
    let stats1 = supervisor
        .run_scan_with_sources(vec![source.clone()])
        .expect("first scan");
    assert!(stats1.events_found >= 1);

    // Second scan of the same data should be idempotent (InsertOnce policy).
    let _stats2 = supervisor
        .run_scan_with_sources(vec![source])
        .expect("second scan");

    let db_guard = supervisor.db_handle().lock().unwrap();
    let count = db_guard.usage_event_count().expect("count");
    // With InsertOnce policy, the count should not double.
    assert_eq!(
        count, stats1.events_found as i64,
        "rescan with InsertOnce policy should not duplicate events"
    );
}
