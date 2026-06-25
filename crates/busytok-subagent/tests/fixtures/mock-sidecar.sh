#!/usr/bin/env bash
# Minimal mock sidecar for Rust integration tests. Reads newline-delimited
# JSON-RPC from stdin, writes canned responses to stdout.
# Env vars:
#   BUSYTOK_MOCK_CRASH_AFTER=N   Exit (crash) after processing N messages
#                                (the Nth response IS sent, then the process
#                                exits 1). -1 (default) = never crash.
#   BUSYTOK_MOCK_DELAY_MS=N      Delay each response by N ms.

set -euo pipefail
CRASH_AFTER="${BUSYTOK_MOCK_CRASH_AFTER:--1}"
DELAY_MS="${BUSYTOK_MOCK_DELAY_MS:-0}"
COUNT=0
while IFS= read -r line; do
  COUNT=$((COUNT + 1))
  if [[ "$DELAY_MS" -gt 0 ]]; then
    awk -v ms="$DELAY_MS" 'BEGIN { system("sleep " ms/1000) }'
  fi
  # Extract method and id without jq (sed on single-line JSON).
  METHOD=$(printf '%s' "$line" | sed -n 's/.*"method"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  ID=$(printf '%s' "$line" | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p')
  case "$METHOD" in
    adapter.initialize)
      printf '{"jsonrpc":"2.0","result":{"protocol_version":1,"sidecar_version":"mock-1.0"},"id":%s}\n' "$ID"
      ;;
    adapter.health)
      printf '{"jsonrpc":"2.0","result":{"status":"healthy","sessions":0,"rss_mb":42},"id":%s}\n' "$ID"
      ;;
    adapter.shutdown)
      printf '{"jsonrpc":"2.0","result":{"ok":true},"id":%s}\n' "$ID"
      exit 0
      ;;
    session.turn_auto)
      printf '{"jsonrpc":"2.0","result":{"adapter_session_id":"pi_sess_mock_%s","session_reused":false,"status":"completed","result":{"task_summary":"mock turn completed"},"usage":{"model":"deepseek-chat","provider":"deepseek","input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.001}},"id":%s}\n' "$COUNT" "$ID"
      ;;
    session.prepare_hibernate)
      printf '{"jsonrpc":"2.0","result":{"memory_delta":null,"stats":{}},"id":%s}\n' "$ID"
      ;;
    session.close)
      printf '{"jsonrpc":"2.0","result":{"ok":true},"id":%s}\n' "$ID"
      ;;
    *)
      printf '{"jsonrpc":"2.0","error":{"code":-32601,"message":"method not found: %s"},"id":%s}\n' "$METHOD" "$ID"
      ;;
  esac
  # Crash AFTER sending the response for the Nth message. This lets the
  # supervisor's `ensure_started` (which sends adapter.initialize as message 1)
  # succeed; the crash surfaces on the next supervision-loop poll via try_wait.
  if [[ "$CRASH_AFTER" -ge 0 && "$COUNT" -ge "$CRASH_AFTER" ]]; then
    echo "mock-sidecar crashing after $CRASH_AFTER messages" >&2
    exit 1
  fi
done
