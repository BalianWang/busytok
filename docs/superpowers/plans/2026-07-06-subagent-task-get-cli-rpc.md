# Subagent Task Get CLI + RPC Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `task_id`-addressable read path so callers can query one subagent task's live status and final result directly after `busytok delegate` returns.

**Architecture:** Reuse the existing `subagent_tasks` row as the single source of truth. Add one narrow read RPC, `subagent.task_get`, that reads the existing task row plus its owning subagent name, expose it through the control dispatcher, and surface it in the CLI as `busytok subagent task --task-id <TASK_ID>` with text and JSON output.

**Tech Stack:** Rust (`busytok-protocol`, `busytok-control`, `busytok-runtime`, `apps/cli`), SQLite via existing `busytok-store` read helpers, `clap` for CLI parsing, `serde`/TS export DTO generation, existing in-process control-server test harnesses.

---

## Global Constraints

- Reuse the existing `subagent_tasks` table and `Database::subagent_get_task()` read path. Do **not** add schema changes, duplicate SQL, or a second task-status source.
- Keep `task_id` as the only lookup key for this feature. Do **not** make callers re-supply `subagent_id` or subagent name.
- Do not add cancel/retry/wait/stream semantics in this plan. This is a read-only status/result lookup.
- Return task data exactly as stored plus a resolved `subagent_name`; do **not** expose provider API keys, base URLs, or other execution-only secrets. The DTO intentionally omits `intent`, `output_schema_name`, `output_schema_version`, and `result_json` from `SubagentTaskRow` — these are execution-internal fields not useful for status display (consistent with the existing `SubagentTaskSummaryDto`). The JSON output includes all DTO fields; the text output is selective for readability.
- **Task-domain not-found must be explicit.** Do **not** reuse `SubagentError::NotFound` /
  `subagent.not_found` for a missing `task_id`; that would produce a misleading
  "logical subagent not found: task_xxx" message. This feature must return a
  task-specific error contract such as code `subagent.task_not_found` with message
  `task not found: <task_id>`.
