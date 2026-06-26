#!/usr/bin/env bash
# Minimal mock sidecar for Rust integration tests. Reads newline-delimited
# JSON-RPC from stdin, writes canned responses to stdout.
# Env vars:
#   BUSYTOK_MOCK_CRASH_AFTER=N   Exit (crash) after processing N messages
#                                (the Nth response IS sent, then the process
#                                exits 1). -1 (default) = never crash.
#   BUSYTOK_MOCK_DELAY_MS=N      Delay each response by N ms.
#   BUSYTOK_MOCK_EMPTY_SESSION=1 Emit an empty adapter_session_id in
#                                session.turn_auto responses (regression
#                                fixture for the warm-path fallback).
#                                0/unset = emit a real session id.
#   BUSYTOK_MOCK_STDERR_LINES=N  Write N lines to stderr before each response
#                                (exercises the supervisor's stderr reader —
#                                verifies the pipe doesn't fill and block).
#   BUSYTOK_MOCK_HOT_SESSION_LIMIT=N
#                                When > 0, track active hot sessions and
#                                return HOT_SESSION_LIMIT_REACHED (-32002)
#                                with data.candidate=<LRU session id> once the
#                                pool is full. session.close releases a slot.
#                                0/unset = unlimited (legacy behavior).

set -euo pipefail
CRASH_AFTER="${BUSYTOK_MOCK_CRASH_AFTER:--1}"
DELAY_MS="${BUSYTOK_MOCK_DELAY_MS:-0}"
EMPTY_SESSION="${BUSYTOK_MOCK_EMPTY_SESSION:-0}"
STDERR_LINES="${BUSYTOK_MOCK_STDERR_LINES:-0}"
HOT_LIMIT="${BUSYTOK_MOCK_HOT_SESSION_LIMIT:-0}"

# Track active hot sessions as a newline-separated string (bash 3.x — no
# associative arrays on macOS /bin/bash). Each entry is the adapter_session_id
# assigned when the session was created; LRU order is insertion order (the
# first line is the oldest).
ACTIVE_SESSIONS=""

COUNT=0
while IFS= read -r line; do
  COUNT=$((COUNT + 1))
  if [[ "$DELAY_MS" -gt 0 ]]; then
    awk -v ms="$DELAY_MS" 'BEGIN { system("sleep " ms/1000) }'
  fi
  # Write N stderr lines before each response (P1-1 fixture: verifies the
  # supervisor drains stderr so the pipe doesn't fill and block the child).
  if [[ "$STDERR_LINES" -gt 0 ]]; then
    for i in $(seq 1 "$STDERR_LINES"); do
      echo "[mock-sidecar stderr] line $i for msg $COUNT" >&2
    done
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
      if [[ "$EMPTY_SESSION" == "1" ]]; then
        printf '{"jsonrpc":"2.0","result":{"adapter_session_id":"","session_reused":false,"status":"completed","result":{"task_summary":"mock turn completed"},"usage":{"model":"deepseek-chat","provider":"deepseek","input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.001}},"id":%s}\n' "$ID"
      elif [[ "$HOT_LIMIT" -gt 0 ]]; then
        # Count active sessions (lines in ACTIVE_SESSIONS).
        ACTIVE_COUNT=0
        if [[ -n "$ACTIVE_SESSIONS" ]]; then
          ACTIVE_COUNT=$(printf '%s\n' "$ACTIVE_SESSIONS" | grep -c . || true)
        fi
        if [[ "$ACTIVE_COUNT" -ge "$HOT_LIMIT" ]]; then
          # Evict the LRU (first session in insertion order).
          CANDIDATE=$(printf '%s\n' "$ACTIVE_SESSIONS" | head -n1)
          printf '{"jsonrpc":"2.0","error":{"code":-32002,"message":"hot session limit reached","data":{"candidate":"%s"}},"id":%s}\n' "$CANDIDATE" "$ID"
        else
          SESS_ID="pi_sess_mock_${COUNT}"
          if [[ -z "$ACTIVE_SESSIONS" ]]; then
            ACTIVE_SESSIONS="$SESS_ID"
          else
            ACTIVE_SESSIONS="${ACTIVE_SESSIONS}"$'\n'"${SESS_ID}"
          fi
          printf '{"jsonrpc":"2.0","result":{"adapter_session_id":"%s","session_reused":false,"status":"completed","result":{"task_summary":"mock turn completed"},"usage":{"model":"deepseek-chat","provider":"deepseek","input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.001}},"id":%s}\n' "$SESS_ID" "$ID"
        fi
      else
        printf '{"jsonrpc":"2.0","result":{"adapter_session_id":"pi_sess_mock_%s","session_reused":false,"status":"completed","result":{"task_summary":"mock turn completed"},"usage":{"model":"deepseek-chat","provider":"deepseek","input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.001}},"id":%s}\n' "$COUNT" "$ID"
      fi
      ;;
    session.prepare_hibernate)
      # Return a non-null memory_delta so the executor's eviction flow writes
      # hot_summary to the store. The stats field is opaque to the executor
      # (passed through to the resource event detail_json).
      printf '{"jsonrpc":"2.0","result":{"memory_delta":{"hot_summary":"mock hot summary"},"stats":{"turns":3,"tokens":120}},"id":%s}\n' "$ID"
      ;;
    session.close)
      # Extract adapter_session_id from params (sed on single-line JSON) and
      # remove it from ACTIVE_SESSIONS so the slot is released.
      CLOSE_SESS=$(printf '%s' "$line" | sed -n 's/.*"adapter_session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' || true)
      if [[ -n "$CLOSE_SESS" && -n "$ACTIVE_SESSIONS" ]]; then
        NEW_SESSIONS=""
        while IFS= read -r s; do
          [[ -z "$s" ]] && continue
          if [[ "$s" != "$CLOSE_SESS" ]]; then
            if [[ -z "$NEW_SESSIONS" ]]; then
              NEW_SESSIONS="$s"
            else
              NEW_SESSIONS="${NEW_SESSIONS}"$'\n'"${s}"
            fi
          fi
        done <<< "$ACTIVE_SESSIONS"
        ACTIVE_SESSIONS="$NEW_SESSIONS"
      fi
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
