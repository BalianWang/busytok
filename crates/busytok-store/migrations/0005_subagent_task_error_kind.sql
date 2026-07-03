-- 0005_subagent_task_error_kind.sql
-- Phase 3 Task 5: persist the classified error kind on the task row so
-- downstream consumers (Activity page, error analytics) can filter/group
-- subagent task failures without re-parsing the error string. Mirrors the
-- `TaskErrorKind` enum in `busytok-subagent::models` (snake_case serialization).
-- Do NOT modify 0003_subagent.sql — existing DBs need the ALTER TABLE.

ALTER TABLE subagent_tasks ADD COLUMN error_kind TEXT;
