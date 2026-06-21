CREATE TABLE log_sources (
  id TEXT PRIMARY KEY,
  agent TEXT NOT NULL,
  source_type TEXT NOT NULL,
  root_path TEXT NOT NULL,
  configured_by_user INTEGER NOT NULL DEFAULT 0,
  default_discovery_enabled INTEGER NOT NULL DEFAULT 1,
  status TEXT NOT NULL,
  last_scan_started_at_ms INTEGER,
  last_scan_completed_at_ms INTEGER,
  last_error TEXT,
  first_seen_at_ms INTEGER NOT NULL,
  last_seen_at_ms INTEGER NOT NULL,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
CREATE TABLE log_files (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL,
  agent TEXT NOT NULL,
  path TEXT NOT NULL,
  inode TEXT,
  size_bytes INTEGER NOT NULL DEFAULT 0,
  offset_bytes INTEGER NOT NULL DEFAULT 0,
  last_mtime_ms INTEGER,
  first_seen_at_ms INTEGER NOT NULL,
  last_seen_at_ms INTEGER NOT NULL,
  state TEXT NOT NULL,
  last_error TEXT,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
CREATE TABLE usage_events (
  id TEXT PRIMARY KEY,
  agent TEXT NOT NULL,
  source_file_id TEXT NOT NULL,
  source_path TEXT NOT NULL,
  source_line INTEGER NOT NULL,
  source_offset_start INTEGER NOT NULL,
  source_offset_end INTEGER NOT NULL,
  session_id TEXT NOT NULL,
  turn_id TEXT,
  source_request_id TEXT,
  message_id TEXT,
  timestamp_ms INTEGER NOT NULL,
  project_path TEXT,
  project_hash TEXT,
  cwd TEXT,
  model TEXT,
  model_provider TEXT,
  agent_version TEXT,
  client_kind TEXT,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  cached_input_tokens INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0,
  thoughts_tokens INTEGER NOT NULL DEFAULT 0,
  tool_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL,
  estimated_cost_usd REAL,
  cost_currency TEXT,
  cost_source TEXT NOT NULL DEFAULT 'unknown',
  price_catalog_version TEXT,
  is_error INTEGER NOT NULL DEFAULT 0,
  error_type TEXT,
  raw_event_hash TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  speed TEXT,
  usage_limit_reset_time_ms INTEGER,
  generation_id TEXT,
  dedupe_key TEXT
);
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  agent TEXT NOT NULL,
  project_hash TEXT,
  started_at_ms INTEGER,
  last_seen_at_ms INTEGER,
  model_list_json TEXT NOT NULL DEFAULT '[]',
  total_tokens INTEGER NOT NULL DEFAULT 0,
  total_cost_usd REAL,
  event_count INTEGER NOT NULL DEFAULT 0,
  is_active INTEGER NOT NULL DEFAULT 0,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
CREATE TABLE projects (
  id TEXT PRIMARY KEY,
  project_hash TEXT NOT NULL,
  project_path TEXT,
  agent TEXT,
  display_name TEXT,
  first_seen_at_ms INTEGER NOT NULL,
  last_seen_at_ms INTEGER NOT NULL,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  total_cost_usd REAL,
  session_count INTEGER NOT NULL DEFAULT 0,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
CREATE TABLE tool_events (
  id TEXT PRIMARY KEY,
  agent TEXT NOT NULL,
  session_id TEXT NOT NULL,
  message_id TEXT,
  tool_name TEXT NOT NULL,
  status TEXT,
  timestamp_ms INTEGER,
  project_hash TEXT,
  created_at_ms INTEGER NOT NULL,
  source_file_id TEXT NOT NULL DEFAULT '',
  source_path TEXT NOT NULL DEFAULT '',
  source_line INTEGER NOT NULL DEFAULT 0,
  source_offset_start INTEGER NOT NULL DEFAULT 0,
  source_offset_end INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE diagnostic_events (
  id TEXT PRIMARY KEY,
  agent TEXT,
  source_id TEXT,
  source_file_id TEXT,
  source_path TEXT,
  source_line INTEGER,
  severity TEXT NOT NULL,
  code TEXT NOT NULL,
  message TEXT NOT NULL,
  details_json TEXT,
  happened_at_ms INTEGER NOT NULL,
  created_at_ms INTEGER NOT NULL
);
CREATE TABLE daily_usage (
  date TEXT NOT NULL,
  timezone TEXT NOT NULL,
  agent TEXT NOT NULL,
  project_hash TEXT NOT NULL DEFAULT '',
  model TEXT NOT NULL DEFAULT '',
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  cached_input_tokens INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0,
  thoughts_tokens INTEGER NOT NULL DEFAULT 0,
  tool_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL,
  estimated_cost_usd REAL,
  event_count INTEGER NOT NULL DEFAULT 0,
  generation_id TEXT NOT NULL,
  PRIMARY KEY (date, timezone, agent, project_hash, model, generation_id)
);
CREATE TABLE model_usage (
  model TEXT NOT NULL,
  agent TEXT NOT NULL,
  timezone TEXT NOT NULL,
  date TEXT NOT NULL DEFAULT '',
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  cached_input_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL,
  event_count INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (model, agent, timezone, date)
);
CREATE TABLE realtime_summary (
  key TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
CREATE TABLE codex_token_snapshots (
  id TEXT PRIMARY KEY,
  source_file_id TEXT NOT NULL,
  source_line INTEGER NOT NULL,
  source_offset_start INTEGER NOT NULL,
  source_offset_end INTEGER NOT NULL,
  session_id TEXT NOT NULL,
  turn_id TEXT,
  token_event_ordinal INTEGER NOT NULL,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  cached_input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  raw_usage_json TEXT NOT NULL,
  emitted_event_id TEXT,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  model TEXT
);
CREATE INDEX idx_usage_events_time ON usage_events(timestamp_ms);
CREATE INDEX idx_usage_events_agent_time ON usage_events(agent, timestamp_ms);
CREATE INDEX idx_usage_events_project_hash_time ON usage_events(project_hash, timestamp_ms);
CREATE INDEX idx_usage_events_model_time ON usage_events(model, timestamp_ms);
CREATE INDEX idx_usage_events_session ON usage_events(session_id);
CREATE INDEX idx_usage_events_source_request ON usage_events(source_request_id);
CREATE INDEX idx_usage_events_message ON usage_events(message_id);
CREATE TABLE model_summary (
  model TEXT PRIMARY KEY,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  total_cost_usd REAL,
  event_count INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_usage_events_time_id_desc
    ON usage_events(timestamp_ms DESC, id DESC);
CREATE INDEX idx_tool_events_session ON tool_events(session_id);
CREATE INDEX idx_tool_events_timestamp ON tool_events(timestamp_ms);
CREATE INDEX idx_diagnostic_events_code ON diagnostic_events(code);
CREATE INDEX idx_diagnostic_events_severity ON diagnostic_events(severity);
CREATE INDEX idx_diagnostic_events_happened_at ON diagnostic_events(happened_at_ms);
CREATE INDEX idx_codex_snapshots_model_lookup
ON codex_token_snapshots(source_file_id, created_at_ms DESC, token_event_ordinal DESC)
WHERE model IS NOT NULL AND model != '';
CREATE TABLE audit_generations (
  generation_id TEXT PRIMARY KEY,
  state TEXT NOT NULL,          -- 'building', 'promoted', 'failed'
  started_at_ms INTEGER NOT NULL,
  promoted_at_ms INTEGER,
  is_active INTEGER NOT NULL DEFAULT 0,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
CREATE TABLE source_file_checkpoints (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL,
  agent TEXT NOT NULL,
  path TEXT NOT NULL,
  inode TEXT,
  offset_bytes INTEGER NOT NULL DEFAULT 0,
  size_bytes INTEGER NOT NULL DEFAULT 0,
  last_mtime_ms INTEGER,
  state TEXT NOT NULL DEFAULT 'active',
  last_error TEXT,
  first_seen_at_ms INTEGER NOT NULL,
  last_seen_at_ms INTEGER NOT NULL,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
CREATE TABLE generation_file_observations (
  generation_id TEXT NOT NULL,
  source_file_id TEXT NOT NULL,
  observed_at_ms INTEGER NOT NULL,
  offset_bytes INTEGER NOT NULL DEFAULT 0,
  size_bytes INTEGER NOT NULL DEFAULT 0,
  last_mtime_ms INTEGER,
  scan_status TEXT,
  scan_errors TEXT,
  PRIMARY KEY (generation_id, source_file_id)
);
CREATE TABLE tail_replay_queue (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  source_file_id TEXT NOT NULL,
  event_seq INTEGER NOT NULL,
  event_data_json TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  last_attempt_at_ms INTEGER,
  status TEXT NOT NULL DEFAULT 'pending',
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
CREATE TABLE event_sequence_state (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  latest_event_seq INTEGER NOT NULL DEFAULT 0,
  latest_event_timestamp_ms INTEGER,
  updated_at_ms INTEGER NOT NULL
);
CREATE TABLE service_state (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  writer_queue_depth INTEGER NOT NULL DEFAULT 0,
  aggregate_lag_ms INTEGER NOT NULL DEFAULT 0,
  readiness TEXT,
  active_generation_id TEXT,
  last_exact_rebuild_at_ms INTEGER,
  updated_at_ms INTEGER NOT NULL,
  read_model_watermark_ms INTEGER,
  read_model_status TEXT NOT NULL DEFAULT 'unknown',
  last_successful_read_model_rebuild_at_ms INTEGER,
  consistency_check_status TEXT NOT NULL DEFAULT 'unknown'
);
CREATE TABLE outbox_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  event_seq INTEGER NOT NULL,
  envelope_json TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL
);
CREATE INDEX idx_outbox_log_event_seq ON outbox_log(event_seq);
CREATE INDEX idx_outbox_log_id ON outbox_log(id);
CREATE INDEX idx_audit_generations_active
  ON audit_generations(is_active);
CREATE INDEX idx_audit_generations_state
  ON audit_generations(state);
CREATE INDEX idx_audit_generations_time
  ON audit_generations(started_at_ms);
CREATE INDEX idx_source_file_checkpoints_source
  ON source_file_checkpoints(source_id);
CREATE INDEX idx_generation_file_obs_gen
  ON generation_file_observations(generation_id);
CREATE INDEX idx_generation_file_obs_file
  ON generation_file_observations(source_file_id);
CREATE INDEX idx_tail_replay_queue_status
  ON tail_replay_queue(status, created_at_ms);
CREATE INDEX idx_tail_replay_queue_file
  ON tail_replay_queue(source_file_id);
CREATE UNIQUE INDEX idx_event_sequence_state_singleton
  ON event_sequence_state(id);
CREATE UNIQUE INDEX idx_usage_events_generation_dedupe
  ON usage_events(generation_id, dedupe_key);
CREATE INDEX idx_diagnostic_events_retention
  ON diagnostic_events(created_at_ms);
CREATE TABLE prompt_entries (
    id              TEXT PRIMARY KEY,
    content         TEXT NOT NULL,
    content_normalized TEXT NOT NULL,
    alias           TEXT,
    alias_normalized TEXT,
    is_pinned       INTEGER NOT NULL DEFAULT 0,
    usage_count     INTEGER NOT NULL DEFAULT 0,
    last_used_at_ms INTEGER,
    created_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_prompt_alias_unique
    ON prompt_entries(alias_normalized) WHERE alias_normalized IS NOT NULL;
CREATE INDEX idx_prompt_entries_updated_at ON prompt_entries(updated_at_ms DESC);
CREATE INDEX idx_prompt_entries_last_used_at ON prompt_entries(last_used_at_ms DESC);
CREATE INDEX idx_prompt_entries_pinned_last_used
    ON prompt_entries(is_pinned DESC, last_used_at_ms DESC);
CREATE TABLE prompt_entry_tags (
    prompt_entry_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    tag_normalized TEXT NOT NULL,
    FOREIGN KEY(prompt_entry_id) REFERENCES prompt_entries(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX idx_prompt_entry_tags_unique
    ON prompt_entry_tags(prompt_entry_id, tag_normalized);
CREATE INDEX idx_prompt_entry_tags_lookup
    ON prompt_entry_tags(tag_normalized, prompt_entry_id);
CREATE TABLE prompt_entry_uses (
    id TEXT PRIMARY KEY,
    prompt_entry_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('copy', 'paste')),
    surface TEXT NOT NULL CHECK(surface IN ('overlay', 'page')),
    outcome TEXT NOT NULL CHECK(outcome IN ('copy', 'paste_attempted', 'paste_fell_back_to_copy')),
    failure_reason TEXT CHECK(failure_reason IN ('permission_missing', 'focus_lost', 'injection_failed', 'unsupported_platform')),
    used_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    FOREIGN KEY(prompt_entry_id) REFERENCES prompt_entries(id) ON DELETE CASCADE
);
CREATE INDEX idx_prompt_entry_uses_entry_time
    ON prompt_entry_uses(prompt_entry_id, used_at_ms DESC);
CREATE TABLE usage_buckets_2s (
  generation_id TEXT NOT NULL,
  bucket_start_ms INTEGER NOT NULL,
  agent TEXT NOT NULL DEFAULT '',
  model TEXT NOT NULL DEFAULT '',
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL,
  cost_status TEXT NOT NULL DEFAULT 'unknown',
  event_count INTEGER NOT NULL DEFAULT 0,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (generation_id, bucket_start_ms, agent, model)
);
CREATE TABLE usage_buckets_hour (
  generation_id TEXT NOT NULL,
  bucket_start_ms INTEGER NOT NULL,
  agent TEXT NOT NULL DEFAULT '',
  model TEXT NOT NULL DEFAULT '',
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL,
  cost_status TEXT NOT NULL DEFAULT 'unknown',
  event_count INTEGER NOT NULL DEFAULT 0,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (generation_id, bucket_start_ms, agent, model)
);
CREATE TABLE usage_buckets_day (
  generation_id TEXT NOT NULL,
  bucket_start_ms INTEGER NOT NULL,
  agent TEXT NOT NULL DEFAULT '',
  model TEXT NOT NULL DEFAULT '',
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL,
  cost_status TEXT NOT NULL DEFAULT 'unknown',
  event_count INTEGER NOT NULL DEFAULT 0,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (generation_id, bucket_start_ms, agent, model)
);
CREATE TABLE usage_by_project_day (
  generation_id TEXT NOT NULL,
  date TEXT NOT NULL,
  project_id TEXT NOT NULL,
  project_path TEXT,
  agent TEXT NOT NULL DEFAULT '',
  model TEXT NOT NULL DEFAULT '',
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL,
  cost_status TEXT NOT NULL DEFAULT 'unknown',
  event_count INTEGER NOT NULL DEFAULT 0,
  last_active_at_ms INTEGER,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (generation_id, date, project_id, agent, model)
);
CREATE TABLE usage_by_model_day (
  generation_id TEXT NOT NULL,
  date TEXT NOT NULL,
  agent TEXT NOT NULL DEFAULT '',
  model TEXT NOT NULL DEFAULT '',
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL,
  cost_status TEXT NOT NULL DEFAULT 'unknown',
  event_count INTEGER NOT NULL DEFAULT 0,
  last_active_at_ms INTEGER,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (generation_id, date, agent, model)
);
CREATE TABLE usage_by_session_day (
  generation_id TEXT NOT NULL,
  date TEXT NOT NULL,
  session_id TEXT NOT NULL,
  agent TEXT NOT NULL DEFAULT '',
  client_kind TEXT,
  project_path TEXT,
  project_hash TEXT,
  model TEXT,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL,
  cost_status TEXT NOT NULL DEFAULT 'unknown',
  event_count INTEGER NOT NULL DEFAULT 0,
  last_active_at_ms INTEGER,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (generation_id, date, session_id, agent)
);
CREATE TABLE usage_by_client_day (
  generation_id TEXT NOT NULL,
  date TEXT NOT NULL,
  client_kind TEXT NOT NULL,
  agent TEXT NOT NULL DEFAULT '',
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL,
  cost_status TEXT NOT NULL DEFAULT 'unknown',
  event_count INTEGER NOT NULL DEFAULT 0,
  last_active_at_ms INTEGER,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (generation_id, date, client_kind, agent)
);
CREATE TABLE source_health_summary (
  generation_id TEXT NOT NULL,
  source_id TEXT NOT NULL,
  agent TEXT NOT NULL,
  root_path TEXT NOT NULL,
  source_type TEXT NOT NULL,
  status TEXT NOT NULL,
  configured_by_user INTEGER NOT NULL DEFAULT 0,
  last_scan_at_ms INTEGER,
  file_count INTEGER NOT NULL DEFAULT 0,
  parsed_file_count INTEGER NOT NULL DEFAULT 0,
  event_count INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  latest_activity_at_ms INTEGER,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (generation_id, source_id)
);
CREATE INDEX idx_usage_buckets_2s_range ON usage_buckets_2s(generation_id, bucket_start_ms);
CREATE INDEX idx_usage_buckets_hour_range ON usage_buckets_hour(generation_id, bucket_start_ms);
CREATE INDEX idx_usage_buckets_day_range ON usage_buckets_day(generation_id, bucket_start_ms);
CREATE INDEX idx_usage_by_project_day_range ON usage_by_project_day(generation_id, date, total_tokens DESC);
CREATE INDEX idx_usage_by_model_day_range ON usage_by_model_day(generation_id, date, total_tokens DESC);
CREATE INDEX idx_usage_by_session_day_range ON usage_by_session_day(generation_id, date, last_active_at_ms DESC);
CREATE INDEX idx_usage_by_client_day_range ON usage_by_client_day(generation_id, date, total_tokens DESC);
CREATE INDEX idx_source_health_summary_order ON source_health_summary(generation_id, last_scan_at_ms DESC, source_id DESC);
