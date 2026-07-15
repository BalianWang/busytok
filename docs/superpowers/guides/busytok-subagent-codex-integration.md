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

When this guide is explicitly invoked through the
`Busytok: Subagent Offloading` skill (`subagent-offloading`), the caller must perform a real delegation
and close the task lifecycle. Reading the catalog alone is not a successful
offload. If the service or catalog blocks delegation, report that blocker
instead of silently doing the work in the controller.

Structured JSON is written to stdout. Diagnostics are written to stderr.
Parse stdout only. Do not merge streams with `2>&1`, and do not discard
stderr by default in automation.

## Installation

The skill is published from this repository in the open Agent Skills format.
Install it for Codex and Claude Code with the cross-agent installer:

```bash
npx skills add BalianWang/busytok \
  --skill subagent-offloading \
  --agent codex --agent claude-code --yes
```

Or install the native plugin from the repository marketplace:

```bash
codex plugin marketplace add BalianWang/busytok
codex plugin add busytok@busytok
claude plugin marketplace add BalianWang/busytok
claude plugin install busytok@busytok
```

Start a new agent session after installation so the host reloads the plugin's
skill. Installation does not install or configure the `busytok` executable;
the desktop app and CLI remain prerequisites for delegation.

If you installed the preview package under the old
`busytok-subagent-offloading` ID, reinstall it as `busytok@busytok` and
`subagent-offloading`.

## Prerequisites

The Busytok desktop app must be installed and the service must be running:

```bash
busytok status
```

Parse the JSON and require `ready: true` before continuing. A non-ready service
or an empty matching catalog is a routing/configuration failure, not a reason
to guess a model.

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

The optional provider-level `--model` creates one enabled catalog entry with
CLI defaults (`context_window=200000`, `max_tokens=8192`, `reasoning=true`,
and the model ID as display name). Use `busytok provider model add` or `update`
when custom model metadata is required.

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

Set `REPO_ROOT` to the absolute repository or worktree path before using the
examples (for example, `REPO_ROOT="$(pwd -P)"` from the repository root).

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
  --cwd "$REPO_ROOT" \
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
| `--intent` | Optional durable long-term goal included in subagent context |
| `--cwd` | Absolute repository/worktree path; pass it explicitly in automation |
| `--bind-provider` | Provider UUID from catalog (enabled provider name is also accepted by the CLI; UUID is safer) |
| `--bind-model` | Model ID from catalog |
| `--reuse-policy create|reuse|fail` | Name reuse behavior |
| `--model` | One-task override; not the same as `--bind-model` |
| `--timeout` | Task runtime limit in seconds |
| `--wait-timeout` | Caller-side deadline for `--wait`; expiry exits 124 |
| `--poll-interval` | `--wait` polling interval in seconds (minimum effective value is 1) |
| `--output json` | Machine-readable output |

### Reuse Semantics

Reusing a `--subagent` name does not rebind it. The CLI policies are exact:

- `create`: fail if a subagent with that name exists; otherwise create it
- `reuse`: fail if no subagent with that name exists; otherwise reuse it
- `fail`: alias for `create`
- omitted: create-or-reuse, but bound-field mismatches are rejected

If you want a different provider/model routing decision, create a fresh
subagent name instead of reusing the old one.

## Step 4 — Choose a Completion Strategy

### Strategy A: Wait in One Command

For short, bounded work, prefer `--wait` with a client-side deadline:

