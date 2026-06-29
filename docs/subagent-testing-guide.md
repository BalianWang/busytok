# Busytok Subagent Testing Guide

**Audience:** QA / Test Engineering  
**Scope:** Logical subagent + Pi sidecar capability delivered across Plan 1-Plan 6  
**Branch / Worktree:** `feat/subagent-foundation` at `.worktrees/feat-subagent-foundation`  
**Last Updated:** 2026-06-29

---

## 1. Purpose

This document describes how to test the Busytok subagent feature safely on a machine that already has the released Busytok app installed.

The key rule is:

**Do not test the source build against the release build's default data, config, or runtime directories.**

If the source build and the installed release share the same default environment, they will compete for:

- the same SQLite database
- the same control socket
- the same settings file
- the same log and artifact directories

That can cause false failures, state corruption, misleading results, or accidental pollution of real user data.

---

## 2. Conflict Risk and Isolation Strategy

### 2.1 Why conflict happens

By default, Busytok resolves paths from standard XDG directories under the `busytok` app name. That means the source build and the installed release will use the same locations unless explicitly isolated.

Shared defaults include:

- data dir: `.../busytok`
- config dir: `.../busytok`
- runtime dir / socket dir: `.../busytok`
- database: `busytok.db`
- control socket: `busytok.sock`

### 2.2 Required testing rule

For all source-based testing in this document:

- run the source-built `busytok-service`
- run the source-built `busytok` CLI
- use dedicated temporary XDG directories
- do not use the installed release binary for functional verification of this branch

### 2.3 Industry best practice

Recommended validation layers:

1. Automated regression in repo
2. Isolated source-build manual E2E
3. Small smoke test on packaged/release app only after source-build confidence is high

This keeps development verification reproducible and prevents local production state from affecting test outcomes.

---

## 3. Test Environment Setup

### 3.1 Working directory

Run all source-build commands from:

```bash
/Users/wsd/Data/Busytok/busytok/.worktrees/feat-subagent-foundation
```

### 3.2 Required shell environment

Use an isolated environment root:

```bash
export BUSYTOK_DEV_ROOT=/tmp/busytok-subagent-dev
rm -rf "$BUSYTOK_DEV_ROOT"
mkdir -p "$BUSYTOK_DEV_ROOT"/{data,config,runtime,logs}

export XDG_DATA_HOME="$BUSYTOK_DEV_ROOT/data"
export XDG_CONFIG_HOME="$BUSYTOK_DEV_ROOT/config"
export XDG_RUNTIME_DIR="$BUSYTOK_DEV_ROOT/runtime"
export BUSYTOK_LOG_DIR="$BUSYTOK_DEV_ROOT/logs"
```

### 3.3 Optional safety step

If the installed release service is running, stop it before manual testing. This is not strictly required when the XDG environment is isolated, but it reduces operator confusion.

### 3.4 Verification that isolation is active

Before running tests, confirm:

- `$XDG_DATA_HOME` points to `/tmp/busytok-subagent-dev/data`
- `$XDG_CONFIG_HOME` points to `/tmp/busytok-subagent-dev/config`
- `$XDG_RUNTIME_DIR` points to `/tmp/busytok-subagent-dev/runtime`
- `$BUSYTOK_LOG_DIR` points to `/tmp/busytok-subagent-dev/logs`

Expected isolated artifacts during testing:

- DB: `/tmp/busytok-subagent-dev/data/busytok/busytok.db`
- socket: `/tmp/busytok-subagent-dev/runtime/busytok/busytok.sock`
- logs: `/tmp/busytok-subagent-dev/logs`

### 3.5 Settings file location

The source-build service reads settings from:

```bash
$XDG_CONFIG_HOME/busytok/settings.toml
```

With the recommended isolated environment, that becomes:

```bash
/tmp/busytok-subagent-dev/config/busytok/settings.toml
```

### 3.6 Switching between mock and real sidecar modes

The testing mode is controlled by:

```toml
[subagent.pi_sidecar]
enabled = false
```

Mode meaning:

- `enabled = false`: mock / degraded path
- `enabled = true`: real Pi sidecar path

Recommended QA workflow:

1. Run the lifecycle and storage tests first with `enabled = false`
2. Run real sidecar spawn / protocol / restart checks with `enabled = true`

Minimal examples:

Mock mode:

```toml
[subagent]
enabled = true

[subagent.pi_sidecar]
enabled = false
```

Real sidecar mode:

```toml
[subagent]
enabled = true

[subagent.pi_sidecar]
enabled = true
```

