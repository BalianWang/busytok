use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use busytok_domain::now_ms;
use rusqlite::{params, Connection, OptionalExtension};
use tracing::{debug, info};

use crate::repository::{
    CodexTokenSnapshotRow, DailyUsageRow, DiagnosticEventRow, LogFileRow, LogSourceRow,
    ModelSummaryRow, ModelUsageRow, ProjectRow, RollupRows, SessionRow, StoreHealthInfo,
    StoreWriteBatch, SubagentHarnessBindingRow, SubagentLogicalSubagentRow, SubagentMemoryRow,
    SubagentResourceEventRow, SubagentTaskRow, SubagentUsageRecordRow,
};
use crate::schema;
use crate::subagent_queries;

/// SQLite database handle for the Busytok store.
///
/// Owns a single `rusqlite::Connection`. All operations are synchronous.
/// The connection is opened with WAL mode for safe concurrent reads.
pub struct Database {
    conn: Connection,
}

/// Token fields of an event before replacement, used for delta rollup computation.
#[derive(Debug, Clone)]
pub struct OldEventTokens {
    pub event_id: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub reasoning_tokens: i64,
    pub thoughts_tokens: i64,
    pub tool_tokens: i64,
    pub cost_usd: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
}

impl OldEventTokens {
    /// Compute the delta event for rollup adjustment when an event is replaced.
    ///
    /// Returns a new `NormalizedUsageEvent` whose token fields are `new - old`,
    /// which can be passed directly to `build_scan_mutations` for incremental
    /// rollup correction.
    pub fn compute_delta(
        &self,
        new: &busytok_domain::NormalizedUsageEvent,
    ) -> busytok_domain::NormalizedUsageEvent {
        let mut delta = new.clone();
        delta.input_tokens = new.input_tokens - self.input_tokens;
        delta.output_tokens = new.output_tokens - self.output_tokens;
        delta.total_tokens = new.total_tokens - self.total_tokens;
        delta.cached_input_tokens = new.cached_input_tokens - self.cached_input_tokens;
        delta.cache_creation_tokens = new.cache_creation_tokens - self.cache_creation_tokens;
        delta.cache_read_tokens = new.cache_read_tokens - self.cache_read_tokens;
        delta.reasoning_tokens = new.reasoning_tokens - self.reasoning_tokens;
        delta.thoughts_tokens = new.thoughts_tokens - self.thoughts_tokens;
        delta.tool_tokens = new.tool_tokens - self.tool_tokens;
        delta.cost_usd = match (new.cost_usd, self.cost_usd) {
            (Some(n), Some(o)) => Some(n - o),
            (Some(n), None) => Some(n),
            (None, _) => None,
        };
        delta.estimated_cost_usd = match (new.estimated_cost_usd, self.estimated_cost_usd) {
            (Some(n), Some(o)) => Some(n - o),
            (Some(n), None) => Some(n),
            (None, _) => None,
        };
        delta
    }
}

/// Result of an atomic batch ingest, reporting which events were newly created.
pub struct IngestResult {
    /// IDs of usage events that were actually inserted (new logical events).
    pub inserted_event_ids: Vec<String>,
}

/// `INSERT OR IGNORE` into `usage_events` keyed on `id` only. Omits
/// `generation_id`, `dedupe_key`, and `is_sidechain` (defaults). Only used by
/// the test helper [`Database::write_usage_event`]; production paths route
/// through the consolidated sidechain-aware function in `write_queries.rs`.
const WRITE_USAGE_IGNORE_SQL: &str = "\
    INSERT OR IGNORE INTO usage_events (\
        id, agent, source_file_id, source_path, source_line, \
        source_offset_start, source_offset_end, session_id, turn_id, \
        source_request_id, message_id, timestamp_ms, project_path, \
        project_hash, cwd, model, model_provider, agent_version, \
        client_kind, speed, input_tokens, output_tokens, total_tokens, \
        cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
        reasoning_tokens, thoughts_tokens, tool_tokens, cost_usd, \
        estimated_cost_usd, cost_currency, cost_source, \
        price_catalog_version, is_error, error_type, raw_event_hash, \
        usage_limit_reset_time_ms, created_at_ms, updated_at_ms, \
        provider_payload_shape, prompt_input_total_tokens, prompt_input_non_cached_tokens\
    ) VALUES (\
        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, \
        ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, \
        ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, \
        ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40, \
        ?41, ?42, ?43\
    )";

/// Upsert `usage_events` keyed on `id` only, preserving `created_at_ms`.
/// Same scope as [`WRITE_USAGE_IGNORE_SQL`] — only used by
/// [`Database::write_usage_event`] for tests.
const WRITE_USAGE_REPLACE_SQL: &str = "\
    INSERT INTO usage_events (\
        id, agent, source_file_id, source_path, source_line, \
        source_offset_start, source_offset_end, session_id, turn_id, \
        source_request_id, message_id, timestamp_ms, project_path, \
        project_hash, cwd, model, model_provider, agent_version, \
        client_kind, speed, input_tokens, output_tokens, total_tokens, \
        cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
        reasoning_tokens, thoughts_tokens, tool_tokens, cost_usd, \
        estimated_cost_usd, cost_currency, cost_source, \
        price_catalog_version, is_error, error_type, raw_event_hash, \
        usage_limit_reset_time_ms, created_at_ms, updated_at_ms, \
        provider_payload_shape, prompt_input_total_tokens, prompt_input_non_cached_tokens\
    ) VALUES (\
        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, \
        ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, \
        ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, \
        ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40, \
        ?41, ?42, ?43\
    ) ON CONFLICT(id) DO UPDATE SET \
        speed = excluded.speed, \
        usage_limit_reset_time_ms = excluded.usage_limit_reset_time_ms, \
        input_tokens = excluded.input_tokens, \
        output_tokens = excluded.output_tokens, \
        total_tokens = excluded.total_tokens, \
        cached_input_tokens = excluded.cached_input_tokens, \
        cache_creation_tokens = excluded.cache_creation_tokens, \
        cache_read_tokens = excluded.cache_read_tokens, \
        reasoning_tokens = excluded.reasoning_tokens, \
        thoughts_tokens = excluded.thoughts_tokens, \
        tool_tokens = excluded.tool_tokens, \
        cost_usd = excluded.cost_usd, \
        estimated_cost_usd = excluded.estimated_cost_usd, \
        cost_source = excluded.cost_source, \
        price_catalog_version = excluded.price_catalog_version, \
        raw_event_hash = excluded.raw_event_hash, \
        provider_payload_shape = excluded.provider_payload_shape, \
        prompt_input_total_tokens = excluded.prompt_input_total_tokens, \
        prompt_input_non_cached_tokens = excluded.prompt_input_non_cached_tokens, \
        created_at_ms = usage_events.created_at_ms, \
        updated_at_ms = CASE WHEN excluded.updated_at_ms > usage_events.updated_at_ms \
            THEN excluded.updated_at_ms ELSE usage_events.updated_at_ms END";

impl Database {
    /// Open a database file, enable WAL mode, and run any pending migrations.
    pub fn open(path: &Path) -> Result<Self> {
        info!(path = %path.display(), "opening database");
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open database at {}", path.display()))?;
        let db = Self { conn };
        db.configure()?;
        db.run_migrations()?;
        Ok(db)
    }

