---
name: busytok-subagent-offloading
description: Use when a coding agent needs to offload a subtask through Busytok, select from its live model catalog, bind a logical subagent, or retrieve a delegated task result.
---

# Busytok Subagent Offloading

## Overview

Busytok is the task-routing/lifecycle boundary: choose a model, provide
complete context, and judge the task result—not process exit code alone.

## Explicit invocation contract

If the user explicitly invokes this skill, perform one real `busytok delegate`
for the requested subtask and report its outcome. Reading or discussing
delegation does not count. If blocked, report the command, relevant
stdout/stderr, and next action; do not silently work locally.

Close the lifecycle: wait or poll `task_id` to a deadline; if abandoned, cancel
and report the cancellation result.

## Protocol

1. Verify service readiness and read the enabled catalog. Require `ready: true`
   and a matching enabled provider/model. Otherwise report a
   configuration/routing blocker—never blindly use `.[0]`.

   ```bash
   busytok status
   busytok models --tag <TAG> --reasoning --sort context_window_desc --json
   ```

   `status` uses JSON stdout without a flag. `models` accepts repeatable,
   case-sensitive `--tag`, `--reasoning`, the documented sort keys, and excludes
   disabled entries unless `--all` is passed.

2. Prepare a complete prompt with task, scope, acceptance criteria, files/SHAs,
   read/write permission, tests, and result format. Use exactly one of
   positional prompt, `--prompt-file`, `--stdin`, or `--artifact-ref`; prefer a
   file or artifact for large/untrusted input.

3. Delegate with an absolute `--cwd`, explicit provider/model binding, a
   role-oriented unique name, and an explicit reuse policy.

   ```bash
   busytok delegate --cwd "$REPO_ROOT" \
     --subagent "reviewer-<MODEL_ID>-<UNIQUE_SUFFIX>" \
     --profile "pi/review-cheap" \
     --bind-provider "<PROVIDER_ID>" --bind-model "<MODEL_ID>" \
     --reuse-policy create --output json --prompt-file "$PROMPT_FILE" \
     --wait --wait-timeout 120 --poll-interval 2
   ```

   `--bind-provider` accepts a provider UUID or enabled provider name; use the
   UUID in automation. `--timeout` is the task runtime limit; `--wait-timeout`
   is only the caller's polling deadline. `--intent` is an optional durable
   long-term goal; keep per-task instructions in the prompt.

4. Interpret the result. Only `completed` is success. For `failed` or
   `cancelled`, surface `error`, `error_kind`, and summary. For `queued` or
   `running`, inspect `queue_reason` when present, then poll:

   ```bash
   busytok subagent task --task-id "<TASK_ID>" --output json
   busytok subagent cancel --task-id "<TASK_ID>" \
     --reason "caller deadline exceeded" --output json
   ```

## Binding, safety, and output rules

- `create`: fail if the name exists; otherwise create. `fail` aliases `create`.
  `reuse`: fail if absent; otherwise reuse without rebinding. Omitted means
  create-or-reuse, with bound-field mismatch rejected.
- `--model` is a one-task override, not `--bind-model`; reusing a name never
  rebinds it.
- JSON is stdout and diagnostics are stderr; never use `2>&1`. Without
  `--wait`, delegate returns `task_id`/`summary`; with `--wait` and via
  `subagent task`, it returns task-detail `id`/`result_summary`.
- Review/search tasks are read-only by default. Mutating tasks require an
  isolated branch/worktree, no concurrent writes to one worktree, and no direct
  writes to `main`; verify tests, diff, and commit identity independently.
- For implementation plans, use separate implementer, spec-reviewer, and
  quality-reviewer identities; fix findings and re-review at each gate.

## Reference

- [Integration guide](../../docs/superpowers/guides/busytok-subagent-codex-integration.md)