If real sidecar mode is used on a QA machine without a valid runtime bundle, doctor and delegate failures caused by missing runtime artifacts should be treated as environment findings, not as logic regressions.

---

## 4. Test Layers

### 4.1 Layer A: Automated regression

Run full automated coverage first:

```bash
cargo test --workspace
cargo clippy --workspace --tests -- -D warnings
cargo fmt --all -- --check
```

Expected result:

- all tests pass
- clippy clean
- fmt clean

### 4.2 Layer B: Subagent-focused automated suites

Recommended targeted suites:

```bash
cargo test -p busytok-subagent
cargo test -p busytok-runtime --test subagent_e2e_sidecar
cargo test -p busytok-runtime --test supervisor_control
cargo test -p busytok-store --test subagent_queries
```

Primary coverage intent:

- manager logic
- queue / pressure behavior
- sidecar lifecycle
- doctor checks
- persistence and deletion semantics

### 4.3 Layer C: Manual isolated E2E

Start the source service in one terminal:

```bash
cd /Users/wsd/Data/Busytok/busytok/.worktrees/feat-subagent-foundation

export BUSYTOK_DEV_ROOT=/tmp/busytok-subagent-dev
export XDG_DATA_HOME="$BUSYTOK_DEV_ROOT/data"
export XDG_CONFIG_HOME="$BUSYTOK_DEV_ROOT/config"
export XDG_RUNTIME_DIR="$BUSYTOK_DEV_ROOT/runtime"
export BUSYTOK_LOG_DIR="$BUSYTOK_DEV_ROOT/logs"

cargo run -p busytok-service
```

Use a second terminal for CLI verification:

```bash
cd /Users/wsd/Data/Busytok/busytok/.worktrees/feat-subagent-foundation

export BUSYTOK_DEV_ROOT=/tmp/busytok-subagent-dev
export XDG_DATA_HOME="$BUSYTOK_DEV_ROOT/data"
export XDG_CONFIG_HOME="$BUSYTOK_DEV_ROOT/config"
export XDG_RUNTIME_DIR="$BUSYTOK_DEV_ROOT/runtime"
export BUSYTOK_LOG_DIR="$BUSYTOK_DEV_ROOT/logs"
```

### 4.4 Recommended execution matrix

Manual testing should be split into two modes:

| Mode | Purpose | Expected behavior |
|------|---------|-------------------|
| Mock / degraded path | Validate logical subagent lifecycle, queueing, deletion, diagnostics wiring, and non-sidecar fallback behavior | Commands succeed without requiring a real Pi bundle |
| Real Pi sidecar path | Validate spawn, health, turn execution, hibernate/restart behavior, and doctor checks against an actual sidecar runtime | Requires valid sidecar runtime/bundle configuration |

If the QA machine does not have a valid sidecar runtime bundle available, that is acceptable for the first mode. In that case:

- lifecycle and storage behavior should still be tested
- doctor should surface environment-dependent `warning` or `error` results honestly
- failures caused only by missing runtime artifacts should not be misclassified as product logic regressions

---

## 5. Manual Test Checklist

### 5.1 Service health

Run:

```bash
cargo run -p busytok -- status
```

Pass criteria:

- CLI successfully connects
- no "connecting to Busytok service" failure
- service health payload returns normally

### 5.2 Doctor / diagnostics

Run:

```bash
cargo run -p busytok -- doctor
```

Pass criteria:

- command completes successfully
- subagent doctor section is printed
- all required checks appear
- `warning` vs `error` behavior matches actual environment

The 11 expected doctor checks are:

- `service_running`
- `sqlite_readable`
- `sidecar_launchable`
- `bundled_node_arch`
- `bundle_manifest_readable`
- `protocol_version`
- `default_model_config`
- `pi_runtime_installed`
- `artifact_store_writable`
- `resource_policy_valid`
- `subagents_unused_30d`

Notes:

- `busytok doctor` reuses existing `settings.diagnostics`
- warnings are acceptable where the branch intentionally models non-fatal conditions

### 5.3 Empty list baseline

Run:

```bash
cargo run -p busytok -- subagent list
```

Pass criteria:

- command succeeds
- initial output is empty or contains only intentionally pre-created rows

### 5.4 Create and delegate to a subagent

Run:

```bash
cargo run -p busytok -- delegate \
  --subagent demo \
  --cwd . \
  --profile default \
  "hello from qa"
```

Pass criteria:

- returns `task_id`
- returns subagent identity
- returns terminal status or queued status depending on pressure state
- no unexpected RPC error

### 5.4a Delegate JSON output

Run:

```bash
cargo run -p busytok -- delegate \
  --subagent demo-json \
  --cwd . \
  --profile default \
  --output json \
  "hello in json"
```

Pass criteria:

- command succeeds
- stdout is valid JSON
- payload includes delegate result fields such as task/subagent/status

### 5.4b Delegate invalid output mode

Run:

```bash
cargo run -p busytok -- delegate \
  --subagent demo-invalid \
  --cwd . \
  --profile default \
  --output invalid \
  "should fail at clap parsing"
```

Pass criteria:

- command is rejected by CLI argument parsing
- no RPC call is attempted
- failure clearly indicates the allowed values

### 5.5 Show subagent by name

Run:

```bash
cargo run -p busytok -- subagent show demo --cwd .
```

Pass criteria:

- returns same logical subagent
- status is one of expected states: `hot`, `warm`, `cold`, `deleted`
- metadata is coherent

### 5.6 List tasks

Run:

```bash
cargo run -p busytok -- subagent tasks demo --cwd .
```

Pass criteria:

- task list includes the delegated task
- task status matches delegate output
- summaries and timestamps are present when expected

### 5.7 Hibernate

Run:

```bash
cargo run -p busytok -- subagent hibernate demo --cwd .
```

Pass criteria:

- command succeeds
- acknowledgement includes the resolved subagent id
- subsequent `show` reflects non-hot state

### 5.8 Soft delete

Run:

```bash
cargo run -p busytok -- subagent delete demo --cwd . --yes
```

Pass criteria:

- command succeeds
- `subagent show demo --cwd .` no longer resolves as active
- row is treated as tombstoned, not revived by plain lookup

### 5.9 Hard delete

Create a fresh subagent, then:

```bash
cargo run -p busytok -- subagent delete demo2 --cwd . --hard --yes
```

Pass criteria:

- command succeeds
- follow-up list/show/tasks do not expose the deleted entity
- no partial-delete behavior

### 5.10 Doctor after activity

Run again:

```bash
cargo run -p busytok -- doctor
```

Pass criteria:

- doctor still succeeds after delegate / hibernate / delete activity
- no corruption symptoms

### 5.11 Per-subagent serialization manual check

This verifies that the same logical subagent does not run two tasks concurrently.

Recommended mode:

- use mock mode first
- if a delaying or slow-running fixture is available in the QA environment, use it to widen the overlap window

Example shell sequence:

```bash
cargo run -p busytok -- delegate \
  --subagent serial-demo \
  --cwd . \
  --profile default \
  "first task" &

FIRST_PID=$!
sleep 0.2

cargo run -p busytok -- delegate \
  --subagent serial-demo \
  --cwd . \
  --profile default \
  "second task"

wait "$FIRST_PID"
```

Pass criteria:

- the first task starts normally
- the second task is queued or delayed rather than executing concurrently on the same subagent
- follow-up `subagent tasks serial-demo --cwd .` shows serialized behavior consistent with the queue design

### 5.12 `prompt_artifact_ref` coverage note

The end-to-end `prompt_artifact_ref` path is implemented in the backend and covered by automated tests, but the current CLI does not expose a `--prompt-artifact-ref` flag.

QA expectation:

- verify this path through automated test coverage and RPC-level tests
- do not mark the CLI as failing merely because there is no direct flag for this field yet

This is a current surface-area limitation of the CLI, not a backend contract gap.

---

## 6. High-Priority Behavioral Scenarios

These are the most important scenarios to validate because they map directly to the subagent spec and the recent implementation work.

### 6.1 Name and id resolution contract

Verify:

- `show`, `tasks`, `hibernate`, `delete` accept either name + `--cwd` or `--id`
- they reject invalid combinations
- they do not silently create subagents during lookup-only operations

### 6.2 Tombstone protection

Verify:

- soft-deleted subagents do not resolve as active
- soft-deleted rows are not accidentally revived by later operations

### 6.3 Per-subagent serialization

Verify:

- when one subagent already has a running task, a second task for the same subagent queues instead of running concurrently
- tasks for different subagents may still proceed independently

Suggested manual method:

- use the concurrent shell example in §5.11
- then compare with two different subagent names to confirm cross-subagent independence

### 6.4 Queue-only pressure behavior

Verify:

- under pressure, new work is queued rather than rejected
- delegate returns a queued result instead of a fatal error
- queued work is later dispatched when pressure clears

### 6.5 Crash and recovery behavior

Verify:

- sidecar crash does not permanently wedge the service
- tasks and bindings reconcile correctly
- logical subagent state rolls back consistently

### 6.6 Doctor check realism

Verify:

