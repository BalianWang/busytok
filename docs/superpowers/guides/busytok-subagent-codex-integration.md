# Busytok Subagent Delegation — AI Agent Integration Guide

## Overview

Busytok exposes a JSON-over-stdout CLI that lets AI coding agents (Codex,
Claude Code) delegate subtasks to remote LLMs via Busytok subagents. This
document describes the full programmatic flow: discover available models →
select one → delegate a task → retrieve the result.

**CLI convention:** structured JSON goes to **stdout**; log messages go to
**stderr**. Always redirect `stderr` to `/dev/null` when parsing JSON output.

## Prerequisites

The Busytok desktop app must be installed and running (macOS). Verify:

```bash
busytok status 2>/dev/null
# {"db_healthy":true,"ready":true,"scan_state":"completed"}
```

At least one provider with an API key and at least one model must be
configured. Use the GUI (Settings → Providers) or the CLI:

```bash
busytok provider add \
  --name "MyProvider" \
  --kind "openai_compatible" \
  --url "https://api.example.com/v1" \
  --key "sk-..." \
  --model "my-model"
```

## Step 1 — Discover Available Models

Get the full model catalog in JSON. Each entry includes the provider UUID
needed for delegation:

```bash
busytok models --json 2>/dev/null
```

Example output:

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

### Filter by tag

Use `--tag` to narrow the list (AND semantics):

```bash
busytok models --tag Coding --json 2>/dev/null
```

### Extract the fields needed for delegation

From each model entry, extract:
- `provider_id` → `--bind-provider` argument
- `model_id` → `--bind-model` argument

## Step 2 — Select a Model

Choose a model based on task requirements. Example heuristics:

| Task type | Prefer |
|-----------|--------|
| Coding, code review | `tags` contains "Coding", `reasoning: true` |
| Writing, summarization | `tags` contains "Writing" |
| Fast, simple queries | Smaller `context_window`, `reasoning: false` |
| Complex analysis | Larger `context_window`, `reasoning: true` |

Pick the first matching model, or implement a scoring function over the JSON.

## Step 3 — Delegate the Task

Use the `delegate` subcommand with bound provider/model. The `--bind-*`
flags are required when creating a new subagent.

```bash
busytok delegate \
  --subagent "<NAME>" \
  --profile "pi/search-cheap" \
  --bind-provider "<PROVIDER_ID>" \
  --bind-model "<MODEL_ID>" \
  --output json \
  "<YOUR PROMPT>" 2>/dev/null
```

| Flag | Required | Description |
|------|----------|-------------|
| `--subagent` | yes | Name for the logical subagent (reused across calls) |
| `--profile` | yes | `pi/search-cheap` (read-only), `pi/review-cheap`, or `pi/plan-cheap` |
| `--bind-provider` | for new subagents | Provider UUID from Step 1 |
| `--bind-model` | for new subagents | Model ID from Step 1 |
| `--cwd` | no | Working directory (default `.`) |
| `--timeout` | no | Task timeout in seconds |
| `--output` | no | `json` or `text` (default `text`) |

### Handling the response

Two possible outcomes:

**A. Immediate completion:**

```json
{
  "status": "completed",
  "task_id": "task_xxx",
  "subagent_id": "xxx",
  "subagent_name": "my-task",
  "model": "deepseek-v4-pro",
  "summary": "The subagent's response text...",
  "usage": {
    "model": "deepseek-v4-pro",
    "provider": "5e3a4034-...",
    "input_tokens": 4562,
    "output_tokens": 120,
    "cost_usd": 0.0
  }
}
```

**B. Queued (gate paused or subagent busy):**

```json
{
  "status": "queued",
  "task_id": "task_xxx",
  "subagent_id": "xxx",
  "summary": null,
  "model": null
}
```

If `status` is `"queued"`, proceed to Step 4 to poll for the result.

### Reusing an existing subagent

Once a subagent is created (first call), subsequent calls with the same
`--subagent` name reuse it — `--bind-provider` and `--bind-model` are no
longer required:

```bash
# First call — creates the subagent
busytok delegate --subagent "code-reviewer" --profile "pi/search-cheap" \
  --bind-provider "<ID>" --bind-model "deepseek-v4-pro" \
  --output json "Review this code..." 2>/dev/null

# Subsequent calls — reuse, no bind flags needed
busytok delegate --subagent "code-reviewer" --profile "pi/search-cheap" \
  --output json "Review this other code..." 2>/dev/null
```