- Use the current logging system (`tracing`) for runtime-side not-found / lookup failures where helpful; do not add ad-hoc print debugging.
- Rust coverage must remain above 90%: `cargo llvm-cov --workspace --fail-under-lines 90`
- Required green gates:
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`

## File Structure

### Existing files to modify

- `crates/busytok-protocol/src/dto.rs`
  - Add `SubagentTaskGetRequestDto` and `SubagentTaskDetailDto`
- `crates/busytok-protocol/src/methods.rs`
  - Register `subagent.task_get` in the manifest
- `crates/busytok-protocol/src/ts.rs`
  - Export the new DTOs to generated TS declarations
- `crates/busytok-control/src/dispatch.rs`
  - Extend `RuntimeControl`, dispatch routing, and the default test stub
- `crates/busytok-runtime/src/supervisor.rs`
  - Implement the runtime handler that reads one task row + subagent name and maps to the new DTO
- `apps/cli/src/main.rs`
  - Add the new `subagent task` subcommand and wire it to the handler
- `apps/cli/src/commands_subagent.rs`
  - Add the CLI handler + output formatting + focused tests
  - Update two `RuntimeControl` test impls: `SubagentRuntime` + `FailingListRuntime`
- `apps/cli/src/commands/mod.rs`
  - Update the local `RuntimeControl` test wrapper with the new trait method
- `apps/cli/src/commands/models.rs`
  - Update `ModelsRuntime` with the new trait method
- `apps/cli/tests/coverage_gaps.rs`
  - Update the test runtime wrapper with the new trait method
- `apps/cli/tests/prompt.rs`
  - Update the test runtime wrapper with the new trait method
- `apps/cli/tests/coverage_gaps_cli.rs`
  - Update the configurable runtime wrapper with the new trait method
- `crates/busytok-control/tests/coverage_gaps.rs`
  - Update `RuntimeControl` test runtimes: `SettingsValidationRuntime` + `RuntimeWithLatestSeq`
- `crates/busytok-control/tests/server.rs`
  - Update the server test runtime with the new trait method
- `crates/busytok-control/tests/coverage_gaps_dispatch.rs`
  - Update the dispatch test runtimes with the new trait method

### Existing files to reuse without structural changes

- `crates/busytok-store/src/db.rs`
  - Already exposes `subagent_get_task(&self, id: &str) -> Result<Option<SubagentTaskRow>>`
- `crates/busytok-store/src/subagent_queries.rs`
  - Already contains the single-row SQL query

### Test files affected

- `crates/busytok-protocol/src/dto.rs`
- `crates/busytok-protocol/src/methods.rs`
- `crates/busytok-control/src/dispatch.rs`
- `crates/busytok-control/tests/coverage_gaps.rs`
- `crates/busytok-control/tests/server.rs`
- `crates/busytok-control/tests/coverage_gaps_dispatch.rs`
- `crates/busytok-runtime/tests/supervisor_control.rs`
- `apps/cli/src/commands_subagent.rs`
- `apps/cli/src/commands/mod.rs`
- `apps/cli/src/commands/models.rs`
- `apps/cli/tests/coverage_gaps.rs`
- `apps/cli/tests/prompt.rs`
- `apps/cli/tests/coverage_gaps_cli.rs`

## API Shape

### New RPC

Method:

```text
subagent.task_get
```

Request:

```rust
pub struct SubagentTaskGetRequestDto {
    pub task_id: String,
}
```

Response:

```rust
pub struct SubagentTaskDetailDto {
    pub id: String,
    pub subagent_id: String,
    pub subagent_name: Option<String>,
    pub source_harness: Option<String>,
    pub source_session_id: Option<String>,
    pub profile: String,
    pub status: String,
    pub prompt: Option<String>,
    pub prompt_artifact_ref: Option<String>,
    pub result_summary: Option<String>,
    pub error: Option<String>,
    pub error_kind: Option<String>,
    pub model_override: Option<String>,
    pub timeout_seconds: Option<i64>,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
}
```

### New CLI

```bash
busytok subagent task --task-id task_xxx
busytok subagent task --task-id task_xxx --output json
```

Text output target:

```text
id:                task_xxx
subagent_id:       sa_xxx
subagent_name:     review-bot
status:            running
profile:           pi/review-cheap
model_override:    gpt-5
source_harness:    cli
source_session_id: sess_123
created_at_ms:     1750000000000
started_at_ms:     1750000001234
completed_at_ms:   -
result_summary:    -
error:             -
error_kind:        -
```

## Task 1: Add the Protocol DTOs and Method Manifest

**Files:**
- Modify: `crates/busytok-protocol/src/dto.rs`
- Modify: `crates/busytok-protocol/src/methods.rs`
- Modify: `crates/busytok-protocol/src/ts.rs`

- [ ] **Step 1: Write failing protocol tests for the new DTOs**

Add tests in `dto.rs` that:
- serialize `SubagentTaskGetRequestDto { task_id: "task-1".into() }`
- round-trip a fully populated `SubagentTaskDetailDto`
- verify optional fields can be `null` / omitted cleanly

- [ ] **Step 2: Run the focused protocol tests and confirm they fail**

Run:

```bash
cargo test -p busytok-protocol subagent_task
```

Expected: compile/test failure because the DTOs do not exist yet.

- [ ] **Step 3: Add the DTOs to `dto.rs`**

Implementation constraints:
- Place the new request next to other subagent request DTOs
- Place the new detail DTO near `SubagentTaskSummaryDto`
- Derive the same trait set the neighboring DTOs use (`Debug`, `Clone`, `Default` where sensible, `Serialize`, `Deserialize`, `TS`)
- Keep `subagent_name` optional so runtime can degrade gracefully if the parent subagent row is gone

- [ ] **Step 4: Register the RPC method and TS exports**

Implementation:
- Add `"subagent.task_get"` to `method_manifest()` in `methods.rs`
- Add both DTO declarations to `ts.rs`

- [ ] **Step 5: Run the focused protocol tests again**

Run:

```bash
cargo test -p busytok-protocol subagent_task
```

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/busytok-protocol/src/dto.rs crates/busytok-protocol/src/methods.rs crates/busytok-protocol/src/ts.rs
git commit -m "feat(protocol): add subagent task_get DTOs and method"
```

