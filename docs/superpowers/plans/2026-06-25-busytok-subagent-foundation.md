# Busytok Logical Subagent Foundation — Implementation Plan (Step 1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the SQLite schema, the `busytok-subagent` crate, the logical-subagent management layer, control-protocol wiring, and CLI commands so that `busytok delegate` / `busytok subagent …` work end-to-end with a **mock** task executor (no Pi sidecar yet).

**Architecture:** A new `busytok-subagent` crate owns the domain models and `SubagentManager`. It reuses the existing single-connection `Arc<Mutex<Database>>` store (new tables via migration `0002_subagent.sql`, query functions in a new `subagent_queries` store module following the existing `&Connection` query-fn pattern). `SubagentManager` is constructed and held by `BusytokSupervisor`; the control dispatcher exposes `subagent.*` methods that dispatch into the manager via the existing `RuntimeControl` trait. CLI commands call the service over the existing `busytok-control` IPC.

**Tech Stack:** Rust 1.88, edition 2021, rusqlite 0.39 (bundled), tokio 1, serde/serde_json, clap 4, tracing. Tests: built-in `#[test]` / `#[tokio::test]`, `tempfile`, in-memory SQLite. Coverage: `cargo-llvm-cov`, target ≥ 90% lines, CI gate 85%.

This is **Plan 1 of 5**. It corresponds to spec Step 1. Plans 2–5 (Pi sidecar, session pool, context/memory, resource monitoring) build on this foundation.

---

## Global Constraints

Copied verbatim from the approved spec — every task implicitly carries these.

