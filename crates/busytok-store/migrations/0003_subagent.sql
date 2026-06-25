-- Logical subagent runtime schema (Step 1).
-- See docs/superpowers/specs/2026-06-25-busytok-pi-sidecar-logical-subagent-design.md

CREATE TABLE subagent_logical_subagents (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    project_id TEXT NOT NULL,
    repo_path TEXT NOT NULL,
    repo_hash TEXT NOT NULL,
    branch TEXT,
    intent TEXT,
    default_profile TEXT NOT NULL,
    default_model TEXT,
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