## Task 2: Add the Control-Layer RPC Surface

**Files:**
- Modify: `crates/busytok-control/src/dispatch.rs`
- Modify: `crates/busytok-control/tests/coverage_gaps.rs`
- Modify: `crates/busytok-control/tests/server.rs`
- Modify: `crates/busytok-control/tests/coverage_gaps_dispatch.rs`
- Modify: `apps/cli/src/commands/mod.rs`
- Modify: `apps/cli/src/commands/models.rs`
- Modify: `apps/cli/src/commands_subagent.rs` (two impls: `SubagentRuntime` + `FailingListRuntime`)
- Modify: `apps/cli/tests/coverage_gaps.rs`
- Modify: `apps/cli/tests/prompt.rs`
- Modify: `apps/cli/tests/coverage_gaps_cli.rs`

- [ ] **Step 1: Write failing control dispatch tests**

Add focused tests that:
- dispatch `"subagent.task_get"` to `RuntimeControl::subagent_task_get`
- ensure invalid params are surfaced as a dispatch error

- [ ] **Step 2: Run the focused control tests and confirm they fail**

Run:

```bash
cargo test -p busytok-control subagent_task_get
```

Expected: compile/test failure because the trait method and route do not exist yet.

- [ ] **Step 3: Extend `RuntimeControl` and dispatch routing**

Implementation:
- Add:

```rust
async fn subagent_task_get(
    &self,
    req: busytok_protocol::dto::SubagentTaskGetRequestDto,
) -> Result<busytok_protocol::dto::SubagentTaskDetailDto>;
```

- Add the new route to `Dispatcher::dispatch`
- Update the default stub implementation (`TestRuntimeControl` in `dispatch.rs`) to return `Ok(Default::default())`
- Update the `Arc<T>` forwarding impl in `dispatch.rs`
- Update **every** in-repo `impl RuntimeControl for ...` so the workspace
  still compiles under the expanded trait. The exhaustive list (verified
  via `grep -n "impl RuntimeControl for"` against `main`):
  - `crates/busytok-control/src/dispatch.rs` — `TestRuntimeControl` (default stub) + `Arc<T>` forwarding impl
  - `crates/busytok-control/tests/coverage_gaps.rs` — `SettingsValidationRuntime` + `RuntimeWithLatestSeq`
  - `crates/busytok-control/tests/server.rs` — `MethodDispatchErrorRuntime`
  - `crates/busytok-control/tests/coverage_gaps_dispatch.rs` — `SuccessRuntime` + `AllErrorRuntime`
  - `apps/cli/src/commands/mod.rs` — `TestRuntimeWrapper`
  - `apps/cli/src/commands/models.rs` — `ModelsRuntime`
  - `apps/cli/src/commands_subagent.rs` — `SubagentRuntime` + `FailingListRuntime` (two impls in this file)
  - `apps/cli/tests/coverage_gaps.rs` — `DoctorRuntime`
  - `apps/cli/tests/prompt.rs` — `AliasConflictRuntime`
  - `apps/cli/tests/coverage_gaps_cli.rs` — `ConfigurableRuntime`
  - `crates/busytok-runtime/src/supervisor.rs` — `BusytokSupervisor` (the real impl, implemented in Task 3)

  For test wrappers that delegate to an inner runtime, the method body is
  `self.inner.subagent_task_get(req).await` (or `(**self).subagent_task_get(req).await` for the `Arc<T>` forwarding impl). For stub runtimes that return canned errors (e.g. `AllErrorRuntime`), follow the pattern of the existing methods in that struct.

- [ ] **Step 4: Run the focused control tests again**

Run:

```bash
cargo test -p busytok-control subagent_task_get
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-control/src/dispatch.rs crates/busytok-control/tests/coverage_gaps.rs crates/busytok-control/tests/server.rs crates/busytok-control/tests/coverage_gaps_dispatch.rs apps/cli/src/commands/mod.rs apps/cli/src/commands/models.rs apps/cli/src/commands_subagent.rs apps/cli/tests/coverage_gaps.rs apps/cli/tests/prompt.rs apps/cli/tests/coverage_gaps_cli.rs
git commit -m "feat(control): add subagent task_get dispatch route"
```