```bash
busytok delegate \
  --cwd "$REPO_ROOT" \
  --subagent "reviewer-deepseek-v4-pro-001" \
  --profile "pi/review-cheap" \
  --bind-provider "5e3a4034-f1fd-4d50-a092-54022adbfa3e" \
  --bind-model "deepseek-v4-pro" \
  --reuse-policy create \
  --output json \
  --wait \
  --wait-timeout 120 \
  --poll-interval 2 \
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
- `running` → still in progress
- `failed` → surface `error`, `error_kind`, and summary
- `cancelled` → treat as non-success and surface the cancellation context
- `queued` → inspect `queue_reason` when present; it distinguishes resource
  gating or subagent serialization from an unknown delay

Output shape differences:

- `delegate --output json` returns `task_id` and `summary`
- `subagent task --output json` returns `id` and `result_summary`
- `delegate --wait` returns the task-detail shape, not the initial delegate shape

For implementation work, treat a `completed` task as execution success only;
the controller still has to verify the diff, tests, and requested acceptance
criteria. A useful semantic result envelope is:

```json
{
  "outcome": "DONE",
  "findings": [],
  "tests": [],
  "concerns": []
}
```

Use `DONE_WITH_CONCERNS`, `NEEDS_CONTEXT`, or `BLOCKED` when the subagent did
not produce a clean handoff.

## Development workflow and isolation

Busytok supplies the execution protocol; it does not replace development
workflow gates. For an implementation plan, use separate logical identities
for:

```text
implementer → spec reviewer → code-quality reviewer → final reviewer
```

Fix findings and re-review before advancing. Review/search tasks should be
read-only. Mutating tasks require an isolated branch/worktree, must not run in
parallel against the same worktree, and must never write directly to `main`.
Pass the absolute worktree path with `--cwd`, then independently inspect the
resulting diff and commit identity.

## Step 7 — Inspect Existing Subagents

List subagents:

```bash
busytok subagent list --output json
```

Resolve a single subagent:

```bash
busytok subagent show "reviewer-deepseek-v4-pro-001" \
  --cwd "$REPO_ROOT" --output json
```

List recent tasks for a subagent:

```bash
busytok subagent tasks "reviewer-deepseek-v4-pro-001" \
  --cwd "$REPO_ROOT" --limit 20 --output json
```

Name-based `show`, `tasks`, `hibernate`, and `delete` resolution is scoped to
`--cwd` (which defaults to `.`). Pass the same absolute repository/worktree
path used for delegation; use `--id` when path-independent lookup is needed.

## Complete Example (Bash)

```bash
#!/bin/bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd -P)}"
CATALOG=$(busytok models --tag Coding --reasoning --sort context_window_desc --json)
COUNT=$(echo "$CATALOG" | jq 'length')
if [ "$COUNT" -eq 0 ]; then
  echo "No enabled Coding reasoning models available" >&2
  exit 1
fi

# The CLI sort is a useful first ordering, but make the tie-break policy
# explicit instead of treating catalog position as "best".
MODEL=$(echo "$CATALOG" | jq -c 'sort_by(-(.context_window // 0), -(.max_tokens // 0), .provider_id, .model_id) | .[0]')
PROVIDER_ID=$(echo "$MODEL" | jq -r '.provider_id')
MODEL_ID=$(echo "$MODEL" | jq -r '.model_id')

PROMPT_FILE=$(mktemp)
trap 'rm -f "$PROMPT_FILE"' EXIT
cat >"$PROMPT_FILE" <<'EOF'
Review the scoped patch in the current repository.
Return ranked findings with file, line, impact, and evidence.
Do not modify files. Report DONE, DONE_WITH_CONCERNS, NEEDS_CONTEXT, or BLOCKED.
EOF

set +e
RESP=$(busytok delegate \
  --cwd "$REPO_ROOT" \
  --subagent "reviewer-${MODEL_ID}-$(date +%s)" \
  --profile "pi/review-cheap" \
  --bind-provider "$PROVIDER_ID" \
  --bind-model "$MODEL_ID" \
  --reuse-policy create \
  --output json \
  --wait \
  --wait-timeout 120 \
  --poll-interval 2 \
  --prompt-file "$PROMPT_FILE")
RC=$?
set -e

if [ "$RC" -eq 124 ]; then
  TASK_ID=$(echo "$RESP" | jq -r '.id // .task_id // empty')
  echo "Timed out waiting; last known state:"
  echo "$RESP" | jq .
  if [ -n "$TASK_ID" ]; then
    if ! busytok subagent cancel --task-id "$TASK_ID" \
      --reason "caller deadline exceeded" --output json | jq .; then
      echo "Warning: cancellation failed; retain the task_id and poll it separately." >&2
    fi
  fi
  exit 124
fi

STATUS=$(echo "$RESP" | jq -r '.status')
if [ "$STATUS" != "completed" ]; then
  echo "$RESP" | jq .
  exit 1
fi

echo "$RESP" | jq -r '.result_summary'

# Optional lifecycle audit: retain the task/subagent identity for evidence.
TASK_ID=$(echo "$RESP" | jq -r '.id // .task_id')
busytok subagent task --task-id "$TASK_ID" --output json | jq '{id, status, error, error_kind, queue_reason}'
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
