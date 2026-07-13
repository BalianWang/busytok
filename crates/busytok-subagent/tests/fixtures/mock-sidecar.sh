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
#   BUSYTOK_MOCK_PREPARE_HIBERNATE_EVICTS=1  session.prepare_hibernate removes
#                                     the session from the mock sidecar's pool
#                                     before returning (simulates a concurrent
#                                     evictor's close having already freed the
#                                     slot). Used with a pre-flipped DB binding
#                                     (is_hot=0) to test the AlreadyEvicted +
#                                     retry-succeeds path.
#   BUSYTOK_MOCK_PREPARE_HIBERNATE_NOT_FOUND=1  session.prepare_hibernate removes
#                                     the session from the mock pool (if present)
#                                     and returns SESSION_NOT_FOUND (-32001).
#                                     Simulates a concurrent evictor having
#                                     already closed the session — tests the
#                                     SESSION_NOT_FOUND → AlreadyEvicted path
#                                     (distinct from the DB is_hot=0 path).
#   BUSYTOK_MOCK_TURN_AUTO_FAILS_AFTER_CLOSE=1  After a successful session.close,
#                                     the next session.turn_auto returns a
#                                     JSON-RPC error (-32603). Exercises the
#                                     retry-turn_auto-failed-after-eviction path.
#   BUSYTOK_MOCK_HOT_LIMIT_NO_CANDIDATE=1  Emit HOT_SESSION_LIMIT_REACHED with no
#                                     data.candidate field (sidecar protocol
#                                     violation). Exercises
#                                     classify_hot_limit_error ProtocolViolation path.
#   BUSYTOK_MOCK_HOT_LIMIT_ALL_BUSY=1  Emit HOT_SESSION_LIMIT_REACHED with
#                                     data.candidate=null + data.all_busy=true
#                                     (all sessions in-use). Exercises the
#                                     classify_hot_limit_error AllBusy path —
#                                     executor skips eviction and surfaces
#                                     SubagentError::HotSessionLimit.
#   BUSYTOK_MOCK_MEMORY_UPDATE=1
#                                When set, session.turn_auto includes a
#                                `result.memory_update` object with
#                                current_state_summary, key_files, decisions,
#                                and open_questions. 0/unset = no memory_update
#                                (hot_summary preserved — no destructive clear).
#   BUSYTOK_MOCK_AUTH_FAIL=1     session.turn_auto returns a JSON-RPC error
#                                with code -32010 (AUTH_FAILURE) and message
#                                "401 Unauthorized" instead of the normal
#                                success response. Used to test the
#                                SidecarTaskExecutor auth-fail kill path
#                                (pool.remove_worker_and_kill).
#   BUSYTOK_MOCK_REFILL_AFTER_CLOSE=1  After a successful session.close, a
#                                fake session for a different subagent is
#                                immediately added to the mock pool. Simulates
#                                a concurrent task grabbing the freed slot
#                                before the evictor's retry turn_auto. The
#                                retry then hits HOT_SESSION_LIMIT_REACHED again
#                                (with the fake session as candidate), exercising
#                                the Evicted + retry-hot-limit path.
#   BUSYTOK_MOCK_REFILL_AFTER_CLOSE_LIMIT=N  When set, limits the number of
#                                refills to N. After N closes, subsequent
#                                closes clear all fake refill sessions from
#                                the pool — the next eviction genuinely frees
#                                the slot, allowing the task to complete. Used
#                                by the executor-level test. -1 (default) =
#                                unlimited refills.
#   BUSYTOK_MOCK_HOT_LIMIT_RESPONSES=N  Return HOT_SESSION_LIMIT_REACHED for
#                                the first N turn_auto calls that would
#                                trigger it, then stop (let the session be
#                                created). Simulates a concurrent task
#                                finishing after N hot-limit responses.
#                                Used by manager/runtime e2e tests to verify
#                                the running → queued → running → completed
#                                cycle. -1 (default) = always return
#                                hot-limit when pool is full.
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
PREPARE_HIBERNATE_EVICTS="${BUSYTOK_MOCK_PREPARE_HIBERNATE_EVICTS:-0}"
PREPARE_HIBERNATE_NOT_FOUND="${BUSYTOK_MOCK_PREPARE_HIBERNATE_NOT_FOUND:-0}"
TURN_AUTO_FAILS_AFTER_CLOSE="${BUSYTOK_MOCK_TURN_AUTO_FAILS_AFTER_CLOSE:-0}"
HOT_LIMIT_NO_CANDIDATE="${BUSYTOK_MOCK_HOT_LIMIT_NO_CANDIDATE:-0}"
HOT_LIMIT_ALL_BUSY="${BUSYTOK_MOCK_HOT_LIMIT_ALL_BUSY:-0}"
MEMORY_UPDATE="${BUSYTOK_MOCK_MEMORY_UPDATE:-0}"
AUTH_FAIL="${BUSYTOK_MOCK_AUTH_FAIL:-0}"
REFILL_AFTER_CLOSE="${BUSYTOK_MOCK_REFILL_AFTER_CLOSE:-0}"
REFILL_AFTER_CLOSE_LIMIT="${BUSYTOK_MOCK_REFILL_AFTER_CLOSE_LIMIT:--1}"
HOT_LIMIT_RESPONSES="${BUSYTOK_MOCK_HOT_LIMIT_RESPONSES:--1}"
COUNT=0
# Number of successful session.close responses sent. Used by
# BUSYTOK_MOCK_TURN_AUTO_FAILS_AFTER_CLOSE to fail the retry turn_auto issued
# after an eviction close.
CLOSES=0
# Number of HOT_SESSION_LIMIT_REACHED responses sent. Used by
# BUSYTOK_MOCK_HOT_LIMIT_RESPONSES to stop returning hot-limit after N
# responses (simulating a concurrent task finishing and freeing the slot).
HOT_LIMIT_COUNT=0

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

