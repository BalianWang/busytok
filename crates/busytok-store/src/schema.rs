/// Schema version tracking for the Busytok store.
///
/// The baseline schema is applied as a single migration when a new database
/// is created. Future schema changes will increment from v1.
pub const SCHEMA_VERSION: u32 = 6;

/// SQL to create the schema version tracking table.
pub const CREATE_SCHEMA_VERSION_TABLE: &str = "\
    CREATE TABLE IF NOT EXISTS _schema_version (\
        version INTEGER PRIMARY KEY, \
        applied_at_ms INTEGER NOT NULL\
    );\
";

/// Baseline schema SQL — creates all tables, indexes, and constraints for v1.
pub const BASELINE_SQL: &str = include_str!("../migrations/0001_baseline.sql");

/// v2 cache-metrics migration SQL — adds unified cache-metric columns to
/// `usage_events`.
const CACHE_METRICS_SQL: &str = include_str!("../migrations/0002_cache_metrics.sql");

/// v3 logical-subagent migration SQL — creates the `subagent_*` runtime tables.
pub const SUBAGENT_SQL: &str = include_str!("../migrations/0003_subagent.sql");

/// v4 subagent-task-fields migration SQL — adds `timeout_seconds` +
/// `model_override` columns to `subagent_tasks` so the row is the single
/// source of truth for execution params (Task 7 Round 3 Finding 3 fix:
/// incremental ALTER TABLE, not modifying `0003`).
const SUBAGENT_TASK_FIELDS_SQL: &str = include_str!("../migrations/0004_subagent_task_fields.sql");

/// v5 subagent-task-error-kind migration SQL — adds the `error_kind` column
/// to `subagent_tasks` so the classified failure kind (Task 5) is persisted
/// on the task row and available to downstream consumers without re-parsing
/// the error string.
const SUBAGENT_TASK_ERROR_KIND_SQL: &str =
    include_str!("../migrations/0005_subagent_task_error_kind.sql");

/// v6 provider-catalog migration SQL — creates the `providers`, `models`, and
/// `model_tags` tables that replace settings.toml provider persistence +
/// keychain credential storage.
const PROVIDER_CATALOG_SQL: &str = include_str!("../migrations/0006_provider_catalog.sql");

/// All migrations in order, from the v1 baseline through the latest version.
pub fn migrations() -> Vec<(u32, &'static str)> {
    vec![
        (1, BASELINE_SQL),
        (2, CACHE_METRICS_SQL),
        (3, SUBAGENT_SQL),
        (4, SUBAGENT_TASK_FIELDS_SQL),
        (5, SUBAGENT_TASK_ERROR_KIND_SQL),
        (6, PROVIDER_CATALOG_SQL),
    ]
}