- **Timestamp columns MUST use the `_ms` suffix** (millisecond epoch, `INTEGER`), matching the existing schema convention (`usage_events.created_at_ms`, etc.). Never bare `created_at`.
- **Schema**: single `busytok.db`; new migration file `crates/busytok-store/migrations/0002_subagent.sql`; bump `SCHEMA_VERSION` 1 → 2; register in `schema::migrations()`; all new tables use the `subagent_` prefix.
- **Crate boundary**: `busytok-subagent` does NOT depend on `busytok-runtime`. `busytok-runtime` depends on `busytok-subagent`. Schema + row structs + query fns live in `busytok-store`.
- **DB access**: `SubagentManager` holds a clone of the existing `Arc<std::sync::Mutex<Database>>` (the supervisor's `db` field is `Arc<std::sync::Mutex<Database>>`, NOT `tokio::sync::Mutex`). Manager methods lock with `self.db.lock().unwrap()` (synchronous std mutex — the work inside is synchronous SQLite calls, no `.await` held across the lock) and call the synchronous `Database` query methods. No new connection, no connection pool, no new writer-actor variants. The store's `Database.conn` field is **private**; always use the `self.conn()` accessor (returns `&Connection`), never `&self.conn`.
- **Logging**: use `tracing` with structured `event_code = "subagent.<area>.<event>"` fields on every log event. Match the existing convention (e.g. `service_app.rs`).
- **New control method = touch all four**: `RuntimeControl` trait method, `ControlDispatcher::dispatch` match arm, `Arc<T>` blanket impl, `method_manifest()` entry.
- **Config**: nested `SubagentSettings` follows the existing `PrivacySettings` pattern — `#[derive(Debug, Clone, Serialize, Deserialize)]` + `impl Default`, every field `#[serde(default = …)]`. TOML `[subagent.*]` sections.
- **subagent commands do NOT support offline mode.** If the service is down, return a clear error.
- **MVP scope of this plan**: `session.turn_auto` / `session.prepare_hibernate` / `session.close` are NOT implemented here (sidecar is mock). `delegate` records the task, returns a mock result, updates memory + status. Real Pi execution is Plan 2.
- **No `any`/TODO/placeholders.** Run `cargo fmt`, `cargo clippy`, and `./scripts/coverage.sh` (or the relevant test command) before each commit.

---

## File Structure

New crate `crates/busytok-subagent/`:
- `Cargo.toml` — manifest, `dep.workspace = true`, `busytok-store`/`busytok-config` deps.
- `src/lib.rs` — re-exports `Manager`, `models`, `error`.
- `src/models.rs` — `LogicalSubagent`, `SubagentMemory`, `SubagentTask`, `SubagentStatus`, `TaskStatus`, plus request/result types (`DelegateRequest`, `DelegateResult`, `MockTaskResult`).
- `src/error.rs` — `SubagentError` (thiserror), maps to control error codes.
- `src/manager.rs` — `SubagentManager` (the public API: `delegate`, `list`, `show`, `tasks`, `hibernate`, `delete`).
- `src/resolver.rs` — name → id resolution (`resolve_by_name`, `resolve_by_id`).
- `src/mock_executor.rs` — the mock task executor used in this plan (replaced by the sidecar client in Plan 2).

Modified store crate `crates/busytok-store/`:
- `src/schema.rs` — `SCHEMA_VERSION = 2`, `SUBAGENT_SQL` constant, register in `migrations()`.
- `migrations/0002_subagent.sql` — the 6 tables + indices + CHECK constraints.
- `src/subagent_queries.rs` — **new module**: `&Connection` query functions (upserts + reads).
- `src/repository.rs` — 6 new row structs (`SubagentLogicalSubagentRow`, `SubagentMemoryRow`, `SubagentTaskRow`, `SubagentHarnessBindingRow`, `SubagentUsageRecordRow`, `SubagentResourceEventRow`) with `for_test` constructors.
- `src/lib.rs` — `pub mod subagent_queries;` + re-export new row structs.
- `src/db.rs` — `Database` thin wrapper methods for the new queries.
- `tests/migrations.rs` — update `baseline_single_migration()` count, add `subagent_migration_creates_tables()`.

Modified config crate `crates/busytok-config/`:
- `src/lib.rs` — add `subagent: SubagentSettings` field to `BusytokSettings`; define `SubagentSettings`, `SubagentPiSidecarConfig`, `SubagentContextConfig`, `SubagentModelsConfig`, `SubagentProfileConfig`.
- `src/paths.rs` — `artifacts_dir()`; update `ensure_dirs_exist()` to create artifacts dir. (`sidecar_runtime_dir()` deferred to Plan 2.)

Modified protocol crate `crates/busytok-protocol/`:
- `src/dto.rs` — new request/response DTOs (`SubagentDelegateRequestDto`, `SubagentListResponseDto`, …).
- `src/methods.rs` — append `subagent.*` entries.

Modified control crate `crates/busytok-control/`:
- `src/dispatch.rs` — 7 match arms in `dispatch()`; trait methods + `Arc<T>` blanket impl; `TestRuntimeControl` stubs.

Modified runtime crate `crates/busytok-runtime/`:
- `src/supervisor.rs` — hold `subagent_manager: Arc<SubagentManager>`; implement new `RuntimeControl` methods.

Modified CLI `apps/cli/`:
- `src/main.rs` — new `Delegate` command + `Subagent` subcommand group. (`Doctor` is deferred — not in this plan.)
- `src/commands_subagent.rs` — **new**: handlers calling `ControlClient` (sibling to the existing flat `commands.rs`).

---

## Task Dependency Order

Tasks must be executed in order; each builds on the previous task's `Produces` interface.

1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11 → 12

---

### Task 1: Create the `busytok-subagent` crate scaffold

**Files:**
- Create: `crates/busytok-subagent/Cargo.toml`
- Create: `crates/busytok-subagent/src/lib.rs`
- Create: `crates/busytok-subagent/src/error.rs`
- Modify: `Cargo.toml` (root workspace `members`)

**Interfaces:**
- Consumes: nothing yet.
- Produces: an empty compilable crate `busytok-subagent` exporting `pub mod error;` and an `Error` type.

- [ ] **Step 1: Create `crates/busytok-subagent/Cargo.toml`**

```toml
[package]
name = "busytok-subagent"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
busytok-store = { path = "../busytok-store" }
busytok-config = { path = "../busytok-config" }
busytok-domain = { path = "../busytok-domain" }
anyhow.workspace = true
async-trait.workspace = true
serde = { workspace = true }
serde_json.workspace = true
thiserror.workspace = true
tokio = { workspace = true }
tracing.workspace = true
uuid.workspace = true

[dev-dependencies]
tempfile.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }

[lints.rust]
unexpected_cfgs = "allow"
```

`[workspace.package]` in the root `Cargo.toml` defines only `edition`, `rust-version`, `license` (no `version`), so this crate sets `version = "0.1.0"` explicitly. `edition`/`rust-version`/`license` inherit via `.workspace = true`. The root already pins `uuid = { version = "1", features = ["v4"] }`.

- [ ] **Step 2: Create `crates/busytok-subagent/src/error.rs`**

```rust
//! Errors for the logical-subagent management layer.
//!
//! Each variant maps to a control-protocol error code so the dispatcher can
//! surface a stable, machine-readable failure to clients.

use thiserror::Error;

/// A logical-subagent management error.
///
/// `code()` returns the stable string emitted in `ControlResponse` payloads.
#[derive(Debug, Error)]
pub enum SubagentError {
    #[error("logical subagent not found: {0}")]
    NotFound(String),

    #[error("ambiguous subagent name: {0}")]
    AmbiguousName(String),

    #[error("invalid subagent name: {0}")]
    InvalidName(String),

    #[error("profile not found: {0}")]
    ProfileNotFound(String),

    #[error("subagent feature is disabled")]
    Disabled,

    #[error("database error")]
    Store(#[from] anyhow::Error),
}

impl SubagentError {
    /// Stable machine-readable code used by the control dispatcher.
    pub fn code(&self) -> &'static str {
        match self {
            SubagentError::NotFound(_) => "subagent.not_found",
            SubagentError::AmbiguousName(_) => "subagent.ambiguous_name",
            SubagentError::InvalidName(_) => "subagent.invalid_name",
            SubagentError::ProfileNotFound(_) => "subagent.profile_not_found",
            SubagentError::Disabled => "subagent.disabled",
            SubagentError::Store(_) => "subagent.store_error",
        }
    }
}

pub type Result<T> = std::result::Result<T, SubagentError>;
```

- [ ] **Step 3: Create `crates/busytok-subagent/src/lib.rs`**

```rust
//! Busytok logical-subagent runtime.
//!
//! Owns long-lived subagent identity, memory, and task history. In this plan
//! (Step 1) task execution is a mock; the Pi sidecar executor lands in Plan 2.

pub mod error;
pub mod models;

pub use error::{Result, SubagentError};
```

- [ ] **Step 4: Add a placeholder `models.rs` so the crate compiles**

Create `crates/busytok-subagent/src/models.rs`:

```rust
//! Domain models for logical subagents. Populated in Task 3.
```

- [ ] **Step 5: Register the crate in the root workspace**

Modify `/Users/wsd/Data/Busytok/busytok/Cargo.toml` — append `"crates/busytok-subagent",` to the `members` array (alphabetical, after `busytok-store`).

- [ ] **Step 6: Verify it builds and lints**

Run: `cargo build -p busytok-subagent`
Expected: builds with no errors.

Run: `cargo fmt -p busytok-subagent -- --check`
Expected: no output (already formatted).

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/busytok-subagent
git commit -m "feat(subagent): scaffold busytok-subagent crate"
```

---

### Task 2: Migration `0002_subagent.sql` + schema registration

**Files:**
- Create: `crates/busytok-store/migrations/0002_subagent.sql`
- Modify: `crates/busytok-store/src/schema.rs`
- Modify: `crates/busytok-store/tests/migrations.rs`

**Interfaces:**
- Consumes: existing `schema::migrations()` / `SCHEMA_VERSION` pattern.
- Produces: schema v2 with 6 new tables, queryable via `Database::open_in_memory()`.

- [ ] **Step 1: Write the failing migration test**

Append to `crates/busytok-store/tests/migrations.rs`:

```rust
#[test]
fn subagent_migration_creates_tables() {
    let db = Database::open_in_memory().unwrap();
    let tables = db.table_names().unwrap();
    for name in [
        "subagent_logical_subagents",
        "subagent_memory",
        "subagent_tasks",
        "subagent_harness_bindings",
        "subagent_usage_records",
        "subagent_resource_events",
    ] {
        assert!(
            tables.contains(&name.to_string()),
            "missing table {name}"
        );
    }
}

#[test]
fn schema_version_is_two() {
    assert_eq!(schema::SCHEMA_VERSION, 2);
    let max_version = schema::migrations().iter().map(|(v, _)| *v).max().unwrap();
    assert_eq!(max_version, schema::SCHEMA_VERSION);
}
```

Also update the existing `baseline_single_migration()` test — rename it and fix the count:

```rust
#[test]
fn migrations_registered_in_order() {
    assert_eq!(schema::migrations().len(), 2, "expected baseline + subagent migrations");
    assert_eq!(schema::migrations()[0].0, 1);
    assert_eq!(schema::migrations()[1].0, 2);
    assert_eq!(schema::SCHEMA_VERSION, 2);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p busytok-store --test migrations`
Expected: FAIL — `subagent_*` tables missing, version still 1.

- [ ] **Step 3: Write `crates/busytok-store/migrations/0002_subagent.sql`**

```sql
-- Logical subagent runtime schema (Step 1).
-- See docs/superpowers/specs/2026-06-25-busytok-pi-sidecar-logical-subagent-design.md

CREATE TABLE subagent_logical_subagents (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    project_id TEXT NOT NULL,
    repo_path TEXT NOT NULL,
    repo_hash TEXT NOT NULL,
    branch TEXT,
    intent TEXT,
    default_profile TEXT NOT NULL,
    default_model TEXT,
    status TEXT NOT NULL DEFAULT 'cold'
        CHECK (status IN ('hot', 'warm', 'cold', 'deleted')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    last_active_at_ms INTEGER
);

CREATE INDEX idx_subagent_logical_project
    ON subagent_logical_subagents(project_id, repo_hash, status);
CREATE INDEX idx_subagent_logical_last_active
    ON subagent_logical_subagents(last_active_at_ms);
CREATE UNIQUE INDEX idx_subagent_unique_active_name
    ON subagent_logical_subagents(project_id, repo_hash, name)
    WHERE status != 'deleted';

CREATE TABLE subagent_memory (
    id TEXT PRIMARY KEY,
    subagent_id TEXT NOT NULL UNIQUE,
    hot_summary TEXT,
    long_summary TEXT,
    key_files_json TEXT,
    decisions_json TEXT,
    attempts_json TEXT,
    open_questions_json TEXT,
    artifact_refs_json TEXT,
    last_compacted_at_ms INTEGER,
    last_compacted_task_id TEXT,
    updated_at_ms INTEGER NOT NULL,
    FOREIGN KEY (subagent_id) REFERENCES subagent_logical_subagents(id)
);

CREATE TABLE subagent_tasks (
    id TEXT PRIMARY KEY,
    subagent_id TEXT NOT NULL,
    source_harness TEXT,
    source_session_id TEXT,
    intent TEXT,
    profile TEXT NOT NULL,
    prompt TEXT,
    prompt_artifact_ref TEXT,
    output_schema_name TEXT,
    output_schema_version INTEGER DEFAULT 1,
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
    result_summary TEXT,
    result_json TEXT,
    error TEXT,
    created_at_ms INTEGER NOT NULL,
    started_at_ms INTEGER,
    completed_at_ms INTEGER,
    CHECK (prompt IS NOT NULL OR prompt_artifact_ref IS NOT NULL),
    FOREIGN KEY (subagent_id) REFERENCES subagent_logical_subagents(id)
);

CREATE INDEX idx_subagent_tasks_subagent ON subagent_tasks(subagent_id, created_at_ms);
CREATE INDEX idx_subagent_tasks_status ON subagent_tasks(status, created_at_ms);
CREATE INDEX idx_subagent_tasks_source ON subagent_tasks(source_harness, source_session_id);

CREATE TABLE subagent_harness_bindings (
    id TEXT PRIMARY KEY,
    subagent_id TEXT NOT NULL,
    harness TEXT NOT NULL,
    adapter_session_id TEXT,
    adapter_process_id TEXT,
    is_hot INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'warm'
        CHECK (status IN ('hot', 'warm', 'closed', 'crashed')),
    created_at_ms INTEGER NOT NULL,
    last_used_at_ms INTEGER,
    closed_at_ms INTEGER,
    detail_json TEXT,
    FOREIGN KEY (subagent_id) REFERENCES subagent_logical_subagents(id)
);

CREATE UNIQUE INDEX idx_subagent_binding_one_hot
    ON subagent_harness_bindings(subagent_id, harness)
    WHERE is_hot = 1;
CREATE INDEX idx_subagent_bindings_hot
    ON subagent_harness_bindings(subagent_id, is_hot);

CREATE TABLE subagent_usage_records (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    subagent_id TEXT NOT NULL,
    source_usage_event_id TEXT,
    harness TEXT NOT NULL,
    provider TEXT,
    model TEXT,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_read_tokens INTEGER,
    cache_write_tokens INTEGER,
    total_cost_usd REAL,
    duration_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    FOREIGN KEY (task_id) REFERENCES subagent_tasks(id),
    FOREIGN KEY (subagent_id) REFERENCES subagent_logical_subagents(id)
);

CREATE INDEX idx_subagent_usage_task ON subagent_usage_records(task_id);

CREATE TABLE subagent_resource_events (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    target_id TEXT,
    rss_mb REAL,
    cpu_percent REAL,
    detail_json TEXT,
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX idx_subagent_events_type ON subagent_resource_events(event_type, created_at_ms);
CREATE INDEX idx_subagent_events_target ON subagent_resource_events(target_id, created_at_ms);
```

- [ ] **Step 4: Register the migration in `schema.rs`**

Modify `crates/busytok-store/src/schema.rs`:

```rust
pub const SCHEMA_VERSION: u32 = 2;

pub const CREATE_SCHEMA_VERSION_TABLE: &str = "\
    CREATE TABLE IF NOT EXISTS _schema_version (\
        version INTEGER PRIMARY KEY, \
        applied_at_ms INTEGER NOT NULL\
    );\
";

pub const BASELINE_SQL: &str = include_str!("../migrations/0001_baseline.sql");
pub const SUBAGENT_SQL: &str = include_str!("../migrations/0002_subagent.sql");

pub fn migrations() -> Vec<(u32, &'static str)> {
    vec![(1, BASELINE_SQL), (2, SUBAGENT_SQL)]
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p busytok-store --test migrations`
Expected: PASS — all 3 tests green.

- [ ] **Step 6: Run the full store test suite to confirm no regressions**

Run: `cargo test -p busytok-store`
Expected: all tests pass (existing tests unaffected — migration is additive).

- [ ] **Step 7: Commit**

```bash
git add crates/busytok-store
git commit -m "feat(store): add 0002_subagent schema migration (logical subagent tables)"
```

---

### Task 3: Store row structs for the subagent tables

**Files:**
- Modify: `crates/busytok-store/src/repository.rs`
- Modify: `crates/busytok-store/src/lib.rs`

**Interfaces:**
- Consumes: schema from Task 2.
- Produces: 6 `…Row` structs + `for_test` constructors, re-exported from the store crate.

- [ ] **Step 1: Write a failing row-construction test**

Append to `crates/busytok-store/tests/migrations.rs` (or a new `tests/subagent_rows.rs`):

```rust
use busytok_store::repository::{
    SubagentLogicalSubagentRow, SubagentMemoryRow, SubagentTaskRow,
};

#[test]
fn subagent_row_for_test_constructors_build_minimal_rows() {
    let sa = SubagentLogicalSubagentRow::for_test("sa-1", "reviewer");
    assert_eq!(sa.id, "sa-1");
    assert_eq!(sa.name, "reviewer");
    assert_eq!(sa.status, "cold");

    let mem = SubagentMemoryRow::for_test("sa-1");
    assert_eq!(mem.subagent_id, "sa-1");

    let task = SubagentTaskRow::for_test("task-1", "sa-1", "pi/search-cheap", "find the bug");
    assert_eq!(task.prompt, Some("find the bug".to_string()));
    assert_eq!(task.status, "queued");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p busytok-store --test subagent_rows`
Expected: FAIL — structs not defined.

- [ ] **Step 3: Add the 6 row structs to `repository.rs`**

Append to `crates/busytok-store/src/repository.rs`. Follow the existing plain-`#[derive(Debug, Clone)]` convention; `Option` for nullable columns; `i32` for boolean-as-integer columns.

```rust
// ---------------------------------------------------------------------------
// Logical subagent runtime rows (migration 0002)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SubagentLogicalSubagentRow {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub repo_path: String,
    pub repo_hash: String,
    pub branch: Option<String>,
    pub intent: Option<String>,
    pub default_profile: String,
    pub default_model: Option<String>,
    pub status: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_active_at_ms: Option<i64>,
}

impl SubagentLogicalSubagentRow {
    /// Minimal row for tests. Timestamps seeded from `now_ms()`; status `cold`.
    pub fn for_test(id: &str, name: &str) -> Self {
        let now = busytok_domain::now_ms();
        Self {
            id: id.to_string(),
            name: name.to_string(),
            project_id: "repo-hash-test".to_string(),
            repo_path: "/tmp/repo".to_string(),
            repo_hash: "repo-hash-test".to_string(),
            branch: None,
            intent: None,
            default_profile: "pi/search-cheap".to_string(),
            default_model: None,
            status: "cold".to_string(),
            created_at_ms: now,
            updated_at_ms: now,
            last_active_at_ms: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubagentMemoryRow {
    pub id: String,
    pub subagent_id: String,
    pub hot_summary: Option<String>,
    pub long_summary: Option<String>,
    pub key_files_json: Option<String>,
    pub decisions_json: Option<String>,
    pub attempts_json: Option<String>,
    pub open_questions_json: Option<String>,
    pub artifact_refs_json: Option<String>,
    pub last_compacted_at_ms: Option<i64>,
    pub last_compacted_task_id: Option<String>,
    pub updated_at_ms: i64,
}

impl SubagentMemoryRow {
    pub fn for_test(subagent_id: &str) -> Self {
        Self {
            id: format!("mem-{subagent_id}"),
            subagent_id: subagent_id.to_string(),
            hot_summary: None,
            long_summary: None,
            key_files_json: None,
            decisions_json: None,
            attempts_json: None,
            open_questions_json: None,
            artifact_refs_json: None,
            last_compacted_at_ms: None,
            last_compacted_task_id: None,
            updated_at_ms: busytok_domain::now_ms(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubagentTaskRow {
    pub id: String,
    pub subagent_id: String,
    pub source_harness: Option<String>,
    pub source_session_id: Option<String>,
    pub intent: Option<String>,
    pub profile: String,
    pub prompt: Option<String>,
    pub prompt_artifact_ref: Option<String>,
    pub output_schema_name: Option<String>,
    pub output_schema_version: i64,
    pub status: String,
    pub result_summary: Option<String>,
    pub result_json: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
}

impl SubagentTaskRow {
    pub fn for_test(id: &str, subagent_id: &str, profile: &str, prompt: &str) -> Self {
        Self {
            id: id.to_string(),
            subagent_id: subagent_id.to_string(),
            source_harness: None,
            source_session_id: None,
            intent: None,
            profile: profile.to_string(),
            prompt: Some(prompt.to_string()),
            prompt_artifact_ref: None,
            output_schema_name: None,
            output_schema_version: 1,
            status: "queued".to_string(),
            result_summary: None,
            result_json: None,
            error: None,
            created_at_ms: busytok_domain::now_ms(),
            started_at_ms: None,
            completed_at_ms: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubagentHarnessBindingRow {
    pub id: String,
    pub subagent_id: String,
    pub harness: String,
    pub adapter_session_id: Option<String>,
    pub adapter_process_id: Option<String>,
    pub is_hot: i32,
    pub status: String,
    pub created_at_ms: i64,
    pub last_used_at_ms: Option<i64>,
    pub closed_at_ms: Option<i64>,
    pub detail_json: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SubagentUsageRecordRow {
    pub id: String,
    pub task_id: String,
    pub subagent_id: String,
    pub source_usage_event_id: Option<String>,
    pub harness: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub total_cost_usd: Option<f64>,
    pub duration_ms: Option<i64>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct SubagentResourceEventRow {
    pub id: String,
    pub event_type: String,
    pub target_id: Option<String>,
    pub rss_mb: Option<f64>,
    pub cpu_percent: Option<f64>,
    pub detail_json: Option<String>,
    pub created_at_ms: i64,
}
```

- [ ] **Step 4: Re-export the structs from `lib.rs`**

In `crates/busytok-store/src/lib.rs`, add to the existing `pub use repository::{ … };` block:

```rust
pub use repository::{
    SubagentHarnessBindingRow, SubagentLogicalSubagentRow, SubagentMemoryRow,
    SubagentResourceEventRow, SubagentTaskRow, SubagentUsageRecordRow,
};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p busytok-store --test subagent_rows`
Expected: PASS.

Run: `cargo build -p busytok-store`
Expected: clean build.

- [ ] **Step 6: Commit**

```bash
git add crates/busytok-store/src/repository.rs crates/busytok-store/src/lib.rs crates/busytok-store/tests/subagent_rows.rs
git commit -m "feat(store): add subagent row structs"
```

---

### Task 4: Subagent query functions + `Database` wrappers

**Files:**
- Create: `crates/busytok-store/src/subagent_queries.rs`
- Modify: `crates/busytok-store/src/lib.rs`
- Modify: `crates/busytok-store/src/db.rs`

**Interfaces:**
- Consumes: row structs (Task 3).
- Produces: `&Connection` query functions and `Database::subagent_*` methods used by `SubagentManager` in Tasks 7–8. Key signatures are listed below.

- [ ] **Step 1: Write failing query tests**

Create `crates/busytok-store/tests/subagent_queries.rs`:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]

use busytok_store::repository::{SubagentLogicalSubagentRow, SubagentTaskRow};
use busytok_store::Database;

fn db() -> Database {
    Database::open_in_memory().unwrap()
}

#[test]
fn upsert_then_get_logical_subagent_round_trips() {
    let db = db();
    let mut row = SubagentLogicalSubagentRow::for_test("sa-1", "reviewer");
    row.status = "hot".to_string();
    db.subagent_upsert_logical(&row).unwrap();

    let got = db.subagent_get_logical("sa-1").unwrap().unwrap();
    assert_eq!(got.name, "reviewer");
    assert_eq!(got.status, "hot");
}

#[test]
fn list_active_subagents_excludes_deleted() {
    let db = db();
    let mut a = SubagentLogicalSubagentRow::for_test("sa-a", "a");
    a.repo_hash = "h".to_string();
    a.project_id = "h".to_string();
    let mut b = SubagentLogicalSubagentRow::for_test("sa-b", "b");
    b.repo_hash = "h".to_string();
    b.project_id = "h".to_string();
    b.status = "deleted".to_string();
    db.subagent_upsert_logical(&a).unwrap();
    db.subagent_upsert_logical(&b).unwrap();

    let active = db.subagent_list_active_by_repo("h").unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].name, "a");
}

#[test]
fn unique_active_name_per_repo_rejects_duplicate() {
    let db = db();
    let mut a = SubagentLogicalSubagentRow::for_test("sa-a", "dup");
    a.repo_hash = "h".to_string();
    a.project_id = "h".to_string();
    let mut b = SubagentLogicalSubagentRow::for_test("sa-b", "dup");
    b.repo_hash = "h".to_string();
    b.project_id = "h".to_string();
    db.subagent_upsert_logical(&a).unwrap();
    // second active row with same (project, repo, name) must violate the partial unique index
    let err = db.subagent_upsert_logical(&b).unwrap_err();
    assert!(format!("{err}").to_lowercase().contains("unique"));
}

#[test]
fn insert_task_and_mark_completed_round_trips() {
    let db = db();
    let sa = SubagentLogicalSubagentRow::for_test("sa-1", "r");
    db.subagent_upsert_logical(&sa).unwrap();
    let task = SubagentTaskRow::for_test("t-1", "sa-1", "pi/search-cheap", "go");
    db.subagent_insert_task(&task).unwrap();

    db.subagent_set_task_status("t-1", "completed", Some("done".to_string()), None).unwrap();
    let got = db.subagent_get_task("t-1").unwrap().unwrap();
    assert_eq!(got.status, "completed");
    assert_eq!(got.result_summary.as_deref(), Some("done"));
    assert!(got.completed_at_ms.is_some());
}

#[test]
fn memory_upsert_is_idempotent_on_subagent_id() {
    let db = db();
    let sa = SubagentLogicalSubagentRow::for_test("sa-1", "r");
    db.subagent_upsert_logical(&sa).unwrap();
    let mut mem = busytok_store::repository::SubagentMemoryRow::for_test("sa-1");
    mem.hot_summary = Some("first".to_string());
    db.subagent_upsert_memory(&mem).unwrap();
    mem.hot_summary = Some("second".to_string());
    db.subagent_upsert_memory(&mem).unwrap();

    let got = db.subagent_get_memory("sa-1").unwrap().unwrap();
    assert_eq!(got.hot_summary.as_deref(), Some("second"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p busytok-store --test subagent_queries`
Expected: FAIL — methods not defined.

- [ ] **Step 3: Create `crates/busytok-store/src/subagent_queries.rs`**

All functions take `&Connection` (run inside the caller's transaction or standalone). Mirror the existing query-fn style: `rusqlite::params!`, `anyhow::Context`.

```rust
//! SQL query functions for the logical-subagent runtime tables.
//!
//! Each function takes a `&rusqlite::Connection` so it can run inside the
//! caller's transaction. `Database` thin wrappers live in `db.rs`.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::repository::{
    SubagentHarnessBindingRow, SubagentLogicalSubagentRow, SubagentMemoryRow, SubagentResourceEventRow,
    SubagentTaskRow, SubagentUsageRecordRow,
};

// --- logical_subagents -----------------------------------------------------

pub fn upsert_logical_subagent(conn: &Connection, row: &SubagentLogicalSubagentRow) -> Result<()> {
    conn.execute(
        "INSERT INTO subagent_logical_subagents \
             (id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, \
              default_model, status, created_at_ms, updated_at_ms, last_active_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
         ON CONFLICT(id) DO UPDATE SET \
             name=excluded.name, project_id=excluded.project_id, repo_path=excluded.repo_path, \
             repo_hash=excluded.repo_hash, branch=excluded.branch, intent=excluded.intent, \
             default_profile=excluded.default_profile, default_model=excluded.default_model, \
             status=excluded.status, updated_at_ms=excluded.updated_at_ms, \
             last_active_at_ms=excluded.last_active_at_ms",
        params![
            row.id, row.name, row.project_id, row.repo_path, row.repo_hash, row.branch,
            row.intent, row.default_profile, row.default_model, row.status,
            row.created_at_ms, row.updated_at_ms, row.last_active_at_ms,
        ],
    )
    .map(|_| ())
    .with_context(|| format!("upsert logical subagent {}", row.id))
}

pub fn get_logical_subagent(conn: &Connection, id: &str) -> Result<Option<SubagentLogicalSubagentRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, \
                default_model, status, created_at_ms, updated_at_ms, last_active_at_ms \
         FROM subagent_logical_subagents WHERE id = ?1",
    )?;
    let row_opt = stmt
        .query_row(params![id], |row| {
            Ok(SubagentLogicalSubagentRow {
                id: row.get(0)?,
                name: row.get(1)?,
                project_id: row.get(2)?,
                repo_path: row.get(3)?,
                repo_hash: row.get(4)?,
                branch: row.get(5)?,
                intent: row.get(6)?,
                default_profile: row.get(7)?,
                default_model: row.get(8)?,
                status: row.get(9)?,
                created_at_ms: row.get(10)?,
                updated_at_ms: row.get(11)?,
                last_active_at_ms: row.get(12)?,
            })
        })
        .ok();
    Ok(row_opt)
}

pub fn list_active_by_repo(conn: &Connection, repo_hash: &str) -> Result<Vec<SubagentLogicalSubagentRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, \
                default_model, status, created_at_ms, updated_at_ms, last_active_at_ms \
         FROM subagent_logical_subagents \
         WHERE repo_hash = ?1 AND status != 'deleted' \
         ORDER BY last_active_at_ms DESC NULLS LAST",
    )?;
    let rows = stmt
        .query_map(params![repo_hash], |row| {
            Ok(SubagentLogicalSubagentRow {
                id: row.get(0)?,
                name: row.get(1)?,
                project_id: row.get(2)?,
                repo_path: row.get(3)?,
                repo_hash: row.get(4)?,
                branch: row.get(5)?,
                intent: row.get(6)?,
                default_profile: row.get(7)?,
                default_model: row.get(8)?,
                status: row.get(9)?,
                created_at_ms: row.get(10)?,
                updated_at_ms: row.get(11)?,
                last_active_at_ms: row.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn find_by_name_in_repo(
    conn: &Connection,
    project_id: &str,
    repo_hash: &str,
    name: &str,
) -> Result<Vec<SubagentLogicalSubagentRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, \
                default_model, status, created_at_ms, updated_at_ms, last_active_at_ms \
         FROM subagent_logical_subagents \
         WHERE project_id = ?1 AND repo_hash = ?2 AND name = ?3 AND status != 'deleted'",
    )?;
    let rows = stmt
        .query_map(params![project_id, repo_hash, name], |row| {
            Ok(SubagentLogicalSubagentRow {
                id: row.get(0)?,
                name: row.get(1)?,
                project_id: row.get(2)?,
                repo_path: row.get(3)?,
                repo_hash: row.get(4)?,
                branch: row.get(5)?,
                intent: row.get(6)?,
                default_profile: row.get(7)?,
                default_model: row.get(8)?,
                status: row.get(9)?,
                created_at_ms: row.get(10)?,
                updated_at_ms: row.get(11)?,
                last_active_at_ms: row.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// --- memory ----------------------------------------------------------------

pub fn upsert_memory(conn: &Connection, row: &SubagentMemoryRow) -> Result<()> {
    conn.execute(
        "INSERT INTO subagent_memory \
             (id, subagent_id, hot_summary, long_summary, key_files_json, decisions_json, \
              attempts_json, open_questions_json, artifact_refs_json, last_compacted_at_ms, \
              last_compacted_task_id, updated_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
         ON CONFLICT(subagent_id) DO UPDATE SET \
             hot_summary=excluded.hot_summary, long_summary=excluded.long_summary, \
             key_files_json=excluded.key_files_json, decisions_json=excluded.decisions_json, \
             attempts_json=excluded.attempts_json, open_questions_json=excluded.open_questions_json, \
             artifact_refs_json=excluded.artifact_refs_json, \
             last_compacted_at_ms=excluded.last_compacted_at_ms, \
             last_compacted_task_id=excluded.last_compacted_task_id, \
             updated_at_ms=excluded.updated_at_ms",
        params![
            row.id, row.subagent_id, row.hot_summary, row.long_summary, row.key_files_json,
            row.decisions_json, row.attempts_json, row.open_questions_json,
            row.artifact_refs_json, row.last_compacted_at_ms, row.last_compacted_task_id,
            row.updated_at_ms,
        ],
    )
    .map(|_| ())
    .with_context(|| format!("upsert memory for subagent {}", row.subagent_id))
}

pub fn get_memory(conn: &Connection, subagent_id: &str) -> Result<Option<SubagentMemoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, hot_summary, long_summary, key_files_json, decisions_json, \
                attempts_json, open_questions_json, artifact_refs_json, last_compacted_at_ms, \
                last_compacted_task_id, updated_at_ms \
         FROM subagent_memory WHERE subagent_id = ?1",
    )?;
    let row_opt = stmt
        .query_row(params![subagent_id], |row| {
            Ok(SubagentMemoryRow {
                id: row.get(0)?,
                subagent_id: row.get(1)?,
                hot_summary: row.get(2)?,
                long_summary: row.get(3)?,
                key_files_json: row.get(4)?,
                decisions_json: row.get(5)?,
                attempts_json: row.get(6)?,
                open_questions_json: row.get(7)?,
                artifact_refs_json: row.get(8)?,
                last_compacted_at_ms: row.get(9)?,
                last_compacted_task_id: row.get(10)?,
                updated_at_ms: row.get(11)?,
            })
        })
        .ok();
    Ok(row_opt)
}

// --- tasks -----------------------------------------------------------------

pub fn insert_task(conn: &Connection, row: &SubagentTaskRow) -> Result<()> {
    conn.execute(
        "INSERT INTO subagent_tasks \
             (id, subagent_id, source_harness, source_session_id, intent, profile, prompt, \
              prompt_artifact_ref, output_schema_name, output_schema_version, status, \
              result_summary, result_json, error, created_at_ms, started_at_ms, completed_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            row.id, row.subagent_id, row.source_harness, row.source_session_id, row.intent,
            row.profile, row.prompt, row.prompt_artifact_ref, row.output_schema_name,
            row.output_schema_version, row.status, row.result_summary, row.result_json,
            row.error, row.created_at_ms, row.started_at_ms, row.completed_at_ms,
        ],
    )
    .map(|_| ())
    .with_context(|| format!("insert task {}", row.id))
}

pub fn get_task(conn: &Connection, id: &str) -> Result<Option<SubagentTaskRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, source_harness, source_session_id, intent, profile, prompt, \
                prompt_artifact_ref, output_schema_name, output_schema_version, status, \
                result_summary, result_json, error, created_at_ms, started_at_ms, completed_at_ms \
         FROM subagent_tasks WHERE id = ?1",
    )?;
    let row_opt = stmt
        .query_row(params![id], |row| {
            Ok(SubagentTaskRow {
                id: row.get(0)?,
                subagent_id: row.get(1)?,
                source_harness: row.get(2)?,
                source_session_id: row.get(3)?,
                intent: row.get(4)?,
                profile: row.get(5)?,
                prompt: row.get(6)?,
                prompt_artifact_ref: row.get(7)?,
                output_schema_name: row.get(8)?,
                output_schema_version: row.get(9)?,
                status: row.get(10)?,
                result_summary: row.get(11)?,
                result_json: row.get(12)?,
                error: row.get(13)?,
                created_at_ms: row.get(14)?,
                started_at_ms: row.get(15)?,
                completed_at_ms: row.get(16)?,
            })
        })
        .ok();
    Ok(row_opt)
}

pub fn list_tasks(conn: &Connection, subagent_id: &str, limit: i64) -> Result<Vec<SubagentTaskRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, source_harness, source_session_id, intent, profile, prompt, \
                prompt_artifact_ref, output_schema_name, output_schema_version, status, \
                result_summary, result_json, error, created_at_ms, started_at_ms, completed_at_ms \
         FROM subagent_tasks WHERE subagent_id = ?1 ORDER BY created_at_ms DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![subagent_id, limit], |row| {
            Ok(SubagentTaskRow {
                id: row.get(0)?,
                subagent_id: row.get(1)?,
                source_harness: row.get(2)?,
                source_session_id: row.get(3)?,
                intent: row.get(4)?,
                profile: row.get(5)?,
                prompt: row.get(6)?,
                prompt_artifact_ref: row.get(7)?,
                output_schema_name: row.get(8)?,
                output_schema_version: row.get(9)?,
                status: row.get(10)?,
                result_summary: row.get(11)?,
                result_json: row.get(12)?,
                error: row.get(13)?,
                created_at_ms: row.get(14)?,
                started_at_ms: row.get(15)?,
                completed_at_ms: row.get(16)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn set_task_status(
    conn: &Connection,
    id: &str,
    status: &str,
    result_summary: Option<String>,
    error: Option<String>,
) -> Result<()> {
    let now = busytok_domain::now_ms();
    let completed_at: Option<i64> = (status == "completed" || status == "failed" || status == "cancelled")
        .then_some(now);
    conn.execute(
        "UPDATE subagent_tasks SET status = ?2, result_summary = COALESCE(?3, result_summary), \
            error = COALESCE(?4, error), completed_at_ms = COALESCE(?5, completed_at_ms) \
         WHERE id = ?1",
        params![id, status, result_summary, error, completed_at],
    )
    .map(|_| ())
    .with_context(|| format!("set task {} status {}", id, status))
}

// --- harness bindings ------------------------------------------------------

pub fn upsert_binding(conn: &Connection, row: &SubagentHarnessBindingRow) -> Result<()> {
    conn.execute(
        "INSERT INTO subagent_harness_bindings \
             (id, subagent_id, harness, adapter_session_id, adapter_process_id, is_hot, status, \
              created_at_ms, last_used_at_ms, closed_at_ms, detail_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
         ON CONFLICT(id) DO UPDATE SET \
             adapter_session_id=excluded.adapter_session_id, \
             adapter_process_id=excluded.adapter_process_id, is_hot=excluded.is_hot, \
             status=excluded.status, last_used_at_ms=excluded.last_used_at_ms, \
             closed_at_ms=excluded.closed_at_ms, detail_json=excluded.detail_json",
        params![
            row.id, row.subagent_id, row.harness, row.adapter_session_id,
            row.adapter_process_id, row.is_hot, row.status, row.created_at_ms,
            row.last_used_at_ms, row.closed_at_ms, row.detail_json,
        ],
    )
    .map(|_| ())
    .with_context(|| format!("upsert binding {}", row.id))
}

pub fn hot_binding(conn: &Connection, subagent_id: &str, harness: &str) -> Result<Option<SubagentHarnessBindingRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, harness, adapter_session_id, adapter_process_id, is_hot, status, \
                created_at_ms, last_used_at_ms, closed_at_ms, detail_json \
         FROM subagent_harness_bindings WHERE subagent_id = ?1 AND harness = ?2 AND is_hot = 1",
    )?;
    let row_opt = stmt
        .query_row(params![subagent_id, harness], |row| {
            Ok(SubagentHarnessBindingRow {
                id: row.get(0)?,
                subagent_id: row.get(1)?,
                harness: row.get(2)?,
                adapter_session_id: row.get(3)?,
                adapter_process_id: row.get(4)?,
                is_hot: row.get(5)?,
                status: row.get(6)?,
                created_at_ms: row.get(7)?,
                last_used_at_ms: row.get(8)?,
                closed_at_ms: row.get(9)?,
                detail_json: row.get(10)?,
            })
        })
        .ok();
    Ok(row_opt)
}

// --- usage + resource events ----------------------------------------------

pub fn insert_usage_record(conn: &Connection, row: &SubagentUsageRecordRow) -> Result<()> {
    conn.execute(
        "INSERT INTO subagent_usage_records \
             (id, task_id, subagent_id, source_usage_event_id, harness, provider, model, \
              input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, \
              total_cost_usd, duration_ms, created_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            row.id, row.task_id, row.subagent_id, row.source_usage_event_id, row.harness,
            row.provider, row.model, row.input_tokens, row.output_tokens,
            row.cache_read_tokens, row.cache_write_tokens, row.total_cost_usd,
            row.duration_ms, row.created_at_ms,
        ],
    )
    .map(|_| ())
    .with_context(|| format!("insert usage record {}", row.id))
}

pub fn insert_resource_event(conn: &Connection, row: &SubagentResourceEventRow) -> Result<()> {
    conn.execute(
        "INSERT INTO subagent_resource_events \
             (id, event_type, target_id, rss_mb, cpu_percent, detail_json, created_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.id, row.event_type, row.target_id, row.rss_mb, row.cpu_percent,
            row.detail_json, row.created_at_ms,
        ],
    )
    .map(|_| ())
    .with_context(|| format!("insert resource event {}", row.event_type))
}
```

- [ ] **Step 4: Register the module in `lib.rs`**

In `crates/busytok-store/src/lib.rs`:

```rust
pub mod subagent_queries;
```

- [ ] **Step 5: Add `Database` thin wrappers in `db.rs`**

Append to `crates/busytok-store/src/db.rs` (inside `impl Database`). These are the methods the manager calls. **Use `self.conn()` (the public accessor returning `&Connection`) — the `conn` field is private.**

```rust
// --- subagent runtime ------------------------------------------------------

pub fn subagent_upsert_logical(&self, row: &SubagentLogicalSubagentRow) -> Result<()> {
    subagent_queries::upsert_logical_subagent(self.conn(), row)
}
pub fn subagent_get_logical(&self, id: &str) -> Result<Option<SubagentLogicalSubagentRow>> {
    subagent_queries::get_logical_subagent(self.conn(), id)
}
pub fn subagent_list_active_by_repo(&self, repo_hash: &str) -> Result<Vec<SubagentLogicalSubagentRow>> {
    subagent_queries::list_active_by_repo(self.conn(), repo_hash)
}
pub fn subagent_find_by_name_in_repo(
    &self,
    project_id: &str,
    repo_hash: &str,
    name: &str,
) -> Result<Vec<SubagentLogicalSubagentRow>> {
    subagent_queries::find_by_name_in_repo(self.conn(), project_id, repo_hash, name)
}
pub fn subagent_upsert_memory(&self, row: &SubagentMemoryRow) -> Result<()> {
    subagent_queries::upsert_memory(self.conn(), row)
}
pub fn subagent_get_memory(&self, subagent_id: &str) -> Result<Option<SubagentMemoryRow>> {
    subagent_queries::get_memory(self.conn(), subagent_id)
}
pub fn subagent_insert_task(&self, row: &SubagentTaskRow) -> Result<()> {
    subagent_queries::insert_task(self.conn(), row)
}
pub fn subagent_get_task(&self, id: &str) -> Result<Option<SubagentTaskRow>> {
    subagent_queries::get_task(self.conn(), id)
}
pub fn subagent_list_tasks(&self, subagent_id: &str, limit: i64) -> Result<Vec<SubagentTaskRow>> {
    subagent_queries::list_tasks(self.conn(), subagent_id, limit)
}
pub fn subagent_set_task_status(
    &self,
    id: &str,
    status: &str,
    result_summary: Option<String>,
    error: Option<String>,
) -> Result<()> {
    subagent_queries::set_task_status(self.conn(), id, status, result_summary, error)
}
pub fn subagent_upsert_binding(&self, row: &SubagentHarnessBindingRow) -> Result<()> {
    subagent_queries::upsert_binding(self.conn(), row)
}
pub fn subagent_hot_binding(&self, subagent_id: &str, harness: &str) -> Result<Option<SubagentHarnessBindingRow>> {
    subagent_queries::hot_binding(self.conn(), subagent_id, harness)
}
pub fn subagent_insert_usage_record(&self, row: &SubagentUsageRecordRow) -> Result<()> {
    subagent_queries::insert_usage_record(self.conn(), row)
}
pub fn subagent_insert_resource_event(&self, row: &SubagentResourceEventRow) -> Result<()> {
    subagent_queries::insert_resource_event(self.conn(), row)
}
```

Add the needed `use` at the top of `db.rs`:

```rust
use crate::repository::{
    SubagentHarnessBindingRow, SubagentLogicalSubagentRow, SubagentMemoryRow,
    SubagentResourceEventRow, SubagentTaskRow, SubagentUsageRecordRow,
};
use crate::subagent_queries;
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p busytok-store`
Expected: all store tests pass, including the 5 new `subagent_queries` tests.

- [ ] **Step 7: Commit**

```bash
git add crates/busytok-store/src/subagent_queries.rs crates/busytok-store/src/db.rs crates/busytok-store/src/lib.rs crates/busytok-store/tests/subagent_queries.rs
git commit -m "feat(store): add subagent query functions and Database wrappers"
```

---

### Task 5: Config — `SubagentSettings`

**Files:**
- Modify: `crates/busytok-config/src/lib.rs`
- Modify: `crates/busytok-config/tests/` (existing settings tests, if a round-trip test exists)

**Interfaces:**
- Consumes: existing `BusytokSettings` derive pattern.
- Produces: `busytok_config::SubagentSettings` (with `pi_sidecar`, `context`, `models`, `profiles`), accessed by `SubagentManager` and the CLI.

- [ ] **Step 1: Write a failing config round-trip test**

Create `crates/busytok-config/tests/subagent_settings.rs`:

```rust
#![allow(clippy::unwrap_used)]

use busytok_config::{BusytokSettings, SubagentSettings};

#[test]
fn missing_subagent_section_defaults_to_enabled() {
    let toml = r#"
timezone = "UTC"
week_starts_on = 1
"#;
    let settings = BusytokSettings::load_from_str(toml).unwrap();
    assert!(settings.subagent.enabled);
    assert_eq!(settings.subagent.pi_sidecar.max_hot_sessions, 3);
}

#[test]
fn subagent_settings_round_trip_through_toml() {
    let toml = r#"
timezone = "UTC"
[subagent]
enabled = true
[subagent.pi_sidecar]
max_hot_sessions = 7
idle_exit_seconds = 99
[subagent.models]
default_cheap_model = "deepseek-chat"
"#;
    let settings = BusytokSettings::load_from_str(toml).unwrap();
    assert_eq!(settings.subagent.pi_sidecar.max_hot_sessions, 7);
    assert_eq!(settings.subagent.pi_sidecar.idle_exit_seconds, 99);
    assert_eq!(settings.subagent.models.default_cheap_model, "deepseek-chat");

    let _reloaded: SubagentSettings = settings.subagent.clone();
}

#[test]
fn default_subagent_settings_serialize_to_valid_toml() {
    // Serialize a full BusytokSettings so the `[subagent]` table header is
    // emitted (serializing SubagentSettings alone yields `[pi_sidecar]` etc.,
    // with no `[subagent]` prefix because SubagentSettings IS that section).
    let settings = BusytokSettings::load_from_str(
        "timezone = \"UTC\"\nweek_starts_on = 1\n",
    )
    .unwrap();
    let doc = toml::to_string(&settings).unwrap();
    assert!(doc.contains("[subagent]"), "doc should emit the [subagent] section");
    assert!(doc.contains("[subagent.resource_policy]"));
    assert!(doc.contains("[subagent.pi_sidecar]"));
}
```

> Note: check whether `BusytokSettings` already exposes a `load_from_str` test helper; if it only has `load_from_file`, write the test using `tempfile` + `save_to_file` instead. Inspect `crates/busytok-config/src/lib.rs` for the existing test helpers and reuse the one that exists.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p busytok-config --test subagent_settings`
Expected: FAIL — `SubagentSettings` undefined.

- [ ] **Step 3: Implement `SubagentSettings` in `lib.rs`**

Add to `crates/busytok-config/src/lib.rs`. Follow the existing `PrivacySettings` pattern exactly: derive, `impl Default`, every field `#[serde(default = …)]` or `#[serde(default)]`.

```rust
// --- subagent settings -----------------------------------------------------

fn default_false() -> bool { false }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub pi_sidecar: SubagentPiSidecarConfig,
    #[serde(default)]
    pub context: SubagentContextConfig,
    #[serde(default)]
    pub resource_policy: SubagentResourcePolicyConfig,
    #[serde(default)]
    pub models: SubagentModelsConfig,
    #[serde(default)]
    pub profiles: std::collections::HashMap<String, SubagentProfileConfig>,
}
impl Default for SubagentSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            pi_sidecar: SubagentPiSidecarConfig::default(),
            context: SubagentContextConfig::default(),
            resource_policy: SubagentResourcePolicyConfig::default(),
            models: SubagentModelsConfig::default(),
            profiles: default_profiles(),
        }
    }
}

fn default_max_hot_sessions() -> u32 { 3 }
fn default_idle_exit_seconds() -> u64 { 300 }
fn default_hibernate_after_seconds() -> u64 { 600 }
fn default_task_timeout_seconds() -> u64 { 300 }
fn default_task_queue_max() -> u32 { 50 }
fn default_memory_soft_limit_mb() -> u32 { 800 }
fn default_memory_hard_limit_mb() -> u32 { 1200 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentPiSidecarConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// "bundled" | "system"
    #[serde(default = "default_bundled_runtime")]
    pub node_runtime: String,
    #[serde(default)]
    pub system_node_path: String,
    #[serde(default = "default_max_hot_sessions")]
    pub max_hot_sessions: u32,
    #[serde(default = "default_idle_exit_seconds")]
    pub idle_exit_seconds: u64,
    #[serde(default = "default_hibernate_after_seconds")]
    pub hibernate_after_seconds: u64,
    #[serde(default = "default_task_timeout_seconds")]
    pub task_timeout_seconds: u64,
    #[serde(default = "default_memory_soft_limit_mb")]
    pub memory_soft_limit_mb: u32,
    #[serde(default = "default_memory_hard_limit_mb")]
    pub memory_hard_limit_mb: u32,
    #[serde(default = "default_task_queue_max")]
    pub task_queue_max: u32,
}
impl Default for SubagentPiSidecarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            node_runtime: default_bundled_runtime(),
            system_node_path: String::new(),
            max_hot_sessions: default_max_hot_sessions(),
            idle_exit_seconds: default_idle_exit_seconds(),
            hibernate_after_seconds: default_hibernate_after_seconds(),
            task_timeout_seconds: default_task_timeout_seconds(),
            memory_soft_limit_mb: default_memory_soft_limit_mb(),
            memory_hard_limit_mb: default_memory_hard_limit_mb(),
            task_queue_max: default_task_queue_max(),
        }
    }
}
fn default_bundled_runtime() -> String { "bundled".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentContextConfig {
    #[serde(default = "default_budget_tokens")]
    pub default_budget_tokens: u32,
    #[serde(default = "default_max_budget_tokens")]
    pub max_budget_tokens: u32,
    #[serde(default = "default_recent_tasks_limit")]
    pub recent_tasks_limit: u32,
    #[serde(default = "default_compaction_tasks_threshold")]
    pub compaction_tasks_threshold: u32,
    #[serde(default = "default_compaction_budget_ratio")]
    pub compaction_budget_ratio: f64,
}
impl Default for SubagentContextConfig {
    fn default() -> Self {
        Self {
            default_budget_tokens: default_budget_tokens(),
            max_budget_tokens: default_max_budget_tokens(),
            recent_tasks_limit: default_recent_tasks_limit(),
            compaction_tasks_threshold: default_compaction_tasks_threshold(),
            compaction_budget_ratio: default_compaction_budget_ratio(),
        }
    }
}
fn default_budget_tokens() -> u32 { 4000 }
fn default_max_budget_tokens() -> u32 { 8000 }
fn default_recent_tasks_limit() -> u32 { 5 }
fn default_compaction_tasks_threshold() -> u32 { 5 }
fn default_compaction_budget_ratio() -> f64 { 0.7 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResourcePolicyConfig {
    /// System free-memory threshold below which the runtime applies backpressure.
    #[serde(default = "default_memory_pressure_free_mb")]
    pub memory_pressure_free_mb: u32,
    /// Resource sampling interval for ResourceMonitor (Plan 5).
    #[serde(default = "default_monitor_interval_seconds")]
    pub monitor_interval_seconds: u64,
}
impl Default for SubagentResourcePolicyConfig {
    fn default() -> Self {
        Self {
            memory_pressure_free_mb: default_memory_pressure_free_mb(),
            monitor_interval_seconds: default_monitor_interval_seconds(),
        }
    }
}
fn default_memory_pressure_free_mb() -> u32 { 2048 }
fn default_monitor_interval_seconds() -> u64 { 30 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentModelsConfig {
    #[serde(default = "default_cheap_model")]
    pub default_cheap_model: String,
    #[serde(default = "default_review_model")]
    pub default_review_model: String,
    #[serde(default = "default_reasoning_model")]
    pub default_reasoning_model: String,
    #[serde(default = "default_coder_model")]
    pub default_coder_model: String,
}
impl Default for SubagentModelsConfig {
    fn default() -> Self {
        Self {
            default_cheap_model: default_cheap_model(),
            default_review_model: default_review_model(),
            default_reasoning_model: default_reasoning_model(),
            default_coder_model: default_coder_model(),
        }
    }
}
fn default_cheap_model() -> String { "deepseek-chat".to_string() }
fn default_review_model() -> String { "qwen-coder".to_string() }
fn default_reasoning_model() -> String { "deepseek-reasoner".to_string() }
fn default_coder_model() -> String { "qwen-coder".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentProfileConfig {
    #[serde(default = "default_false")]
    pub write_access: bool,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_budget_tokens")]
    pub context_budget_tokens: u32,
    #[serde(default = "default_task_timeout_seconds")]
    pub timeout_seconds: u64,
}

/// The built-in read-only profiles for MVP. `pi/patch-small` is deferred.
fn default_profiles() -> std::collections::HashMap<String, SubagentProfileConfig> {
    let mut m = std::collections::HashMap::new();
    m.insert(
        "pi/search-cheap".to_string(),
        SubagentProfileConfig {
            write_access: false,
            tools: vec!["read".to_string(), "grep".to_string()],
            model: default_cheap_model(),
            context_budget_tokens: 3000,
            timeout_seconds: 120,
        },
    );
    m.insert(
        "pi/review-cheap".to_string(),
        SubagentProfileConfig {
            write_access: false,
            tools: vec!["read".to_string(), "grep".to_string(), "git_diff".to_string()],
            model: default_review_model(),
            context_budget_tokens: 5000,
            timeout_seconds: 180,
        },
    );
    m.insert(
        "pi/plan-cheap".to_string(),
        SubagentProfileConfig {
            write_access: false,
            tools: vec!["read".to_string(), "grep".to_string(), "git_diff".to_string()],
            model: default_reasoning_model(),
            context_budget_tokens: 6000,
            timeout_seconds: 300,
        },
    );
    m
}
```

Add the field to `BusytokSettings`:

```rust
pub struct BusytokSettings {
    pub timezone: String,
    #[serde(default = "default_week_starts_on")]
    pub week_starts_on: u8,
    #[serde(default)]
    pub privacy: PrivacySettings,
    #[serde(default)]
    pub discovery: DiscoverySettings,
    #[serde(default)]
    pub prompt_palette_default_action: PromptDefaultAction,
    #[serde(default)]
    pub subagent: SubagentSettings,
}
```

And add the field to `BusytokSettings`. **No call-site edits are required:** every existing `BusytokSettings { ... }` literal in the codebase (verified in `supervisor_control.rs`, `config` tests) uses `..BusytokSettings::default()`, so the new field is filled automatically. Only update a literal if you find one that enumerates every field without `..Default::default()` (none currently exist).

- [ ] **Step 4: Expose `load_from_str` if not present**

If `BusytokSettings` has no `load_from_str`, add it next to the existing `load_from_file`. **Do NOT gate with `#[cfg(test)]`** — `#[cfg(test)]` items are invisible to integration tests (which compile as separate crates), so the test in Step 1 would fail to link. Make it a permanent `pub fn`:

```rust
/// Parse settings from a TOML string (no filesystem canonicalization/validation).
/// Used by tests; mirrors `load_from_file`.
pub fn load_from_str(toml: &str) -> anyhow::Result<Self> {
    let s: Self = toml::from_str(toml)?;
    Ok(s)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p busytok-config`
Expected: PASS, including legacy-settings-backfill tests (they rely on `#[serde(default)]` everywhere).

- [ ] **Step 6: Commit**

```bash
git add crates/busytok-config
git commit -m "feat(config): add SubagentSettings (pi_sidecar, context, models, profiles)"
```

---

### Task 6: `BusytokPaths::artifacts_dir()`

**Files:**
- Modify: `crates/busytok-config/src/paths.rs`

**Interfaces:**
- Consumes: existing `BusytokPaths` fields.
- Produces: `paths.artifacts_dir()`.

> `sidecar_runtime_dir()` is **deferred to Plan 2**. Its resolution is deployment-aware (packaged Tauri bundle resources vs `apps/pi-sidecar/dist` dev mode vs service-only) and belongs with the real sidecar supervisor — freezing `data_dir/sidecars` here would encode the wrong filesystem contract. Plan 1 only needs the artifact store.

- [ ] **Step 1: Write failing path tests**

Create `crates/busytok-config/tests/subagent_paths.rs`:

```rust
#![allow(clippy::unwrap_used)]

use busytok_config::BusytokPaths;

#[test]
fn artifacts_dir_lives_under_data_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    assert_eq!(paths.artifacts_dir(), paths.data_dir().join("artifacts"));
}

#[test]
fn ensure_dirs_exist_creates_artifacts_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    paths.ensure_dirs_exist().unwrap();
    assert!(paths.artifacts_dir().is_dir());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p busytok-config --test subagent_paths`
Expected: FAIL — method undefined.

- [ ] **Step 3: Add the method in `paths.rs`**

Add the constant and method to `crates/busytok-config/src/paths.rs`:

```rust
pub const ARTIFACTS_DIR_NAME: &str = "artifacts";
```

In `impl BusytokPaths`:

```rust
/// Root for large subagent artifacts (logs, patches, traces).
/// Full layout: `<artifacts_dir>/<subagent_id>/<task_id>/...`.
pub fn artifacts_dir(&self) -> PathBuf {
    self.data_dir.join(ARTIFACTS_DIR_NAME)
}
```

Update `ensure_dirs_exist()` to create the artifacts dir:

```rust
pub fn ensure_dirs_exist(&self) -> std::io::Result<()> {
    std::fs::create_dir_all(self.data_dir())?;
    std::fs::create_dir_all(self.config_dir())?;
    std::fs::create_dir_all(self.runtime_dir())?;
    std::fs::create_dir_all(self.log_dir())?;
    std::fs::create_dir_all(&self.artifacts_dir())?; // NEW
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p busytok-config --test subagent_paths`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-config/src/paths.rs crates/busytok-config/tests/subagent_paths.rs
git commit -m "feat(config): add artifacts_dir to BusytokPaths"
```

---

### Task 7: Domain models for the subagent layer

**Files:**
- Modify: `crates/busytok-subagent/src/models.rs`
- Modify: `crates/busytok-subagent/src/lib.rs`

**Interfaces:**
- Consumes: store row structs (Task 3).
- Produces: domain types consumed by `SubagentManager` (Task 8) and the control DTOs (Task 9).

- [ ] **Step 1: Write a failing model-conversion test**

Create `crates/busytok-subagent/src/models.rs` with the types, then test conversion from rows. First write the test at `crates/busytok-subagent/tests/models.rs`:

```rust
#![allow(clippy::unwrap_used)]

use busytok_subagent::models::{DelegateRequest, LogicalSubagent, SubagentStatus, TaskStatus};

#[test]
fn subagent_status_parses_known_values() {
    assert_eq!("hot".parse::<SubagentStatus>().unwrap(), SubagentStatus::Hot);
    assert_eq!(SubagentStatus::Warm.as_str(), "warm");
    assert!("bogus".parse::<SubagentStatus>().is_err());
}

#[test]
fn task_status_parses_known_values() {
    assert_eq!("queued".parse::<TaskStatus>().unwrap(), TaskStatus::Queued);
    assert_eq!(TaskStatus::Completed.as_str(), "completed");
}

#[test]
fn delegate_request_requires_subagent_and_prompt() {
    let req = DelegateRequest {
        subagent_name: "reviewer".to_string(),
        subagent_id: None,
        cwd: "/tmp/repo".to_string(),
        profile: "pi/search-cheap".to_string(),
        intent: None,
        prompt: "find it".to_string(),
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
    };
    assert_eq!(req.subagent_name, "reviewer");
    assert!(LogicalSubagent::is_valid_name(&req.subagent_name));
    assert!(!LogicalSubagent::is_valid_name(""));
    assert!(!LogicalSubagent::is_valid_name("has space"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p busytok-subagent --test models`
Expected: FAIL — types undefined.

- [ ] **Step 3: Implement `models.rs`**

```rust
//! Domain models for the logical-subagent layer.
//!
//! These are the in-memory types the manager works with. Persistence uses the
//! `…Row` structs in `busytok_store::repository`; conversions happen at the
//! manager boundary.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Runtime status of a logical subagent (see spec §3.3 state machine).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentStatus {
    Hot,
    Warm,
    Cold,
    Deleted,
}

impl SubagentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SubagentStatus::Hot => "hot",
            SubagentStatus::Warm => "warm",
            SubagentStatus::Cold => "cold",
            SubagentStatus::Deleted => "deleted",
        }
    }
}

impl FromStr for SubagentStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "hot" => Ok(Self::Hot),
            "warm" => Ok(Self::Warm),
            "cold" => Ok(Self::Cold),
            "deleted" => Ok(Self::Deleted),
            other => Err(format!("invalid subagent status: {other}")),
        }
    }
}

