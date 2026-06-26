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
#   BUSYTOK_MOCK_HOT_SESSION_LIMIT=N  When N sessions are active, the next
#                                     session.turn_auto for a NEW subagent
#                                     returns HOT_SESSION_LIMIT_REACHED
#                                     (-32002) with data.candidate.
#   BUSYTOK_MOCK_CLOSE_FAILS=1        session.close returns a JSON-RPC error
#                                     (-32001 SESSION_NOT_FOUND) instead of
#                                     ok. Used to test the fatal-close-failure
#                                     eviction path.
#   BUSYTOK_MOCK_TURN_STATUS=<s>      Override the "status" field in
#                                     session.turn_auto success responses
#                                     (default "completed"). Set to "failed",
#                                     "timeout", or any unknown value to
#                                     exercise parse_turn_auto_result arms.
#   BUSYTOK_MOCK_PREPARE_HIBERNATE_FAILS=1  session.prepare_hibernate returns a
#                                     JSON-RPC error (-32001) instead of the
#                                     memory_delta payload. Exercises the
#                                     evict_session prepare_hibernate error path.
#   BUSYTOK_MOCK_NULL_MEMORY_DELTA=1  session.prepare_hibernate returns
#                                     {"memory_delta":null,...} so the executor
#                                     skips write_hot_summary.
#   BUSYTOK_MOCK_TURN_AUTO_FAILS_AFTER_CLOSE=1  After a successful session.close,
#                                     the next session.turn_auto returns a
#                                     JSON-RPC error (-32603). Exercises the
#                                     retry-turn_auto-failed-after-eviction path.
#   BUSYTOK_MOCK_HOT_LIMIT_NO_CANDIDATE=1  Emit HOT_SESSION_LIMIT_REACHED with no
#                                     data.candidate field (sidecar protocol
#                                     violation). Exercises
#                                     extract_candidate_from_data error path.
set -euo pipefail
CRASH_AFTER="${BUSYTOK_MOCK_CRASH_AFTER:--1}"
DELAY_MS="${BUSYTOK_MOCK_DELAY_MS:-0}"
EMPTY_SESSION="${BUSYTOK_MOCK_EMPTY_SESSION:-0}"
STDERR_LINES="${BUSYTOK_MOCK_STDERR_LINES:-0}"
HOT_LIMIT="${BUSYTOK_MOCK_HOT_SESSION_LIMIT:-0}"
CLOSE_FAILS="${BUSYTOK_MOCK_CLOSE_FAILS:-0}"
TURN_STATUS="${BUSYTOK_MOCK_TURN_STATUS:-}"
PREPARE_HIBERNATE_FAILS="${BUSYTOK_MOCK_PREPARE_HIBERNATE_FAILS:-0}"
NULL_MEMORY_DELTA="${BUSYTOK_MOCK_NULL_MEMORY_DELTA:-0}"
TURN_AUTO_FAILS_AFTER_CLOSE="${BUSYTOK_MOCK_TURN_AUTO_FAILS_AFTER_CLOSE:-0}"
HOT_LIMIT_NO_CANDIDATE="${BUSYTOK_MOCK_HOT_LIMIT_NO_CANDIDATE:-0}"
COUNT=0
# Number of successful session.close responses sent. Used by
# BUSYTOK_MOCK_TURN_AUTO_FAILS_AFTER_CLOSE to fail the retry turn_auto issued
# after an eviction close.
CLOSES=0

# Track active sessions using parallel indexed arrays (portable to bash 3.2
# which does NOT support `declare -A`). SUB_IDS[i] maps to SESS_IDS[i].
# SESS_ORDER tracks LRU order: index 0 = oldest (LRU), last = newest (MRU).
SUB_IDS=()
SESS_IDS=()
SESS_ORDER=()
SESS_COUNTER=0

# Look up a subagent's session by iterating the parallel arrays.
# Echoes the session_id, or empty string if not found.
sub_to_sess_lookup() {
  local target="$1" i
  for i in "${!SUB_IDS[@]}"; do
    if [[ "${SUB_IDS[$i]}" == "$target" ]]; then
      printf '%s' "${SESS_IDS[$i]}"
      return 0
    fi
  done
  return 0  # not found, prints nothing
}

