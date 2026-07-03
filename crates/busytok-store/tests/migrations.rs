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
use std::collections::HashMap;

use busytok_store::{schema, Database, NewPromptEntryRow, PromptListQuery, PromptSortRow};

/// Query PRAGMA table_info and return a map from column name to (type, notnull, pk, dflt_value).
fn table_info_map(
    db: &Database,
    table: &str,
) -> HashMap<String, (String, bool, bool, Option<String>)> {
    let sql = format!("PRAGMA table_info({})", table);
    let mut stmt = db.conn().prepare(&sql).unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,               // name
                row.get::<_, String>(2)?,               // type
                row.get::<_, i32>(3).unwrap_or(0) != 0, // notnull
                row.get::<_, i32>(5).unwrap_or(0) != 0, // pk
                row.get::<_, Option<String>>(4)?,       // dflt_value
            ))
        })
        .unwrap();
    let mut map = HashMap::new();
    for row in rows {
        let (name, typ, notnull, pk, dflt) = row.unwrap();
        map.insert(name, (typ, notnull, pk, dflt));
    }
    map
}

// ── Baseline schema sanity ──────────────────────────────────────────────────

#[test]
fn baseline_plus_cache_metrics_migrations() {
    let migs = busytok_store::schema::migrations();
    assert_eq!(migs.len(), 6);
    assert_eq!(busytok_store::schema::SCHEMA_VERSION, 6);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(busytok_store::schema::CREATE_SCHEMA_VERSION_TABLE)
        .unwrap();
    for (_, sql) in &migs {
        conn.execute_batch(sql).unwrap();
    }
    // v2 columns exist on usage_events.
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(usage_events)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(cols.contains(&"provider_payload_shape".to_string()));
    assert!(cols.contains(&"prompt_input_total_tokens".to_string()));
    assert!(
        cols.contains(&"prompt_input_non_cached_tokens".to_string()),
        "usage_events must have prompt_input_non_cached_tokens column"
    );
    // v6 provider catalog tables exist.
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    for table in ["providers", "models", "model_tags"] {
        assert!(
            tables.contains(&table.to_string()),
            "missing v6 table {table}"
        );
    }
}

#[test]
fn baseline_creates_core_tables() {
    let db = Database::open_in_memory().unwrap();
    let tables = db.table_names().unwrap();

    for name in [
        "log_sources",
        "log_files",
        "usage_events",
        "daily_usage",
        "model_usage",
        "sessions",
        "projects",
        "tool_events",
        "diagnostic_events",
        "realtime_summary",
        "codex_token_snapshots",
        "model_summary",
    ] {
        assert!(tables.contains(&name.to_string()), "missing table {name}");
    }
}

#[test]
fn baseline_creates_core_indexes() {
    let db = Database::open_in_memory().unwrap();
    let indexes = db.index_names().unwrap();

    for index in [
        "idx_usage_events_time",
        "idx_usage_events_agent_time",
        "idx_usage_events_project_hash_time",
        "idx_usage_events_model_time",
        "idx_usage_events_session",
        "idx_usage_events_source_request",
        "idx_usage_events_message",
        "idx_tool_events_session",
        "idx_tool_events_timestamp",
        "idx_diagnostic_events_code",
        "idx_diagnostic_events_severity",
        "idx_diagnostic_events_happened_at",
    ] {
        assert!(
            indexes.contains(&index.to_string()),
            "missing index {index}"
        );
    }
}

#[test]
fn baseline_has_no_dead_tables() {
    let db = Database::open_in_memory().unwrap();
    let tables = db.table_names().unwrap();

    for dead in ["source_files", "scan_runs", "diagnostic_summary_by_source"] {
        assert!(
            !tables.contains(&dead.to_string()),
            "dead table {dead} must not exist in baseline"
        );
    }
}

#[test]
fn source_health_summary_has_no_severity_column() {
    let db = Database::open_in_memory().unwrap();
    let cols = table_info_map(&db, "source_health_summary");
    assert!(
        !cols.contains_key("severity"),
        "source_health_summary must not have severity column"
    );
}