/// Lifecycle status of a single delegated task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Queued => "queued",
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }
}

impl FromStr for TaskStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("invalid task status: {other}")),
        }
    }
}

/// A logical subagent — long-lived identity stored in SQLite.
#[derive(Debug, Clone, Serialize)]
pub struct LogicalSubagent {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub repo_path: String,
    pub repo_hash: String,
    pub branch: Option<String>,
    pub intent: Option<String>,
    pub default_profile: String,
    pub default_model: Option<String>,
    pub status: SubagentStatus,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_active_at_ms: Option<i64>,
}

impl LogicalSubagent {
    /// Names must be 1..=64 chars of `[A-Za-z0-9._-]`, no leading dot.
    pub fn is_valid_name(name: &str) -> bool {
        if name.is_empty() || name.len() > 64 || name.starts_with('.') {
            return false;
        }
        name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    }
}

/// A single delegated task (summary view).
#[derive(Debug, Clone, Serialize)]
pub struct SubagentTaskSummary {
    pub id: String,
    pub subagent_id: String,
    pub profile: String,
    pub status: TaskStatus,
    pub prompt: Option<String>,
    pub result_summary: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}

/// Request to create-or-continue a subagent and run one task.
#[derive(Debug, Clone, Deserialize)]
pub struct DelegateRequest {
    pub subagent_name: String,
    /// UUID shortcut, bypassing name resolution.
    pub subagent_id: Option<String>,
    pub cwd: String,
    pub profile: String,
    pub intent: Option<String>,
    pub prompt: String,
    pub timeout_seconds: Option<u64>,
    pub model_override: Option<String>,
    pub source_harness: Option<String>,
    pub source_session_id: Option<String>,
}