## Step 4 — Poll for Result (if queued)

When `status` is `"queued"`, poll with `subagent task`:

```bash
busytok subagent task --task-id "task_xxx" --output json 2>/dev/null
```

Example polling loop (bash):

```bash
TASK_ID="task_xxx"
while true; do
  RESULT=$(busytok subagent task --task-id "$TASK_ID" --output json 2>/dev/null)
  STATUS=$(echo "$RESULT" | jq -r '.status')
  if [ "$STATUS" = "completed" ] || [ "$STATUS" = "failed" ]; then
    echo "$RESULT"
    break
  fi
  sleep 2
done
```

Response shape:

```json
{
  "id": "task_xxx",
  "subagent_id": "xxx",
  "subagent_name": "my-task",
  "profile": "pi/search-cheap",
  "status": "completed",
  "result_summary": "The subagent's response text...",
  "error": null,
  "error_kind": null,
  "timeout_seconds": 120,
  "model_override": "deepseek-v4-pro",
  "created_at_ms": 1783300000000,
  "started_at_ms": 1783300000100,
  "completed_at_ms": 1783300005000
}
```

## Step 5 — List Existing Subagents

See all subagents and their bound provider/model:

```bash
busytok subagent list 2>/dev/null
```

```
SUBAGENT_ID                            NAME           BINDING                              STATUS
03daa136-c22d-4af0-b7e6-5d5e21c1d14c   verify-test    5e3a4034.../deepseek-v4-pro          cold
```

## Complete Example (Bash)

```bash
#!/bin/bash
# Full automated flow: discover → select → delegate → get result

# Step 1: find a coding model
MODEL=$(busytok models --tag Coding --json 2>/dev/null | jq -r '.[0]')
if [ -z "$MODEL" ] || [ "$MODEL" = "null" ]; then
  echo "No Coding models found, falling back to first available"
  MODEL=$(busytok models --json 2>/dev/null | jq -r '.[0]')
fi

PROVIDER_ID=$(echo "$MODEL" | jq -r '.provider_id')
MODEL_ID=$(echo "$MODEL" | jq -r '.model_id')
echo "Selected: provider=$PROVIDER_ID model=$MODEL_ID"

# Step 2: delegate
RESP=$(busytok delegate \
  --subagent "auto-demo" \
  --profile "pi/search-cheap" \
  --bind-provider "$PROVIDER_ID" \
  --bind-model "$MODEL_ID" \
  --output json \
  "用一句话介绍中国的长城" 2>/dev/null)

STATUS=$(echo "$RESP" | jq -r '.status')
TASK_ID=$(echo "$RESP" | jq -r '.task_id')

echo "Delegate status: $STATUS (task: $TASK_ID)"

# Step 3: get result (poll if queued)
if [ "$STATUS" = "completed" ]; then
  echo "Summary: $(echo "$RESP" | jq -r '.summary')"
elif [ "$STATUS" = "queued" ]; then
  echo "Task queued, polling..."
  for i in $(seq 1 30); do
    sleep 2
    TASK=$(busytok subagent task --task-id "$TASK_ID" --output json 2>/dev/null)
    TS=$(echo "$TASK" | jq -r '.status')
    if [ "$TS" = "completed" ] || [ "$TS" = "failed" ]; then
      echo "Status: $TS"
      echo "Summary: $(echo "$TASK" | jq -r '.result_summary')"
      break
    fi
    echo "  ... still $TS"
  done
fi
```

## Troubleshooting

### Task returns `status: "queued"` on every delegate

The Busytok pressure gate is paused. This happens when a previous task got
stuck in `running` status (e.g., due to a network timeout). Fix by
restarting the Busytok service:

```bash
launchctl kickstart -k gui/$(id -u)/com.busytok.service
sleep 3
```

### Delegate returns `dispatch_timeout` error

The LLM response took >30 seconds. The task may still complete — check with
`subagent task --task-id <id>`. For longer tasks, increase `--timeout`:

```bash
busytok delegate ... --timeout 300
```

### `provider list` shows empty table (display bug)

This is a known rendering bug (v0.0.10). Use `--json` instead:

```bash
busytok provider list --json 2>/dev/null
```

### `--bind-provider` requires a UUID, not a name

Provider IDs are UUIDs. Always extract them from `models --json` or
`provider list --json` output — never hardcode.

### Log lines interfere with JSON parsing

All log output goes to stderr. Always redirect when parsing:

```bash
busytok <command> --json 2>/dev/null | jq .
```
