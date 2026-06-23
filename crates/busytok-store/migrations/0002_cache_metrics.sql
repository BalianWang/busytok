-- v2: unified cache-metrics columns on usage_events. Additive; NO backfill and
-- NO stale-row mixing — existing dev DBs are RESET (delete the SQLite file;
-- busytok rescans logs on relaunch and rebuilds all rows with correct unified
-- fields). DEFAULT 'codex' matches NormalizedUsageEvent::minimal_for_test; the
-- defaults exist only so the columns are non-null during migration itself.
ALTER TABLE usage_events ADD COLUMN provider_payload_shape TEXT NOT NULL DEFAULT 'codex';
ALTER TABLE usage_events ADD COLUMN prompt_input_total_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE usage_events ADD COLUMN prompt_input_non_cached_tokens INTEGER NOT NULL DEFAULT 0;
