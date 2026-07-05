-- Provider catalog: providers / models / model_tags
-- Replaces settings.toml provider persistence + keychain credential storage.

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