/// Resolution params for single-subagent operations (show/tasks/hibernate/delete).
/// Exactly one of `id` (UUID) or `name` (+ `cwd`) is set.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResolveParams {
    pub name: Option<String>,
    pub id: Option<String>,
    pub cwd: Option<String>,
}

/// Token usage returned by a task (mock in this plan).
#[derive(Debug, Clone, Default, Serialize)]
pub struct TaskUsage {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
}

/// The result of a `delegate` call.
#[derive(Debug, Clone, Serialize)]
pub struct DelegateResult {
    pub task_id: String,
    pub subagent_id: String,
    pub subagent_name: String,
    pub adapter: String,
    pub adapter_session_id: Option<String>,
    pub session_reused: bool,
    pub status: TaskStatus,
    pub profile: String,
    pub model: Option<String>,
    pub summary: Option<String>,
    pub usage: TaskUsage,
}
```

- [ ] **Step 4: Export the module from `lib.rs`**

Update `crates/busytok-subagent/src/lib.rs` `pub mod models;` is already declared.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p busytok-subagent`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/busytok-subagent/src/models.rs crates/busytok-subagent/tests/models.rs
git commit -m "feat(subagent): add domain models (status, subagent, task, delegate)"
```

---

### Task 8: `SubagentManager` — CRUD, resolution, mock delegate

**Files:**
- Create: `crates/busytok-subagent/src/resolver.rs`
- Create: `crates/busytok-subagent/src/mock_executor.rs`
- Create: `crates/busytok-subagent/src/manager.rs`
- Modify: `crates/busytok-subagent/src/lib.rs`

**Interfaces:**
- Consumes: store `Database` wrappers (Task 4), config (Task 5), models (Task 7).
- Produces: `SubagentManager` with async methods `delegate`, `list`, `show`, `tasks`, `hibernate`, `delete`, `new`.

- [ ] **Step 1: Write failing manager tests**

Create `crates/busytok-subagent/tests/manager.rs`:

```rust
#![allow(clippy::unwrap_used)]

use busytok_config::{BusytokPaths, SubagentSettings};
use busytok_store::Database;
use busytok_subagent::manager::SubagentManager;
use busytok_subagent::models::DelegateRequest;

async fn manager() -> SubagentManager {
    // std::sync::Mutex — matches the supervisor's db field type.
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    SubagentManager::new(db, SubagentSettings::default(), "pi")
}

fn req(name: &str, prompt: &str) -> DelegateRequest {
    DelegateRequest {
        subagent_name: name.to_string(),
        subagent_id: None,
        cwd: "/tmp/repo".to_string(),
        profile: "pi/search-cheap".to_string(),
        intent: None,
        prompt: prompt.to_string(),
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
    }
}

#[tokio::test]
async fn delegate_creates_subagent_then_reuses_it() {
    let m = manager().await;
    let r1 = m.delegate(req("reviewer", "step one")).await.unwrap();
    assert_eq!(r1.subagent_name, "reviewer");
    assert_eq!(r1.status.as_str(), "completed");

    let r2 = m.delegate(req("reviewer", "step two")).await.unwrap();
    assert_eq!(r2.subagent_id, r1.subagent_id, "same subagent reused");
}