# Build the memory_update JSON fragment (empty when MEMORY_UPDATE != 1).
# Interpolated into the turn_auto response to avoid duplicating the JSON
# payload across the create/reuse/empty branches.
build_mem_fragment() {
  if [[ "$MEMORY_UPDATE" == "1" ]]; then
    NOW_MS="$(date +%s)000"
    printf ',"memory_update":{"current_state_summary":"Investigated context; produced memory update.","key_files":[{"path":"src/auth/token.ts","reason":"refresh logic","last_seen_at_ms":%s,"score":3}],"decisions":["Focus on read-only analysis"],"open_questions":[{"question":"Concurrent refresh handled?","status":"open","created_at_ms":%s,"last_seen_at_ms":%s}]}' "$NOW_MS" "$NOW_MS" "$NOW_MS"
  fi
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
      if [[ "$AUTH_FAIL" == "1" ]]; then
        # Simulate a 401 from the upstream provider — exercises the
        # SidecarTaskExecutor auth-fail kill path (classify_sidecar_error
        # maps -32010 to TaskErrorKind::Auth -> pool.remove_worker_and_kill).
        printf '{"jsonrpc":"2.0","error":{"code":-32010,"message":"401 Unauthorized"},"id":%s}\n' "$ID"
      elif [[ "$TURN_AUTO_FAILS_AFTER_CLOSE" == "1" && "$CLOSES" -gt 0 ]]; then
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
        # Extract context.compact_context from the request to echo it back in
        # task_summary — lets e2e tests verify the context was built from memory.
        # Match "compact_context":"..." anywhere in the line (serde_json sorts
        # object keys alphabetically, so compact_context is NOT necessarily the
        # first key in the context object — budget_tokens comes first).
        COMPACT_CTX="$(printf '%s' "$line" | sed -n 's/.*"compact_context"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
        if [[ -z "$COMPACT_CTX" ]]; then
          COMPACT_CTX="mock turn completed"
        fi
        # Memory-update fragment is empty when MEMORY_UPDATE != 1, keeping the
        # JSON valid ("task_summary":"..." with no trailing comma).
        MEM_FRAGMENT="$(build_mem_fragment)"
        if [[ "$EMPTY_SESSION" == "1" ]]; then
          printf '{"jsonrpc":"2.0","result":{"adapter_session_id":"","session_reused":false,"status":"%s","result":{"task_summary":"%s"%s},"usage":{"model":"deepseek-chat","provider":"deepseek","input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.001}},"id":%s}\n' "$STATUS_OUT" "$COMPACT_CTX" "$MEM_FRAGMENT" "$ID"
        else
          EXISTING_SESS=$(sub_to_sess_lookup "$SUB_ID")
          # When HOT_LIMIT_RESPONSES is set (>= 0), only return hot-limit for
          # the first N qualifying calls. After N responses, fall through to
          # the "create new session" branch (simulating the concurrent task
          # finishing and freeing the slot). This lets manager/runtime e2e
          # tests verify the running → queued → running → completed cycle.
          HOT_LIMIT_BUDGET_OK=1
          if [[ "$HOT_LIMIT_RESPONSES" -ge 0 && "$HOT_LIMIT_COUNT" -ge "$HOT_LIMIT_RESPONSES" ]]; then
            HOT_LIMIT_BUDGET_OK=0
          fi
          if [[ "$HOT_LIMIT" -gt 0 && "${#SESS_ORDER[@]}" -ge "$HOT_LIMIT" && -z "$EXISTING_SESS" && "$HOT_LIMIT_BUDGET_OK" == "1" ]]; then
            HOT_LIMIT_COUNT=$((HOT_LIMIT_COUNT + 1))
            # Pool is full and this is a NEW subagent — return HOT_SESSION_LIMIT_REACHED
            CANDIDATE="${SESS_ORDER[0]}"  # LRU = oldest = index 0
            if [[ "$HOT_LIMIT_NO_CANDIDATE" == "1" ]]; then
              # Omit data entirely — sidecar protocol violation. Exercises
              # executor.rs classify_hot_limit_error ProtocolViolation path.
              printf '{"jsonrpc":"2.0","error":{"code":-32002,"message":"hot session limit reached"},"id":%s}\n' "$ID"
            elif [[ "$HOT_LIMIT_ALL_BUSY" == "1" ]]; then
              # All sessions in-use — legitimate "all busy" signal. Exercises
              # executor.rs classify_hot_limit_error AllBusy path (skip eviction).
              printf '{"jsonrpc":"2.0","error":{"code":-32002,"message":"hot session limit reached — all sessions busy","data":{"candidate":null,"all_busy":true}},"id":%s}\n' "$ID"
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
            printf '{"jsonrpc":"2.0","result":{"adapter_session_id":"%s","session_reused":%s,"status":"%s","result":{"task_summary":"%s"%s},"usage":{"model":"deepseek-chat","provider":"deepseek","input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.001}},"id":%s}\n' "$SESS" "$REUSED" "$STATUS_OUT" "$COMPACT_CTX" "$MEM_FRAGMENT" "$ID"
          else
            # Create new session
            SESS_COUNTER=$((SESS_COUNTER + 1))
            SESS="pi_sess_mock_${SESS_COUNTER}"
            SUB_IDS+=("$SUB_ID")
            SESS_IDS+=("$SESS")
            SESS_ORDER+=("$SESS")
            REUSED="false"
            printf '{"jsonrpc":"2.0","result":{"adapter_session_id":"%s","session_reused":%s,"status":"%s","result":{"task_summary":"%s"%s},"usage":{"model":"deepseek-chat","provider":"deepseek","input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.001}},"id":%s}\n' "$SESS" "$REUSED" "$STATUS_OUT" "$COMPACT_CTX" "$MEM_FRAGMENT" "$ID"
          fi
        fi
      fi
      ;;
    session.prepare_hibernate)
      if [[ "$PREPARE_HIBERNATE_FAILS" == "1" ]]; then
        # Simulate a sidecar that fails prepare_hibernate with a generic
        # error (NOT -32001/SESSION_NOT_FOUND, which is now intercepted by
        # the concurrent-eviction fix as "already evicted"). Using -32050
        # ensures the error propagates through the generic SidecarError
        # path, exercising the evict_session prepare_hibernate error path.
        printf '{"jsonrpc":"2.0","error":{"code":-32050,"message":"prepare_hibernate failed: adapter error"},"id":%s}\n' "$ID"
      elif [[ "$PREPARE_HIBERNATE_NOT_FOUND" == "1" ]]; then
        # Simulate a concurrent evictor having already closed the session.
        # Remove the session from the mock pool (if present) so the retry
        # turn_auto sees a free slot, then return SESSION_NOT_FOUND (-32001).
        # The executor's evict_session intercepts -32001 and returns
        # AlreadyEvicted (distinct from the DB is_hot=0 path).
        for i in "${!SESS_ORDER[@]}"; do
          if [[ "${SESS_ORDER[$i]}" == "$SESS_ID_PARAM" ]]; then
            unset 'SESS_ORDER[i]'
            SESS_ORDER=(${SESS_ORDER[@]+"${SESS_ORDER[@]}"})
            break
          fi
        done
        sub_to_sess_remove_by_sess "$SESS_ID_PARAM"
        printf '{"jsonrpc":"2.0","error":{"code":-32001,"message":"session not found: %s"},"id":%s}\n' "$SESS_ID_PARAM" "$ID"
      elif [[ "$NULL_MEMORY_DELTA" == "1" ]]; then
        # Return a null memory_delta — exercises executor.rs null-delta skip
        # path (lines 189-190): write_hot_summary is skipped.
        printf '{"jsonrpc":"2.0","result":{"memory_delta":null,"stats":{}},"id":%s}\n' "$ID"
      else
        # BUSYTOK_MOCK_PREPARE_HIBERNATE_EVICTS=1: simulate a concurrent
        # evictor's close having already freed the slot. The session is
        # removed from the mock pool BEFORE returning the memory delta, so
        # the retry turn_auto sees a free slot and succeeds. Used with a
        # pre-flipped DB binding (is_hot=0) to test the AlreadyEvicted +
        # retry-succeeds path.
        if [[ "$PREPARE_HIBERNATE_EVICTS" == "1" ]]; then
          for i in "${!SESS_ORDER[@]}"; do
            if [[ "${SESS_ORDER[$i]}" == "$SESS_ID_PARAM" ]]; then
              unset 'SESS_ORDER[i]'
              SESS_ORDER=(${SESS_ORDER[@]+"${SESS_ORDER[@]}"})
              break
            fi
          done
          sub_to_sess_remove_by_sess "$SESS_ID_PARAM"
        fi
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
        # BUSYTOK_MOCK_REFILL_AFTER_CLOSE=1: simulate a concurrent task
        # grabbing the freed slot immediately after close. A fake session
        # for a different subagent is added to the pool so the next turn_auto
        # for a NEW subagent sees the pool as full and gets
        # HOT_SESSION_LIMIT_REACHED again. This exercises the Evicted +
        # retry-hot-limit path (concurrent slot takeover after eviction).
        #
        # BUSYTOK_MOCK_REFILL_AFTER_CLOSE_LIMIT=N: stop refilling after N
        # closes AND clear all fake refill sessions from the pool. This
        # simulates the concurrent task finishing and releasing its slot,
        # so the next eviction genuinely frees the slot and the task can
        # complete. This lets manager/runtime e2e tests verify the full
        # running → queued → running → completed cycle under transient
        # contention. -1 (default) = unlimited refills (never clear).
        if [[ "$REFILL_AFTER_CLOSE" == "1" ]]; then
          if [[ "$REFILL_AFTER_CLOSE_LIMIT" -lt 0 ]] || [[ "$CLOSES" -le "$REFILL_AFTER_CLOSE_LIMIT" ]]; then
            SESS_COUNTER=$((SESS_COUNTER + 1))
            FAKE_SESS="pi_sess_refill_${SESS_COUNTER}"
            FAKE_SUB="concurrent_sub_${SESS_COUNTER}"
            SUB_IDS+=("$FAKE_SUB")
            SESS_IDS+=("$FAKE_SESS")
            SESS_ORDER+=("$FAKE_SESS")
          else
            # Limit exceeded — simulate the concurrent task finishing by
            # clearing all fake refill sessions from the pool. The pool
            # becomes empty, so the next turn_auto creates a new session
            # and succeeds. (Fake sessions have no DB binding and can't be
            # evicted by the executor — they MUST be removed here.)
            SESS_ORDER=()
            SUB_IDS=()
            SESS_IDS=()
          fi
        fi
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