## Task 3: Implement the Runtime Handler Using Existing Store Reads

**Files:**
- Modify: `crates/busytok-runtime/src/supervisor.rs`
- Test: `crates/busytok-runtime/tests/supervisor_control.rs`

- [ ] **Step 1: Write failing runtime tests**

Add tests in `supervisor_control.rs` that cover:
- existing task id returns populated `SubagentTaskDetailDto`
- missing task id returns a clear error
- deleted/missing subagent row still returns task data with `subagent_name: None`

Seed the DB using existing row helpers (`SubagentLogicalSubagentRow`, `SubagentTaskRow`) instead of mocking a parallel store path.

- [ ] **Step 2: Run the focused runtime tests and confirm they fail**

Run:

```bash
cargo test -p busytok-runtime subagent_task_get
```

Expected: compile/test failure because the handler does not exist yet.

- [ ] **Step 3: Implement `subagent_task_get` in the supervisor**

Implementation constraints:
- **DB access pattern:** The supervisor's existing subagent handlers route
  through `self.subagent_manager().XXX()`, but `SubagentManager` does not
  expose a `get_task(task_id)` method. Use the supervisor's own DB handle
  directly — `let db = self.db.lock().unwrap();` — the same pattern used
  by the provider/model handlers in `supervisor.rs` (e.g. around line 5699+).
  This avoids adding a one-off method to `SubagentManager` for a read-only
  lookup that the manager doesn't currently support.
- Use `db.subagent_get_task(&req.task_id)` as the canonical lookup
- If the task is missing, return a **task-specific** not-found error contract:
  - machine code: `subagent.task_not_found`
  - human message: `task not found: <task_id>`
  Do **not** reuse `SubagentError::NotFound` / `subagent.not_found`.
  Construct the error via `MethodDispatchError::from_read_error("subagent.task_not_found", format!("task not found: {task_id}"), serde_json::Value::Null)` — the same pattern used by `map_subagent_error` in `supervisor.rs:2668`.
- Resolve `subagent_name` with `db.subagent_get_logical(&task.subagent_id)`; if absent, return `None` rather than failing the whole read. (`SubagentLogicalSubagentRow.name` is the field to use.)
- Map fields one-to-one from `SubagentTaskRow` to `SubagentTaskDetailDto`.
  The DTO omits `intent`, `output_schema_name`, `output_schema_version`,
  and `result_json` from the row (per Global Constraints — consistent with
  `SubagentTaskSummaryDto`).
- Add `tracing::debug!` for successful lookups and `tracing::warn!` for not-found, with stable `event_code` values (e.g. `subagent.task_get.hit` / `subagent.task_get.miss`)

- [ ] **Step 3a: Choose and implement one explicit task-not-found seam**

Allowed implementation shapes:
- add a tiny runtime-local helper that constructs the control error directly, or
- add a dedicated task-domain error variant that maps to `subagent.task_not_found`

Do **not** leave this implicit. The final RPC/CLI surface must never say
`logical subagent not found: task_xxx`.

- [ ] **Step 4: Add a small mapping helper**

Create a private helper in `supervisor.rs`, for example:

```rust
fn subagent_task_detail(
    task: busytok_store::repository::SubagentTaskRow,
    subagent_name: Option<String>,
) -> SubagentTaskDetailDto
```

Keep this separate from `subagent_task_summary(...)`; do not overload the summary mapper.

- [ ] **Step 5: Run the focused runtime tests again**

Run:

```bash
cargo test -p busytok-runtime subagent_task_get
```

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/busytok-runtime/src/supervisor.rs crates/busytok-runtime/tests/supervisor_control.rs
git commit -m "feat(runtime): add subagent task_get handler"
```

## Task 4: Add the CLI Command and Output Formatting

**Files:**
- Modify: `apps/cli/src/main.rs`
- Modify: `apps/cli/src/commands_subagent.rs`

- [ ] **Step 1: Write failing CLI tests**

Add tests covering:
- `handle_task_get()` issues the `subagent.task_get` RPC and prints text output
- JSON output returns the raw DTO
- missing task surfaces the RPC error text
- text formatter renders `-` for absent optional fields

Also add parser coverage for the new subcommand:

```text
busytok subagent task --task-id task-1
```

- [ ] **Step 2: Run the focused CLI tests and confirm they fail**

Run:

```bash
cargo test -p busytok subagent_task
```

Expected: compile/test failure because the subcommand and handler do not exist yet.

- [ ] **Step 3: Add the new subcommand to `main.rs`**

Implementation:

```rust
SubagentCommand::Task {
    #[arg(long)]
    task_id: String,
    #[arg(long, default_value = "text", value_parser = ["json", "text"])]
    output: String,
}
```

Wire it to:

```rust
commands_subagent::handle_task_get(task_id, output).await
```

- [ ] **Step 4: Implement `handle_task_get` in `commands_subagent.rs`**

Implementation sketch:

```rust
pub async fn handle_task_get(task_id: String, output: String) -> Result<()> {
    let mut client = connect().await?;
    let req = SubagentTaskGetRequestDto { task_id };
    let resp = client
        .call(ControlRequest::new(
            "subagent.task_get",
            serde_json::to_value(&req)?,
        ))
        .await
        .context("subagent.task_get RPC failed")?;
    let data = unwrap_ok(resp)?;
    print_task_detail(&data, &output)
}
```

- [ ] **Step 5: Add a dedicated `print_task_detail` formatter**

Do **not** reuse `print_detail()` because that formatter is for subagent identity, not task records.

Format all optional fields through a small helper:

```rust
fn or_dash(v: Option<&str>) -> &str { v.unwrap_or("-") }
```

Fields to print:
- id
- subagent_id
- subagent_name
- status
- profile
- model_override
- source_harness
- source_session_id
- created_at_ms
- started_at_ms
- completed_at_ms
- result_summary
- error
- error_kind

- [ ] **Step 6: Run the focused CLI tests again**

Run:

```bash
cargo test -p busytok subagent_task
```

Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add apps/cli/src/main.rs apps/cli/src/commands_subagent.rs
git commit -m "feat(cli): add subagent task lookup by task id"
```

## Task 5: Full Verification and Coverage Gate

**Files:**
- No new files; verification only

- [ ] **Step 1: Run formatter**

```bash
cargo fmt --all
```

Expected: no diff after formatting fixes are applied

- [ ] **Step 2: Run clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS

- [ ] **Step 3: Run full Rust test suite**

```bash
cargo test --workspace
```

Expected: PASS

- [ ] **Step 4: Run Rust coverage gate**

```bash
cargo llvm-cov --workspace --fail-under-lines 90
```

Expected: PASS

- [ ] **Step 5: Final commit (if verification required follow-up fixes)**

```bash
git add -A
git commit -m "test: verify subagent task_get integration"
```

Use this step only if the verification pass required code or test fixes after Task 4. If no files changed, skip this commit.

## Acceptance Criteria

- `busytok delegate` returning `task_id` is now paired with a direct lookup path:
  - `busytok subagent task --task-id <id>`
- The new RPC `subagent.task_get` returns the persisted task state from `subagent_tasks`
- Queued tasks can be queried directly by `task_id` and observed transitioning through:
  - `queued`
  - `running`
  - `completed` / `failed` / `cancelled`
- Text CLI output is stable and human-readable
- JSON CLI output matches `SubagentTaskDetailDto`
- Missing task ids return clear errors
- Missing task ids specifically surface `subagent.task_not_found` / `task not found: <task_id>`
- No schema migration is added
- All Rust quality gates remain green, including the >90% coverage threshold

## Out of Scope

- `task_id`-based cancellation
- `task_id`-based retry
- blocking `wait --task-id ...`
- push/subscription-based task completion notifications
- returning full `result_json` artifacts or expanding artifact fetch
- adding provider/model execution internals to the task detail read