    /// Open a read-only connection to the same database file.
    ///
    /// Uses WAL mode so writes on the primary connection do not block reads.
    /// The returned handle shares no state with the primary `Database`.
    pub fn open_readonly(path: &Path) -> Result<Self> {
        debug!(path = %path.display(), "opening read-only database connection");
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("failed to open read-only database at {}", path.display()))?;
        // WAL mode must already be set on the file; just enable busy_timeout.
        conn.execute_batch("PRAGMA busy_timeout = 5000;")
            .context("failed to set read-only pragmas")?;
        Ok(Self { conn })
    }

    /// Open an in-memory database for testing.
    pub fn open_in_memory() -> Result<Self> {
        debug!("opening in-memory database");
        let conn = Connection::open_in_memory().context("failed to open in-memory database")?;
        let db = Self { conn };
        db.configure()?;
        db.run_migrations()?;
        Ok(db)
    }

    /// Access the underlying connection for store query helpers and tests.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// List prompt palette entries.
    pub fn list_prompt_entries(
        &self,
        query: crate::prompt_entries::PromptListQuery,
    ) -> Result<crate::prompt_entries::PromptListResult> {
        crate::prompt_entries::list_prompt_entries(&self.conn, query)
    }

    /// Retrieve a prompt palette entry by ID.
    pub fn get_prompt_entry(
        &self,
        id: &str,
    ) -> Result<Option<crate::prompt_entries::PromptEntryRow>> {
        crate::prompt_entries::get_prompt_entry(&self.conn, id)
    }

    /// Create a prompt palette entry.
    pub fn create_prompt_entry(
        &self,
        row: crate::prompt_entries::NewPromptEntryRow,
    ) -> Result<crate::prompt_entries::PromptEntryRow> {
        crate::prompt_entries::create_prompt_entry(&self.conn, row)
    }

    /// Update a prompt palette entry.
    pub fn update_prompt_entry(
        &self,
        row: crate::prompt_entries::UpdatePromptEntryRow,
    ) -> Result<crate::prompt_entries::PromptEntryRow> {
        crate::prompt_entries::update_prompt_entry(&self.conn, row)
    }

    /// Delete a prompt palette entry by ID.
    pub fn delete_prompt_entry(&self, id: &str) -> Result<bool> {
        crate::prompt_entries::delete_prompt_entry(&self.conn, id)
    }

    /// Record a prompt palette use event and update usage counters.
    pub fn record_prompt_use(
        &self,
        row: crate::prompt_entries::PromptUseRow,
    ) -> Result<crate::prompt_entries::PromptUseResultRow> {
        crate::prompt_entries::record_prompt_use(&self.conn, row)
    }

    /// Suggest tags matching a prefix, deduplicated by normalized form.
    pub fn suggest_tags(&self, prefix: &str, limit: i64) -> Result<Vec<String>> {
        crate::prompt_entries::suggest_tags(&self.conn, prefix, limit)
    }

    // ── Provider catalog ───────────────────────────────────────────
    pub fn create_provider(
        &self,
        req: crate::provider_catalog::CreateProviderReq,
    ) -> anyhow::Result<busytok_domain::Provider> {
        crate::provider_catalog::create_provider(&self.conn, req)
    }
    pub fn update_provider(
        &self,
        id: &str,
        patch: crate::provider_catalog::UpdateProviderPatch,
    ) -> anyhow::Result<busytok_domain::Provider> {
        crate::provider_catalog::update_provider(&self.conn, id, patch)
    }
    pub fn delete_provider(&self, id: &str) -> anyhow::Result<()> {
        crate::provider_catalog::delete_provider(&self.conn, id)
    }
    pub fn get_provider_with_secret(
        &self,
        id: &str,
    ) -> anyhow::Result<Option<busytok_domain::Provider>> {
        crate::provider_catalog::get_provider_with_secret(&self.conn, id)
    }
    pub fn list_providers(&self) -> anyhow::Result<Vec<busytok_domain::ProviderSummary>> {
        crate::provider_catalog::list_providers(&self.conn)
    }
    pub fn create_model(
        &self,
        req: crate::provider_catalog::CreateModelReq,
    ) -> anyhow::Result<busytok_domain::Model> {
        crate::provider_catalog::create_model(&self.conn, req)
    }
    pub fn update_model(
        &self,
        id: &str,
        patch: crate::provider_catalog::UpdateModelPatch,
    ) -> anyhow::Result<busytok_domain::Model> {
        crate::provider_catalog::update_model(&self.conn, id, patch)
    }
    pub fn delete_model(&self, id: &str) -> anyhow::Result<()> {
        crate::provider_catalog::delete_model(&self.conn, id)
    }
    pub fn get_model_by_id(&self, id: &str) -> anyhow::Result<Option<busytok_domain::Model>> {
        crate::provider_catalog::get_model_by_id(&self.conn, id)
    }
    pub fn get_model_by_provider_and_model_id(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> anyhow::Result<Option<busytok_domain::Model>> {
        crate::provider_catalog::get_model_by_provider_and_model_id(
            &self.conn,
            provider_id,
            model_id,
        )
    }
    pub fn list_models_filtered(
        &self,
        filter: busytok_domain::ModelCatalogFilter,
        sort: Option<&str>,
        reasoning: Option<bool>,
    ) -> anyhow::Result<Vec<busytok_domain::ModelCatalogEntry>> {
        crate::provider_catalog::list_models_filtered(&self.conn, filter, sort, reasoning)
    }
    pub fn list_models_by_provider(
        &self,
        provider_id: &str,
    ) -> anyhow::Result<Vec<busytok_domain::ModelCatalogEntry>> {
        crate::provider_catalog::list_models_by_provider(&self.conn, provider_id)
    }
    pub fn list_tags(&self) -> anyhow::Result<Vec<String>> {
        crate::provider_catalog::list_tags(&self.conn)
    }
    pub fn set_model_tags(&self, model_id: &str, tags: &[String]) -> anyhow::Result<()> {
        crate::provider_catalog::set_model_tags(&self.conn, model_id, tags)
    }

    /// Reopen this database from the same backing file, when one exists.
    ///
    /// File-backed databases return a fresh connection to the same path so
    /// long-running scans can avoid monopolizing an existing shared handle.
    /// In-memory databases return `None` because they have no reopenable path.
    pub fn reopen(&self) -> Result<Option<Self>> {
        let Some(path) = self.conn.path() else {
            return Ok(None);
        };
        if path.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self::open(Path::new(path))?))
    }

    /// Reopen this database as a read-only connection, when a backing file exists.
    ///
    /// Uses `SQLITE_OPEN_READ_ONLY` so the connection cannot accidentally write.
    /// In-memory databases return `None` because they have no file path to reopen.
    pub fn reopen_readonly(&self) -> Result<Option<Self>> {
        let Some(path) = self.conn.path() else {
            return Ok(None);
        };
        if path.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self::open_readonly(Path::new(path))?))
    }

    /// Return the file-backed database path when this handle can be reopened.
    pub fn path_buf(&self) -> Option<PathBuf> {
        let path = self.conn.path()?;
        if path.is_empty() {
            None
        } else {
            Some(PathBuf::from(path))
        }
    }

    /// Run a passive WAL checkpoint to move pages from the WAL file back
    /// into the main database. Called periodically by the writer actor to
    /// prevent unbounded WAL growth now that auto-checkpoint is disabled.
    pub fn checkpoint_wal(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(PASSIVE);")
            .context("wal_checkpoint failed")?;
        Ok(())
    }

    /// Configure SQLite pragmas for the Busytok workload.
    fn configure(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "PRAGMA journal_mode = WAL; \
                 PRAGMA foreign_keys = ON; \
                 PRAGMA busy_timeout = 5000; \
                 PRAGMA wal_autocheckpoint = 0;",
            )
            .context("failed to set pragmas")?;
        Ok(())
    }

    /// Run all pending migrations in a transaction.
    fn run_migrations(&self) -> Result<()> {
        self.conn
            .execute_batch(schema::CREATE_SCHEMA_VERSION_TABLE)
            .context("failed to create schema version table")?;

        let applied: u32 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM _schema_version",
                [],
                |row| row.get(0),
            )
            .context("failed to read schema version")?;

        for (version, sql) in schema::migrations() {
            if version > applied {
                info!(version, "applying migration");
                let tx = self
                    .conn
                    .unchecked_transaction()
                    .with_context(|| format!("failed to start migration v{version} transaction"))?;
                tx.execute_batch(sql)
                    .with_context(|| format!("migration v{version} failed"))?;
                let now_ms = now_ms();
                tx.execute(
                    "INSERT INTO _schema_version (version, applied_at_ms) VALUES (?1, ?2)",
                    params![version, now_ms],
                )
                .with_context(|| format!("failed to record migration v{version}"))?;
                tx.commit()
                    .with_context(|| format!("failed to commit migration v{version}"))?;
            }
        }

        Ok(())
    }

    /// Return a list of all user table names.
    pub fn table_names(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_schema_version' ORDER BY name")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut names = Vec::new();
        for name in rows {
            names.push(name?);
        }
        Ok(names)
    }

    /// Return a list of all index names.
    pub fn index_names(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut names = Vec::new();
        for name in rows {
            names.push(name?);
        }
        Ok(names)
    }

    // ── Usage events ──────────────────────────────────────────────────

    /// Write a usage event with the given policy. Uses 43‑column SQL that
    /// omits `generation_id`, `dedupe_key`, and `is_sidechain` so the
    /// dedupe_key unique index does not fire — this helper is exclusively
    /// for tests where each event is a standalone row. Also syncs the
    /// centralized `cache_metric` diagnostic to the event's current invariant
    /// state (mirrors the production write path).
    pub fn write_usage_event(
        &self,
        event: &busytok_domain::NormalizedUsageEvent,
        policy: busytok_domain::UsageWritePolicy,
    ) -> Result<()> {
        debug!(id = %event.id, policy = ?policy, "writing usage event");
        let sql = match policy {
            busytok_domain::UsageWritePolicy::InsertOnce => WRITE_USAGE_IGNORE_SQL,
            busytok_domain::UsageWritePolicy::Replace => WRITE_USAGE_REPLACE_SQL,
        };
        self.conn
            .execute(
                sql,
                rusqlite::params![
                    event.id,
                    event.agent.as_str(),
                    event.source_file_id,
                    event.source_path,
                    event.source_line as i64,
                    event.source_offset_start as i64,
                    event.source_offset_end as i64,
                    event.session_id,
                    event.turn_id,
                    event.source_request_id,
                    event.message_id,
                    event.timestamp_ms,
                    event.project_path,
                    event.project_hash,
                    event.cwd,
                    event.model,
                    event.model_provider,
                    event.agent_version,
                    event.client_kind,
                    event.speed.as_deref(),
                    event.input_tokens,
                    event.output_tokens,
                    event.total_tokens,
                    event.cached_input_tokens,
                    event.cache_creation_tokens,
                    event.cache_read_tokens,
                    event.reasoning_tokens,
                    event.thoughts_tokens,
                    event.tool_tokens,
                    event.cost_usd,
                    event.estimated_cost_usd,
                    event.cost_currency,
                    event.cost_source.as_deref().unwrap_or("unknown"),
                    event.price_catalog_version,
                    event.is_error as i32,
                    event.error_type,
                    event.raw_event_hash,
                    event.usage_limit_reset_time_ms,
                    event.created_at_ms,
                    event.updated_at_ms,
                    event.provider_payload_shape.as_str(),
                    event.prompt_input_total_tokens,
                    event.prompt_input_non_cached_tokens,
                ],
            )
            .context("failed to write usage event")?;
        crate::write_queries::sync_cache_metric_diagnostic(&self.conn, event)?;
        Ok(())
    }

    /// Retrieve a usage event by ID.
    pub fn get_usage_event(
        &self,
        id: &str,
    ) -> Result<Option<busytok_domain::NormalizedUsageEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, agent, source_file_id, source_path, source_line, \
             source_offset_start, source_offset_end, session_id, turn_id, \
             source_request_id, message_id, timestamp_ms, project_path, \
             project_hash, cwd, model, model_provider, agent_version, \
             client_kind, speed, input_tokens, output_tokens, total_tokens, \
             cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
             reasoning_tokens, thoughts_tokens, tool_tokens, cost_usd, \
             estimated_cost_usd, cost_currency, cost_source, \
             price_catalog_version, is_error, error_type, raw_event_hash, \
             usage_limit_reset_time_ms, created_at_ms, updated_at_ms, \
             is_sidechain, dedupe_key, \
             provider_payload_shape, prompt_input_total_tokens, \
             prompt_input_non_cached_tokens \
             FROM usage_events WHERE id = ?1",
        )?;

        let result = stmt
            .query_row(params![id], |row| Ok(row_to_usage_event(row)))
            .optional()
            .context("failed to query usage event")?;

        Ok(result)
    }

    /// Count usage events.
    pub fn usage_event_count(&self) -> Result<i64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM usage_events", [], |row| row.get(0))
            .context("failed to count usage events")?;
        Ok(count)
    }

    /// Return all usage events ordered by timestamp.
    pub fn all_usage_events(&self) -> Result<Vec<busytok_domain::NormalizedUsageEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, agent, source_file_id, source_path, source_line, \
             source_offset_start, source_offset_end, session_id, turn_id, \
             source_request_id, message_id, timestamp_ms, project_path, \
             project_hash, cwd, model, model_provider, agent_version, \
             client_kind, speed, input_tokens, output_tokens, total_tokens, \
             cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
             reasoning_tokens, thoughts_tokens, tool_tokens, cost_usd, \
             estimated_cost_usd, cost_currency, cost_source, \
             price_catalog_version, is_error, error_type, raw_event_hash, \
             usage_limit_reset_time_ms, created_at_ms, updated_at_ms, \
             is_sidechain, dedupe_key, \
             provider_payload_shape, prompt_input_total_tokens, \
             prompt_input_non_cached_tokens \
             FROM usage_events ORDER BY timestamp_ms",
        )?;

        let rows = stmt.query_map([], |row| Ok(row_to_usage_event(row)))?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Return usage events scoped to a specific generation.
    ///
    /// Used by `handle_rebuild_rollups` to rebuild `daily_usage` from only
    /// the active generation's events, not the full cross-generation set.
    pub fn usage_events_for_generation(
        &self,
        generation_id: &str,
    ) -> Result<Vec<busytok_domain::NormalizedUsageEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, agent, source_file_id, source_path, source_line, \
             source_offset_start, source_offset_end, session_id, turn_id, \
             source_request_id, message_id, timestamp_ms, project_path, \
             project_hash, cwd, model, model_provider, agent_version, \
             client_kind, speed, input_tokens, output_tokens, total_tokens, \
             cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
             reasoning_tokens, thoughts_tokens, tool_tokens, cost_usd, \
             estimated_cost_usd, cost_currency, cost_source, \
             price_catalog_version, is_error, error_type, raw_event_hash, \
             usage_limit_reset_time_ms, created_at_ms, updated_at_ms, \
             is_sidechain, dedupe_key, \
             provider_payload_shape, prompt_input_total_tokens, \
             prompt_input_non_cached_tokens \
             FROM usage_events WHERE generation_id = ?1 ORDER BY timestamp_ms",
        )?;

        let rows = stmt.query_map(params![generation_id], |row| Ok(row_to_usage_event(row)))?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ── Diagnostic events ─────────────────────────────────────────────

    /// Record a diagnostic event.
    pub fn record_diagnostic_event(
        &self,
        event: &busytok_domain::OperationalDiagnosticEvent,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO diagnostic_events (\
                id, agent, source_id, source_file_id, source_path, source_line, \
                severity, code, message, details_json, happened_at_ms, created_at_ms\
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    event.id,
                    event.agent.as_ref().map(|a| a.as_str()),
                    event.source_id,
                    event.source_file_id,
                    event.source_path.as_deref(),
                    event.source_line,
                    event.severity,
                    event.category, // maps category -> code
                    event.message,
                    event.detail_json,
                    event.happened_at_ms,
                    event.created_at_ms,
                ],
            )
            .context("failed to record diagnostic event")?;
        Ok(())
    }

    /// Count tool events.
    pub fn tool_event_count(&self) -> Result<i64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM tool_events", [], |row| row.get(0))
            .context("failed to count tool events")?;
        Ok(count)
    }

    /// Count diagnostic events.
    pub fn diagnostic_event_count(&self) -> Result<i64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM diagnostic_events", [], |row| {
                row.get(0)
            })
            .context("failed to count diagnostic events")?;
        Ok(count)
    }

    /// List recent diagnostic events filtered by category and severity.
    pub fn list_diagnostic_events(
        &self,
        category: &str,
        limit: i64,
    ) -> Result<Vec<DiagnosticEventRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, severity, code, message, happened_at_ms \
             FROM diagnostic_events \
             WHERE code = ?1 AND severity IN ('warning', 'error') \
             ORDER BY happened_at_ms DESC LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![category, limit], |row| {
            Ok(DiagnosticEventRow {
                id: row.get(0)?,
                severity: row.get(1)?,
                code: row.get(2)?,
                message: row.get(3)?,
                happened_at_ms: row.get(4)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// List all diagnostic events ordered by happened_at_ms DESC with a limit.
    pub fn list_all_diagnostic_events(&self, limit: i64) -> Result<Vec<DiagnosticEventRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, severity, code, message, happened_at_ms \
             FROM diagnostic_events \
             ORDER BY happened_at_ms DESC LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit], |row| {
            Ok(DiagnosticEventRow {
                id: row.get(0)?,
                severity: row.get(1)?,
                code: row.get(2)?,
                message: row.get(3)?,
                happened_at_ms: row.get(4)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ── Codex token snapshots ──────────────────────────────────────────

    /// Upsert a Codex token snapshot row.
    pub fn upsert_codex_snapshot(&self, row: &CodexTokenSnapshotRow) -> Result<()> {
        debug!(id = %row.id, ordinal = row.token_event_ordinal, "upserting codex snapshot");
        self.conn
            .execute(
                "INSERT INTO codex_token_snapshots (\
                id, source_file_id, source_line, source_offset_start, source_offset_end, \
                session_id, turn_id, token_event_ordinal, \
                input_tokens, cached_input_tokens, output_tokens, reasoning_tokens, total_tokens, \
                model, raw_usage_json, emitted_event_id, created_at_ms, updated_at_ms\
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18) \
            ON CONFLICT(id) DO UPDATE SET \
                input_tokens = excluded.input_tokens, \
                cached_input_tokens = excluded.cached_input_tokens, \
                output_tokens = excluded.output_tokens, \
                reasoning_tokens = excluded.reasoning_tokens, \
                total_tokens = excluded.total_tokens, \
                model = excluded.model, \
                raw_usage_json = excluded.raw_usage_json, \
                emitted_event_id = excluded.emitted_event_id, \
                updated_at_ms = excluded.updated_at_ms",
                params![
                    row.id,
                    row.source_file_id,
                    row.source_line,
                    row.source_offset_start,
                    row.source_offset_end,
                    row.session_id,
                    row.turn_id,
                    row.token_event_ordinal,
                    row.input_tokens,
                    row.cached_input_tokens,
                    row.output_tokens,
                    row.reasoning_tokens,
                    row.total_tokens,
                    row.model,
                    row.raw_usage_json,
                    row.emitted_event_id,
                    row.created_at_ms,
                    row.updated_at_ms,
                ],
            )
            .context("failed to upsert codex snapshot")?;
        Ok(())
    }

    /// Return the next ordinal for a Codex snapshot scope.
    ///
    /// Returns `MAX(token_event_ordinal) + 1` for the given
    /// `source_file_id + session_id + turn_id` scope, or `1` if no rows exist.
    /// Handles NULL turn_id correctly by using IS NULL comparison when turn_id is empty.
    pub fn next_codex_ordinal(
        &self,
        source_file_id: &str,
        session_id: &str,
        turn_id: &str,
    ) -> Result<i64> {
        let max_ordinal: i64 = if turn_id.is_empty() {
            self.conn
                .query_row(
                    "SELECT COALESCE(MAX(token_event_ordinal), 0) FROM codex_token_snapshots \
                 WHERE source_file_id = ?1 AND session_id = ?2 AND turn_id IS NULL",
                    params![source_file_id, session_id],
                    |row| row.get(0),
                )
                .context("failed to query next codex ordinal")?
        } else {
            self.conn
                .query_row(
                    "SELECT COALESCE(MAX(token_event_ordinal), 0) FROM codex_token_snapshots \
                 WHERE source_file_id = ?1 AND session_id = ?2 AND turn_id = ?3",
                    params![source_file_id, session_id, turn_id],
                    |row| row.get(0),
                )
                .context("failed to query next codex ordinal")?
        };
        Ok(max_ordinal + 1)
    }

    /// Get the latest Codex snapshot for a given scope.
    ///
    /// Returns the snapshot with the highest `token_event_ordinal` for the
    /// given `source_file_id + session_id + turn_id` scope.
    /// Handles NULL turn_id correctly by using IS NULL comparison when turn_id is empty.
    pub fn get_latest_codex_snapshot(
        &self,
        source_file_id: &str,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Option<CodexTokenSnapshotRow>> {
        let mut stmt = if turn_id.is_empty() {
            self.conn.prepare(
                "SELECT id, source_file_id, source_line, source_offset_start, source_offset_end, \
                 session_id, turn_id, token_event_ordinal, \
                 input_tokens, cached_input_tokens, output_tokens, reasoning_tokens, total_tokens, \
                 model, raw_usage_json, emitted_event_id, created_at_ms, updated_at_ms \
                 FROM codex_token_snapshots \
                 WHERE source_file_id = ?1 AND session_id = ?2 AND turn_id IS NULL \
                 ORDER BY token_event_ordinal DESC LIMIT 1",
            )?
        } else {
            self.conn.prepare(
                "SELECT id, source_file_id, source_line, source_offset_start, source_offset_end, \
                 session_id, turn_id, token_event_ordinal, \
                 input_tokens, cached_input_tokens, output_tokens, reasoning_tokens, total_tokens, \
                 model, raw_usage_json, emitted_event_id, created_at_ms, updated_at_ms \
                 FROM codex_token_snapshots \
                 WHERE source_file_id = ?1 AND session_id = ?2 AND turn_id = ?3 \
                 ORDER BY token_event_ordinal DESC LIMIT 1",
            )?
        };

        let params: &[&dyn rusqlite::ToSql] = if turn_id.is_empty() {
            &[&source_file_id, &session_id]
        } else {
            &[&source_file_id, &session_id, &turn_id]
        };

        let result = stmt
            .query_row(params, |row| {
                Ok(CodexTokenSnapshotRow {
                    id: row.get(0)?,
                    source_file_id: row.get(1)?,
                    source_line: row.get(2)?,
                    source_offset_start: row.get(3)?,
                    source_offset_end: row.get(4)?,
                    session_id: row.get(5)?,
                    turn_id: row.get(6)?,
                    token_event_ordinal: row.get(7)?,
                    input_tokens: row.get(8)?,
                    cached_input_tokens: row.get(9)?,
                    output_tokens: row.get(10)?,
                    reasoning_tokens: row.get(11)?,
                    total_tokens: row.get(12)?,
                    model: row.get(13)?,
                    raw_usage_json: row.get(14)?,
                    emitted_event_id: row.get(15)?,
                    created_at_ms: row.get(16)?,
                    updated_at_ms: row.get(17)?,
                })
            })
            .optional()
            .context("failed to query latest codex snapshot")?;

        Ok(result)
    }

    /// Get the latest non-empty model observed in persisted Codex snapshots.
    pub fn latest_codex_snapshot_model_for_source_file(
        &self,
        source_file_id: &str,
    ) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT model FROM codex_token_snapshots \
                 WHERE source_file_id = ?1 AND model IS NOT NULL AND model != '' \
                 ORDER BY created_at_ms DESC, token_event_ordinal DESC LIMIT 1",
                params![source_file_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("failed to query latest codex snapshot model for source file")
    }

    /// Get the latest non-empty model observed for a given source file + agent.
    pub fn latest_usage_event_model_for_source_file(
        &self,
        source_file_id: &str,
        agent: &str,
    ) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT model FROM usage_events \
                 WHERE source_file_id = ?1 AND agent = ?2 AND model IS NOT NULL AND model != '' \
                 ORDER BY timestamp_ms DESC, created_at_ms DESC LIMIT 1",
                params![source_file_id, agent],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("failed to query latest usage event model for source file")
    }

    /// Get all distinct non-empty models for a Codex session, across all batches.
    /// Used to decide whether cross-batch backfill is safe (exactly one model).
    pub fn distinct_codex_models_for_session(
        &self,
        source_file_id: &str,
        session_id: &str,
    ) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT model FROM usage_events \
             WHERE source_file_id = ?1 AND session_id = ?2 AND agent = 'codex' \
             AND model IS NOT NULL AND model != '' \
             UNION \
             SELECT DISTINCT model FROM codex_token_snapshots \
             WHERE source_file_id = ?1 AND session_id = ?2 \
             AND model IS NOT NULL AND model != ''",
        )?;
        let rows = stmt
            .query_map(params![source_file_id, session_id], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Backfill NULL/empty model on earlier Codex events for a session.
    /// Called after a batch resolves a model, to fix events from prior batches
    /// that were persisted with NULL model before the model was known.
    /// Returns the total number of rows updated across both `usage_events`
    /// and `codex_token_snapshots`. Both updates are wrapped in a single
    /// transaction so readers never observe a half-updated state.
    pub fn backfill_codex_model_for_session(
        &self,
        source_file_id: &str,
        session_id: &str,
        model: &str,
    ) -> Result<usize> {
        let now = now_ms();
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to begin transaction for codex model backfill")?;
        let updated_events = tx
            .execute(
                "UPDATE usage_events SET model = ?1, updated_at_ms = ?2 \
                 WHERE source_file_id = ?3 AND session_id = ?4 AND agent = 'codex' \
                 AND (model IS NULL OR model = '')",
                params![model, now, source_file_id, session_id],
            )
            .context("failed to backfill usage_events model")?;
        let updated_snapshots = tx
            .execute(
                "UPDATE codex_token_snapshots SET model = ?1, updated_at_ms = ?2 \
                 WHERE source_file_id = ?3 AND session_id = ?4 \
                 AND (model IS NULL OR model = '')",
                params![model, now, source_file_id, session_id],
            )
            .context("failed to backfill codex_token_snapshots model")?;
        tx.commit()
            .context("failed to commit codex model backfill transaction")?;
        Ok(updated_events + updated_snapshots)
    }

    /// Find all Codex session_ids for a source_file_id that have NULL/empty
    /// model events or snapshots. Used to identify sessions that need
    /// cross-batch backfill. Checks both `usage_events` and
    /// `codex_token_snapshots` because some sessions may have snapshots
    /// but not yet have produced usage events (e.g. first heartbeat).
    pub fn codex_sessions_with_null_model(&self, source_file_id: &str) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT session_id FROM usage_events \
             WHERE source_file_id = ?1 AND agent = 'codex' \
             AND (model IS NULL OR model = '') \
             UNION \
             SELECT DISTINCT session_id FROM codex_token_snapshots \
             WHERE source_file_id = ?1 \
             AND (model IS NULL OR model = '')",
        )?;
        let rows = stmt
            .query_map(params![source_file_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Delete all rows from `daily_usage`. Used before full rebuild after
    /// cross-batch model backfill, since the model field is part of the
    /// grouping key and incremental upserts can't fix stale groupings.
    pub fn delete_daily_usage(&self) -> Result<()> {
        self.conn().execute("DELETE FROM daily_usage", [])?;
        Ok(())
    }

    /// Atomically replace all `daily_usage` rows. Wraps DELETE + upsert in a
    /// single transaction so readers never observe an empty or half-rebuilt
    /// table. Mirrors the `replace_model_summaries` / `replace_sessions`
    /// pattern used elsewhere in the rebuild path.
    pub fn replace_daily_usage(
        &self,
        events: &[busytok_domain::NormalizedUsageEvent],
        rtz: &busytok_domain::ReportingTimezone,
        generation_id: &str,
    ) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to begin transaction for daily_usage rebuild")?;
        tx.execute("DELETE FROM daily_usage", [])
            .context("failed to clear daily_usage")?;
        // Re-use the shared upsert helper, but against the transaction's
        // connection so the DELETE + INSERTs commit atomically.
        crate::write_queries::upsert_daily_usage_for_events(&tx, events, rtz, generation_id)
            .context("failed to upsert daily_usage rows during rebuild")?;
        tx.commit()
            .context("failed to commit daily_usage rebuild transaction")?;
        Ok(())
    }

    // ── Log files ─────────────────────────────────────────────────────

    /// Update the checkpoint offset for a log file (upsert).
    pub fn checkpoint_log_file(
        &self,
        file_id: &str,
        offset: u64,
        agent: &str,
        source_id: &str,
        path: &str,
        inode: Option<&str>,
    ) -> Result<()> {
        let now_ms = now_ms();

        self.conn
            .execute(
                "INSERT INTO log_files (id, source_id, agent, path, inode, offset_bytes, state, \
             first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7, ?7, ?7, ?7) \
             ON CONFLICT(id) DO UPDATE SET offset_bytes = ?6, inode = ?5, updated_at_ms = ?7",
                params![
                    file_id,
                    source_id,
                    agent,
                    path,
                    inode,
                    offset as i64,
                    now_ms
                ],
            )
            .context("failed to checkpoint log file")?;
        Ok(())
    }

    /// Retrieve a log file row by ID.
    pub fn get_log_file(&self, id: &str) -> Result<Option<LogFileRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_id, agent, path, inode, size_bytes, offset_bytes, \
             last_mtime_ms, first_seen_at_ms, last_seen_at_ms, state, last_error, \
             created_at_ms, updated_at_ms \
             FROM log_files WHERE id = ?1",
        )?;

        let result = stmt
            .query_row(params![id], |row| {
                Ok(LogFileRow {
                    id: row.get(0)?,
                    source_id: row.get(1)?,
                    agent: row.get(2)?,
                    path: row.get(3)?,
                    inode: row.get(4)?,
                    size_bytes: row.get(5)?,
                    offset_bytes: row.get(6)?,
                    last_mtime_ms: row.get(7)?,
                    first_seen_at_ms: row.get(8)?,
                    last_seen_at_ms: row.get(9)?,
                    state: row.get(10)?,
                    last_error: row.get(11)?,
                    created_at_ms: row.get(12)?,
                    updated_at_ms: row.get(13)?,
                })
            })
            .optional()
            .context("failed to query log file")?;

        Ok(result)
    }

    // ── Log sources ───────────────────────────────────────────────────

    /// Read counts needed for shell-status chip hydration.
    ///
    /// Returns `(total_usage_event_count, active_source_count)`.
    pub fn read_chip_hydration_counts(&self) -> Result<(i64, i64)> {
        let total: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))?;
        let sources: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM log_sources WHERE status != 'removed'",
            [],
            |r| r.get(0),
        )?;
        Ok((total, sources))
    }

    /// Upsert a log source.
    pub fn upsert_log_source(&self, source: &LogSourceRow) -> Result<()> {
        debug!(id = %source.id, "upserting log source");
        self.conn.execute(
            "INSERT INTO log_sources (\
                id, agent, source_type, root_path, configured_by_user, \
                default_discovery_enabled, status, last_scan_started_at_ms, \
                last_scan_completed_at_ms, last_error, first_seen_at_ms, \
                last_seen_at_ms, created_at_ms, updated_at_ms\
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
            ON CONFLICT(id) DO UPDATE SET \
                agent = excluded.agent, \
                source_type = excluded.source_type, \
                root_path = excluded.root_path, \
                status = excluded.status, \
                last_scan_started_at_ms = CASE \
                    WHEN excluded.last_scan_started_at_ms IS NOT NULL THEN excluded.last_scan_started_at_ms \
                    ELSE log_sources.last_scan_started_at_ms END, \
                last_scan_completed_at_ms = excluded.last_scan_completed_at_ms, \
                last_error = excluded.last_error, \
                last_seen_at_ms = excluded.last_seen_at_ms, \
                updated_at_ms = excluded.updated_at_ms, \
                first_seen_at_ms = log_sources.first_seen_at_ms, \
                created_at_ms = log_sources.created_at_ms",
            params![
                source.id,
                source.agent,
                source.source_type,
                source.root_path,
                source.configured_by_user,
                source.default_discovery_enabled,
                source.status,
                source.last_scan_started_at_ms,
                source.last_scan_completed_at_ms,
                source.last_error,
                source.first_seen_at_ms,
                source.last_seen_at_ms,
                source.created_at_ms,
                source.updated_at_ms,
            ],
        ).context("failed to upsert log source")?;
        Ok(())
    }

    /// List all log sources.
    pub fn list_log_sources(&self) -> Result<Vec<LogSourceRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, agent, source_type, root_path, configured_by_user, \
             default_discovery_enabled, status, last_scan_started_at_ms, \
             last_scan_completed_at_ms, last_error, first_seen_at_ms, \
             last_seen_at_ms, created_at_ms, updated_at_ms \
             FROM log_sources ORDER BY id",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(LogSourceRow {
                id: row.get(0)?,
                agent: row.get(1)?,
                source_type: row.get(2)?,
                root_path: row.get(3)?,
                configured_by_user: row.get(4)?,
                default_discovery_enabled: row.get(5)?,
                status: row.get(6)?,
                last_scan_started_at_ms: row.get(7)?,
                last_scan_completed_at_ms: row.get(8)?,
                last_error: row.get(9)?,
                first_seen_at_ms: row.get(10)?,
                last_seen_at_ms: row.get(11)?,
                created_at_ms: row.get(12)?,
                updated_at_ms: row.get(13)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ── Daily usage ───────────────────────────────────────────────────

    /// Return total tokens for a given date and generation.
    pub fn daily_usage_total_tokens(&self, date: &str, generation_id: &str) -> Result<i64> {
        let total: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(total_tokens), 0) FROM daily_usage WHERE date = ?1 AND generation_id = ?2",
                params![date, generation_id],
                |row| row.get(0),
            )
            .context("failed to query daily usage total tokens")?;
        Ok(total)
    }

    /// List all daily usage rows (for testing/debugging).
    pub fn daily_usage_rows(&self) -> Result<Vec<DailyUsageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT date, timezone, agent, project_hash, model, \
             input_tokens, output_tokens, total_tokens, \
             cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
             reasoning_tokens, thoughts_tokens, tool_tokens, \
             cost_usd, estimated_cost_usd, event_count, generation_id \
             FROM daily_usage ORDER BY date, agent",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(DailyUsageRow {
                date: row.get(0)?,
                timezone: row.get(1)?,
                agent: row.get(2)?,
                project_hash: row.get(3)?,
                model: row.get(4)?,
                input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                cached_input_tokens: row.get(8)?,
                cache_creation_tokens: row.get(9)?,
                cache_read_tokens: row.get(10)?,
                reasoning_tokens: row.get(11)?,
                thoughts_tokens: row.get(12)?,
                tool_tokens: row.get(13)?,
                cost_usd: row.get(14)?,
                estimated_cost_usd: row.get(15)?,
                event_count: row.get(16)?,
                generation_id: row.get(17)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ── Sessions ────────────────────────────────────────────────────

    /// List all session rows.
    pub fn session_rows(&self) -> Result<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, agent, project_hash, started_at_ms, last_seen_at_ms, \
             model_list_json, total_tokens, total_cost_usd, event_count, \
             is_active, created_at_ms, updated_at_ms \
             FROM sessions ORDER BY started_at_ms",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                agent: row.get(1)?,
                project_hash: row.get(2)?,
                started_at_ms: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                last_seen_at_ms: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                model_list_json: row
                    .get::<_, Option<String>>(5)?
                    .unwrap_or_else(|| "[]".to_string()),
                total_tokens: row.get(6)?,
                total_cost_usd: row.get(7)?,
                event_count: row.get(8)?,
                is_active: row.get::<_, Option<i32>>(9)?.unwrap_or(0),
                created_at_ms: row.get::<_, Option<i64>>(10)?.unwrap_or(0),
                updated_at_ms: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Replace all sessions rows atomically (full-rebuild pattern).
    pub fn replace_sessions(&self, rows: &[SessionRow]) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to begin transaction for sessions rebuild")?;

        tx.execute("DELETE FROM sessions", [])
            .context("failed to clear sessions")?;

        for s in rows {
            tx.execute(
                "INSERT INTO sessions (\
                    id, agent, project_hash, started_at_ms, last_seen_at_ms, \
                    model_list_json, total_tokens, total_cost_usd, event_count, \
                    is_active, created_at_ms, updated_at_ms\
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    s.id,
                    s.agent,
                    s.project_hash,
                    s.started_at_ms,
                    s.last_seen_at_ms,
                    s.model_list_json,
                    s.total_tokens,
                    s.total_cost_usd,
                    s.event_count,
                    s.is_active,
                    s.created_at_ms,
                    s.updated_at_ms,
                ],
            )
            .with_context(|| format!("failed to write session row id={}", s.id))?;
        }

        tx.commit()
            .context("failed to commit sessions rebuild transaction")?;
        Ok(())
    }

    // ── Projects ────────────────────────────────────────────────────

    /// List all project rows.
    pub fn project_rows(&self) -> Result<Vec<ProjectRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_hash, project_path, agent, display_name, \
             first_seen_at_ms, last_seen_at_ms, total_tokens, total_cost_usd, \
             session_count, created_at_ms, updated_at_ms \
             FROM projects ORDER BY project_hash",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                project_hash: row.get(1)?,
                project_path: row.get(2)?,
                agent: row.get(3)?,
                display_name: row.get(4)?,
                first_seen_at_ms: row.get(5)?,
                last_seen_at_ms: row.get(6)?,
                total_tokens: row.get(7)?,
                total_cost_usd: row.get(8)?,
                session_count: row.get(9)?,
                created_at_ms: row.get(10)?,
                updated_at_ms: row.get(11)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Replace all project rows atomically (full-rebuild pattern).
    pub fn replace_projects(&self, rows: &[ProjectRow]) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to begin transaction for projects rebuild")?;

        tx.execute("DELETE FROM projects", [])
            .context("failed to clear projects")?;

        for p in rows {
            tx.execute(
                "INSERT INTO projects (\
                    id, project_hash, project_path, agent, display_name, \
                    first_seen_at_ms, last_seen_at_ms, total_tokens, \
                    total_cost_usd, session_count, created_at_ms, updated_at_ms\
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    p.id,
                    p.project_hash,
                    p.project_path,
                    p.agent,
                    p.display_name,
                    p.first_seen_at_ms,
                    p.last_seen_at_ms,
                    p.total_tokens,
                    p.total_cost_usd,
                    p.session_count,
                    p.created_at_ms,
                    p.updated_at_ms,
                ],
            )
            .with_context(|| format!("failed to write project row id={}", p.id))?;
        }

        tx.commit()
            .context("failed to commit projects rebuild transaction")?;
        Ok(())
    }

    // ── Model summaries ─────────────────────────────────────────────

    /// List all model summary rows.
    pub fn model_summary_rows(&self) -> Result<Vec<ModelSummaryRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT model, total_tokens, total_cost_usd, event_count \
             FROM model_summary ORDER BY model",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(ModelSummaryRow {
                model: row.get(0)?,
                total_tokens: row.get(1)?,
                total_cost_usd: row.get(2)?,
                event_count: row.get(3)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Replace all model_summary rows atomically (full-rebuild pattern).
    pub fn replace_model_summaries(&self, rows: &[ModelSummaryRow]) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to begin transaction for model_summary rebuild")?;

        tx.execute("DELETE FROM model_summary", [])
            .context("failed to clear model_summary")?;

        for m in rows {
            tx.execute(
                "INSERT INTO model_summary (\
                    model, total_tokens, total_cost_usd, event_count\
                ) VALUES (?1, ?2, ?3, ?4)",
                params![m.model, m.total_tokens, m.total_cost_usd, m.event_count,],
            )
            .with_context(|| format!("failed to write model_summary row model={}", m.model))?;
        }

        tx.commit()
            .context("failed to commit model_summary rebuild transaction")?;
        Ok(())
    }

    // ── Model usage ───────────────────────────────────────────────────

    /// List all model usage rows.
    pub fn model_usage_rows(&self) -> Result<Vec<ModelUsageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT model, agent, timezone, date, \
             input_tokens, output_tokens, total_tokens, \
             cached_input_tokens, reasoning_tokens, cost_usd, event_count \
             FROM model_usage ORDER BY model, agent",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(ModelUsageRow {
                model: row.get(0)?,
                agent: row.get(1)?,
                timezone: row.get(2)?,
                date: row.get(3)?,
                input_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                total_tokens: row.get(6)?,
                cached_input_tokens: row.get(7)?,
                reasoning_tokens: row.get(8)?,
                cost_usd: row.get(9)?,
                event_count: row.get(10)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ── Realtime summary ──────────────────────────────────────────────

    /// Get a realtime summary value by key.
    pub fn realtime_summary_value(&self, key: &str) -> Result<Option<String>> {
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT value_json FROM realtime_summary WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .context("failed to query realtime summary")?
            .flatten();
        Ok(result)
    }

    /// Read all realtime summary entries as a HashMap.
    pub fn read_realtime_summary(&self) -> Result<std::collections::HashMap<String, String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value_json FROM realtime_summary ORDER BY key")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut result = std::collections::HashMap::new();
        for row in rows {
            let (key, value) = row?;
            result.insert(key, value);
        }
        Ok(result)
    }

    /// Replace all realtime summary entries atomically.
    pub fn replace_realtime_summary(&self, entries: &[(String, String)]) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to begin transaction for realtime_summary replace")?;
        let now_ms = now_ms();
        tx.execute("DELETE FROM realtime_summary", [])
            .context("failed to clear realtime_summary")?;
        for (key, value_json) in entries {
            tx.execute(
                "INSERT INTO realtime_summary (key, value_json, updated_at_ms) VALUES (?1, ?2, ?3)",
                params![key, value_json, now_ms],
            )
            .with_context(|| format!("failed to write realtime_summary key={key}"))?;
        }
        tx.commit()
            .context("failed to commit realtime_summary replace transaction")?;
        Ok(())
    }

    /// Count log files by source, returning (total_count, parsed_count).
    pub fn count_log_files_by_source(&self, source_id: &str) -> Result<(i64, i64)> {
        let total: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM log_files WHERE source_id = ?1",
                params![source_id],
                |row| row.get(0),
            )
            .context("failed to count log files")?;
        let parsed: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM log_files WHERE source_id = ?1 AND offset_bytes > 0",
                params![source_id],
                |row| row.get(0),
            )
            .context("failed to count parsed log files")?;
        Ok((total, parsed))
    }

    /// Update the status of a log source.
    pub fn update_log_source_status(&self, id: &str, status: &str) -> Result<()> {
        let now_ms = now_ms();
        self.conn
            .execute(
                "UPDATE log_sources SET status = ?2, updated_at_ms = ?3 WHERE id = ?1",
                params![id, status, now_ms],
            )
            .context("failed to update log source status")?;
        Ok(())
    }

    /// Query usage events with cursor-based pagination.
    ///
    /// Returns events ordered by timestamp_ms DESC, id DESC.
    /// The cursor is a composite of (timestamp_ms, id) encoded as a string.
    /// If cursor is None, returns the most recent events.
    /// If cursor is provided, returns events older than the cursor position.
    pub fn query_usage_events_paginated(
        &self,
        cursor: Option<(&str, i64)>,
        limit: u32,
    ) -> Result<Vec<busytok_domain::NormalizedUsageEvent>> {
        let sql = "SELECT id, agent, source_file_id, source_path, source_line, \
             source_offset_start, source_offset_end, session_id, turn_id, \
             source_request_id, message_id, timestamp_ms, project_path, \
             project_hash, cwd, model, model_provider, agent_version, \
             client_kind, speed, input_tokens, output_tokens, total_tokens, \
             cached_input_tokens, cache_creation_tokens, cache_read_tokens, \
             reasoning_tokens, thoughts_tokens, tool_tokens, cost_usd, \
             estimated_cost_usd, cost_currency, cost_source, \
             price_catalog_version, is_error, error_type, raw_event_hash, \
             usage_limit_reset_time_ms, created_at_ms, updated_at_ms, \
             is_sidechain, dedupe_key, \
             provider_payload_shape, prompt_input_total_tokens, \
             prompt_input_non_cached_tokens \
             FROM usage_events \
             WHERE (timestamp_ms < ?2) OR (timestamp_ms = ?2 AND id < ?3) \
             ORDER BY timestamp_ms DESC, id DESC LIMIT ?1";

        // When no cursor, use values that match all rows (max timestamp, empty id).
        let (cursor_ts, cursor_id): (i64, &str) = match cursor {
            Some((id, ts)) => (ts, id),
            None => (i64::MAX, ""),
        };

        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![limit as i64, cursor_ts, cursor_id], |row| {
            Ok(row_to_usage_event(row))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ── Batch ingest (atomic transaction) ─────────────────────────────

    /// Atomically ingest a batch of writes. If any write fails, the entire
    /// transaction rolls back and the checkpoint must not advance.
    ///
    /// The `build_rollups` closure is called inside the transaction after raw
    /// events are written. It receives the IDs of truly inserted events and
    /// must return the corresponding aggregate rows. This keeps raw events,
    /// rollups, realtime_summary, and checkpoint inside a single transaction.
    ///
    /// When the batch contains Replace-policy events that actually replace
    /// existing rows, the closure receives the OLD event values (fetched before
    /// replacement) so it can compute the correct delta for rollups.
    pub fn ingest_store_batch<F>(
        &self,
        batch: StoreWriteBatch,
        generation_id: &str,
        build_rollups: F,
    ) -> Result<IngestResult>
    where
        F: FnOnce(&[busytok_domain::NormalizedUsageEvent], &str) -> Result<RollupRows>,
    {
        let usage_count = batch.usage_events.len();
        let diag_count = batch.diagnostic_events.len();
        debug!(
            source_id = %batch.source_id,
            file_id = ?batch.source_file_id,
            usage_events = usage_count,
            diagnostic_events = diag_count,
            "ingesting store batch"
        );
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to begin transaction")?;

        // Write usage events, tracking which are truly inserted and which are replacements.
        // For Replace events, fetch the old event BEFORE replacing so the rollup builder
        // can compute the correct delta.
        // Write usage events through the single sidechain-aware entry point.
        // `effective_events` carries full tokens for inserts and `new − old`
        // deltas for replacements, which the rollup builder applies additively.
        let (usage_events, policies): (
            Vec<busytok_domain::NormalizedUsageEvent>,
            Vec<busytok_domain::UsageWritePolicy>,
        ) = batch.usage_events.iter().cloned().unzip();
        let outcome = crate::write_queries::upsert_usage_events_dedup_aware(
            &tx,
            &usage_events,
            &policies,
            generation_id,
        )
        .context("failed to upsert usage events batch")?;
        let effective_events = outcome.effective_events;
        let inserted_event_ids = outcome.inserted_ids;

        // Write tool events
        for tool in &batch.tool_events {
            tx.execute(
                "INSERT OR IGNORE INTO tool_events (\
                    id, agent, source_file_id, source_path, source_line, \
                    source_offset_start, source_offset_end, session_id, \
                    message_id, tool_name, status, timestamp_ms, \
                    project_hash, created_at_ms\
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    tool.id,
                    tool.agent.as_str(),
                    tool.source_file_id,
                    tool.source_path,
                    tool.source_line as i64,
                    tool.source_offset_start as i64,
                    tool.source_offset_end as i64,
                    tool.session_id,
                    tool.message_id,
                    tool.tool_name,
                    tool.status,
                    tool.timestamp_ms,
                    tool.project_hash,
                    tool.created_at_ms,
                ],
            )
            .with_context(|| format!("failed to write tool event {}", tool.id))?;
        }

        // Write diagnostic events
        for diag in &batch.diagnostic_events {
            tx.execute(
                "INSERT OR REPLACE INTO diagnostic_events (\
                    id, agent, source_id, source_file_id, source_path, source_line, \
                    severity, code, message, details_json, happened_at_ms, created_at_ms\
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    diag.id,
                    diag.agent.as_ref().map(|a| a.as_str()),
                    diag.source_id,
                    diag.source_file_id,
                    diag.source_path.as_deref(),
                    diag.source_line,
                    diag.severity,
                    diag.category,
                    diag.message,
                    diag.detail_json,
                    diag.happened_at_ms,
                    diag.created_at_ms,
                ],
            )
            .with_context(|| format!("failed to write diagnostic event {}", diag.id))?;
        }

        // Write Codex token snapshot rows
        for snap in &batch.codex_snapshots {
            tx.execute(
                "INSERT INTO codex_token_snapshots (\
                    id, source_file_id, source_line, source_offset_start, source_offset_end, \
                    session_id, turn_id, token_event_ordinal, \
                    input_tokens, cached_input_tokens, output_tokens, reasoning_tokens, total_tokens, \
                    model, raw_usage_json, emitted_event_id, created_at_ms, updated_at_ms\
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18) \
                ON CONFLICT(id) DO UPDATE SET \
                    input_tokens = excluded.input_tokens, \
                    cached_input_tokens = excluded.cached_input_tokens, \
                    output_tokens = excluded.output_tokens, \
                    reasoning_tokens = excluded.reasoning_tokens, \
                    total_tokens = excluded.total_tokens, \
                    model = excluded.model, \
                    raw_usage_json = excluded.raw_usage_json, \
                    emitted_event_id = excluded.emitted_event_id, \
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    snap.id,
                    snap.source_file_id,
                    snap.source_line,
                    snap.source_offset_start,
                    snap.source_offset_end,
                    snap.session_id,
                    snap.turn_id,
                    snap.token_event_ordinal,
                    snap.input_tokens,
                    snap.cached_input_tokens,
                    snap.output_tokens,
                    snap.reasoning_tokens,
                    snap.total_tokens,
                    snap.model,
                    snap.raw_usage_json,
                    snap.emitted_event_id,
                    snap.created_at_ms,
                    snap.updated_at_ms,
                ],
            ).with_context(|| format!("failed to write codex snapshot {}", snap.id))?;
        }

        // Build rollup rows inside the transaction from the effective events
        // (full inserts + new−old deltas), which the closure applies additively.
        let rollups = build_rollups(&effective_events, generation_id)
            .context("failed to build rollup rows")?;

        // Write daily usage rows via shared function (single ON CONFLICT SQL).
        for daily in &rollups.daily_usage_rows {
            validate_date_format(&daily.date)
                .with_context(|| format!("invalid date in daily_usage row: {}", daily.date))?;
        }
        crate::write_queries::upsert_daily_usage_rows(&tx, &rollups.daily_usage_rows)
            .context("failed to upsert daily_usage rows")?;

        // Write model usage rows (incremental upsert)
        for model in &rollups.model_usage_rows {
            tx.execute(
                "INSERT INTO model_usage (\
                    model, agent, timezone, date, \
                    input_tokens, output_tokens, total_tokens, \
                    cached_input_tokens, reasoning_tokens, cost_usd, event_count\
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
                ON CONFLICT(model, agent, timezone, date) DO UPDATE SET \
                    input_tokens = input_tokens + excluded.input_tokens, \
                    output_tokens = output_tokens + excluded.output_tokens, \
                    total_tokens = total_tokens + excluded.total_tokens, \
                    cached_input_tokens = cached_input_tokens + excluded.cached_input_tokens, \
                    reasoning_tokens = reasoning_tokens + excluded.reasoning_tokens, \
                    cost_usd = CASE WHEN cost_usd IS NOT NULL AND excluded.cost_usd IS NOT NULL THEN cost_usd + excluded.cost_usd \
                        WHEN cost_usd IS NOT NULL THEN cost_usd \
                        ELSE excluded.cost_usd END, \
                    event_count = event_count + excluded.event_count",
                params![
                    model.model,
                    model.agent,
                    model.timezone,
                    model.date,
                    model.input_tokens,
                    model.output_tokens,
                    model.total_tokens,
                    model.cached_input_tokens,
                    model.reasoning_tokens,
                    model.cost_usd,
                    model.event_count,
                ],
            ).with_context(|| format!("failed to write model_usage row for model={}", model.model))?;
        }

        // Write session rows (incremental upsert)
        for s in &rollups.session_rows {
            let now_ms = now_ms();
            tx.execute(
                "INSERT INTO sessions (\
                    id, agent, project_hash, started_at_ms, last_seen_at_ms, \
                    model_list_json, total_tokens, total_cost_usd, event_count, \
                    is_active, created_at_ms, updated_at_ms\
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
                ON CONFLICT(id) DO UPDATE SET \
                    total_tokens = total_tokens + excluded.total_tokens, \
                    total_cost_usd = CASE WHEN total_cost_usd IS NOT NULL AND excluded.total_cost_usd IS NOT NULL THEN total_cost_usd + excluded.total_cost_usd \
                        WHEN total_cost_usd IS NOT NULL THEN total_cost_usd \
                        ELSE excluded.total_cost_usd END, \
                    event_count = event_count + excluded.event_count, \
                    last_seen_at_ms = CASE WHEN excluded.last_seen_at_ms > last_seen_at_ms THEN excluded.last_seen_at_ms ELSE last_seen_at_ms END, \
                    model_list_json = excluded.model_list_json, \
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    s.id,
                    s.agent,
                    s.project_hash,
                    s.started_at_ms,
                    s.last_seen_at_ms,
                    s.model_list_json,
                    s.total_tokens,
                    s.total_cost_usd,
                    s.event_count,
                    s.is_active,
                    s.created_at_ms,
                    now_ms,
                ],
            ).with_context(|| format!("failed to write session row id={}", s.id))?;
        }

        // Write project rows (incremental upsert)
        for p in &rollups.project_rows {
            let now_ms = now_ms();
            tx.execute(
                "INSERT INTO projects (\
                    id, project_hash, project_path, agent, display_name, \
                    first_seen_at_ms, last_seen_at_ms, total_tokens, \
                    total_cost_usd, session_count, created_at_ms, updated_at_ms\
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
                ON CONFLICT(id) DO UPDATE SET \
                    total_tokens = total_tokens + excluded.total_tokens, \
                    total_cost_usd = CASE WHEN total_cost_usd IS NOT NULL AND excluded.total_cost_usd IS NOT NULL THEN total_cost_usd + excluded.total_cost_usd \
                        WHEN total_cost_usd IS NOT NULL THEN total_cost_usd \
                        ELSE excluded.total_cost_usd END, \
                    session_count = session_count + excluded.session_count, \
                    last_seen_at_ms = CASE WHEN excluded.last_seen_at_ms > last_seen_at_ms THEN excluded.last_seen_at_ms ELSE last_seen_at_ms END, \
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    p.id,
                    p.project_hash,
                    p.project_path,
                    p.agent,
                    p.display_name,
                    p.first_seen_at_ms,
                    p.last_seen_at_ms,
                    p.total_tokens,
                    p.total_cost_usd,
                    p.session_count,
                    p.created_at_ms,
                    now_ms,
                ],
            ).with_context(|| format!("failed to write project row id={}", p.id))?;
        }

        // Write model_summary rows (incremental upsert)
        for m in &rollups.model_summary_rows {
            tx.execute(
                "INSERT INTO model_summary (\
                    model, total_tokens, total_cost_usd, event_count\
                ) VALUES (?1, ?2, ?3, ?4) \
                ON CONFLICT(model) DO UPDATE SET \
                    total_tokens = total_tokens + excluded.total_tokens, \
                    total_cost_usd = CASE WHEN total_cost_usd IS NOT NULL AND excluded.total_cost_usd IS NOT NULL THEN total_cost_usd + excluded.total_cost_usd \
                        WHEN total_cost_usd IS NOT NULL THEN total_cost_usd \
                        ELSE excluded.total_cost_usd END, \
                    event_count = event_count + excluded.event_count",
                params![
                    m.model,
                    m.total_tokens,
                    m.total_cost_usd,
                    m.event_count,
                ],
            ).with_context(|| format!("failed to write model_summary row model={}", m.model))?;
        }

        // Checkpoint log file offset
        if let Some(ref file_id) = batch.source_file_id {
            let now_ms = now_ms();

            tx.execute(
                "INSERT INTO log_files (id, source_id, agent, path, inode, offset_bytes, state, \
                 first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7, ?7, ?7, ?7) \
                 ON CONFLICT(id) DO UPDATE SET offset_bytes = excluded.offset_bytes, inode = excluded.inode, last_seen_at_ms = excluded.last_seen_at_ms, updated_at_ms = excluded.updated_at_ms",
                params![
                    file_id,
                    batch.source_id,
                    batch.source_file_agent,
                    batch.source_file_path,
                    batch.source_file_inode,
                    batch.checkpoint_offset.unwrap_or(0) as i64,
                    now_ms,
                ],
            ).with_context(|| format!("failed to checkpoint log file {file_id}"))?;
        }

        tx.commit().context("failed to commit batch transaction")?;
        info!(
            source_id = %batch.source_id,
            usage_events = usage_count,
            diagnostic_events = diag_count,
            "batch ingested"
        );
        Ok(IngestResult { inserted_event_ids })
    }

    /// Comprehensive health check for the SQLite database.
    pub fn health_check(&self) -> Result<StoreHealthInfo> {
        // PRAGMA integrity_check
        let integrity: String = self
            .conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .context("integrity check failed")?;

        // Database file size from page count * page size
        let page_count: i64 = self
            .conn
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .context("page count query failed")?;
        let page_size: i64 = self
            .conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .context("page size query failed")?;
        let db_size_bytes: i64 = page_count * page_size;

        // Usage event count
        let usage_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM usage_events", [], |row| row.get(0))
            .context("usage count failed")?;

        // Last checkpoint timestamp from log_files
        let last_log_checkpoint_ms: Option<i64> = self
            .conn
            .query_row("SELECT MAX(updated_at_ms) FROM log_files", [], |row| {
                row.get(0)
            })
            .optional()
            .context("checkpoint query failed")?
            .flatten();

        Ok(StoreHealthInfo {
            healthy: integrity == "ok",
            integrity_message: integrity,
            migration_version: crate::schema::SCHEMA_VERSION as i32,
            db_size_bytes,
            usage_event_count: usage_count,
            last_log_checkpoint_ms,
        })
    }

    // --- subagent runtime ------------------------------------------------------

    pub fn subagent_upsert_logical(&self, row: &SubagentLogicalSubagentRow) -> Result<()> {
        subagent_queries::upsert_logical_subagent(self.conn(), row)
    }
    pub fn subagent_get_logical(&self, id: &str) -> Result<Option<SubagentLogicalSubagentRow>> {
        subagent_queries::get_logical_subagent(self.conn(), id)
    }
    pub fn subagent_list_active_by_repo(
        &self,
        repo_hash: &str,
    ) -> Result<Vec<SubagentLogicalSubagentRow>> {
        subagent_queries::list_active_by_repo(self.conn(), repo_hash)
    }
    pub fn subagent_find_by_name_in_repo(
        &self,
        project_id: &str,
        repo_hash: &str,
        name: &str,
    ) -> Result<Vec<SubagentLogicalSubagentRow>> {
        subagent_queries::find_by_name_in_repo(self.conn(), project_id, repo_hash, name)
    }
    pub fn subagent_upsert_memory(&self, row: &SubagentMemoryRow) -> Result<()> {
        subagent_queries::upsert_memory(self.conn(), row)
    }
    pub fn subagent_get_memory(&self, subagent_id: &str) -> Result<Option<SubagentMemoryRow>> {
        subagent_queries::get_memory(self.conn(), subagent_id)
    }
    pub fn subagent_insert_task(&self, row: &SubagentTaskRow) -> Result<()> {
        subagent_queries::insert_task(self.conn(), row)
    }
    pub fn subagent_get_task(&self, id: &str) -> Result<Option<SubagentTaskRow>> {
        subagent_queries::get_task(self.conn(), id)
    }
    pub fn subagent_list_tasks(
        &self,
        subagent_id: &str,
        limit: i64,
    ) -> Result<Vec<SubagentTaskRow>> {
        subagent_queries::list_tasks(self.conn(), subagent_id, limit)
    }
    /// Recent tasks across all subagents (spec §4 Phase 2 `tasks_recent`).
    pub fn subagent_list_recent_tasks_all(&self, limit: i64) -> Result<Vec<SubagentTaskRow>> {
        subagent_queries::list_recent_tasks_all(self.conn(), limit)
    }
    /// Per-subagent task counts (spec §4 Phase 2 `subagents[].task_count`).
    pub fn subagent_count_tasks_by_subagent(&self) -> Result<Vec<(String, u32)>> {
        subagent_queries::count_tasks_by_subagent(self.conn())
    }
    /// Per-subagent last task: `(subagent_id, created_at_ms, status)`
    /// (spec §4 Phase 2 `subagents[].last_task_{created_at,status}`).
    pub fn subagent_last_task_by_subagent(&self) -> Result<Vec<(String, i64, String)>> {
        subagent_queries::last_task_by_subagent(self.conn())
    }
    /// Count tasks with `created_at_ms > since_ms` (compaction trigger (a)).
    pub fn subagent_count_tasks_since(&self, subagent_id: &str, since_ms: i64) -> Result<u32> {
        subagent_queries::count_tasks_since(self.conn(), subagent_id, since_ms)
    }
    /// Atomically pick the oldest queued task and flip it to "running"
    /// (Task 7 §8.3 step 2 dispatcher). `None` when no queued task is
    /// eligible (no queued rows OR the only queued rows belong to
    /// subagents that already have a running task — per-subagent FIFO).
    pub fn subagent_pick_oldest_queued_task(&self) -> rusqlite::Result<Option<SubagentTaskRow>> {
        subagent_queries::pick_oldest_queued_task(self.conn())
    }
    /// Whether the subagent has a task currently in `'running'` status.
    /// Used by `delegate()` for per-subagent serialization (spec §6.4).
    pub fn subagent_has_running_task(&self, subagent_id: &str) -> rusqlite::Result<bool> {
        subagent_queries::has_running_task(self.conn(), subagent_id)
    }
    /// Count subagent tasks by status. Returns (queued, running).
    pub fn subagent_task_counts_by_status(&self) -> Result<(u32, u32)> {
        crate::subagent_queries::task_counts_by_status(&self.conn)
    }
    /// Reap orphaned `running` tasks whose age exceeds their own timeout.
    /// See [`subagent_queries::reap_orphaned_running_tasks`].
    pub fn subagent_reap_orphaned_running_tasks(
        &self,
        now_ms: i64,
        default_timeout_seconds: i64,
        buffer_ms: i64,
    ) -> Result<Vec<String>> {
        subagent_queries::reap_orphaned_running_tasks(
            self.conn(),
            now_ms,
            default_timeout_seconds,
            buffer_ms,
        )
    }
    pub fn subagent_set_task_status(
        &self,
        id: &str,
        status: &str,
        result_summary: Option<String>,
        error: Option<String>,
    ) -> Result<()> {
        subagent_queries::set_task_status(self.conn(), id, status, result_summary, error)
    }

    /// Like `subagent_set_task_status` but refuses to overwrite a cancelled
    /// task. Used by the executor's terminal-write path. Returns `true` if
    /// the row was updated, `false` if the task was already cancelled.
    pub fn subagent_set_task_status_if_not_cancelled(
        &self,
        id: &str,
        status: &str,
        result_summary: Option<String>,
        error: Option<String>,
    ) -> Result<bool> {
        subagent_queries::set_task_status_if_not_cancelled(
            self.conn(),
            id,
            status,
            result_summary,
            error,
        )
    }
    /// Conditionally cancel a task (idempotent on terminal states).
    /// Returns `true` when the row was updated, `false` when the task was
    /// already terminal or not found. See
    /// [`subagent_queries::cancel_task_if_not_terminal`].
    pub fn subagent_cancel_task_if_not_terminal(
        &self,
        task_id: &str,
        reason: Option<&str>,
        now: i64,
    ) -> Result<bool> {
        subagent_queries::cancel_task_if_not_terminal(self.conn(), task_id, reason, now)
    }
    /// Set the classified `error_kind` on a task row (Task 5).
    pub fn subagent_set_task_error_kind(&self, id: &str, error_kind: Option<&str>) -> Result<()> {
        subagent_queries::set_task_error_kind(self.conn(), id, error_kind)
    }
    pub fn subagent_upsert_binding(&self, row: &SubagentHarnessBindingRow) -> Result<()> {
        subagent_queries::upsert_binding(self.conn(), row)
    }
    pub fn subagent_upsert_hot_binding(&self, row: &SubagentHarnessBindingRow) -> Result<()> {
        let conn = self.conn();
        subagent_queries::upsert_hot_binding(conn, row)
    }
    pub fn subagent_commit_hot_binding_and_status(
        &self,
        binding: &SubagentHarnessBindingRow,
        subagent_id: &str,
    ) -> Result<()> {
        let conn = self.conn();
        subagent_queries::commit_hot_binding_and_status(conn, binding, subagent_id)
    }
    pub fn subagent_commit_hibernate_binding_and_status(
        &self,
        binding: &SubagentHarnessBindingRow,
        subagent_id: &str,
        new_status: &str,
    ) -> Result<()> {
        let conn = self.conn();
        subagent_queries::commit_hibernate_binding_and_status(
            conn,
            binding,
            subagent_id,
            new_status,
        )
    }
    pub fn subagent_hot_binding(
        &self,
        subagent_id: &str,
        harness: &str,
    ) -> Result<Option<SubagentHarnessBindingRow>> {
        subagent_queries::hot_binding(self.conn(), subagent_id, harness)
    }
    pub fn subagent_find_hot_binding_by_session(
        &self,
        adapter_session_id: &str,
        harness: &str,
    ) -> Result<Option<SubagentHarnessBindingRow>> {
        let conn = self.conn();
        subagent_queries::find_hot_binding_by_session(conn, adapter_session_id, harness)
    }
    pub fn subagent_find_lru_hot_binding(
        &self,
        harness: &str,
    ) -> Result<Option<SubagentHarnessBindingRow>> {
        let conn = self.conn();
        subagent_queries::find_lru_hot_binding(conn, harness)
    }
    pub fn subagent_write_hot_summary(&self, subagent_id: &str, hot_summary: &str) -> Result<()> {
        let conn = self.conn();
        subagent_queries::write_hot_summary(conn, subagent_id, hot_summary)
    }
    /// Atomically commit an eviction: optional hot_summary write + binding
    /// flip + logical status computed from final memory state (§3.3).
    pub fn subagent_commit_eviction(
        &self,
        binding: &SubagentHarnessBindingRow,
        subagent_id: &str,
        hot_summary: Option<&str>,
    ) -> Result<String> {
        let conn = self.conn();
        subagent_queries::commit_eviction(conn, binding, subagent_id, hot_summary)
    }
    pub fn subagent_insert_usage_record(&self, row: &SubagentUsageRecordRow) -> Result<()> {
        subagent_queries::insert_usage_record(self.conn(), row)
    }
    pub fn subagent_insert_resource_event(&self, row: &SubagentResourceEventRow) -> Result<()> {
        subagent_queries::insert_resource_event(self.conn(), row)
    }
    pub fn subagent_list_resource_events(
        &self,
        target_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<SubagentResourceEventRow>> {
        subagent_queries::list_resource_events(self.conn(), target_id, limit)
    }
    pub fn subagent_reconcile_sidecar_crash(
        &self,
        harness: &str,
    ) -> Result<subagent_queries::CrashReconciliationCounts> {
        subagent_queries::reconcile_sidecar_crash(self.conn(), harness)
    }
    pub fn subagent_release_hot_bindings_for_shutdown(
        &self,
        harness: &str,
    ) -> Result<subagent_queries::ShutdownReconciliationCounts> {
        subagent_queries::release_hot_bindings_for_shutdown(self.conn(), harness)
    }
    pub fn subagent_list_filtered(
        &self,
        status: Option<&str>,
        project: Option<&str>,
        include_deleted: bool,
    ) -> Result<Vec<SubagentLogicalSubagentRow>> {
        subagent_queries::list_filtered(self.conn(), status, project, include_deleted)
    }
    pub fn subagent_hard_delete(&self, id: &str) -> Result<()> {
        subagent_queries::hard_delete_logical_subagent(self.conn(), id)
    }
}

