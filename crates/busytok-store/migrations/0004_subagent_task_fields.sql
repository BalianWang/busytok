-- 0004_subagent_task_fields.sql
-- Round 3 Finding 3 fix: incremental migration for new task row columns.
-- Do NOT modify 0003_subagent.sql — existing DBs need the ALTER TABLE.

ALTER TABLE subagent_tasks ADD COLUMN timeout_seconds INTEGER;
ALTER TABLE subagent_tasks ADD COLUMN model_override TEXT;
