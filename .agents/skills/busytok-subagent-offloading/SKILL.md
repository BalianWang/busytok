---
name: busytok-subagent-offloading
description: Use when a coding agent needs to offload a subtask through Busytok, select from its live model catalog, bind a logical subagent, or retrieve a delegated task result.
---

# Busytok Subagent Offloading

## Overview

Busytok provides task-routing primitives; the calling agent chooses the model. Use JSON on stdout, bind or reuse logical subagents deliberately, and determine outcome from task `status`, not only process exit code.

## When to Use

- Offloading code review, repository search, planning, or writing
- Selecting a configured model from tags, reasoning, and context capacity
- Inspecting a task that is `queued`, `running`, `failed`, or `cancelled`

Do not use this skill to choose a model-selection policy.

## Protocol

1. Verify availability, then read the enabled live catalog. If no candidate matches, stop and report the configuration/routing failure; never blindly select `.[0]`.

```bash
busytok status
busytok models --tag <TAG> --reasoning --sort context_window_desc --json
```

`status` always emits JSON on stdout — no `--json` or `--output` flag needed. Tags are free-form, case-sensitive strings defined per model — discover yours with `busytok models --json` before filtering. `--tag` is repeatable with AND semantics. Valid `--sort` values: `name` (default), `context_window_desc`, `max_tokens_desc`.

Route with `provider_id` + `model_id`; use `tags`, `reasoning`, `context_window`, and `max_tokens` as selection inputs. Define a deterministic policy yourself: catalog order is not "latest" or "best". `models` already excludes disabled entries unless `--all` is passed.

2. For a new binding, use a unique name that describes the stable role/binding, not a generic task title. For automation, pass both bind fields and an explicit reuse policy.

```bash
busytok delegate \
  --subagent "reviewer-<MODEL_ID>-<UNIQUE_SUFFIX>" \
  --profile "pi/review-cheap" \
  --bind-provider "<PROVIDER_ID>" \
  --bind-model "<MODEL_ID>" \
  --reuse-policy create \
  --output json \
  --prompt-file "<PROMPT_FILE>" \
  --wait \
  --wait-timeout 120
```

`<PROMPT_FILE>` should contain the full instructions, e.g. "Review the scoped patch. Return ranked findings with file, line, impact, and evidence."

Profiles: `pi/review-cheap` (review), `pi/search-cheap` (search), `pi/plan-cheap` (planning).

3. Choose one completion mode. For bounded short work, prefer `delegate --wait --wait-timeout <SECONDS> --poll-interval <SECONDS>`; timeout returns the last known task JSON and exits `124`. Default poll interval is 2s. For longer orchestration, read the initial `.status`, then poll `task_id` until your own deadline. If you abandon the task, cancel it explicitly.

```bash
busytok subagent task --task-id "<TASK_ID>" --output json
busytok subagent cancel --task-id "<TASK_ID>" --reason "caller deadline exceeded" --output json
```

## Binding and Output Rules

- `--model` is a one-task override; it is not `--bind-model`.
- `--reuse-policy create|reuse|fail` controls whether an existing name may be reused. Reusing a `--subagent` name does not rebind it.
- `--bind-model` without `--bind-provider` works only if the model ID is unique. Automation should pass both catalog fields.
- `delegate --output json` returns `task_id` / `summary` plus `created`; `subagent task` and `delegate --wait` return `id` / `result_summary`.
- Only `completed` is success. On `failed`/`cancelled`, read and surface `error` and `error_kind` as well as the summary.
- JSON is stdout; diagnostics are stderr. Never merge streams with `2>&1`, and do not discard stderr by default.
- `models` uses `--json` (boolean); `delegate` and all `subagent` subcommands use `--output json` (string enum). Do not mix them.
- Prompt input is exactly one of: positional prompt, `--prompt-file`, `--stdin`, or `--artifact-ref`. Prefer `--prompt-file` or `--artifact-ref` for large or untrusted multi-line inputs.

## Reference

- [Integration guide](/Users/wsd/Data/Busytok/busytok/docs/superpowers/guides/busytok-subagent-codex-integration.md)