#[tokio::test]
async fn list_returns_active_subagents() {
    let m = manager().await;
    m.delegate(req("a", "do")).await.unwrap();
    m.delegate(req("b", "do")).await.unwrap();
    // no filters → all active subagents
    let list = m.list(None, None, false).await.unwrap();
    assert_eq!(list.len(), 2);
    // status filter narrows the set
    let warm = m.list(Some(SubagentStatus::Warm), None, false).await.unwrap();
    assert_eq!(warm.len(), 2, "both go warm after a mock task");
}

#[tokio::test]
async fn delete_then_lookup_fails() {
    let m = manager().await;
    let r = m.delegate(req("reviewer", "do")).await.unwrap();
    m.delete(ResolveParams { id: Some(r.subagent_id.clone()), ..Default::default() }, false)
        .await
        .unwrap();
    // soft-deleted rows are excluded from the active list
    let list = m.list(None, None, false).await.unwrap();
    assert!(list.iter().all(|s| s.id != r.subagent_id));
}

#[tokio::test]
async fn hibernate_clears_hot_binding_keeps_state() {
    let m = manager().await;
    let r = m.delegate(req("reviewer", "do")).await.unwrap();
    m.hibernate(ResolveParams { id: Some(r.subagent_id.clone()), ..Default::default() })
        .await
        .unwrap();
    let detail = m.show(ResolveParams { id: Some(r.subagent_id.clone()), ..Default::default() })
        .await
        .unwrap();
    // after hibernate the subagent still exists (warm — memory was written), not deleted
    assert_ne!(detail.status.as_str(), "deleted");
}

#[tokio::test]
async fn reject_invalid_subagent_name() {
    let m = manager().await;
    let bad = req("bad name!", "do");
    assert!(m.delegate(bad).await.is_err());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p busytok-subagent --test manager`
Expected: FAIL — `SubagentManager` undefined.

- [ ] **Step 3: Implement `resolver.rs`**

`crates/busytok-subagent/src/resolver.rs`:

```rust
//! Name / id resolution for logical subagents.

use busytok_domain::derive_project_hash;
use busytok_store::{SubagentLogicalSubagentRow, SubagentMemoryRow};

use crate::error::{Result, SubagentError};
use crate::models::LogicalSubagent;

/// Resolved identity for a delegate request.
pub struct Resolved {
    pub subagent: LogicalSubagent,
    pub created: bool,
}

/// Look up or create a subagent by name within the repo scope of `cwd`.
///
/// MVP: `project_id == repo_hash`.
pub fn resolve_by_name(
    db: &busytok_store::Database,
    name: &str,
    cwd: &str,
    default_profile: &str,
    default_model: Option<&str>,
) -> Result<Resolved> {
    if !LogicalSubagent::is_valid_name(name) {
        return Err(SubagentError::InvalidName(name.to_string()));
    }
    // Canonicalize cwd at this single chokepoint so callers (CLI, e2e) agree
    // on repo_hash regardless of whether they pre-canonicalized.
    let canonical_cwd = std::fs::canonicalize(cwd)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| cwd.to_string());
    let repo_hash = derive_project_hash(&canonical_cwd);
    let matches = db
        .subagent_find_by_name_in_repo(&repo_hash, &repo_hash, name)
        .map_err(SubagentError::Store)?;
    match matches.len() {
        0 => Ok(Resolved {
            subagent: create_subagent(db, name, &canonical_cwd, &repo_hash, default_profile, default_model)?,
            created: true,
        }),
        1 => Ok(Resolved {
            subagent: row_to_model(&matches[0]),
            created: false,
        }),
        _ => Err(SubagentError::AmbiguousName(name.to_string())),
    }
}

/// Look up by UUID directly.
pub fn resolve_by_id(db: &busytok_store::Database, id: &str) -> Result<LogicalSubagent> {
    db.subagent_get_logical(id)
        .map_err(SubagentError::Store)?
        .map(|r| row_to_model(&r))
        .ok_or_else(|| SubagentError::NotFound(id.to_string()))
}

/// Look up (WITHOUT creating) a subagent by name within the repo scope of `cwd`.
/// Used by read-only operations (show/tasks/hibernate/delete); delegate uses the
/// create-or-lookup `resolve_by_name`.
pub fn lookup_by_name(db: &busytok_store::Database, name: &str, cwd: &str) -> Result<LogicalSubagent> {
    if !LogicalSubagent::is_valid_name(name) {
        return Err(SubagentError::InvalidName(name.to_string()));
    }
    let canonical_cwd = std::fs::canonicalize(cwd)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| cwd.to_string());
    let repo_hash = derive_project_hash(&canonical_cwd);
    let matches = db
        .subagent_find_by_name_in_repo(&repo_hash, &repo_hash, name)
        .map_err(SubagentError::Store)?;
    match matches.len() {
        0 => Err(SubagentError::NotFound(name.to_string())),
        1 => Ok(row_to_model(&matches[0])),
        _ => Err(SubagentError::AmbiguousName(name.to_string())),
    }
}

fn create_subagent(
    db: &busytok_store::Database,
    name: &str,
    cwd: &str,
    repo_hash: &str,
    default_profile: &str,
    default_model: Option<&str>,
) -> Result<LogicalSubagent> {
    let now = busytok_domain::now_ms();
    // Plain UUID v4 (spec §3.2). The adapter_session_id/task_id prefixes are
    // only for those entities; logical subagent ids are bare UUIDs.
    let id = uuid::Uuid::new_v4().to_string();
    let row = SubagentLogicalSubagentRow {
        id: id.clone(),
        name: name.to_string(),
        project_id: repo_hash.to_string(),
        repo_path: cwd.to_string(),
        repo_hash: repo_hash.to_string(),
        branch: None,
        intent: None,
        default_profile: default_profile.to_string(),
        default_model: default_model.map(|s| s.to_string()),
        status: "cold".to_string(),
        created_at_ms: now,
        updated_at_ms: now,
        last_active_at_ms: None,
    };
    db.subagent_upsert_logical(&row).map_err(SubagentError::Store)?;
    // seed an empty memory row so hibernate/restore always finds one
    db.subagent_upsert_memory(&SubagentMemoryRow::for_test(&id))
        .map_err(SubagentError::Store)?;
    Ok(row_to_model(&row))
}

pub fn row_to_model(r: &SubagentLogicalSubagentRow) -> LogicalSubagent {
    LogicalSubagent {
        id: r.id.clone(),
        name: r.name.clone(),
        project_id: r.project_id.clone(),
        repo_path: r.repo_path.clone(),
        repo_hash: r.repo_hash.clone(),
        branch: r.branch.clone(),
        intent: r.intent.clone(),
        default_profile: r.default_profile.clone(),
        default_model: r.default_model.clone(),
        status: r.status.parse().unwrap_or(crate::models::SubagentStatus::Cold),
        created_at_ms: r.created_at_ms,
        updated_at_ms: r.updated_at_ms,
        last_active_at_ms: r.last_active_at_ms,
    }
}
```

- [ ] **Step 4: Implement `mock_executor.rs`**

`crates/busytok-subagent/src/mock_executor.rs` — the in-process executor for Step 1. It returns a canned result and synthesizes usage. Plan 2 replaces the `TaskExecutor` impl with the sidecar client.

```rust
//! In-process mock task executor (Step 1 only).
//!
//! Produces a deterministic canned result so the management layer, store,
//! control protocol, and CLI can be validated without the Pi sidecar.
//! Plan 2 swaps in the real sidecar-backed `TaskExecutor`.

use crate::models::{TaskStatus, TaskUsage};

pub struct MockTaskOutput {
    pub status: TaskStatus,
    pub summary: String,
    pub usage: TaskUsage,
}

/// Run a mock task. The summary echoes the prompt so tests can assert on it.
pub fn run_mock(prompt: &str, model: Option<&str>) -> MockTaskOutput {
    let summary = format!("[mock] no sidecar wired yet; prompt was: {prompt}");
    MockTaskOutput {
        status: TaskStatus::Completed,
        summary,
        usage: TaskUsage {
            model: model.map(|s| s.to_string()),
            provider: Some("mock".to_string()),
            input_tokens: Some(prompt.len() as i64),
            output_tokens: Some(summary.len() as i64),
            ..Default::default()
        },
    }
}
```

- [ ] **Step 5: Implement `manager.rs`**

`crates/busytok-subagent/src/manager.rs`:

```rust
//! The public logical-subagent manager.

use std::sync::{Arc, Mutex};

use busytok_config::SubagentSettings;
use busytok_store::{
    SubagentMemoryRow, SubagentResourceEventRow, SubagentTaskRow, SubagentUsageRecordRow,
};
use tracing::{info, warn};

use crate::error::{Result, SubagentError};
use crate::models::{
    DelegateRequest, DelegateResult, LogicalSubagent, ResolveParams, SubagentStatus,
    SubagentTaskSummary, TaskStatus,
};
use crate::mock_executor::run_mock;
use crate::resolver::{resolve_by_id, resolve_by_name, row_to_model};

type SharedDb = Arc<Mutex<busytok_store::Database>>;

pub struct SubagentManager {
    db: SharedDb,
    settings: SubagentSettings,
    adapter: String,
}

impl SubagentManager {
    pub fn new(db: SharedDb, settings: SubagentSettings, adapter: &str) -> Self {
        Self {
            db,
            settings,
            adapter: adapter.to_string(),
        }
    }

    /// Create-or-continue a subagent and run one task (mock execution in this plan).
    pub async fn delegate(&self, req: DelegateRequest) -> Result<DelegateResult> {
        if !self.settings.enabled {
            warn!(event_code = "subagent.delegate.rejected", reason = "disabled");
            return Err(SubagentError::Disabled);
        }
        let profile_model = self.profile_model(&req.profile);

        // 1. resolve subagent (create if needed). `resolve_by_name` canonicalizes
        //    cwd and validates the name; errors propagate with a reject log.
        let (subagent, created) = {
            let db = self.db.lock().expect("subagent db lock poisoned");
            if let Some(id) = &req.subagent_id {
                (resolve_by_id(&db, id)?, false)
            } else {
                match resolve_by_name(
                    &db,
                    &req.subagent_name,
                    &req.cwd,
                    &req.profile,
                    profile_model.as_deref(),
                ) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(
                            event_code = "subagent.delegate.rejected",
                            reason = e.code(),
                            name = %req.subagent_name,
                        );
                        return Err(e);
                    }
                }
            }
        };

        // 2. insert task row (queued)
        let task_id = format!("task_{}", uuid::Uuid::new_v4());
        {
            let db = self.db.lock().expect("subagent db lock poisoned");
            db.subagent_insert_task(&SubagentTaskRow {
                id: task_id.clone(),
                subagent_id: subagent.id.clone(),
                source_harness: req.source_harness.clone(),
                source_session_id: req.source_session_id.clone(),
                intent: req.intent.clone(),
                profile: req.profile.clone(),
                prompt: Some(req.prompt.clone()),
                prompt_artifact_ref: None,
                output_schema_name: None,
                output_schema_version: 1,
                status: "queued".to_string(),
                result_summary: None,
                result_json: None,
                error: None,
                created_at_ms: busytok_domain::now_ms(),
                started_at_ms: None,
                completed_at_ms: None,
            })
            .map_err(SubagentError::Store)?;
        }

        info!(
            event_code = "subagent.delegate.start",
            subagent_id = %subagent.id,
            created,
            profile = %req.profile,
            "delegating task"
        );

        // 3. mock-execute (Plan 2: sidecar turn). No lock held during execution.
        let model = req.model_override.clone().or(profile_model);
        let started = busytok_domain::now_ms();
        let out = run_mock(&req.prompt, model.as_deref());
        let duration_ms = busytok_domain::now_ms().saturating_sub(started);

        // 4. persist results: task status, usage, memory (hot_summary), status.
        //    Writing hot_summary satisfies the `warm` invariant (recoverable
        //    memory exists). Plan 1 records NO hot binding (no real session),
        //    so status is Warm, not Hot — consistent with spec §3.3.
        {
            let db = self.db.lock().expect("subagent db lock poisoned");
            db.subagent_set_task_status(
                &task_id,
                out.status.as_str(),
                Some(out.summary.clone()),
                None,
            )
            .map_err(SubagentError::Store)?;
            db.subagent_insert_usage_record(&SubagentUsageRecordRow {
                id: format!("usage_{task_id}"),
                task_id: task_id.clone(),
                subagent_id: subagent.id.clone(),
                source_usage_event_id: None,
                harness: self.adapter.clone(),
                provider: out.usage.provider.clone(),
                model: out.usage.model.clone(),
                input_tokens: out.usage.input_tokens,
                output_tokens: out.usage.output_tokens,
                cache_read_tokens: out.usage.cache_read_tokens,
                cache_write_tokens: out.usage.cache_write_tokens,
                total_cost_usd: out.usage.cost_usd,
                duration_ms: Some(duration_ms),
                created_at_ms: busytok_domain::now_ms(),
            })
            .map_err(SubagentError::Store)?;

            // memory: write hot_summary so hibernate/restore recovers context.
            self.write_hot_summary(&db, &subagent.id, &out.summary)?;
            self.set_logical_status(&db, &subagent.id, SubagentStatus::Warm)?;
        }

        Ok(DelegateResult {
            task_id,
            subagent_id: subagent.id.clone(),
            subagent_name: subagent.name.clone(),
            adapter: self.adapter.clone(),
            adapter_session_id: None,
            session_reused: !created,
            status: out.status,
            profile: req.profile,
            model,
            summary: Some(out.summary),
            usage: out.usage.clone(),
        })
    }

    /// List subagents, optionally filtered by status / project / include-deleted.
    pub async fn list(
        &self,
        status: Option<SubagentStatus>,
        project: Option<&str>,
        include_deleted: bool,
    ) -> Result<Vec<LogicalSubagent>> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        let rows = db
            .subagent_list_filtered(
                status.map(|s| s.as_str()),
                project,
                include_deleted,
            )
            .map_err(SubagentError::Store)?;
        Ok(rows.iter().map(row_to_model).collect::<Vec<_>>())
    }

    pub async fn show(&self, resolve: ResolveParams) -> Result<LogicalSubagent> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        self.resolve(&db, &resolve)
    }

    pub async fn tasks(
        &self,
        resolve: ResolveParams,
        limit: i64,
    ) -> Result<Vec<SubagentTaskSummary>> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        let sub = self.resolve(&db, &resolve)?;
        let rows = db
            .subagent_list_tasks(&sub.id, limit)
            .map_err(SubagentError::Store)?;
        Ok(rows.into_iter().map(task_row_to_summary).collect())
    }

    /// Release any hot binding for this subagent; keep DB state (warm/cold).
    /// Returns the resolved subagent id so callers can echo it back.
    pub async fn hibernate(&self, resolve: ResolveParams) -> Result<String> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        let sub = self.resolve(&db, &resolve)?;
        let binding = db
            .subagent_hot_binding(&sub.id, &self.adapter)
            .map_err(SubagentError::Store)?;
        if let Some(mut b) = binding {
            let now = busytok_domain::now_ms();
            b.is_hot = 0;
            b.status = "closed".to_string();
            b.closed_at_ms = Some(now);
            db.subagent_upsert_binding(&b).map_err(SubagentError::Store)?;
            db.subagent_insert_resource_event(&SubagentResourceEventRow {
                id: format!("re_{}", uuid::Uuid::new_v4()),
                event_type: "session_hibernate".to_string(),
                target_id: Some(sub.id.clone()),
                rss_mb: None,
                cpu_percent: None,
                detail_json: None,
                created_at_ms: now,
            })
            .map_err(SubagentError::Store)?;
            info!(event_code = "subagent.hibernate", subagent_id = %sub.id, "hibernated hot session");
        }
        // status follows the invariant: memory exists → warm, else cold
        let new_status = match db
            .subagent_get_memory(&sub.id)
            .map_err(SubagentError::Store)?
            .and_then(|m| m.hot_summary)
        {
            Some(_) => SubagentStatus::Warm,
            None => SubagentStatus::Cold,
        };
        self.set_logical_status(&db, &sub.id, new_status)?;
        Ok(sub.id)
    }

    /// Soft delete (default) or hard delete with `hard=true`.
    /// Returns the resolved subagent id so callers can echo it back.
    pub async fn delete(&self, resolve: ResolveParams, hard: bool) -> Result<String> {
        let db = self.db.lock().expect("subagent db lock poisoned");
        let sub = self.resolve(&db, &resolve)?;
        if hard {
            // Application-layer cascade (spec §3.5: no DB-level CASCADE).
            // subagent_hard_delete removes usage, tasks, bindings, memory, events, then the row.
            db.subagent_hard_delete(&sub.id).map_err(SubagentError::Store)?;
            warn!(event_code = "subagent.delete.hard", subagent_id = %sub.id, "hard-deleted subagent");
        } else {
            self.set_logical_status(&db, &sub.id, SubagentStatus::Deleted)?;
            info!(event_code = "subagent.delete.soft", subagent_id = %sub.id, "soft-deleted subagent");
        }
        Ok(sub.id)
    }

    // --- helpers ------------------------------------------------------------

    /// Resolve a single subagent by UUID (`id`) or by name + cwd.
    /// Lookup-only — does NOT create (read/delete ops must not mutate identity).
    fn resolve(
        &self,
        db: &busytok_store::Database,
        p: &ResolveParams,
    ) -> Result<LogicalSubagent> {
        if let Some(id) = &p.id {
            return resolve_by_id(db, id);
        }
        let name = p
            .name
            .as_ref()
            .ok_or_else(|| SubagentError::InvalidName("neither id nor name provided".to_string()))?;
        let cwd = p.cwd.as_deref().unwrap_or(".");
        crate::resolver::lookup_by_name(db, name, cwd)
    }

    fn profile_model(&self, profile: &str) -> Option<String> {
        match profile {
            "pi/search-cheap" => Some(self.settings.models.default_cheap_model.clone()),
            "pi/review-cheap" => Some(self.settings.models.default_review_model.clone()),
            "pi/plan-cheap" => Some(self.settings.models.default_reasoning_model.clone()),
            other => {
                self.settings
                    .profiles
                    .get(other)
                    .map(|p| p.model.clone())
                    .filter(|m| !m.is_empty())
            }
        }
    }

    /// Persist the most recent task summary as the recoverable `hot_summary`.
    fn write_hot_summary(&self, db: &busytok_store::Database, subagent_id: &str, summary: &str) -> Result<()> {
        let mut mem = db
            .subagent_get_memory(subagent_id)
            .map_err(SubagentError::Store)?
            .unwrap_or_else(|| SubagentMemoryRow::for_test(subagent_id));
        mem.hot_summary = Some(summary.to_string());
        mem.updated_at_ms = busytok_domain::now_ms();
        db.subagent_upsert_memory(&mem).map_err(SubagentError::Store)?;
        Ok(())
    }

    fn set_logical_status(
        &self,
        db: &busytok_store::Database,
        id: &str,
        status: SubagentStatus,
    ) -> Result<()> {
        let mut row = db
            .subagent_get_logical(id)
            .map_err(SubagentError::Store)?
            .ok_or_else(|| SubagentError::NotFound(id.to_string()))?;
        row.status = status.as_str().to_string();
        row.updated_at_ms = busytok_domain::now_ms();
        row.last_active_at_ms = Some(row.updated_at_ms);
        db.subagent_upsert_logical(&row).map_err(SubagentError::Store)?;
        Ok(())
    }
}

