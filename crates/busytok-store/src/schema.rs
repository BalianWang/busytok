/// Schema version tracking for the Busytok store.
///
/// The baseline schema is applied as a single migration when a new database
/// is created. Future schema changes will increment from v1.
pub const SCHEMA_VERSION: u32 = 3;

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

/// All migrations in order, from the v1 baseline through the latest version.
pub fn migrations() -> Vec<(u32, &'static str)> {
    vec![(1, BASELINE_SQL), (2, CACHE_METRICS_SQL), (3, SUBAGENT_SQL)]
}