/// Map a query row to a `NormalizedUsageEvent`.
///
/// Column indices follow the full-event SELECT (all 4 call sites share the
/// same column order). Indices 0-41 are the legacy columns; 42-44 are the
/// cache-metrics unified columns appended in Task 5's migration:
///   42 = provider_payload_shape (TEXT)
///   43 = prompt_input_total_tokens (INTEGER)
///   44 = prompt_input_non_cached_tokens (INTEGER)
fn row_to_usage_event(row: &rusqlite::Row<'_>) -> busytok_domain::NormalizedUsageEvent {
    let agent_str: String = row.get(1).unwrap_or_default();
    let agent = match agent_str.as_str() {
        "claude_code" => busytok_domain::AgentKind::ClaudeCode,
        "codex" => busytok_domain::AgentKind::Codex,
        _ => busytok_domain::AgentKind::ClaudeCode,
    };

    busytok_domain::NormalizedUsageEvent {
        id: row.get(0).unwrap_or_default(),
        agent,
        source_file_id: row.get(2).unwrap_or_default(),
        source_path: row.get(3).unwrap_or_default(),
        source_line: row.get::<_, i64>(4).unwrap_or(0) as u64,
        source_offset_start: row.get::<_, i64>(5).unwrap_or(0) as u64,
        source_offset_end: row.get::<_, i64>(6).unwrap_or(0) as u64,
        session_id: row.get(7).unwrap_or_default(),
        turn_id: row.get(8).unwrap_or_default(),
        source_request_id: row.get(9).unwrap_or_default(),
        message_id: row.get(10).unwrap_or_default(),
        timestamp_ms: row.get(11).unwrap_or(0),
        project_path: row.get(12).unwrap_or_default(),
        project_hash: row.get(13).unwrap_or_default(),
        cwd: row.get(14).unwrap_or_default(),
        model: row.get(15).unwrap_or_default(),
        model_provider: row.get(16).unwrap_or_default(),
        agent_version: row.get(17).unwrap_or_default(),
        client_kind: row.get(18).unwrap_or_default(),
        speed: row.get(19).unwrap_or_default(),
        input_tokens: row.get(20).unwrap_or(0),
        output_tokens: row.get(21).unwrap_or(0),
        total_tokens: row.get(22).unwrap_or(0),
        cached_input_tokens: row.get(23).unwrap_or(0),
        cache_creation_tokens: row.get(24).unwrap_or(0),
        cache_read_tokens: row.get(25).unwrap_or(0),
        reasoning_tokens: row.get(26).unwrap_or(0),
        thoughts_tokens: row.get(27).unwrap_or(0),
        tool_tokens: row.get(28).unwrap_or(0),
        cost_usd: row.get(29).unwrap_or_default(),
        estimated_cost_usd: row.get(30).unwrap_or_default(),
        cost_currency: row.get(31).unwrap_or_default(),
        cost_source: row.get(32).unwrap_or_default(),
        price_catalog_version: row.get(33).unwrap_or_default(),
        is_error: row.get::<_, i32>(34).unwrap_or(0) != 0,
        error_type: row.get(35).unwrap_or_default(),
        usage_limit_reset_time_ms: row.get(37).unwrap_or_default(),
        raw_event_hash: row.get(36).unwrap_or_default(),
        created_at_ms: row.get(38).unwrap_or(0),
        updated_at_ms: row.get(39).unwrap_or(0),
        is_sidechain: row.get::<_, i32>(40).unwrap_or(0) != 0,
        dedupe_key: row.get(41).unwrap_or_default(),
        provider_payload_shape: busytok_domain::cache_metrics::ProviderPayloadShape::parse(
            &row.get::<_, String>(42).unwrap_or_default(),
        ),
        prompt_input_total_tokens: row.get(43).unwrap_or(0),
        prompt_input_non_cached_tokens: row.get(44).unwrap_or(0),
    }
}

/// Validate that a date string matches YYYY-MM-DD format.
fn validate_date_format(date: &str) -> Result<()> {
    if date.len() != 10 {
        anyhow::bail!("date must be YYYY-MM-DD, got: {date}");
    }
    let bytes = date.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        anyhow::bail!("date must be YYYY-MM-DD, got: {date}");
    }
    let year: i32 = date[0..4]
        .parse()
        .with_context(|| format!("invalid year in date: {date}"))?;
    let month: u32 = date[5..7]
        .parse()
        .with_context(|| format!("invalid month in date: {date}"))?;
    let day: u32 = date[8..10]
        .parse()
        .with_context(|| format!("invalid day in date: {date}"))?;
    if !(2000..=2100).contains(&year) {
        anyhow::bail!("year out of range in date: {date}");
    }
    if !(1..=12).contains(&month) {
        anyhow::bail!("month out of range in date: {date}");
    }
    if !(1..=31).contains(&day) {
        anyhow::bail!("day out of range in date: {date}");
    }
    Ok(())
}