# Remove a subagent→session mapping by subagent_id.
sub_to_sess_remove_by_sub() {
  local target="$1" i
  for i in "${!SUB_IDS[@]}"; do
    if [[ "${SUB_IDS[$i]}" == "$target" ]]; then
      unset 'SUB_IDS[i]'
      unset 'SESS_IDS[i]'
      SUB_IDS=(${SUB_IDS[@]+"${SUB_IDS[@]}"})
      SESS_IDS=(${SESS_IDS[@]+"${SESS_IDS[@]}"})
      return 0
    fi
  done
}

# Remove a subagent→session mapping by session_id.
sub_to_sess_remove_by_sess() {
  local target="$1" i
  for i in "${!SESS_IDS[@]}"; do
    if [[ "${SESS_IDS[$i]}" == "$target" ]]; then
      unset 'SUB_IDS[i]'
      unset 'SESS_IDS[i]'
      SUB_IDS=(${SUB_IDS[@]+"${SUB_IDS[@]}"})
      SESS_IDS=(${SESS_IDS[@]+"${SESS_IDS[@]}"})
      return 0
    fi
  done
}

while IFS= read -r line; do
  COUNT=$((COUNT + 1))
  if [[ "$DELAY_MS" -gt 0 ]]; then
    awk -v ms="$DELAY_MS" 'BEGIN { system("sleep " ms/1000) }'
  fi
  if [[ "$STDERR_LINES" -gt 0 ]]; then
    for i in $(seq 1 "$STDERR_LINES"); do
      echo "[mock-sidecar stderr] line $i for msg $COUNT" >&2
    done
  fi
  METHOD=$(printf '%s' "$line" | sed -n 's/.*"method"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  ID=$(printf '%s' "$line" | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p')
  # Extract logical_subagent_id from params (for turn_auto)
  SUB_ID=$(printf '%s' "$line" | sed -n 's/.*"logical_subagent_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  # Extract adapter_session_id from params (for prepare_hibernate/close)
  SESS_ID_PARAM=$(printf '%s' "$line" | sed -n 's/.*"adapter_session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')

  case "$METHOD" in
    adapter.initialize)
      printf '{"jsonrpc":"2.0","result":{"protocol_version":1,"sidecar_version":"mock-1.0"},"id":%s}\n' "$ID"
      ;;
    adapter.health)
      printf '{"jsonrpc":"2.0","result":{"status":"healthy","sessions":%d,"rss_mb":42},"id":%s}\n' "${#SESS_ORDER[@]}" "$ID"
      ;;
    adapter.shutdown)
      printf '{"jsonrpc":"2.0","result":{"ok":true},"id":%s}\n' "$ID"
      exit 0
      ;;
    session.turn_auto)
      if [[ "$TURN_AUTO_FAILS_AFTER_CLOSE" == "1" && "$CLOSES" -gt 0 ]]; then
        # Simulate a sidecar that fails the turn_auto retry issued AFTER a
        # successful session.close (eviction released the slot, but the retry
        # turn itself errors). Exercises executor.rs lines 92-98.
        printf '{"jsonrpc":"2.0","error":{"code":-32603,"message":"turn_auto failed: internal error"},"id":%s}\n' "$ID"
      else
        if [[ -n "$TURN_STATUS" ]]; then
          STATUS_OUT="$TURN_STATUS"
        else
          STATUS_OUT="completed"
        fi
        if [[ "$EMPTY_SESSION" == "1" ]]; then
          printf '{"jsonrpc":"2.0","result":{"adapter_session_id":"","session_reused":false,"status":"%s","result":{"task_summary":"mock turn completed"},"usage":{"model":"deepseek-chat","provider":"deepseek","input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.001}},"id":%s}\n' "$STATUS_OUT" "$ID"
        else
          EXISTING_SESS=$(sub_to_sess_lookup "$SUB_ID")
          if [[ "$HOT_LIMIT" -gt 0 && "${#SESS_ORDER[@]}" -ge "$HOT_LIMIT" && -z "$EXISTING_SESS" ]]; then
            # Pool is full and this is a NEW subagent — return HOT_SESSION_LIMIT_REACHED
            CANDIDATE="${SESS_ORDER[0]}"  # LRU = oldest = index 0
            if [[ "$HOT_LIMIT_NO_CANDIDATE" == "1" ]]; then
              # Omit data.candidate — sidecar protocol violation. Exercises
              # executor.rs extract_candidate_from_data error path.
              printf '{"jsonrpc":"2.0","error":{"code":-32002,"message":"hot session limit reached"},"id":%s}\n' "$ID"
            else
              printf '{"jsonrpc":"2.0","error":{"code":-32002,"message":"hot session limit reached","data":{"candidate":"%s"}},"id":%s}\n' "$CANDIDATE" "$ID"
            fi
          elif [[ -n "$EXISTING_SESS" ]]; then
            # Reuse existing session for this subagent
            SESS="$EXISTING_SESS"
            REUSED="true"
            # Move to MRU (end of array): remove and re-append
            for i in "${!SESS_ORDER[@]}"; do
              if [[ "${SESS_ORDER[$i]}" == "$SESS" ]]; then
                unset 'SESS_ORDER[i]'
                SESS_ORDER=(${SESS_ORDER[@]+"${SESS_ORDER[@]}"})
                break
              fi
            done
            SESS_ORDER+=("$SESS")
            printf '{"jsonrpc":"2.0","result":{"adapter_session_id":"%s","session_reused":%s,"status":"%s","result":{"task_summary":"mock turn completed"},"usage":{"model":"deepseek-chat","provider":"deepseek","input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.001}},"id":%s}\n' "$SESS" "$REUSED" "$STATUS_OUT" "$ID"
          else
            # Create new session
            SESS_COUNTER=$((SESS_COUNTER + 1))
            SESS="pi_sess_mock_${SESS_COUNTER}"
            SUB_IDS+=("$SUB_ID")
            SESS_IDS+=("$SESS")
            SESS_ORDER+=("$SESS")
            REUSED="false"
            printf '{"jsonrpc":"2.0","result":{"adapter_session_id":"%s","session_reused":%s,"status":"%s","result":{"task_summary":"mock turn completed"},"usage":{"model":"deepseek-chat","provider":"deepseek","input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.001}},"id":%s}\n' "$SESS" "$REUSED" "$STATUS_OUT" "$ID"
          fi
        fi
      fi
      ;;
    session.prepare_hibernate)
      if [[ "$PREPARE_HIBERNATE_FAILS" == "1" ]]; then
        # Simulate a sidecar that fails prepare_hibernate — exercises
        # executor.rs evict_session prepare_hibernate error path (lines 150-156).
        printf '{"jsonrpc":"2.0","error":{"code":-32001,"message":"prepare_hibernate failed: adapter error"},"id":%s}\n' "$ID"
      elif [[ "$NULL_MEMORY_DELTA" == "1" ]]; then
        # Return a null memory_delta — exercises executor.rs null-delta skip
        # path (lines 189-190): write_hot_summary is skipped.
        printf '{"jsonrpc":"2.0","result":{"memory_delta":null,"stats":{}},"id":%s}\n' "$ID"
      else
        printf '{"jsonrpc":"2.0","result":{"memory_delta":{"hot_summary":"hibernated"},"stats":{"adapter_session_id":"%s"}},"id":%s}\n' "$SESS_ID_PARAM" "$ID"
      fi
      ;;
    session.close)
      if [[ "$CLOSE_FAILS" == "1" ]]; then
        # Simulate a sidecar that fails to close the session — used to test
        # the fatal-close-failure eviction path (P1-2 fix).
        printf '{"jsonrpc":"2.0","error":{"code":-32001,"message":"session.close failed: adapter error"},"id":%s}\n' "$ID"
      else
        # Remove from pool
        for i in "${!SESS_ORDER[@]}"; do
          if [[ "${SESS_ORDER[$i]}" == "$SESS_ID_PARAM" ]]; then
            unset 'SESS_ORDER[i]'
            SESS_ORDER=(${SESS_ORDER[@]+"${SESS_ORDER[@]}"})
            break
          fi
        done
        # Remove from subagent map
        sub_to_sess_remove_by_sess "$SESS_ID_PARAM"
        # Track successful closes so BUSYTOK_MOCK_TURN_AUTO_FAILS_AFTER_CLOSE
        # can fail the retry turn_auto issued after an eviction close.
        CLOSES=$((CLOSES + 1))
        printf '{"jsonrpc":"2.0","result":{"ok":true},"id":%s}\n' "$ID"
      fi
      ;;
    *)
      printf '{"jsonrpc":"2.0","error":{"code":-32601,"message":"method not found: %s"},"id":%s}\n' "$METHOD" "$ID"
      ;;
  esac
  if [[ "$CRASH_AFTER" -ge 0 && "$COUNT" -ge "$CRASH_AFTER" ]]; then
    echo "mock-sidecar crashing after $CRASH_AFTER messages" >&2
    exit 1
  fi
done