fn task_row_to_summary(r: SubagentTaskRow) -> SubagentTaskSummary {
    SubagentTaskSummary {
        id: r.id,
        subagent_id: r.subagent_id,
        profile: r.profile,
        status: r.status.parse().unwrap_or(TaskStatus::Queued),
        prompt: r.prompt,
        result_summary: r.result_summary,
        error: r.error,
        created_at_ms: r.created_at_ms,
        completed_at_ms: r.completed_at_ms,
    }
}
```

The manager references two new store methods (`subagent_list_filtered`, `subagent_hard_delete`) that don't exist yet. Add them now — the store layer is the single SQL owner.

- [ ] **Step 6: Add `subagent_list_filtered` + `subagent_hard_delete` store methods**

Add to `crates/busytok-store/src/subagent_queries.rs`. The list query builds its `WHERE` clause dynamically from the optional filters (status / project / include_deleted):

```rust
/// List subagents, optionally filtered by status and/or project.
/// `include_deleted = false` excludes soft-deleted rows.
pub fn list_filtered(
    conn: &Connection,
    status: Option<&str>,
    project: Option<&str>,
    include_deleted: bool,
) -> Result<Vec<SubagentLogicalSubagentRow>> {
    let mut sql = String::from(
        "SELECT id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, \
                default_model, status, created_at_ms, updated_at_ms, last_active_at_ms \
         FROM subagent_logical_subagents WHERE 1=1",
    );
    if !include_deleted {
        sql.push_str(" AND status != 'deleted'");
    }
    if status.is_some() {
        sql.push_str(" AND status = ?status");
    }
    if project.is_some() {
        sql.push_str(" AND project_id = ?project");
    }
    sql.push_str(" ORDER BY last_active_at_ms DESC NULLS LAST");

    let mut stmt = conn.prepare(&sql)?;
    let mut params_vec: Vec<(&str, &dyn rusqlite::ToSql)> = Vec::new();
    let status_val: String;
    if let Some(s) = status {
        status_val = s.to_string();
        params_vec.push((":status", &status_val));
    }
    let project_val: String;
    if let Some(p) = project {
        project_val = p.to_string();
        params_vec.push((":project", &project_val));
    }
    let rows = stmt
        .query_map(params_vec.as_slice(), |row| {
            Ok(SubagentLogicalSubagentRow {
                id: row.get(0)?,
                name: row.get(1)?,
                project_id: row.get(2)?,
                repo_path: row.get(3)?,
                repo_hash: row.get(4)?,
                branch: row.get(5)?,
                intent: row.get(6)?,
                default_profile: row.get(7)?,
                default_model: row.get(8)?,
                status: row.get(9)?,
                created_at_ms: row.get(10)?,
                updated_at_ms: row.get(11)?,
                last_active_at_ms: row.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Hard-delete a logical subagent and all its dependents, in FK-safe order.
///
/// Per spec §3.5 there is **no `ON DELETE CASCADE`** on the subagent tables —
/// audit data must never be silently removed. Hard delete is explicit, at the
/// application (store) layer: delete children in dependency order, then the row.
pub fn hard_delete_logical_subagent(conn: &Connection, id: &str) -> Result<()> {
    // usage_records reference both tasks and the logical row → delete first.
    conn.execute(
        "DELETE FROM subagent_usage_records WHERE subagent_id = ?1",
        params![id],
    )
    .with_context(|| format!("delete usage records for subagent {id}"))?;
    conn.execute("DELETE FROM subagent_tasks WHERE subagent_id = ?1", params![id])
        .with_context(|| format!("delete tasks for subagent {id}"))?;
    conn.execute("DELETE FROM subagent_harness_bindings WHERE subagent_id = ?1", params![id])
        .with_context(|| format!("delete bindings for subagent {id}"))?;
    conn.execute("DELETE FROM subagent_memory WHERE subagent_id = ?1", params![id])
        .with_context(|| format!("delete memory for subagent {id}"))?;
    // resource_events.target_id is a free-text column (no FK); subagent-scoped
    // events carry the subagent id there. Per spec §3.5 hard delete removes events.
    conn.execute("DELETE FROM subagent_resource_events WHERE target_id = ?1", params![id])
        .with_context(|| format!("delete resource events for subagent {id}"))?;
    conn.execute(
        "DELETE FROM subagent_logical_subagents WHERE id = ?1",
        params![id],
    )
    .map(|_| ())
    .with_context(|| format!("hard-delete logical subagent {id}"))
}
```

Add the wrappers in `db.rs` (use `self.conn()`):

```rust
pub fn subagent_list_filtered(
    &self,
    status: Option<&str>,
    project: Option<&str>,
    include_deleted: bool,
) -> Result<Vec<SubagentLogicalSubagentRow>> {
    subagent_queries::list_filtered(self.conn(), status, project, include_deleted)
}
pub fn subagent_hard_delete(&self, id: &str) -> Result<()> {
    subagent_queries::hard_delete_logical_subagent(self.conn(), id)
}
```

(`SubagentLogicalSubagentRow` is already imported into `subagent_queries.rs` via the existing `use crate::repository::{...}` at the top of that module — verify it's in the import list, add it if not.)

Now fix the resolver to canonicalize `cwd` at a single chokepoint (so CLI and manager agree on `repo_hash`). In `resolver::resolve_by_name`, replace the direct `derive_project_hash(cwd)` with a canonicalized form:

```rust
let canonical_cwd = std::fs::canonicalize(cwd)
    .map(|p| p.to_string_lossy().to_string())
    .unwrap_or_else(|_| cwd.to_string());
let repo_hash = derive_project_hash(&canonical_cwd);
```

and store `canonical_cwd` (not the raw `cwd`) into `repo_path`. This makes name resolution deterministic regardless of whether the caller canonicalized.

- [ ] **Step 7: Export modules from `lib.rs`**

`crates/busytok-subagent/src/lib.rs`:

```rust
pub mod error;
pub mod manager;
pub mod models;
pub mod resolver;

pub use error::{Result, SubagentError};
pub use manager::SubagentManager;
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p busytok-subagent`
Expected: PASS — all manager tests green.

- [ ] **Step 9: Commit**

```bash
git add crates/busytok-subagent crates/busytok-store/src/subagent_queries.rs crates/busytok-store/src/db.rs
git commit -m "feat(subagent): SubagentManager with CRUD, resolution, mock delegate"
```

---

### Task 9: Control protocol DTOs + `method_manifest`

**Files:**
- Modify: `crates/busytok-protocol/src/dto.rs`
- Modify: `crates/busytok-protocol/src/methods.rs`

**Interfaces:**
- Consumes: `busytok-subagent` models (Task 7).
- Produces: DTOs consumed by the dispatcher (Task 10) and the CLI (Task 11).

- [ ] **Step 1: Inspect the existing DTO style**

Read `crates/busytok-protocol/src/dto.rs` and mirror an existing request/response DTO pair (derive, `ts_rs` annotations if the file uses them). The new DTOs must serialize to/from the same shape the manager produces.

- [ ] **Step 2: Add typed request AND response DTOs to `dto.rs`**

Typed DTOs (request + response) live in `busytok-protocol`, matching the existing pattern. There is **no cycle**: `busytok-protocol` owns these plain structs; `busytok-runtime` depends on both `busytok-protocol` and `busytok-subagent` and maps the manager's models into the protocol DTOs. This keeps TS generation, response validation, and the dispatch arms (standard `serde_json::to_value(dto)?`) consistent with the rest of the control surface.

The user-facing contract (spec §7.1): `list` filters by `--status`/`--project`/`--include-deleted`; `show`/`tasks`/`hibernate`/`delete` resolve by **name + `--cwd`** or by **`--id`**. So the request DTOs carry resolution params, not bare ids.

```rust
// Mirror every existing DTO's derive set: Debug, Clone, Serialize, Deserialize, TS.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

// --- requests -------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct SubagentDelegateRequestDto {
    pub subagent_name: String,
    pub subagent_id: Option<String>,
    pub cwd: String,
    pub profile: String,
    pub intent: Option<String>,
    pub prompt: String,
    pub timeout_seconds: Option<u64>,
    pub model_override: Option<String>,
    pub source_harness: Option<String>,
    pub source_session_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct SubagentListRequestDto {
    /// "hot" | "warm" | "cold"
    pub status: Option<String>,
    pub project: Option<String>,
    pub include_deleted: Option<bool>,
}

/// Resolution params for single-subagent operations (show/tasks/hibernate/delete).
/// Exactly one of `id` (UUID) or `name` (+ `cwd`) should be set.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct SubagentResolveRequestDto {
    pub name: Option<String>,
    pub id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct SubagentTasksRequestDto {
    pub name: Option<String>,
    pub id: Option<String>,
    pub cwd: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct SubagentDeleteRequestDto {
    pub name: Option<String>,
    pub id: Option<String>,
    pub cwd: Option<String>,
    pub hard: Option<bool>,
}

// --- responses ------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentUsageDto {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentDelegateResponseDto {
    pub task_id: String,
    pub subagent_id: String,
    pub subagent_name: String,
    pub adapter: String,
    pub adapter_session_id: Option<String>,
    pub session_reused: bool,
    pub status: String,
    pub profile: String,
    pub model: Option<String>,
    pub summary: Option<String>,
    pub usage: SubagentUsageDto,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentDetailDto {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub repo_path: String,
    pub repo_hash: String,
    pub branch: Option<String>,
    pub intent: Option<String>,
    pub default_profile: String,
    pub default_model: Option<String>,
    pub status: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_active_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentListResponseDto {
    pub subagents: Vec<SubagentDetailDto>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentTaskSummaryDto {
    pub id: String,
    pub subagent_id: String,
    pub profile: String,
    pub status: String,
    pub prompt: Option<String>,
    pub result_summary: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentTasksResponseDto {
    pub tasks: Vec<SubagentTaskSummaryDto>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentAckDto {
    pub id: String,
    pub status: String,
}
```

TS types are **NOT** auto-exported by the `TS` derive. `crates/busytok-protocol/src/ts.rs` has a hand-maintained `type_defs: Vec<String>` list (a `#[test]` named `generate_typescript_types`) that drives regeneration of `packages/busytok-protocol-types/src/generated.ts`. Add every new DTO to that vec **in dependency order** (referenced types before referrers — `SubagentUsageDto` before `SubagentDelegateResponseDto`; `SubagentDetailDto` before `SubagentListResponseDto`; `SubagentTaskSummaryDto` before `SubagentTasksResponseDto`):

```rust
// inside generate_typescript_types(), append to the type_defs vec:
dto::SubagentUsageDto::decl(),
dto::SubagentDelegateResponseDto::decl(),
dto::SubagentResolveRequestDto::decl(),
dto::SubagentDelegateRequestDto::decl(),
dto::SubagentListRequestDto::decl(),
dto::SubagentDetailDto::decl(),
dto::SubagentListResponseDto::decl(),
dto::SubagentTaskSummaryDto::decl(),
dto::SubagentTasksRequestDto::decl(),
dto::SubagentTasksResponseDto::decl(),
dto::SubagentDeleteRequestDto::decl(),
dto::SubagentAckDto::decl(),
```

Then regenerate and include the `generated.ts` diff in the commit:

```bash
cargo test -p busytok_protocol generate_typescript_types
git add crates/busytok-protocol/src/ts.rs packages/busytok-protocol-types/src/generated.ts
```

**All response DTOs also derive `Default`** (e.g. `#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]`) — the `TestRuntimeControl` stubs in Task 10 return `Default::default()`, and every response field type (`String`, `i64`, `Option<_>`, `Vec<_>`) implements `Default`. Request DTOs do not need `Default`.

- [ ] **Step 3: Register methods in `method_manifest()`**

`method_manifest()` returns `Vec<String>`, so each entry needs `.to_string()` (matching the existing entries). In `crates/busytok-protocol/src/methods.rs`, append to the returned `Vec`:

```rust
"subagent.delegate".to_string(),
"subagent.list".to_string(),
"subagent.show".to_string(),
"subagent.tasks".to_string(),
"subagent.hibernate".to_string(),
"subagent.delete".to_string(),
```

(Do NOT add `doctor.check` here — `doctor` is deferred to a later plan; it does not exist in the manifest today.)

- [ ] **Step 4: Verify the protocol crate builds + tests**

Run: `cargo test -p busytok-protocol`
Expected: PASS (existing tests unaffected; new DTOs are inert until the dispatcher uses them).

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-protocol
git commit -m "feat(protocol): add subagent.* request/response DTOs and method manifest entries"
```

---

### Task 10: Dispatcher arms + `RuntimeControl` trait + supervisor wiring

**Files:**
- Modify: `crates/busytok-control/src/dispatch.rs`
- Modify: `crates/busytok-runtime/src/supervisor.rs`
- Modify: `crates/busytok-runtime/Cargo.toml` (add `busytok-subagent` dep)

**Interfaces:**
- Consumes: `SubagentManager` (Task 8), DTOs (Task 9).
- Produces: `subagent.*` methods reachable over IPC; `BusytokSupervisor` holds the manager.

- [ ] **Step 1: Extend the existing manifest/dispatch smoke test**

`ControlResponse` is an **enum** (`ControlResponse::Ok(serde_json::Value)` / `ControlResponse::Err(ControlError)`), and `ControlRequest` has a `new(method, params)` constructor. `TestRuntimeControl` is a fixture impl that does NOT own a real manager, so we verify *dispatch routing* (method → arm) through the existing smoke test by giving `TestRuntimeControl` stub returns, and verify *real behavior* through the supervisor e2e in Task 12.

In `crates/busytok-control/tests/dispatch.rs`, add the new methods to the param-driven smoke list inside `dispatcher_serves_all_surge_ui_methods` (or a sibling test). Because `TestRuntimeControl` will get stub impls in Step 3 that return `Ok(Default::default())`, these dispatch Ok with valid params:

```rust
// add to the param_methods vec in dispatcher_serves_all_surge_ui_methods:
("subagent.delegate", serde_json::json!({
    "subagent_name": "reviewer", "cwd": "/tmp/repo",
    "profile": "pi/search-cheap", "prompt": "x"
})),
("subagent.list", serde_json::json!({})),
("subagent.show", serde_json::json!({"id": "sa-1"})),
("subagent.tasks", serde_json::json!({"id": "sa-1"})),
("subagent.hibernate", serde_json::json!({"id": "sa-1"})),
("subagent.delete", serde_json::json!({"id": "sa-1"})),
```

Each must dispatch to `ControlResponse::Ok(_)` (the stub returns a `Default` DTO). This proves the arms + deserialization + manifest are wired.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p busytok-control --test dispatch dispatcher_serves_all_surge_ui_methods`
Expected: FAIL — no `subagent.delegate` arm, so the new entries hit `method_not_found`.

- [ ] **Step 3: Add trait methods to `RuntimeControl` + the `Arc<T>` blanket impl + `TestRuntimeControl` stubs**

The trait methods return the **typed response DTOs** defined in Task 9 (Step 2), matching the existing `RuntimeControl` pattern (`Result<ServiceHealthDto>`, …). The dispatch arm then does the standard `ControlResponse::ok(serde_json::to_value(dto)?)`. `busytok-control` already depends on `busytok-protocol` for these DTOs — no new dependency, no cycle.

In `crates/busytok-control/src/dispatch.rs`, add to the `RuntimeControl` trait:

```rust
async fn subagent_delegate(
    &self,
    req: busytok_protocol::dto::SubagentDelegateRequestDto,
) -> Result<busytok_protocol::dto::SubagentDelegateResponseDto>;
async fn subagent_list(
    &self,
    req: busytok_protocol::dto::SubagentListRequestDto,
) -> Result<busytok_protocol::dto::SubagentListResponseDto>;
async fn subagent_show(
    &self,
    req: busytok_protocol::dto::SubagentResolveRequestDto,
) -> Result<busytok_protocol::dto::SubagentDetailDto>;
async fn subagent_tasks(
    &self,
    req: busytok_protocol::dto::SubagentTasksRequestDto,
) -> Result<busytok_protocol::dto::SubagentTasksResponseDto>;
async fn subagent_hibernate(
    &self,
    req: busytok_protocol::dto::SubagentResolveRequestDto,
) -> Result<busytok_protocol::dto::SubagentAckDto>;
async fn subagent_delete(
    &self,
    req: busytok_protocol::dto::SubagentDeleteRequestDto,
) -> Result<busytok_protocol::dto::SubagentAckDto>;
```

The `Arc<T>` blanket impl (`impl<T: RuntimeControl> RuntimeControl for Arc<T>`) manually forwards every method — add all six (delegate to `(**self)`):

```rust
async fn subagent_delegate(&self, req: busytok_protocol::dto::SubagentDelegateRequestDto) -> Result<busytok_protocol::dto::SubagentDelegateResponseDto> { (**self).subagent_delegate(req).await }
async fn subagent_list(&self, req: busytok_protocol::dto::SubagentListRequestDto) -> Result<busytok_protocol::dto::SubagentListResponseDto> { (**self).subagent_list(req).await }
async fn subagent_show(&self, req: busytok_protocol::dto::SubagentResolveRequestDto) -> Result<busytok_protocol::dto::SubagentDetailDto> { (**self).subagent_show(req).await }
async fn subagent_tasks(&self, req: busytok_protocol::dto::SubagentTasksRequestDto) -> Result<busytok_protocol::dto::SubagentTasksResponseDto> { (**self).subagent_tasks(req).await }
async fn subagent_hibernate(&self, req: busytok_protocol::dto::SubagentResolveRequestDto) -> Result<busytok_protocol::dto::SubagentAckDto> { (**self).subagent_hibernate(req).await }
async fn subagent_delete(&self, req: busytok_protocol::dto::SubagentDeleteRequestDto) -> Result<busytok_protocol::dto::SubagentAckDto> { (**self).subagent_delete(req).await }
```

Add all six stub impls to `TestRuntimeControl` returning the DTO `Default` (so the fixture compiles and the dispatch smoke test confirms routing):

```rust
async fn subagent_delegate(&self, _req: busytok_protocol::dto::SubagentDelegateRequestDto) -> Result<busytok_protocol::dto::SubagentDelegateResponseDto> { Ok(Default::default()) }
async fn subagent_list(&self, _req: busytok_protocol::dto::SubagentListRequestDto) -> Result<busytok_protocol::dto::SubagentListResponseDto> { Ok(Default::default()) }
async fn subagent_show(&self, _req: busytok_protocol::dto::SubagentResolveRequestDto) -> Result<busytok_protocol::dto::SubagentDetailDto> { Ok(Default::default()) }
async fn subagent_tasks(&self, _req: busytok_protocol::dto::SubagentTasksRequestDto) -> Result<busytok_protocol::dto::SubagentTasksResponseDto> { Ok(Default::default()) }
async fn subagent_hibernate(&self, _req: busytok_protocol::dto::SubagentResolveRequestDto) -> Result<busytok_protocol::dto::SubagentAckDto> { Ok(Default::default()) }
async fn subagent_delete(&self, _req: busytok_protocol::dto::SubagentDeleteRequestDto) -> Result<busytok_protocol::dto::SubagentAckDto> { Ok(Default::default()) }
```

This requires the response DTOs to impl `Default`. Add `#[derive(Default)]` to each response DTO in Task 9 Step 2 (the request DTOs already derive what they need; `Default` on responses lets the stubs and any future fallback compile cleanly).

(Real behavior is covered by `BusytokSupervisor`'s impl + the Task 12 e2e; `TestRuntimeControl` stays a routing/serialization fixture, matching its existing role.)

- [ ] **Step 4: Add match arms in `ControlDispatcher::dispatch`**

Standard pattern — deserialize params, call the trait method, wrap the typed DTO with `serde_json::to_value`:

```rust
"subagent.delegate" => {
    let req: SubagentDelegateRequestDto = serde_json::from_value(request.params)
        .map_err(|e| anyhow::anyhow!("invalid params for subagent.delegate: {e}"))?;
    let dto = self.runtime.subagent_delegate(req).await?;
    ControlResponse::ok(serde_json::to_value(dto)?)
}
"subagent.list" => {
    let req: SubagentListRequestDto = serde_json::from_value(request.params)
        .map_err(|e| anyhow::anyhow!("invalid params for subagent.list: {e}"))?;
    let dto = self.runtime.subagent_list(req).await?;
    ControlResponse::ok(serde_json::to_value(dto)?)
}
"subagent.show" => {
    let req: SubagentResolveRequestDto = serde_json::from_value(request.params)
        .map_err(|e| anyhow::anyhow!("invalid params for subagent.show: {e}"))?;
    let dto = self.runtime.subagent_show(req).await?;
    ControlResponse::ok(serde_json::to_value(dto)?)
}
"subagent.tasks" => {
    let req: SubagentTasksRequestDto = serde_json::from_value(request.params)
        .map_err(|e| anyhow::anyhow!("invalid params for subagent.tasks: {e}"))?;
    let dto = self.runtime.subagent_tasks(req).await?;
    ControlResponse::ok(serde_json::to_value(dto)?)
}
"subagent.hibernate" => {
    let req: SubagentResolveRequestDto = serde_json::from_value(request.params)
        .map_err(|e| anyhow::anyhow!("invalid params for subagent.hibernate: {e}"))?;
    let dto = self.runtime.subagent_hibernate(req).await?;
    ControlResponse::ok(serde_json::to_value(dto)?)
}
"subagent.delete" => {
    let req: SubagentDeleteRequestDto = serde_json::from_value(request.params)
        .map_err(|e| anyhow::anyhow!("invalid params for subagent.delete: {e}"))?;
    let dto = self.runtime.subagent_delete(req).await?;
    ControlResponse::ok(serde_json::to_value(dto)?)
}
```

Import the request DTOs at the top of `dispatch.rs`:

```rust
use busytok_protocol::dto::{
    SubagentDelegateRequestDto, SubagentDeleteRequestDto, SubagentListRequestDto,
    SubagentResolveRequestDto, SubagentTasksRequestDto,
};
```

- [ ] **Step 5: Wire `SubagentManager` into `BusytokSupervisor`**

In `crates/busytok-runtime/Cargo.toml` add `busytok-subagent = { path = "../busytok-subagent" }`.

`build_with_settings(db: Database, paths, adapters, settings)` owns the raw `Database`, wraps it as `let db = Arc::new(Mutex::new(db));`, and later wraps `settings` as `Arc::new(Mutex::new(settings))`. Add a field and construct the manager **after the `Arc<Mutex<Database>>` exists and before `settings` is moved into its `Arc<Mutex>`**:

```rust
pub struct BusytokSupervisor {
    // ... existing fields ...
    subagent_manager: Arc<busytok_subagent::SubagentManager>,
}
```

Inside `build_with_settings`, immediately after `let db = Arc::new(Mutex::new(db));`:

```rust
let subagent_manager = Arc::new(busytok_subagent::SubagentManager::new(
    Arc::clone(&db),
    settings.subagent.clone(),
    "pi",
));
```

Store it on the returned `Self`. **Only `build_with_settings` constructs the `Self { ... }` struct literal** — verified that `new()`, `with_adapters()`, and `build()` all delegate to it and build no literal of their own, so add `subagent_manager,` to that single struct literal (and nowhere else). Expose an accessor:

```rust
pub fn subagent_manager(&self) -> &busytok_subagent::SubagentManager {
    &self.subagent_manager
}
```

> Note: the manager takes a `SubagentSettings` by value (its own clone), so runtime config changes to `settings.subagent` do NOT propagate to a running manager. This is fine for Plan 1 (mock). If Plan 2+ needs live config updates, have the manager share the supervisor's `Arc<Mutex<BusytokSettings>>` instead — out of scope here.

- [ ] **Step 6: Implement the `RuntimeControl` methods on `BusytokSupervisor`**

The supervisor maps `busytok-subagent` model types into the typed `busytok-protocol` DTOs. Define small conversion helpers (in `supervisor.rs` or a `subagent_bridge` module):

```rust
use busytok_control::dispatch::MethodDispatchError;
use busytok_protocol::dto::{
    SubagentAckDto, SubagentDelegateResponseDto, SubagentDetailDto, SubagentListResponseDto,
    SubagentTaskSummaryDto, SubagentTasksResponseDto, SubagentUsageDto,
};

fn map_subagent_error(e: busytok_subagent::SubagentError) -> anyhow::Error {
    MethodDispatchError::from_read_error(e.code(), e.to_string(), serde_json::Value::Null).into()
}

fn detail(s: busytok_subagent::models::LogicalSubagent) -> SubagentDetailDto {
    SubagentDetailDto {
        id: s.id, name: s.name, project_id: s.project_id, repo_path: s.repo_path,
        repo_hash: s.repo_hash, branch: s.branch, intent: s.intent,
        default_profile: s.default_profile, default_model: s.default_model,
        status: s.status.as_str().to_string(), created_at_ms: s.created_at_ms,
        updated_at_ms: s.updated_at_ms, last_active_at_ms: s.last_active_at_ms,
    }
}

fn task_summary(t: busytok_subagent::models::SubagentTaskSummary) -> SubagentTaskSummaryDto {
    SubagentTaskSummaryDto {
        id: t.id, subagent_id: t.subagent_id, profile: t.profile,
        status: t.status.as_str().to_string(), prompt: t.prompt,
        result_summary: t.result_summary, error: t.error,
        created_at_ms: t.created_at_ms, completed_at_ms: t.completed_at_ms,
    }
}

impl From<busytok_protocol::dto::SubagentDelegateRequestDto> for busytok_subagent::models::DelegateRequest {
    fn from(d: busytok_protocol::dto::SubagentDelegateRequestDto) -> Self {
        busytok_subagent::models::DelegateRequest {
            subagent_name: d.subagent_name, subagent_id: d.subagent_id, cwd: d.cwd,
            profile: d.profile, intent: d.intent, prompt: d.prompt,
            timeout_seconds: d.timeout_seconds, model_override: d.model_override,
            source_harness: d.source_harness, source_session_id: d.source_session_id,
        }
    }
}

impl From<busytok_protocol::dto::SubagentResolveRequestDto> for busytok_subagent::models::ResolveParams {
    fn from(r: busytok_protocol::dto::SubagentResolveRequestDto) -> Self {
        busytok_subagent::models::ResolveParams { name: r.name, id: r.id, cwd: r.cwd }
    }
}
```

Then the six trait handlers (delegate to the manager, map results into DTOs):

```rust
async fn subagent_delegate(&self, req: busytok_protocol::dto::SubagentDelegateRequestDto)
    -> Result<SubagentDelegateResponseDto>
{
    let r = self.subagent_manager.delegate(req.into()).await.map_err(map_subagent_error)?;
    Ok(SubagentDelegateResponseDto {
        task_id: r.task_id, subagent_id: r.subagent_id, subagent_name: r.subagent_name,
        adapter: r.adapter, adapter_session_id: r.adapter_session_id,
        session_reused: r.session_reused, status: r.status.as_str().to_string(),
        profile: r.profile, model: r.model, summary: r.summary,
        usage: SubagentUsageDto {
            model: r.usage.model, provider: r.usage.provider,
            input_tokens: r.usage.input_tokens, output_tokens: r.usage.output_tokens,
            cache_read_tokens: r.usage.cache_read_tokens,
            cache_write_tokens: r.usage.cache_write_tokens, cost_usd: r.usage.cost_usd,
        },
    })
}

async fn subagent_list(&self, req: busytok_protocol::dto::SubagentListRequestDto)
    -> Result<SubagentListResponseDto>
{
    let status = req.status.as_deref().and_then(|s| s.parse().ok());
    let subs = self.subagent_manager
        .list(status, req.project.as_deref(), req.include_deleted.unwrap_or(false))
        .await.map_err(map_subagent_error)?;
    Ok(SubagentListResponseDto { subagents: subs.into_iter().map(detail).collect() })
}

async fn subagent_show(&self, req: busytok_protocol::dto::SubagentResolveRequestDto)
    -> Result<SubagentDetailDto>
{
    let s = self.subagent_manager.show(req.into()).await.map_err(map_subagent_error)?;
    Ok(detail(s))
}

async fn subagent_tasks(&self, req: busytok_protocol::dto::SubagentTasksRequestDto)
    -> Result<SubagentTasksResponseDto>
{
    let resolve = busytok_subagent::models::ResolveParams { name: req.name, id: req.id, cwd: req.cwd };
    let tasks = self.subagent_manager.tasks(resolve, req.limit.unwrap_or(20))
        .await.map_err(map_subagent_error)?;
    Ok(SubagentTasksResponseDto { tasks: tasks.into_iter().map(task_summary).collect() })
}

async fn subagent_hibernate(&self, req: busytok_protocol::dto::SubagentResolveRequestDto)
    -> Result<SubagentAckDto>
{
    let id = self.subagent_manager.hibernate(req.into()).await.map_err(map_subagent_error)?;
    Ok(SubagentAckDto { id, status: "hibernated".to_string() })
}

async fn subagent_delete(&self, req: busytok_protocol::dto::SubagentDeleteRequestDto)
    -> Result<SubagentAckDto>
{
    let resolve = busytok_subagent::models::ResolveParams { name: req.name, id: req.id, cwd: req.cwd };
    let id = self.subagent_manager.delete(resolve, req.hard.unwrap_or(false))
        .await.map_err(map_subagent_error)?;
    Ok(SubagentAckDto { id, status: "deleted".to_string() })
}
```

`control_response_from_error` (called by the server layer) walks the error chain and downcasts to `MethodDispatchError`, surfacing `code`/`message` in `ControlResponse::Err`. Returning the mapped error from the trait method and letting the arm's `?` propagate it is the correct flow (same as other read handlers).

- [ ] **Step 7: Run the dispatch test and the runtime test suite**

Run: `cargo test -p busytok-control --test dispatch`
Expected: PASS — the six `subagent.*` entries now dispatch Ok.

Run: `cargo test -p busytok-runtime`
Expected: PASS — supervisor still boots (manager constructed cheaply at build).

- [ ] **Step 8: Commit**

```bash
git add crates/busytok-control crates/busytok-runtime
git commit -m "feat(runtime,control): wire SubagentManager into supervisor and dispatcher"
```

---

### Task 11: CLI — `delegate` + `subagent`

**Files:**
- Modify: `apps/cli/src/main.rs`
- Create: `apps/cli/src/commands_subagent.rs` (the CLI uses a single `commands.rs` file, not a `commands/` directory — verify by `ls apps/cli/src/`; if it is already a `commands/` dir module, add `commands/subagent.rs` instead. Current state: `apps/cli/src/commands.rs` is one file.)

**Interfaces:**
- Consumes: control client + DTOs (Tasks 9–10).
- Produces: `busytok delegate`, `busytok subagent {list,show,tasks,hibernate,delete}`.

> `doctor` is **deferred** to a later plan (it does not exist in the CLI today). Do not add it here.

- [ ] **Step 1: Write a failing CLI test (snapshot of help text)**

Add to `apps/cli/tests/help.rs` (the package is named `busytok`, so `CARGO_BIN_EXE_busytok` is correct):

```rust
#[test]
fn cli_exposes_delegate_and_subagent_commands() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_busytok"))
        .arg("--help")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("delegate"), "missing delegate subcommand");
    assert!(stdout.contains("subagent"), "missing subagent group");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p busytok --test help cli_exposes_delegate_and_subagent_commands`
Expected: FAIL — `delegate`/`subagent` not yet in `--help`.

- [ ] **Step 3: Add clap subcommands to `main.rs`**

Extend the `Command` enum:

```rust
Delegate {
    #[arg(long)]
    subagent: String,
    #[arg(long)]
    id: Option<String>,
    #[arg(long, default_value = ".")]
    cwd: String,
    #[arg(long)]
    profile: String,
    #[arg(long)]
    intent: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    timeout: Option<u64>,
    #[arg(long, default_value = "text")]
    output: String,
    /// The task prompt (positional)
    prompt: String,
},
Subagent {
    #[command(subcommand)]
    subcommand: SubagentCommand,
},
```

Add the `SubagentCommand` enum (spec §7.1: name-based resolution with optional `--id`/`--cwd`; `list` filters by `--status`/`--project`/`--include-deleted`). The `name`/`--id` mutual-exclusion is enforced **at the clap layer** (`required_unless_present` + `conflicts_with`) so `--help` and parse errors are clear and the manager's `resolve()` is pure fallback:

```rust
#[derive(Debug, Subcommand)]
enum SubagentCommand {
    List {
        /// "hot" | "warm" | "cold"
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        include_deleted: bool,
    },
    /// Resolve by <name> (within --cwd) or by --id <uuid>.
    Show {
        /// Subagent name (within --cwd). Required unless --id is given.
        #[arg(required_unless_present = "id", conflicts_with = "id")]
        name: Option<String>,
        /// Subagent UUID. Mutually exclusive with <name>.
        #[arg(long, conflicts_with = "name")]
        id: Option<String>,
        #[arg(long, default_value = ".")]
        cwd: String,
    },
    Tasks {
        #[arg(required_unless_present = "id", conflicts_with = "id")]
        name: Option<String>,
        #[arg(long, conflicts_with = "name")]
        id: Option<String>,
        #[arg(long, default_value = ".")]
        cwd: String,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    Hibernate {
        #[arg(required_unless_present = "id", conflicts_with = "id")]
        name: Option<String>,
        #[arg(long, conflicts_with = "name")]
        id: Option<String>,
        #[arg(long, default_value = ".")]
        cwd: String,
    },
    Delete {
        #[arg(required_unless_present = "id", conflicts_with = "id")]
        name: Option<String>,
        #[arg(long, conflicts_with = "name")]
        id: Option<String>,
        #[arg(long, default_value = ".")]
        cwd: String,
        #[arg(long)]
        hard: bool,
        #[arg(long)]
        yes: bool,
    },
}
```

With these constraints clap rejects "neither name nor --id" and "both name and --id" at parse time with a clear message; the manager's `resolve()` then only needs to handle the happy path (and `--id` takes priority).

- [ ] **Step 4: Implement `apps/cli/src/commands_subagent.rs`**

Each handler connects via `ControlClient` and calls the matching `subagent.*` method. **`ControlClient::call` takes a `ControlRequest` (built via `ControlRequest::new(method, params)`) and returns `Result<ControlResponse>`; `ControlResponse` is an enum** (`Ok(Value)` / `Err(ControlError)`). Mirror the existing `commands::handle_status()` IPC pattern — read it first for the exact connect helper and how it extracts `ControlResponse::Ok(data)`.

```rust
//! Handlers for `busytok delegate` and `busytok subagent …`.

use anyhow::{bail, Context, Result};
use busytok_control::ControlClient;
use busytok_protocol::{ControlRequest, ControlResponse};
use busytok_protocol::dto::{
    SubagentDeleteRequestDto, SubagentDelegateRequestDto, SubagentListRequestDto,
    SubagentResolveRequestDto, SubagentTasksRequestDto,
};

pub async fn handle_delegate(
    subagent: String,
    id: Option<String>,
    cwd: String,
    profile: String,
    intent: Option<String>,
    model: Option<String>,
    timeout: Option<u64>,
    output: String,
    prompt: String,
) -> Result<()> {
    // Connect the client here (mirrors the existing rpc_call/connect_client pattern).
    // Do NOT canonicalize cwd — the service resolver canonicalizes at one chokepoint.
    let mut client = crate::commands::connect_client()
        .await
        .with_context(|| "busytok-service is not running; subagent commands require the service. Start it and retry.")?;
    let req = SubagentDelegateRequestDto {
        subagent_name: subagent,
        subagent_id: id,
        cwd,
        profile,
        intent,
        prompt,
        timeout_seconds: timeout,
        model_override: model,
        source_harness: Some("cli".to_string()),
        source_session_id: None,
    };
    let resp = client
        .call(ControlRequest::new(
            "subagent.delegate",
            serde_json::to_value(&req)?,
        ))
        .await
        .context("subagent.delegate RPC failed")?;
    let data = unwrap_ok(resp)?;
    print_result(&data, &output)
}

/// Extract the Ok payload or bail with the control error message.
fn unwrap_ok(resp: ControlResponse) -> Result<serde_json::Value> {
    match resp {
        ControlResponse::Ok(v) => Ok(v),
        ControlResponse::Err(e) => bail!("{}: {}", e.code, e.message),
    }
}

fn print_result(value: &serde_json::Value, output: &str) -> Result<()> {
    match output {
        "json" => println!("{}", serde_json::to_string_pretty(value)?),
        _ => {
            let summary = value.get("summary").and_then(|v| v.as_str()).unwrap_or("(no summary)");
            println!("task:     {}", value.get("task_id").and_then(|v| v.as_str()).unwrap_or("?"));
            println!("subagent: {}", value.get("subagent_name").and_then(|v| v.as_str()).unwrap_or("?"));
            println!("status:   {}", value.get("status").and_then(|v| v.as_str()).unwrap_or("?"));
            println!("\n{summary}");
        }
    }
    Ok(())
}
```

`handle_list` / `handle_show` / `handle_tasks` / `handle_hibernate` / `handle_delete` follow the same connect→call→`unwrap_ok`→print shape, building the matching request DTO:

- `handle_list(status, project, include_deleted)` → `SubagentListRequestDto { status, project, include_deleted: Some(include_deleted) }`, method `subagent.list`; print the `subagents` array.
- `handle_show(name, id, cwd)` → `SubagentResolveRequestDto { name, id, cwd: Some(cwd) }`, method `subagent.show`.
- `handle_tasks(name, id, cwd, limit)` → `SubagentTasksRequestDto { name, id, cwd: Some(cwd), limit: Some(limit) }`, method `subagent.tasks`; print the `tasks` array.
- `handle_hibernate(name, id, cwd)` → `SubagentResolveRequestDto { name, id, cwd: Some(cwd) }`, method `subagent.hibernate`.
- `handle_delete(name, id, cwd, hard, yes)` → if `hard && !yes`, prompt for confirmation; then `SubagentDeleteRequestDto { name, id, cwd: Some(cwd), hard: Some(hard) }`, method `subagent.delete`.

**Connecting the client:** the existing handlers use a private `rpc_call` helper that owns a `ControlClient` from `connect_client()` (in `commands.rs`). Mirror that — either add the subagent handlers as functions in `commands.rs` that each call `connect_client().await?` internally, or expose `connect_client` (`pub(crate)`) and call it. The dispatch arms in `main()` then become:

```rust
Command::Delegate { subagent, id, cwd, profile, intent, model, timeout, output, prompt } => {
    commands_subagent::handle_delegate(subagent, id, cwd, profile, intent, model, timeout, output, prompt).await?;
}
Command::Subagent { subcommand } => match subcommand {
    SubagentCommand::Show { name, id, cwd } => commands_subagent::handle_show(name, id, cwd).await?,
    SubagentCommand::List { status, project, include_deleted } =>
        commands_subagent::handle_list(status, project, include_deleted).await?,
    // Tasks / Hibernate / Delete likewise
},
```

If the service is unreachable, `connect_client` already errors; wrap its call with `.with_context(|| "busytok-service is not running; subagent commands require the service. Start it and retry.")` so the guidance is clear.

- [ ] **Step 5: Run the CLI test + manual smoke**

Run: `cargo test -p busytok`
Expected: PASS.

Manual smoke (requires the service running): `cargo run -p busytok -- delegate --subagent reviewer --profile pi/search-cheap --cwd . "summarize this repo" --output json` → expect a JSON result with `status: completed` and a `[mock]` summary.

- [ ] **Step 6: Commit**

```bash
git add apps/cli
git commit -m "feat(cli): add delegate + subagent commands"
```

---

### Task 12: End-to-end integration test + coverage gate

**Files:**
- Modify: `crates/busytok-runtime/tests/supervisor_control.rs` (reuse the existing supervisor test boot + `RuntimeControl` exercise pattern)
- Modify: `scripts/coverage.sh` (raise gate) — see step 5

**Interfaces:**
- Consumes: everything from Tasks 1–11.
- Produces: a green end-to-end test exercising the full delegate→list→show→hibernate→delete flow through the supervisor's `RuntimeControl` impl, and confirms coverage ≥ 85%.

- [ ] **Step 1: Add the e2e test to the existing `supervisor_control.rs`**

`supervisor_control.rs` already constructs a `BusytokSupervisor` over an in-memory DB via the file-local helper `make_supervisor(db: Database, tmp: &TempDir) -> BusytokSupervisor` (which uses the fully-`pub` `BusytokSupervisor::new`). Reuse it — do NOT call `with_adapters_and_settings` (it is `pub(crate)`, unreachable from integration tests). The test drives the new subagent methods through the supervisor's `RuntimeControl` impl:

```rust
#[tokio::test]
async fn subagent_delegate_list_show_hibernate_delete_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let supervisor = make_supervisor(db, &tmp);
    use busytok_control::dispatch::RuntimeControl;

    // delegate (mock execution)
    let delegate_resp = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "reviewer".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/search-cheap".to_string(),
            intent: None,
            prompt: "find the bug".to_string(),
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .unwrap();
    // SubagentDelegateResponseDto serializes with task_id / subagent_id / status.
    let sub_id = delegate_resp.subagent_id.clone();
    assert_eq!(delegate_resp.status, "completed");

    // list (no filters → all active; the just-created subagent must appear).
    // Response is SubagentListResponseDto { subagents: [...] }.
    let list = supervisor
        .subagent_list(SubagentListRequestDto {
            status: None, project: None, include_deleted: Some(false),
        })
        .await
        .unwrap();
    assert!(list.subagents.iter().any(|s| s.id == sub_id));

    // show by UUID
    let shown = supervisor
        .subagent_show(SubagentResolveRequestDto {
            name: None, id: Some(sub_id.clone()), cwd: None,
        })
        .await
        .unwrap();
    assert_eq!(shown.name, "reviewer");

    // hibernate then still resolvable by name (memory written → warm)
    supervisor
        .subagent_hibernate(SubagentResolveRequestDto {
            name: None, id: Some(sub_id.clone()), cwd: None,
        })
        .await
        .unwrap();
    assert!(supervisor
        .subagent_show(SubagentResolveRequestDto {
            name: None, id: Some(sub_id.clone()), cwd: None,
        })
        .await
        .is_ok());

    // soft delete
    supervisor
        .subagent_delete(SubagentDeleteRequestDto {
            name: None, id: Some(sub_id.clone()), cwd: None, hard: Some(false),
        })
        .await
        .unwrap();
    // soft-deleted rows drop out of the active list
    let after_list = supervisor
        .subagent_list(SubagentListRequestDto {
            status: None, project: None, include_deleted: Some(false),
        })
        .await
        .unwrap();
    assert!(after_list.subagents.iter().all(|s| s.id != sub_id));
}
```

If `make_supervisor`'s signature differs from `(db, &tmp)`, match its real signature (read the helper's definition in the same file). The key constraints: use a `pub` supervisor constructor (`new`), an in-memory DB, and a temp dir for paths. Empty adapters are fine — subagent ops don't touch the adapter/tailer pipeline.

- [ ] **Step 2: Run the e2e test**

Run: `cargo test -p busytok-runtime --test supervisor_control subagent`
Expected: PASS.

- [ ] **Step 3: Run the whole workspace test suite**

Run: `cargo test --workspace --exclude busytok-gui`
Expected: all green.

- [ ] **Step 4: Lint + format (match the real gates in `scripts/verify_acceptance.sh`)**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean. Fix any findings.

- [ ] **Step 5: Measure coverage — enforce both gates**

Run the workspace gate (CI floor, 85%):

```bash
COVERAGE_GATE=85 bash scripts/coverage.sh
```

Then enforce the **per-crate 90% bar** the user asked for on the new code — this is the enforceable quality gate, not aspirational:

```bash
cargo llvm-cov -p busytok-subagent --fail-under-lines 90
```

Expected: both PASS. If the per-crate run is under 90%, add targeted tests for uncovered branches (error paths, `profile_model` fallback, name validation edge cases, status parsing, hibernate no-hot-binding path, the `resolve()` id-vs-name branches, hard-delete child ordering) until it passes.

> The enforceable gate is `busytok-subagent` at 90% (above). `busytok-store::subagent_queries` is a **target** of 90%+ (cargo-llvm-cov has no clean per-module gate, and gating the whole `busytok-store` crate at 90% would be too aggressive given its legacy modules sit below that). Aim for 90%+ on `subagent_queries` too; the workspace 85% floor is the only hard gate covering it.

`scripts/coverage.sh` uses `cargo llvm-cov --workspace --exclude busytok-gui`, so the new `busytok-subagent` crate is now in the measured denominator. The workspace currently sits at ~81.8% — a large new crate could pull the total either way. **Ratchet rule:** only commit the `GATE` change if `bash scripts/coverage.sh` passes at 85 *with the new crate included*. If workspace coverage is below 85 after adding the crate, do NOT ratchet — instead add tests until the new `busytok-subagent` / `subagent_queries` code is ≥90% covered, then ratchet:

```diff
-GATE="${COVERAGE_GATE:-80}"
+GATE="${COVERAGE_GATE:-85}"
```

- [ ] **Step 6: Commit**

```bash
git add crates/busytok-runtime/tests/supervisor_control.rs scripts/coverage.sh
git commit -m "test(subagent): end-to-end delegate round-trip via supervisor RuntimeControl + ratchet coverage gate to 85"
```

---

## Self-Review

Run this checklist after writing; results recorded inline.

**1. Spec coverage (Plan 1 = spec Step 1 scope):**
- SQLite schema (6 tables, `_ms` suffix, CHECK, partial unique index) → Task 2 ✓
- Row structs → Task 3 ✓
- Query functions + Database wrappers → Task 4 ✓
- Config (SubagentSettings, profiles, TOML) → Task 5 ✓
- BusytokPaths artifacts_dir → Task 6 ✓ (`sidecar_runtime_dir` deferred to Plan 2)
- Domain models (status enums, subagent, task, delegate) → Task 7 ✓
- SubagentManager (CRUD, name resolution, delegate, hibernate, delete, status invariants) → Task 8 ✓
- Control DTOs + method_manifest → Task 9 ✓
- RuntimeControl trait + dispatcher + supervisor wiring (Arc<T>, TestRuntimeControl) → Task 10 ✓
- CLI delegate/subagent → Task 11 ✓ (`doctor` deferred — does not exist today)
- Observability (tracing event_code) → woven into Tasks 8, 10 ✓
- Coverage ≥ 85% / target 90% → Task 12 ✓
- Out-of-scope-for-Plan-1 (Pi sidecar, real session pool, prepare_hibernate, context builder, resource monitor, `doctor` command, `sysinfo` workspace dep) → explicitly deferred to Plans 2–5. The mock executor stands in. ✓ (`sysinfo` is added in Plan 5 with ResourceMonitor; not needed for the mock layer.)

**2. Placeholder scan:** Tasks contain real code. Two explicit "inspect first" notes (existing test helpers / constructor shapes in `dispatch.rs`, `main.rs`, `dto.rs`) are genuine code-discovery steps, not placeholders — they guard against the plan asserting field/method names that the live codebase may spell differently. Each such step tells the implementer exactly what to look for and how to adapt.

**3. Type consistency (verified against the live codebase shapes):**
- `SubagentStatus` / `TaskStatus` enums: `as_str()` + `FromStr` defined in Task 7, consumed in Task 8 manager and Task 12 e2e ✓
- `DelegateRequest` (models, Task 7) vs `SubagentDelegateRequestDto` (protocol, Task 9): Task 10 Step 6 defines a `From`/`req_into` conversion; fields line up (subagent_name, subagent_id, cwd, profile, intent, prompt, timeout_seconds, model_override, source_harness, source_session_id) ✓
- `DelegateResult` fields (task_id, subagent_id, subagent_name, adapter, adapter_session_id, session_reused, status, profile, model, summary, usage) — Task 7 defines, Task 8 produces, Task 11 CLI reads (task_id/subagent_name/status/summary) ✓
- Store `Database` method names (`subagent_upsert_logical`, `subagent_get_logical`, `subagent_list_active_by_repo`, `subagent_find_by_name_in_repo`, `subagent_upsert_memory`, `subagent_get_memory`, `subagent_insert_task`, `subagent_get_task`, `subagent_list_tasks`, `subagent_set_task_status`, `subagent_upsert_binding`, `subagent_hot_binding`, `subagent_insert_usage_record`, `subagent_insert_resource_event`, `subagent_hard_delete`) — defined in Task 4, consumed in Tasks 8 and 12 ✓
- `SubagentManager::new(db: Arc<Mutex<Database>>, settings, adapter)` — Task 8 defines, Task 10 supervisor wiring uses `Arc::clone(&db)` + `settings.subagent.clone()` ✓
- Control layer (confirmed against `crates/busytok-control/`, `crates/busytok-protocol/`): `ControlResponse` is an enum (`Ok(Value)`/`Err(ControlError)`); `ControlRequest::new(method, params)`; `ControlClient::call(ControlRequest) -> Result<ControlResponse>`; `MethodDispatchError::from_read_error(code, message, payload)`; `dispatch()` arms return `ControlResponse::ok(value)` with `?`-propagated errors. Tasks 9–11 reflect these. ✓
- CLI crate is named `busytok` (not `busytok-cli`); `CARGO_BIN_EXE_busytok` is the correct bin env var. Task 11 test + run commands use `cargo test -p busytok` / `cargo run -p busytok`. ✓
- Domain helpers confirmed present: `busytok_domain::derive_project_hash(&str) -> String` (identity.rs:103), `busytok_domain::now_ms() -> i64` (time.rs:7). Used in Tasks 4, 7, 8. ✓
- Real gates matched: `cargo fmt --all --check` + `cargo clippy --workspace --all-targets -- -D warnings` (`scripts/verify_acceptance.sh`); coverage via `cargo llvm-cov --workspace --exclude busytok-gui --fail-under-lines`. Task 12 reflects these. ✓

No type drift found.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-25-busytok-subagent-foundation.md`. Two execution options:

1. **Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach? (Plans 2–5 for the Pi sidecar, session pool, context/memory, and resource monitoring will be written after this foundation lands.)