// ── Prompt palette ──────────────────────────────────────────────────────────

#[test]
fn prompt_palette_tables_and_indexes_exist() {
    let db = Database::open_in_memory().unwrap();
    let tables = db.table_names().unwrap();
    for table in ["prompt_entries", "prompt_entry_tags", "prompt_entry_uses"] {
        assert!(tables.contains(&table.to_string()), "missing table {table}");
    }

    let prompt_columns = table_info_map(&db, "prompt_entries");
    assert!(
        prompt_columns.contains_key("content_normalized"),
        "prompt_entries must persist normalized content for SQL-side fallback search"
    );

    let indexes = db.index_names().unwrap();
    for index in [
        "idx_prompt_alias_unique",
        "idx_prompt_entries_updated_at",
        "idx_prompt_entries_last_used_at",
        "idx_prompt_entries_pinned_last_used",
        "idx_prompt_entry_tags_unique",
        "idx_prompt_entry_tags_lookup",
        "idx_prompt_entry_uses_entry_time",
    ] {
        assert!(
            indexes.contains(&index.to_string()),
            "missing index {index}"
        );
    }
}

#[test]
fn prompt_palette_crud_works_on_fresh_baseline() {
    let db = Database::open_in_memory().unwrap();

    let created = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "Test prompt".to_string(),
            tags: vec!["Smoke".to_string()],
            alias: Some(";;test".to_string()),
        })
        .unwrap();
    assert_eq!(created.alias.as_deref(), Some(";;test"));

    let list = db
        .list_prompt_entries(PromptListQuery {
            query: None,
            tag: None,
            sort: PromptSortRow::Smart,
            limit: 100,
        })
        .unwrap();
    assert_eq!(list.total_count, 1);

    assert!(db.delete_prompt_entry(&created.id).unwrap());

    let list_after = db
        .list_prompt_entries(PromptListQuery {
            query: None,
            tag: None,
            sort: PromptSortRow::Smart,
            limit: 100,
        })
        .unwrap();
    assert_eq!(list_after.total_count, 0);
}

// ── Realtime audit ──────────────────────────────────────────────────────────

#[test]
fn realtime_audit_tables_exist() {
    let db = Database::open_in_memory().unwrap();
    let tables = db.table_names().unwrap();

    for name in [
        "audit_generations",
        "source_file_checkpoints",
        "generation_file_observations",
        "tail_replay_queue",
        "event_sequence_state",
        "usage_buckets_2s",
        "usage_buckets_hour",
        "usage_buckets_day",
        "usage_by_project_day",
        "usage_by_model_day",
        "usage_by_session_day",
        "usage_by_client_day",
        "service_state",
        "outbox_log",
    ] {
        assert!(tables.contains(&name.to_string()), "missing table {name}");
    }

    // Verify legacy tables still coexist.
    for name in [
        "log_sources",
        "log_files",
        "usage_events",
        "daily_usage",
        "sessions",
        "projects",
        "tool_events",
        "diagnostic_events",
        "model_usage",
        "model_summary",
        "realtime_summary",
        "codex_token_snapshots",
    ] {
        assert!(
            tables.contains(&name.to_string()),
            "legacy table {name} must still exist"
        );
    }

    let indexes = db.index_names().unwrap();
    for index in [
        "idx_audit_generations_active",
        "idx_audit_generations_state",
        "idx_audit_generations_time",
        "idx_source_file_checkpoints_source",
        "idx_generation_file_obs_gen",
        "idx_generation_file_obs_file",
        "idx_tail_replay_queue_status",
        "idx_tail_replay_queue_file",
        "idx_event_sequence_state_singleton",
        "idx_usage_buckets_2s_range",
        "idx_usage_buckets_hour_range",
        "idx_usage_buckets_day_range",
        "idx_usage_by_project_day_range",
        "idx_usage_by_model_day_range",
        "idx_usage_by_session_day_range",
        "idx_usage_by_client_day_range",
        "idx_source_health_summary_order",
        "idx_usage_events_generation_dedupe",
        "idx_diagnostic_events_retention",
        "idx_outbox_log_event_seq",
        "idx_outbox_log_id",
    ] {
        assert!(
            indexes.contains(&index.to_string()),
            "missing index {index}"
        );
    }
}

