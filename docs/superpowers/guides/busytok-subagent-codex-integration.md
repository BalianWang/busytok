# Busytok Subagent Delegation — AI Agent Integration Guide

## Overview

Busytok exposes a JSON-over-stdout CLI that lets coding agents such as Codex
and Claude Code offload subtasks to Busytok subagents. The robust automation
flow is:

1. Verify the service is ready
2. Read the live model catalog
3. Select a model with an explicit policy
4. Delegate with explicit binding or reuse semantics
5. Wait, poll, or cancel using task-level state

Structured JSON is written to stdout. Diagnostics are written to stderr.
Parse stdout only. Do not merge streams with `2>&1`, and do not discard
stderr by default in automation.

## Prerequisites

The Busytok desktop app must be installed and the service must be running:

```bash
busytok status
```

At least one provider with an API key and at least one enabled model must be
configured. Providers can be added in the GUI or by CLI:

```bash
busytok provider add \
  --name "MyProvider" \
  --kind "openai_compatible" \
  --url "https://api.example.com/v1" \
  --key "sk-..." \
  --model "my-model"
```

## Step 1 — Discover the Live Catalog

Read the enabled model catalog in JSON:

```bash
busytok models --json
```

For coding-oriented work, a common starting filter is:

```bash
busytok models --tag Coding --reasoning --sort context_window_desc --json
```

Example entry:

```json
[
  {
    "provider_id": "5e3a4034-f1fd-4d50-a092-54022adbfa3e",
    "provider_name": "Deepseek_openai",
    "provider_kind": "openai_compatible",
    "provider_enabled": true,
    "model_db_id": "ba71ba9a-c696-4fd0-91ec-ac4c036beba1",
    "model_id": "deepseek-v4-pro",
    "model_enabled": true,
    "tags": ["Coding", "Writing"],
    "display_name": "deepseek-v4-pro",
    "reasoning": true,
    "context_window": 1000000,
    "max_tokens": 10000
  }
]
```

Fields used for delegation:

- `provider_id` → `--bind-provider`
- `model_id` → `--bind-model`

Useful selection inputs:

- `tags`
- `reasoning`
- `context_window`
- `max_tokens`

Do not assume catalog order means “best”, “latest”, or “recommended”.
Agents should define their own deterministic selection policy. If no matching
candidate exists, stop and surface a configuration or routing failure instead
of blindly selecting `.[0]`.

## Step 2 — Select a Model Deliberately

Example heuristics:

| Task type | Prefer |
|-----------|--------|
| Code review, debugging, patch generation | `tags` contains `Coding`, `reasoning = true` |
| Repo search, lightweight extraction | smaller `max_tokens`, `reasoning = false` acceptable |
| Deep synthesis across large diffs | larger `context_window`, `reasoning = true` |
| Writing or summarization | `tags` contains `Writing` |

Automation should usually pass both `provider_id` and `model_id`, even if
`--bind-model` alone would currently resolve uniquely.

## Step 3 — Delegate a Task

### Prompt Channels

Exactly one prompt source must be provided:

- positional prompt
- `--prompt-file <PATH>`
- `--stdin`
- `--artifact-ref <REF>`

For large or untrusted multi-line content, prefer `--prompt-file` or
`--artifact-ref`.

### New Binding

For a new logical subagent, use a stable role-oriented name and an explicit
reuse policy:

```bash
busytok delegate \
  --subagent "reviewer-deepseek-v4-pro-001" \
  --profile "pi/review-cheap" \
  --bind-provider "5e3a4034-f1fd-4d50-a092-54022adbfa3e" \
  --bind-model "deepseek-v4-pro" \
  --reuse-policy create \
  --output json \
  --prompt-file "/tmp/review-prompt.txt"
```

Common profiles:

- `pi/review-cheap`
- `pi/search-cheap`
- `pi/plan-cheap`

Important flags:

| Flag | Purpose |
|------|---------|
| `--subagent` | Logical subagent identity |
| `--profile` | Workload profile |
| `--bind-provider` | Provider UUID from catalog |
| `--bind-model` | Model ID from catalog |
| `--reuse-policy create|reuse|fail` | Name reuse behavior |
| `--model` | One-task override; not the same as `--bind-model` |
| `--timeout` | Task runtime timeout in seconds |
| `--output json` | Machine-readable output |

### Reuse Semantics

Reusing a `--subagent` name does not rebind it.

- Use `--reuse-policy create` when you are creating a new routing identity
- Use `--reuse-policy reuse` only when you intentionally want the existing
  binding
- Use `--reuse-policy fail` when name collisions should be surfaced

If you want a different provider/model routing decision, create a fresh
subagent name instead of reusing the old one.

## Step 4 — Choose a Completion Strategy

### Strategy A: Wait in One Command

For short, bounded work, prefer `--wait` with a client-side deadline:

```bash
busytok delegate \
  --subagent "reviewer-deepseek-v4-pro-001" \
  --profile "pi/review-cheap" \
  --bind-provider "5e3a4034-f1fd-4d50-a092-54022adbfa3e" \
  --bind-model "deepseek-v4-pro" \
  --reuse-policy create \
  --output json \
  --wait \
  --wait-timeout 120 \
  --prompt-file "/tmp/review-prompt.txt"
```