- doctor does not return false `ok` for missing bundle/runtime resources
- disabled sidecar paths yield `warning` where intended
- enabled-but-broken sidecar paths yield `error`

---

## 7. Observability and What to Inspect

### 7.1 Where logs go

All logs in this guide should be read from:

```bash
$BUSYTOK_LOG_DIR
```

With the recommended test setup:

```bash
/tmp/busytok-subagent-dev/logs
```

### 7.2 Important event families

Look for these event namespaces during debugging:

- `service.startup.*`
- `cli.control.connect.*`
- `subagent.delegate.*`
- `subagent.context.*`
- `subagent.memory.*`
- `subagent.session.*`
- `subagent.status.*`
- `subagent.sidecar.*`
- `subagent.resource.*`
- `subagent.pressure.*`
- `subagent.doctor.*`

### 7.3 Useful symptoms to correlate

If a test fails, capture:

- CLI command used
- full stdout / stderr
- relevant log slice around the same timestamp
- whether sidecar was enabled or disabled
- whether the test was running under isolated XDG env

---

## 8. Failure Triage Guide

### 8.1 CLI cannot connect

Likely causes:

- source `busytok-service` is not running
- CLI was launched without the isolated env variables
- socket belongs to another environment

Checks:

- confirm `cargo run -p busytok-service` is active
- confirm `$XDG_RUNTIME_DIR`
- confirm socket exists under isolated runtime dir

### 8.2 Unexpected interaction with installed release

Likely causes:

- source CLI launched from a shell missing the exported XDG vars
- tester accidentally used installed `busytok` instead of `cargo run -p busytok`

Checks:

- rerun with explicit `cargo run -p ...`
- inspect DB/socket paths under `/tmp/busytok-subagent-dev`

### 8.3 Doctor returns environment-dependent failures

Likely causes:

- sidecar runtime intentionally missing
- bundle/runtime locator not configured
- node runtime mode mismatch

Checks:

- capture `busytok doctor` output
- capture service logs for `subagent.doctor.*`
- confirm whether the scenario is expected to be `warning` or `error`

### 8.4 Queue / pressure tests behave inconsistently

Likely causes:

- using non-isolated environment
- stale DB state from prior run
- pressure thresholds not matching the intended test setup

Checks:

- wipe `/tmp/busytok-subagent-dev`
- rerun from clean env
- confirm test-specific settings

---

## 9. Exit Criteria

The subagent branch is considered QA-passed only if all of the following are true:

- automated repo tests pass
- subagent-targeted automated suites pass
- isolated manual E2E checklist passes
- no conflicts with installed release are observed under isolated setup
- doctor output is consistent with environment state
- logs show expected control / sidecar / pressure behavior
- no tombstone revival, partial delete, or same-subagent concurrency regression is observed

---

## 10. Commands Summary

### Environment

```bash
export BUSYTOK_DEV_ROOT=/tmp/busytok-subagent-dev
rm -rf "$BUSYTOK_DEV_ROOT"
mkdir -p "$BUSYTOK_DEV_ROOT"/{data,config,runtime,logs}
export XDG_DATA_HOME="$BUSYTOK_DEV_ROOT/data"
export XDG_CONFIG_HOME="$BUSYTOK_DEV_ROOT/config"
export XDG_RUNTIME_DIR="$BUSYTOK_DEV_ROOT/runtime"
export BUSYTOK_LOG_DIR="$BUSYTOK_DEV_ROOT/logs"
```

### Start service

```bash
cargo run -p busytok-service
```

### Core QA commands

```bash
cargo run -p busytok -- status
cargo run -p busytok -- doctor
cargo run -p busytok -- subagent list
cargo run -p busytok -- delegate --subagent demo --cwd . --profile default "hello from qa"
cargo run -p busytok -- delegate --subagent demo-json --cwd . --profile default --output json "hello in json"
cargo run -p busytok -- delegate --subagent demo-invalid --cwd . --profile default --output invalid "should fail at clap parsing"
cargo run -p busytok -- subagent show demo --cwd .
cargo run -p busytok -- subagent tasks demo --cwd .
cargo run -p busytok -- subagent hibernate demo --cwd .
cargo run -p busytok -- subagent delete demo --cwd . --yes
```

### Automated verification

```bash
cargo test --workspace
cargo test -p busytok-subagent
cargo test -p busytok-runtime --test subagent_e2e_sidecar
cargo test -p busytok-runtime --test supervisor_control
cargo test -p busytok-store --test subagent_queries
cargo clippy --workspace --tests -- -D warnings
cargo fmt --all -- --check
```