#[test]
fn audit_generations_columns_match_metadata_schema() {
    let db = Database::open_in_memory().unwrap();
    let cols = table_info_map(&db, "audit_generations");

    assert_eq!(
        cols.get("generation_id").map(|c| (&c.0[..], c.2)),
        Some(("TEXT", true)),
        "generation_id must be TEXT PRIMARY KEY"
    );
    assert_eq!(
        cols.get("state").map(|c| (&c.0[..], c.1)),
        Some(("TEXT", true)),
        "state must be TEXT NOT NULL"
    );
    assert_eq!(
        cols.get("started_at_ms").map(|c| (&c.0[..], c.1)),
        Some(("INTEGER", true)),
        "started_at_ms must be INTEGER NOT NULL"
    );
    assert!(
        cols.contains_key("promoted_at_ms"),
        "missing promoted_at_ms column"
    );
    let is_active = cols.get("is_active").expect("missing is_active column");
    assert_eq!(&is_active.0, "INTEGER", "is_active must be INTEGER");
    assert!(is_active.1, "is_active must be NOT NULL");
    assert_eq!(
        is_active.3.as_deref(),
        Some("0"),
        "is_active must default to 0"
    );
    assert_eq!(
        cols.get("created_at_ms").map(|c| (&c.0[..], c.1)),
        Some(("INTEGER", true)),
        "created_at_ms must be INTEGER NOT NULL"
    );
    assert_eq!(
        cols.get("updated_at_ms").map(|c| (&c.0[..], c.1)),
        Some(("INTEGER", true)),
        "updated_at_ms must be INTEGER NOT NULL"
    );

    for unwanted in &[
        "id",
        "agent",
        "source_file_id",
        "session_id",
        "project_id",
        "model",
        "input_tokens",
        "output_tokens",
        "cache_creation_tokens",
        "cache_read_tokens",
        "reasoning_tokens",
        "total_tokens",
        "cost_usd",
        "cost_status",
        "timestamp_ms",
    ] {
        assert!(
            !cols.contains_key(*unwanted),
            "audit_generations should not have column '{unwanted}'"
        );
    }
}

#[test]
fn event_sequence_state_columns_match_schema() {
    let db = Database::open_in_memory().unwrap();
    let cols = table_info_map(&db, "event_sequence_state");

    assert_eq!(
        cols.get("id").map(|c| (&c.0[..], c.2)),
        Some(("INTEGER", true)),
        "id must be INTEGER PRIMARY KEY"
    );

    let seq = cols
        .get("latest_event_seq")
        .expect("missing latest_event_seq column");
    assert_eq!(&seq.0, "INTEGER");
    assert!(seq.1, "latest_event_seq must be NOT NULL");
    assert_eq!(
        seq.3.as_deref(),
        Some("0"),
        "latest_event_seq must default to 0"
    );

    assert!(
        cols.contains_key("latest_event_timestamp_ms"),
        "missing latest_event_timestamp_ms column"
    );

    assert_eq!(
        cols.get("updated_at_ms").map(|c| (&c.0[..], c.1)),
        Some(("INTEGER", true)),
        "updated_at_ms must be INTEGER NOT NULL"
    );
}