Behavior:

- terminal completion returns full task detail JSON
- wait timeout returns the last known task JSON and exits with code `124`

### Strategy B: Submit Asynchronously and Poll

For longer orchestration, read the initial delegate response and poll by
task id:

```bash
busytok subagent task --task-id "<TASK_ID>" --output json
```

Example initial delegate response:

```json
{
  "status": "queued",
  "task_id": "task_xxx",
  "subagent_id": "subagent_xxx",
  "subagent_name": "reviewer-deepseek-v4-pro-001",
  "summary": null,
  "model": null,
  "created": true
}
```

Example task detail response:

```json
{
  "id": "task_xxx",
  "subagent_id": "subagent_xxx",
  "subagent_name": "reviewer-deepseek-v4-pro-001",
  "profile": "pi/review-cheap",
  "status": "completed",
  "result_summary": "Ranked findings...",
  "error": null,
  "error_kind": null,
  "model_override": null,
  "effective_provider_id": "5e3a4034-f1fd-4d50-a092-54022adbfa3e",
  "effective_model_id": "deepseek-v4-pro",
  "binding_source": "bound",
  "created_at_ms": 1783300000000,
  "started_at_ms": 1783300000100,
  "completed_at_ms": 1783300005000
}
```

## Step 5 — Cancel Abandoned Work

If the caller-owned deadline expires and the task is no longer useful,
cancel it explicitly:

```bash
busytok subagent cancel \
  --task-id "<TASK_ID>" \
  --reason "caller deadline exceeded" \
  --output json
```

Example response:

```json
{
  "id": "task_xxx",
  "previous_status": "running",
  "new_status": "cancelled",
  "cancelled": true
}
```

This is best-effort for real execution. Agents should still treat
`cancelled = true` as the task lifecycle outcome.

## Step 6 — Interpret Status Correctly

Machine callers should determine outcome from task status, not only process
exit code.

- `completed` → success
- `queued` / `running` → still in progress
- `failed` → surface `error`, `error_kind`, and summary
- `cancelled` → treat as non-success and surface the cancellation context

Output shape differences:

- `delegate --output json` returns `task_id` and `summary`
- `subagent task --output json` returns `id` and `result_summary`
- `delegate --wait` returns the task-detail shape, not the initial delegate shape

## Step 7 — Inspect Existing Subagents

List subagents:

```bash
busytok subagent list --output json
```

Resolve a single subagent:

```bash
busytok subagent show "reviewer-deepseek-v4-pro-001" --output json
```

List recent tasks for a subagent:

```bash
busytok subagent tasks "reviewer-deepseek-v4-pro-001" --output json
```

## Complete Example (Bash)

```bash
#!/bin/bash
set -euo pipefail

CATALOG=$(busytok models --tag Coding --reasoning --sort context_window_desc --json)
COUNT=$(echo "$CATALOG" | jq 'length')
if [ "$COUNT" -eq 0 ]; then
  echo "No enabled Coding reasoning models available" >&2
  exit 1
fi

MODEL=$(echo "$CATALOG" | jq '.[0]')
PROVIDER_ID=$(echo "$MODEL" | jq -r '.provider_id')
MODEL_ID=$(echo "$MODEL" | jq -r '.model_id')

PROMPT_FILE=$(mktemp)
cat >"$PROMPT_FILE" <<'EOF'
Review the scoped patch.
Return ranked findings with file, line, impact, and evidence.
EOF

set +e
RESP=$(busytok delegate \
  --subagent "reviewer-${MODEL_ID}-$(date +%s)" \
  --profile "pi/review-cheap" \
  --bind-provider "$PROVIDER_ID" \
  --bind-model "$MODEL_ID" \
  --reuse-policy create \
  --output json \
  --wait \
  --wait-timeout 120 \
  --prompt-file "$PROMPT_FILE")
RC=$?
set -e

if [ "$RC" -eq 124 ]; then
  TASK_ID=$(echo "$RESP" | jq -r '.id')
  echo "Timed out waiting; last known state:"
  echo "$RESP" | jq .
  busytok subagent cancel --task-id "$TASK_ID" --reason "caller deadline exceeded" --output json | jq .
  exit 124
fi

STATUS=$(echo "$RESP" | jq -r '.status')
if [ "$STATUS" != "completed" ]; then
  echo "$RESP" | jq .
  exit 1
fi

echo "$RESP" | jq -r '.result_summary'
```

## Troubleshooting

### No matching model in `busytok models`

The catalog is configuration, not policy. If your filter returns zero rows,
surface that as a routing/configuration failure instead of silently falling
back to the first entry.

### Reused subagent has the wrong binding

You reused a logical name. Use `--reuse-policy fail` to surface collisions,
or create a fresh subagent name for a new provider/model route.

### Prompt is large or multi-line

Use `--prompt-file`, `--stdin`, or `--artifact-ref` instead of shell
interpolation.

### Wait exited `124`

That is a client-side wait timeout, not necessarily a remote task failure.
Inspect the returned JSON and decide whether to keep polling or cancel.

### JSON parsing is unreliable

Read stdout only. Do not merge stderr into stdout.
