/// Schema version tracking for the Busytok store.
///
/// The baseline schema is applied as a single migration when a new database
/// is created. Future schema changes will increment from v1.
pub const SCHEMA_VERSION: u32 = 1;

/// SQL to create the schema version tracking table.
pub const CREATE_SCHEMA_VERSION_TABLE: &str = "\
    CREATE TABLE IF NOT EXISTS _schema_version (\
        version INTEGER PRIMARY KEY, \
        applied_at_ms INTEGER NOT NULL\
    );\
";

/// Baseline schema SQL — creates all tables, indexes, and constraints for v1.
pub const BASELINE_SQL: &str = include_str!("../migrations/0001_baseline.sql");

/// All migrations in order. Currently only the v1 baseline.
pub fn migrations() -> Vec<(u32, &'static str)> {
    vec![(1, BASELINE_SQL)]
}