#[test]
fn service_state_is_single_row_table() {
    let db = Database::open_in_memory().unwrap();
    let cols = table_info_map(&db, "service_state");

    assert_eq!(
        cols.get("id").map(|c| (&c.0[..], c.2)),
        Some(("INTEGER", true)),
        "id must be INTEGER PRIMARY KEY"
    );

    // No key column (old multi-row layout).
    assert!(
        !cols.contains_key("key"),
        "service_state should not have a 'key' column (single-row layout)"
    );
    assert!(
        !cols.contains_key("value_json"),
        "service_state should not have a 'value_json' column (single-row layout)"
    );

    for col in &["writer_queue_depth", "aggregate_lag_ms", "updated_at_ms"] {
        let info = cols
            .get(*col)
            .unwrap_or_else(|| panic!("missing {col} column"));
        assert_eq!(&info.0, "INTEGER", "{col} must be INTEGER");
        assert!(info.1, "{col} must be NOT NULL");
    }

    assert!(cols.contains_key("readiness"), "missing readiness column");
    assert!(
        cols.contains_key("active_generation_id"),
        "missing active_generation_id column"
    );
    assert!(
        cols.contains_key("last_exact_rebuild_at_ms"),
        "missing last_exact_rebuild_at_ms column"
    );
}

#[test]
fn read_plane_materialized_tables_are_generation_scoped() {
    let db = busytok_store::Database::open_in_memory().unwrap();

    // All read-plane materialized tables must be generation-scoped so that
    // active-generation reads never see events from sealed/old generations.
    for table in [
        "usage_buckets_2s",
        "usage_buckets_hour",
        "usage_buckets_day",
        "usage_by_project_day",
        "usage_by_model_day",
        "usage_by_session_day",
        "usage_by_client_day",
        "source_health_summary",
        "daily_usage",
    ] {
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = 'generation_id'",
                rusqlite::params![table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "{table} must include generation_id");
    }

    for table in [
        "usage_by_project_day",
        "usage_by_model_day",
        "usage_by_session_day",
        "usage_by_client_day",
    ] {
        let cols = table_info_map(&db, table);
        let cost_status = cols
            .get("cost_status")
            .unwrap_or_else(|| panic!("{table} must include cost_status"));
        assert_eq!(&cost_status.0, "TEXT", "{table}.cost_status must be TEXT");
        assert!(
            cost_status.1,
            "{table}.cost_status must be NOT NULL for reliable partial-cost reads"
        );
        assert_eq!(
            cost_status.3.as_deref(),
            Some("'unknown'"),
            "{table}.cost_status must default to unknown"
        );
    }
}

#[test]
fn service_state_contains_read_model_metadata() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    for column in [
        "read_model_watermark_ms",
        "read_model_status",
        "last_successful_read_model_rebuild_at_ms",
        "consistency_check_status",
    ] {
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('service_state') WHERE name = ?1",
                rusqlite::params![column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "service_state missing {column}");
    }
}

// ── Logical subagent schema (v3) ────────────────────────────────────────────

#[test]
fn subagent_migration_creates_tables() {
    let db = Database::open_in_memory().unwrap();
    let tables = db.table_names().unwrap();
    for name in [
        "subagent_logical_subagents",
        "subagent_memory",
        "subagent_tasks",
        "subagent_harness_bindings",
        "subagent_usage_records",
        "subagent_resource_events",
    ] {
        assert!(tables.contains(&name.to_string()), "missing table {name}");
    }
}

#[test]
fn migrations_registered_in_order() {
    assert_eq!(
        schema::migrations().len(),
        6,
        "expected baseline + cache-metrics + subagent + subagent-task-fields + subagent-task-error-kind + provider-catalog migrations"
    );
    assert_eq!(schema::migrations()[0].0, 1);
    assert_eq!(schema::migrations()[1].0, 2);
    assert_eq!(schema::migrations()[2].0, 3);
    assert_eq!(schema::migrations()[3].0, 4);
    assert_eq!(schema::migrations()[4].0, 5);
    assert_eq!(schema::migrations()[5].0, 6);
    assert_eq!(schema::SCHEMA_VERSION, 6);
}

#[test]
fn schema_version_is_six() {
    assert_eq!(schema::SCHEMA_VERSION, 6);
    let max_version = schema::migrations().iter().map(|(v, _)| *v).max().unwrap();
    assert_eq!(max_version, schema::SCHEMA_VERSION);
}
