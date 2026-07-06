-- 0005_provider_catalog_and_subagent_route_binding.sql
-- Consolidated v5 migration: provider catalog + subagent route binding +
-- model metadata (merges former 0005/0006/0007 into a single migration).
-- Executed atomically within one transaction by the migrator.
--
-- Sections:
--   A. Provider catalog tables: providers / models / model_tags (former 0006)
--   B. Model metadata columns (former 0007 §1)
--   C. Rebuild subagent_logical_subagents + child tables with NOT NULL
--      bound_provider_id / bound_model_id (former 0007 §2-4)
--
-- Note: error_kind (former 0005) is included inline in the CREATE TABLE
-- subagent_tasks definition in section C, not as a separate ALTER TABLE,
-- because section C drops and recreates the table.

-- ── A. Provider catalog ───────────────────────────────────────────────────

CREATE TABLE providers (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  provider_kind TEXT NOT NULL DEFAULT 'openai_compatible',
  base_url TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  api_key TEXT,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);

CREATE TABLE models (
  id TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL,
  model_id TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE,
  UNIQUE (provider_id, model_id)
);

CREATE TABLE model_tags (
  model_id TEXT NOT NULL,
  tag TEXT NOT NULL,
  FOREIGN KEY (model_id) REFERENCES models(id) ON DELETE CASCADE,
  UNIQUE (model_id, tag)
);

CREATE INDEX idx_models_provider_id ON models(provider_id);
CREATE INDEX idx_model_tags_tag ON model_tags(tag);

-- ── B. Model metadata columns ─────────────────────────────────────────────

ALTER TABLE models ADD COLUMN display_name TEXT;
ALTER TABLE models ADD COLUMN reasoning INTEGER NOT NULL DEFAULT 0;
ALTER TABLE models ADD COLUMN context_window INTEGER;
ALTER TABLE models ADD COLUMN max_tokens INTEGER;

-- Backfill sane defaults for nullable metadata columns. NOT NULL constraints
-- are not added because SQLite cannot add NOT NULL to existing columns
-- without a table rebuild. The application layer fails fast on NULL metadata.
UPDATE models SET context_window = 8192 WHERE context_window IS NULL;
UPDATE models SET max_tokens = 4096 WHERE max_tokens IS NULL;

-- ── C. Rebuild subagent_logical_subagents with bound fields ───────────────

DROP TABLE IF EXISTS subagent_usage_records;
DROP TABLE IF EXISTS subagent_harness_bindings;
DROP TABLE IF EXISTS subagent_tasks;
DROP TABLE IF EXISTS subagent_memory;

DROP TABLE IF EXISTS subagent_logical_subagents;

CREATE TABLE subagent_logical_subagents (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    project_id TEXT NOT NULL,
    repo_path TEXT NOT NULL,
    repo_hash TEXT NOT NULL,
    branch TEXT,
    intent TEXT,
    default_profile TEXT NOT NULL,
    bound_provider_id TEXT NOT NULL,
    bound_model_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'cold'
        CHECK (status IN ('hot', 'warm', 'cold', 'deleted')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    last_active_at_ms INTEGER
);

CREATE INDEX idx_subagent_logical_project
    ON subagent_logical_subagents(project_id, repo_hash, status);
CREATE INDEX idx_subagent_logical_last_active
    ON subagent_logical_subagents(last_active_at_ms);
CREATE UNIQUE INDEX idx_subagent_unique_active_name
    ON subagent_logical_subagents(project_id, repo_hash, name)
    WHERE status != 'deleted';

CREATE TABLE subagent_memory (
    id TEXT PRIMARY KEY,
    subagent_id TEXT NOT NULL UNIQUE,
    hot_summary TEXT,
    long_summary TEXT,
    key_files_json TEXT,
    decisions_json TEXT,
    attempts_json TEXT,
    open_questions_json TEXT,
    artifact_refs_json TEXT,
    last_compacted_at_ms INTEGER,
    last_compacted_task_id TEXT,
    updated_at_ms INTEGER NOT NULL,
    FOREIGN KEY (subagent_id) REFERENCES subagent_logical_subagents(id)
);

CREATE TABLE subagent_tasks (
    id TEXT PRIMARY KEY,
    subagent_id TEXT NOT NULL,
    source_harness TEXT,
    source_session_id TEXT,
    intent TEXT,
    profile TEXT NOT NULL,
    prompt TEXT,
    prompt_artifact_ref TEXT,
    output_schema_name TEXT,
    output_schema_version INTEGER DEFAULT 1,
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
    result_summary TEXT,
    result_json TEXT,
    error TEXT,
    timeout_seconds INTEGER,
    model_override TEXT,
    error_kind TEXT,
    created_at_ms INTEGER NOT NULL,
    started_at_ms INTEGER,
    completed_at_ms INTEGER,
    CHECK (prompt IS NOT NULL OR prompt_artifact_ref IS NOT NULL),
    FOREIGN KEY (subagent_id) REFERENCES subagent_logical_subagents(id)
);

CREATE INDEX idx_subagent_tasks_subagent ON subagent_tasks(subagent_id, created_at_ms);
CREATE INDEX idx_subagent_tasks_status ON subagent_tasks(status, created_at_ms);
CREATE INDEX idx_subagent_tasks_source ON subagent_tasks(source_harness, source_session_id);

CREATE TABLE subagent_harness_bindings (
    id TEXT PRIMARY KEY,
    subagent_id TEXT NOT NULL,
    harness TEXT NOT NULL,
    adapter_session_id TEXT,
    adapter_process_id TEXT,
    is_hot INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'warm'
        CHECK (status IN ('hot', 'warm', 'closed', 'crashed')),
    created_at_ms INTEGER NOT NULL,
    last_used_at_ms INTEGER,
    closed_at_ms INTEGER,
    detail_json TEXT,
    FOREIGN KEY (subagent_id) REFERENCES subagent_logical_subagents(id)
);

CREATE UNIQUE INDEX idx_subagent_binding_one_hot
    ON subagent_harness_bindings(subagent_id, harness)
    WHERE is_hot = 1;
CREATE INDEX idx_subagent_bindings_hot
    ON subagent_harness_bindings(subagent_id, is_hot);

CREATE TABLE subagent_usage_records (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    subagent_id TEXT NOT NULL,
    source_usage_event_id TEXT,
    harness TEXT NOT NULL,
    provider TEXT,
    model TEXT,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_read_tokens INTEGER,
    cache_write_tokens INTEGER,
    total_cost_usd REAL,
    duration_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    FOREIGN KEY (task_id) REFERENCES subagent_tasks(id),
    FOREIGN KEY (subagent_id) REFERENCES subagent_logical_subagents(id)
);

CREATE INDEX idx_subagent_usage_task ON subagent_usage_records(task_id);

DROP TABLE IF EXISTS subagent_resource_events;

CREATE TABLE subagent_resource_events (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    target_id TEXT,
    rss_mb REAL,
    cpu_percent REAL,
    detail_json TEXT,
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX idx_subagent_events_type ON subagent_resource_events(event_type, created_at_ms);
CREATE INDEX idx_subagent_events_target ON subagent_resource_events(target_id, created_at_ms);
