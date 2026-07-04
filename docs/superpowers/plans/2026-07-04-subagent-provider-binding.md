# Subagent Provider/Model Binding + Pi SDK Multi-API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate routing truth to `subagent.bound_provider_id + bound_model_id`, extend `ProviderKind` with `anthropic_compatible`, and integrate Pi SDK via `AuthStorage.inMemory` + `ModelRegistry.create` + `registerProvider` (replacing `OPENAI_*` env var injection).

**Architecture:** The SQL `models` table gains metadata (`display_name` / `reasoning` / `context_window` / `max_tokens`); `subagent_logical_subagents` is rebuilt with NOT NULL `bound_provider_id` / `bound_model_id` (no `default_model`); `Profile` is downgraded to a pure behavior template (no `provider_id` / `model`); the sidecar `defaultSessionFactory` builds a fresh `ModelRegistry` per session miss via `registerProvider` + `AuthStorage.inMemory` (no env var indirection, no global cache); `provider_test_connection` dispatches by `provider_kind`; provider/model delete switches from "block if referenced" to "allow delete, dangling binding, delegate fails".

**Tech Stack:** Rust (rusqlite + anyhow + tracing + tokio + async-trait), TypeScript (Pi SDK `@earendil-works/pi-coding-agent`), React + TanStack Query (GUI), ts_rs (DTO → TS codegen).

## Global Constraints

- All code on branch `feat/provider-model-catalog-refactor` (already checked out)
- Spec: `docs/superpowers/specs/2026-07-04-subagent-provider-binding-design.md` (approved)
- Reuse existing infrastructure: `tracing` for logging (event_code fields), existing migration system, existing `WorkerPool` / `provider_changed` mechanism, existing `ModelRegistry.create` + `AuthStorage.inMemory` SDK APIs
- No `process.env` for provider secrets — `AuthStorage.inMemory` is the sole source
- No file I/O for model registry — `ModelRegistry.create(authStorage)` + `registerProvider` only
- `provider_api_key` is瞬态执行态数据: not written to task row, not logged in plaintext, not in any DTO/response/diagnostic
- Tests must use TDD (write failing test first, then implementation, then commit)
- Rust coverage must remain >90% (`cargo llvm-cov --workspace --fail-under-lines 90`)
- GUI coverage must remain >90% (`pnpm coverage:gui` — vitest with `--coverage.thresholds.lines 90`; also enforces functions/branches/statements 90). This is a hard gate because Task 8 modifies `ProvidersPage.tsx` + `ModelsSection.tsx`.
- `pi-sidecar` has no coverage script in this phase — its quality gate is `pnpm -F pi-sidecar test && pnpm -F pi-sidecar typecheck` only. Do NOT add a `lint` script (doesn't exist in `apps/pi-sidecar/package.json`).
- `cargo clippy --workspace --all-targets -- -D warnings` must pass
- `cargo test --workspace` must pass
- `cargo test -p busytok-protocol --features export-ts` then `pnpm -F busytok-protocol-types build` to regenerate TS types
- Each commit message prefixed with `feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:` as appropriate
- After each task: run `cargo fmt --all` before commit

---

## File Structure

**Migrations (new):**
- `crates/busytok-store/migrations/0007_subagent_route_binding_and_model_metadata.sql` — model metadata + subagent_logical_subagents rebuild

**Store layer (modify):**
- `crates/busytok-store/src/schema.rs` — `SCHEMA_VERSION = 7`, register migration 0007
- `crates/busytok-store/src/provider_catalog.rs` — `CreateModelReq` / `UpdateModelPatch` / `UpdateProviderPatch` (+`provider_kind: Option<ProviderKind>` field, spec §7.1) / `update_provider` (handle `provider_kind` patch) / `row_to_model` / `create_model` / `update_model` / `get_model_by_id` / `get_model_by_provider_and_model_id` / `list_models_filtered` / `delete_provider` / `delete_model` (drop `profile_refs` param)
- `crates/busytok-store/src/subagent_queries.rs` — `subagent_upsert_logical` / `subagent_get_logical` / `subagent_find_by_name_in_repo` / `subagent_list_active_by_repo` / `subagent_list_filtered` SQL columns
- `crates/busytok-store/src/repository.rs` — `SubagentLogicalSubagentRow` (drop `default_model`, add `bound_provider_id` + `bound_model_id`)

**Domain layer (modify):**
- `crates/busytok-domain/src/provider_catalog.rs` — `ProviderKind::AnthropicCompatible`, `Model` metadata fields, **delete** `ProfileModelRef`

**Config layer (modify):**
- `crates/busytok-config/src/lib.rs` — delete `SubagentModelsConfig` + `default_*_model` helpers; `SubagentProfileConfig` drops `provider_id` + `model`; `default_profiles()` drops those fields

**Protocol layer (modify):**
- `crates/busytok-protocol/src/dto.rs` — `SubagentDelegateRequestDto` (+bound fields), `SubagentDetailDto` (-default_model +bound fields), `ModelCreateRequestDto` (+metadata), `ModelUpdateRequestDto` (+metadata patch), `ModelCatalogEntryDto` (+metadata), `ProviderUpdateRequestDto` (+`provider_kind: Option<ProviderKind>` patch, spec §7.1), `ProfileDto` / `ProfileCreateRequestDto` / `ProfileUpdateRequestDto` (-provider_id / -model)

**Subagent crate (modify):**
- `crates/busytok-subagent/src/models.rs` — `LogicalSubagent` (-default_model +bound fields), `DelegateRequest` (+bound fields)
- `crates/busytok-subagent/src/resolver.rs` — `resolve_by_name` / `create_subagent` / `row_to_model` new signatures; creation-time provider/model validation
- `crates/busytok-subagent/src/manager.rs` — delete `profile_model()`; rewrite `delegate()` / `execute_task()` to use `bound_provider_id` + `effective_model_id`
- `crates/busytok-subagent/src/mock_executor.rs` — `ExecutorInput` (-Option provider_id/model, +provider_kind/base_url/api_key)
- `crates/busytok-subagent/src/sidecar/pool.rs` — **delete** `inject_provider_env` (function + callers); `ProviderRuntimeEntry` stays as-is
- `crates/busytok-subagent/src/sidecar/executor.rs` — `turn_auto` params extend with `provider_kind` / `provider_base_url` / `provider_api_key` + model metadata

**Runtime supervisor (modify):**
- `crates/busytok-runtime/src/supervisor.rs` — `construct_sidecar` (no profile model lookup); `subagent_delegate` (new validation chain via bound fields, drop profile provider/model check); `provider_test_connection` (dispatch by `provider_kind`); `provider_update` (plumb `provider_kind` from DTO to `UpdateProviderPatch`; existing `provider_changed` call already kills worker per spec §7.1); `provider_delete` / `model_delete` (drop `collect_profile_refs`); **delete** `collect_profile_refs`

**TypeScript sidecar (modify):**
- `apps/pi-sidecar/src/types.ts` — `TurnAutoParams` (+provider_kind/base_url/api_key + model metadata)
- `apps/pi-sidecar/src/pi_session.ts` — `CreateSessionOpts` (required model + provider fields + metadata); `defaultSessionFactory` rewrite (delete `cachedRegistry` + `resolveModelObject`, inline `AuthStorage.inMemory` + `ModelRegistry.create` + `registerProvider`)
- `apps/pi-sidecar/src/handlers/turn_auto.ts` — thread new params to `pool.ensure()`

**Generated types (regenerate):**
- `packages/busytok-protocol-types/src/generated.ts` — via `cargo test -p busytok-protocol --features export-ts`

**GUI (modify):**
- `apps/gui/src/pages/ProvidersPage.tsx` — provider_kind selector UI
- `apps/gui/src/components/ModelsSection.tsx` — model metadata form (context_window + max_tokens required)

---

## Task 1: Migration 0007 + SCHEMA_VERSION bump + ProviderKind::AnthropicCompatible + Model metadata

**Files:**
- Create: `crates/busytok-store/migrations/0007_subagent_route_binding_and_model_metadata.sql`
- Modify: `crates/busytok-store/src/schema.rs:5` (SCHEMA_VERSION 6 → 7)
- Modify: `crates/busytok-store/src/schema.rs:44-53` (`migrations()` append entry 7)
- Modify: `crates/busytok-domain/src/provider_catalog.rs:8-12` (ProviderKind enum)
- Modify: `crates/busytok-domain/src/provider_catalog.rs:58-66` (Model struct)
- Modify: `crates/busytok-domain/src/provider_catalog.rs:107-148` (tests)
- Modify: `crates/busytok-store/src/provider_catalog.rs:30-40` (CreateModelReq + UpdateModelPatch)
- Modify: `crates/busytok-store/src/provider_catalog.rs:144-200` (create_model / update_model / row_to_model)
- Modify: `crates/busytok-store/src/provider_catalog.rs` (get_model_by_id / get_model_by_provider_and_model_id / list_models_filtered SQL columns)
- Test: `crates/busytok-store/tests/provider_catalog.rs`

**Interfaces:**
- Consumes: existing `ProviderKind` enum, existing `Model` struct, existing `providers`/`models` tables
- Produces:
  - `ProviderKind::AnthropicCompatible` (serde `"anthropic_compatible"`)
  - `Model { display_name: Option<String>, reasoning: bool, context_window: Option<i64>, max_tokens: Option<i64> }`
  - `CreateModelReq { display_name: Option<String>, reasoning: Option<bool>, context_window: Option<i64>, max_tokens: Option<i64> }`
  - `UpdateModelPatch { enabled: Option<bool>, display_name: Option<String>, reasoning: Option<bool>, context_window: Option<i64>, max_tokens: Option<i64> }`
  - SQL migration `0007_subagent_route_binding_and_model_metadata.sql`
  - `SCHEMA_VERSION = 7`

- [ ] **Step 1: Write the migration SQL file**

Create `crates/busytok-store/migrations/0007_subagent_route_binding_and_model_metadata.sql`:

```sql
-- 0007_subagent_route_binding_and_model_metadata.sql
-- Spec §2.2 + §2.3: add model metadata columns + rebuild subagent_logical_subagents
-- with NOT NULL bound_provider_id + bound_model_id (drops default_model).

-- 1. models table metadata (spec §2.2)
ALTER TABLE models ADD COLUMN display_name TEXT;
ALTER TABLE models ADD COLUMN reasoning INTEGER NOT NULL DEFAULT 0;
ALTER TABLE models ADD COLUMN context_window INTEGER;
ALTER TABLE models ADD COLUMN max_tokens INTEGER;

-- 2. Drop child tables that FK to subagent_logical_subagents (no ON DELETE CASCADE).
--    Order matters: drop referencing tables first.
DROP TABLE IF EXISTS subagent_usage_records;
DROP TABLE IF EXISTS subagent_harness_bindings;
DROP TABLE IF EXISTS subagent_tasks;
DROP TABLE IF EXISTS subagent_memory;

-- 3. Drop and recreate the parent table with bound_provider_id + bound_model_id
--    NOT NULL (no default_model). Existing data is discarded (project未上线).
DROP TABLE IF EXISTS subagent_logical_subagents;

CREATE TABLE subagent_logical_subagents (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    project_id TEXT NOT NULL,
    repo_path TEXT NOT NULL,
    repo_hash TEXT NOT NULL,
    branch TEXT,
    intent TEXT,
    default_profile TEXT NOT NULL,
    bound_provider_id TEXT NOT NULL,
    bound_model_id TEXT NOT NULL,
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

-- 4. Recreate child tables with schema equivalent to migrations 0003+0004+0005
--    applied (subagent_tasks has timeout_seconds/model_override from 0004 and
--    error_kind from 0005; do NOT use only the 0003 definition).

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
    timeout_seconds INTEGER,
    model_override TEXT,
    error_kind TEXT,
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
```

- [ ] **Step 2: Update `schema.rs` — bump SCHEMA_VERSION + register migration**

Edit `crates/busytok-store/src/schema.rs`:

```rust
pub const SCHEMA_VERSION: u32 = 7;
```

Add after the `PROVIDER_CATALOG_SQL` const:

```rust
/// v7 subagent-route-binding + model-metadata migration SQL — adds
/// `display_name` / `reasoning` / `context_window` / `max_tokens` to `models`,
/// and rebuilds `subagent_logical_subagents` with NOT NULL
/// `bound_provider_id` + `bound_model_id` (drops `default_model`).
const SUBAGENT_ROUTE_BINDING_AND_MODEL_METADATA_SQL: &str =
    include_str!("../migrations/0007_subagent_route_binding_and_model_metadata.sql");
```

Append to `migrations()`:

```rust
pub fn migrations() -> Vec<(u32, &'static str)> {
    vec![
        (1, BASELINE_SQL),
        (2, CACHE_METRICS_SQL),
        (3, SUBAGENT_SQL),
        (4, SUBAGENT_TASK_FIELDS_SQL),
        (5, SUBAGENT_TASK_ERROR_KIND_SQL),
        (6, PROVIDER_CATALOG_SQL),
        (7, SUBAGENT_ROUTE_BINDING_AND_MODEL_METADATA_SQL),
    ]
}
```

- [ ] **Step 3: Write the failing test for `ProviderKind::AnthropicCompatible`**

Edit `crates/busytok-domain/src/provider_catalog.rs` tests module (append):

```rust
#[test]
fn provider_kind_serde_anthropic_compatible() {
    let json = serde_json::to_string(&ProviderKind::AnthropicCompatible).unwrap();
    assert_eq!(json, "\"anthropic_compatible\"");
    let parsed: ProviderKind = serde_json::from_str("\"anthropic_compatible\"").unwrap();
    assert_eq!(parsed, ProviderKind::AnthropicCompatible);
}
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo test -p busytok-domain --lib provider_catalog::tests::provider_kind_serde_anthropic_compatible`
Expected: FAIL with "no variant named `AnthropicCompatible`" (compile error)

- [ ] **Step 5: Add `AnthropicCompatible` variant**

Edit `crates/busytok-domain/src/provider_catalog.rs` enum:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum ProviderKind {
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    #[serde(rename = "anthropic_compatible")]
    AnthropicCompatible,
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p busytok-domain --lib provider_catalog::tests`
Expected: PASS

- [ ] **Step 7: Write the failing test for Model metadata**

Append to `crates/busytok-domain/src/provider_catalog.rs` tests module:

```rust
#[test]
fn model_struct_carries_metadata_fields() {
    // Verifies the new fields exist on the Model struct (compile-time check)
    let m = Model {
        id: "db-1".into(),
        provider_id: "p1".into(),
        model_id: "gpt-4o".into(),
        enabled: true,
        created_at_ms: 1000,
        updated_at_ms: 1000,
        display_name: Some("GPT-4o".into()),
        reasoning: false,
        context_window: Some(128000),
        max_tokens: Some(16384),
    };
    assert_eq!(m.display_name.as_deref(), Some("GPT-4o"));
    assert!(!m.reasoning);
    assert_eq!(m.context_window, Some(128000));
    assert_eq!(m.max_tokens, Some(16384));
}
```

- [ ] **Step 8: Run test to verify it fails**

Run: `cargo test -p busytok-domain --lib provider_catalog::tests::model_struct_carries_metadata_fields`
Expected: FAIL with "no field `display_name`" (compile error)

- [ ] **Step 9: Add metadata fields to `Model` struct**

Edit `crates/busytok-domain/src/provider_catalog.rs`:

```rust
#[derive(Debug, Clone)]
pub struct Model {
    pub id: String,
    pub provider_id: String,
    pub model_id: String,
    pub enabled: bool,
    pub display_name: Option<String>,
    pub reasoning: bool,
    pub context_window: Option<i64>,
    pub max_tokens: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}
```

- [ ] **Step 10: Run test to verify it passes**

Run: `cargo test -p busytok-domain --lib provider_catalog::tests::model_struct_carries_metadata_fields`
Expected: PASS (but workspace won't compile yet — store layer still uses old shape)

- [ ] **Step 11: Update store `CreateModelReq` + `UpdateModelPatch`**

Edit `crates/busytok-store/src/provider_catalog.rs`:

```rust
pub struct CreateModelReq {
    pub provider_id: String,
    pub model_id: String,
    pub enabled: bool,
    pub tags: Vec<String>,
    pub display_name: Option<String>,
    pub reasoning: Option<bool>,
    pub context_window: Option<i64>,
    pub max_tokens: Option<i64>,
}

pub struct UpdateModelPatch {
    pub enabled: Option<bool>,
    pub display_name: Option<String>,
    pub reasoning: Option<bool>,
    pub context_window: Option<i64>,
    pub max_tokens: Option<i64>,
}
```

- [ ] **Step 12: Update `create_model` SQL + bindings**

Edit `crates/busytok-store/src/provider_catalog.rs::create_model`:

```rust
pub fn create_model(conn: &Connection, req: CreateModelReq) -> Result<Model> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = busytok_domain::now_ms();
    let reasoning = req.reasoning.unwrap_or(false) as i64;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO models (id, provider_id, model_id, enabled, display_name, reasoning, context_window, max_tokens, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        params![
            id,
            req.provider_id,
            req.model_id,
            req.enabled as i64,
            req.display_name,
            reasoning,
            req.context_window,
            req.max_tokens,
            now,
        ],
    )
    .map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            anyhow!("model '{}' already exists for provider '{}'", req.model_id, req.provider_id)
        } else {
            anyhow!(e)
        }
    })?;
    let mut seen = std::collections::HashSet::new();
    for tag in &req.tags {
        if seen.insert(tag.clone()) {
            tx.execute(
                "INSERT INTO model_tags (model_id, tag) VALUES (?1, ?2)",
                params![id, tag],
            )?;
        }
    }
    tx.commit()?;
    info!(event_code = "model.created", model_id = %req.model_id, provider_id = %req.provider_id, "model created");
    get_model_by_id(conn, &id)?.ok_or_else(|| anyhow!("model {} not found after insert", id))
}
```

- [ ] **Step 13: Update `update_model` to handle metadata patches**

Edit `crates/busytok-store/src/provider_catalog.rs::update_model`:

```rust
pub fn update_model(conn: &Connection, id: &str, patch: UpdateModelPatch) -> Result<Model> {
    let now = busytok_domain::now_ms();
    let tx = conn.unchecked_transaction()?;
    let mut dirty = false;
    if let Some(enabled) = patch.enabled {
        tx.execute("UPDATE models SET enabled = ?1, updated_at_ms = ?2 WHERE id = ?3", params![enabled as i64, now, id])?;
        dirty = true;
    }
    if let Some(display_name) = &patch.display_name {
        tx.execute("UPDATE models SET display_name = ?1, updated_at_ms = ?2 WHERE id = ?3", params![display_name, now, id])?;
        dirty = true;
    }
    if let Some(reasoning) = patch.reasoning {
        tx.execute("UPDATE models SET reasoning = ?1, updated_at_ms = ?2 WHERE id = ?3", params![reasoning as i64, now, id])?;
        dirty = true;
    }
    if let Some(context_window) = patch.context_window {
        tx.execute("UPDATE models SET context_window = ?1, updated_at_ms = ?2 WHERE id = ?3", params![context_window, now, id])?;
        dirty = true;
    }
    if let Some(max_tokens) = patch.max_tokens {
        tx.execute("UPDATE models SET max_tokens = ?1, updated_at_ms = ?2 WHERE id = ?3", params![max_tokens, now, id])?;
        dirty = true;
    }
    if !dirty {
        bail!("model update patch is empty");
    }
    // Verify the row exists (UPDATE may have affected 0 rows silently).
    let exists: bool = tx.query_row("SELECT 1 FROM models WHERE id = ?1", params![id], |_| Ok(true)).optional()?.is_some();
    if !exists {
        bail!("model not found: {}", id);
    }
    tx.commit()?;
    info!(event_code = "model.updated", model_db_id = %id, "model updated");
    get_model_by_id(conn, id)?.ok_or_else(|| anyhow!("model {} not found after update", id))
}
```

- [ ] **Step 14: Update `row_to_model` to read new columns**

Find the `row_to_model` function in `crates/busytok-store/src/provider_catalog.rs` and replace its body to read 10 columns (`id`, `provider_id`, `model_id`, `enabled`, `display_name`, `reasoning`, `context_window`, `max_tokens`, `created_at_ms`, `updated_at_ms`):

```rust
fn row_to_model(row: &rusqlite::Row<'_>) -> rusqlite::Result<Model> {
    Ok(Model {
        id: row.get(0)?,
        provider_id: row.get(1)?,
        model_id: row.get(2)?,
        enabled: row.get::<_, i64>(3)? != 0,
        display_name: row.get(4)?,
        reasoning: row.get::<_, i64>(5)? != 0,
        context_window: row.get(6)?,
        max_tokens: row.get(7)?,
        created_at_ms: row.get(8)?,
        updated_at_ms: row.get(9)?,
    })
}
```

- [ ] **Step 15: Update all Model SELECT queries to read 10 columns**

In `crates/busytok-store/src/provider_catalog.rs`, update these query strings to read `id, provider_id, model_id, enabled, display_name, reasoning, context_window, max_tokens, created_at_ms, updated_at_ms`:

- `get_model_by_id`
- `get_model_by_provider_and_model_id`
- `list_models_filtered` (the per-model SELECT; the join columns for `ModelCatalogEntry` will be updated in this same step)

For `get_model_by_id` and `get_model_by_provider_and_model_id` (single-row lookups via `row_to_model`), the SQL becomes:

```sql
SELECT id, provider_id, model_id, enabled, display_name, reasoning, context_window, max_tokens, created_at_ms, updated_at_ms
FROM models WHERE id = ?1
-- or
FROM models WHERE provider_id = ?1 AND model_id = ?2
```

`row_to_model` (updated in Step 14) already reads the 10 columns in this order — no further change needed.

For `list_models_filtered` (the JOIN query producing `ModelCatalogEntry`), update both the SQL and the row mapper. The SQL now selects 12 columns (8 from the join + 4 metadata columns from `models`):

```rust
let sql = format!(
    "SELECT p.id, p.name, p.provider_kind, p.enabled,
            m.id, m.model_id, m.enabled,
            m.display_name, m.reasoning, m.context_window, m.max_tokens,
            COALESCE(GROUP_CONCAT(mt.tag, ','), '') AS tags_csv
     FROM models m
     JOIN providers p ON p.id = m.provider_id
     LEFT JOIN model_tags mt ON mt.model_id = m.id
     WHERE (?1 = 1 OR (p.enabled = 1 AND m.enabled = 1))
       AND (?2 IS NULL OR m.provider_id = ?2)
     GROUP BY m.id
     {tag_clause}
     ORDER BY p.name, m.model_id"
);

let mut stmt = conn.prepare(&sql)?;
// ... (existing params_from_iter binding logic unchanged) ...
let rows = stmt.query_map(params_from_iter(params_vec.iter().map(|b| b.as_ref())), |row| {
    let tags_csv: String = row.get(11)?;
    let tags: Vec<String> = if tags_csv.is_empty() {
        vec![]
    } else {
        tags_csv.split(',').map(|s| s.to_string()).collect()
    };
    let kind_str: String = row.get(2)?;
    let provider_kind: ProviderKind = serde_json::from_str(&kind_str)
        .unwrap_or(ProviderKind::OpenAiCompatible);
    Ok(ModelCatalogEntry {
        provider_id: row.get(0)?,
        provider_name: row.get(1)?,
        provider_kind,
        provider_enabled: row.get(3)?,
        model_db_id: row.get(4)?,
        model_id: row.get(5)?,
        model_enabled: row.get(6)?,
        display_name: row.get(7)?,
        reasoning: row.get::<_, i64>(8)? != 0,
        context_window: row.get(9)?,
        max_tokens: row.get(10)?,
        tags,
    })
})?;
```

Edit `crates/busytok-domain/src/provider_catalog.rs::ModelCatalogEntry`:

```rust
#[derive(Debug, Clone)]
pub struct ModelCatalogEntry {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_kind: ProviderKind,
    pub provider_enabled: bool,
    pub model_db_id: String,
    pub model_id: String,
    pub model_enabled: bool,
    pub tags: Vec<String>,
    pub display_name: Option<String>,
    pub reasoning: bool,
    pub context_window: Option<i64>,
    pub max_tokens: Option<i64>,
}
```

- [ ] **Step 16: Update store tests for create/update with metadata**

Edit `crates/busytok-store/tests/provider_catalog.rs` — update all `CreateModelReq` literals to include the new fields, and add a test for metadata round-trip:

```rust
#[test]
fn model_metadata_round_trip() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    let provider_id = create_test_provider(&db, "test");
    let m = busytok_store::provider_catalog::create_model(&db.conn(), busytok_store::provider_catalog::CreateModelReq {
        provider_id: provider_id.clone(),
        model_id: "claude-sonnet-4-5".into(),
        enabled: true,
        tags: vec![],
        display_name: Some("Claude Sonnet 4.5".into()),
        reasoning: Some(true),
        context_window: Some(200000),
        max_tokens: Some(8192),
    }).unwrap();
    assert_eq!(m.display_name.as_deref(), Some("Claude Sonnet 4.5"));
    assert!(m.reasoning);
    assert_eq!(m.context_window, Some(200000));
    assert_eq!(m.max_tokens, Some(8192));
    let fetched = busytok_store::provider_catalog::get_model_by_id(&db.conn(), &m.id).unwrap().unwrap();
    assert_eq!(fetched.display_name, m.display_name);
    assert_eq!(fetched.context_window, m.context_window);
}
```

(Adjust the helper `create_test_provider` if needed; if the file uses different helper names, match them. The engineer should grep for existing `CreateModelReq` literals in the test file and update each to include the new fields with sensible defaults: `display_name: None, reasoning: None, context_window: None, max_tokens: None` for tests that don't exercise metadata.)

- [ ] **Step 17: Run tests to verify**

Run: `cargo test -p busytok-store --lib provider_catalog && cargo test -p busytok-store --test provider_catalog`
Expected: PASS

- [ ] **Step 18: Run workspace check + clippy**

Run: `cargo check --workspace && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | head -50`
Expected: May still fail in downstream crates (subagent/runtime) that construct `Model` / `CreateModelReq` / `UpdateModelPatch` without the new fields. Fix each compile error by adding `display_name: None, reasoning: None (or Some(false)), context_window: None, max_tokens: None` at construction sites. Grep for `CreateModelReq {` / `UpdateModelPatch {` / `Model {` to find them.

- [ ] **Step 18b: Add `provider_kind` patch to `UpdateProviderPatch` + `update_provider` (spec §7.1)**

Spec §7.1 requires "修改 `provider_kind` → kill worker". The supervisor's `provider_update` handler (Task 4 Step 5) already calls `provider_changed` after every update (which kills the worker), so the runtime side is already wired. What's missing is the store + DTO plumbing so the patch actually flows through.

Edit `crates/busytok-store/src/provider_catalog.rs` `UpdateProviderPatch` (currently only `name` / `base_url` / `enabled` / `api_key`):

```rust
pub struct UpdateProviderPatch {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub enabled: Option<bool>,
    pub provider_kind: Option<ProviderKind>,
    // None=不改, Some(None)=清除, Some(Some(k))=更新
    pub api_key: Option<Option<String>>,
}
```

Edit `update_provider` (same file, line ~75) — add a `provider_kind` branch alongside the existing `name` / `base_url` / `enabled` branches. `ProviderKind` serializes to a JSON string (`"openai_compatible"` / `"anthropic_compatible"`), and the `providers.provider_kind` column is `TEXT` storing that JSON — so persist via `serde_json::to_string(&kind)`:

```rust
if let Some(kind) = &patch.provider_kind {
    let kind_json = serde_json::to_string(kind)
        .map_err(|e| anyhow!("failed to serialize provider_kind: {}", e))?;
    tx.execute(
        "UPDATE providers SET provider_kind = ?1, updated_at_ms = ?2 WHERE id = ?3",
        params![kind_json, now, id],
    )?;
}
```

The `ProviderKind` import is already in scope (re-exported from `busytok_domain` at the top of the file).

- [ ] **Step 18c: Write the failing test for `provider_kind` patch round-trip**

Add to `crates/busytok-store/tests/provider_catalog.rs`:

```rust
#[test]
fn update_provider_persists_provider_kind_patch() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    let provider = busytok_store::provider_catalog::create_provider(&db.conn(), busytok_store::provider_catalog::CreateProviderReq {
        name: "P1".into(),
        provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
        base_url: "https://api.test.com".into(),
        enabled: true,
        api_key: Some("sk-test".into()),
    }).unwrap();
    let updated = busytok_store::provider_catalog::update_provider(&db.conn(), &provider.id, busytok_store::provider_catalog::UpdateProviderPatch {
        name: None,
        base_url: None,
        enabled: None,
        provider_kind: Some(busytok_domain::ProviderKind::AnthropicCompatible),
        api_key: None,
    }).unwrap();
    assert_eq!(updated.provider_kind, busytok_domain::ProviderKind::AnthropicCompatible);
    // Verify round-trip via a fresh read.
    let fetched = busytok_store::provider_catalog::get_provider_with_secret(&db.conn(), &provider.id).unwrap().unwrap();
    assert_eq!(fetched.provider_kind, busytok_domain::ProviderKind::AnthropicCompatible);
}
```

Update any existing `UpdateProviderPatch { ... }` literals in the test file (grep for `UpdateProviderPatch {`) to include `provider_kind: None` so the workspace compiles.

- [ ] **Step 18d: Run tests + clippy for the new patch**

Run: `cargo test -p busytok-store --test provider_catalog update_provider_persists_provider_kind_patch && cargo clippy -p busytok-store --all-targets -- -D warnings`
Expected: PASS

- [ ] **Step 19: Commit**

```bash
git add crates/busytok-store/migrations/0007_subagent_route_binding_and_model_metadata.sql \
        crates/busytok-store/src/schema.rs \
        crates/busytok-store/src/provider_catalog.rs \
        crates/busytok-store/tests/provider_catalog.rs \
        crates/busytok-domain/src/provider_catalog.rs
git commit -m "feat(store): migration 0007 + ProviderKind::AnthropicCompatible + Model metadata + provider_kind patch"
```

---

## Task 2: LogicalSubagent bound fields + resolve_by_name validation chain (atomic migration — drop default_model)

**Files:**
- Modify: `crates/busytok-store/src/repository.rs` (`SubagentLogicalSubagentRow`)
- Modify: `crates/busytok-store/src/subagent_queries.rs` (5 functions: upsert/get/find_by_name/list_active/list_filtered)
- Modify: `crates/busytok-subagent/src/models.rs:83-98` (`LogicalSubagent` + `DelegateRequest`)
- Modify: `crates/busytok-subagent/src/resolver.rs:18-65` (`resolve_by_name` + `create_subagent` + `row_to_model`)
- Modify: `crates/busytok-subagent/src/manager.rs:167-200` (`delegate` callsite)
- Modify: `crates/busytok-subagent/src/error.rs` (add `Validation` variant + extend `code()`)
- Test: `crates/busytok-store/tests/subagent_queries.rs` (or wherever subagent query tests live)
- Test: `crates/busytok-subagent/src/resolver.rs` tests (if present)

**Interfaces:**
- Consumes: Task 1's migration (DB schema has bound columns) + Task 1's `ProviderKind`
- Produces:
  - `SubagentLogicalSubagentRow { bound_provider_id: String, bound_model_id: String }` (no `default_model`)
  - `LogicalSubagent { bound_provider_id: String, bound_model_id: String }` (no `default_model`)
  - `DelegateRequest { bound_provider_id: Option<String>, bound_model_id: Option<String> }`
  - `resolve_by_name(db, name, cwd, default_profile, bound_provider_id: &str, bound_model_id: &str) -> Result<Resolved>`
  - Creation-time validation: provider exists + enabled, model exists + enabled
  - `SubagentError::Validation(String)` + `code() => "subagent.validation_error"`

- [ ] **Step 1: Update `SubagentLogicalSubagentRow` struct**

Find `crates/busytok-store/src/repository.rs` `SubagentLogicalSubagentRow` (grep for the struct). Replace `default_model: Option<String>` with `bound_provider_id: String` and `bound_model_id: String`:

```rust
pub struct SubagentLogicalSubagentRow {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub repo_path: String,
    pub repo_hash: String,
    pub branch: Option<String>,
    pub intent: Option<String>,
    pub default_profile: String,
    pub bound_provider_id: String,
    pub bound_model_id: String,
    pub status: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_active_at_ms: Option<i64>,
}
```

- [ ] **Step 2: Update `subagent_queries.rs` SQL — `subagent_upsert_logical`**

Find the function (line ~20) and replace column list + bindings. The INSERT becomes:

```rust
pub fn subagent_upsert_logical(&self, row: &SubagentLogicalSubagentRow) -> Result<()> {
    self.conn.execute(
        "INSERT INTO subagent_logical_subagents \
         (id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, \
          bound_provider_id, bound_model_id, status, created_at_ms, updated_at_ms, last_active_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
         ON CONFLICT(id) DO UPDATE SET \
          name=excluded.name, project_id=excluded.project_id, repo_path=excluded.repo_path, \
          repo_hash=excluded.repo_hash, branch=excluded.branch, intent=excluded.intent, \
          default_profile=excluded.default_profile, bound_provider_id=excluded.bound_provider_id, \
          bound_model_id=excluded.bound_model_id, status=excluded.status, \
          updated_at_ms=excluded.updated_at_ms, last_active_at_ms=excluded.last_active_at_ms",
        params![
            row.id, row.name, row.project_id, row.repo_path, row.repo_hash,
            row.branch, row.intent, row.default_profile,
            row.bound_provider_id, row.bound_model_id, row.status,
            row.created_at_ms, row.updated_at_ms, row.last_active_at_ms,
        ],
    )?;
    Ok(())
}
```

- [ ] **Step 3: Update remaining SELECT queries in `subagent_queries.rs`**

For each of `subagent_get_logical`, `subagent_find_by_name_in_repo`, `subagent_list_active_by_repo`, `subagent_list_filtered` — update the SELECT column list to read `bound_provider_id, bound_model_id` instead of `default_model`, and update the row construction (`row.get(N)` indices) accordingly. The column at index 8 changes from `default_model` to `bound_provider_id`, and a new column 9 `bound_model_id` is added — all subsequent indices shift by 1.

Engineer: grep for `default_model` in this file and rewrite each function. The SELECT clause should be:
`id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, bound_provider_id, bound_model_id, status, created_at_ms, updated_at_ms, last_active_at_ms` (14 columns).

- [ ] **Step 4: Update `LogicalSubagent` struct**

Edit `crates/busytok-subagent/src/models.rs:83-98`:

```rust
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
    pub bound_provider_id: String,
    pub bound_model_id: String,
    pub status: SubagentStatus,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_active_at_ms: Option<i64>,
}
```

- [ ] **Step 5: Update `DelegateRequest` struct**

Edit `crates/busytok-subagent/src/models.rs` — find the `DelegateRequest` struct and add the bound fields. This is the internal subagent-crate struct (does NOT depend on Task 4's DTO):

```rust
pub struct DelegateRequest {
    // ... existing fields (subagent_name, subagent_id, cwd, profile, intent,
    //     prompt, prompt_artifact_ref, timeout_seconds, model_override,
    //     source_harness, source_session_id) ...
    /// Spec §3.3: when creating a new subagent, both must be provided
    /// together. Ignored when reusing an existing subagent.
    pub bound_provider_id: Option<String>,
    pub bound_model_id: Option<String>,
}
```

- [ ] **Step 6: Update `resolver.rs::row_to_model`**

Edit `crates/busytok-subagent/src/resolver.rs:175-197`:

```rust
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
        bound_provider_id: r.bound_provider_id.clone(),
        bound_model_id: r.bound_model_id.clone(),
        status: r.status.parse().unwrap_or_else(|s| {
            tracing::warn!(
                event_code = "subagent.session.parse_status_failed",
                raw_status = %s,
                "failed to parse subagent status, falling back to Cold"
            );
            crate::models::SubagentStatus::Cold
        }),
        created_at_ms: r.created_at_ms,
        updated_at_ms: r.updated_at_ms,
        last_active_at_ms: r.last_active_at_ms,
    }
}
```

- [ ] **Step 7: Update `resolve_by_name` signature + body**

Edit `crates/busytok-subagent/src/resolver.rs:18-65`:

```rust
pub fn resolve_by_name(
    db: &busytok_store::Database,
    name: &str,
    cwd: &str,
    default_profile: &str,
    bound_provider_id: &str,
    bound_model_id: &str,
) -> Result<Resolved> {
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
    let active: Vec<_> = matches
        .into_iter()
        .filter(|r| r.status != "deleted")
        .collect();
    match active.len() {
        0 => {
            // Creation path: validate provider + model before insert.
            validate_bound_provider_model(db, bound_provider_id, bound_model_id)?;
            Ok(Resolved {
                subagent: create_subagent(
                    db,
                    name,
                    &canonical_cwd,
                    &repo_hash,
                    default_profile,
                    bound_provider_id,
                    bound_model_id,
                )?,
                created: true,
            })
        }
        1 => Ok(Resolved {
            subagent: row_to_model(&active[0]),
            created: false,
        }),
        _ => Err(SubagentError::AmbiguousName(name.to_string())),
    }
}

fn validate_bound_provider_model(
    db: &busytok_store::Database,
    provider_id: &str,
    model_id: &str,
) -> Result<()> {
    let provider = db
        .get_provider_with_secret(provider_id)
        .map_err(SubagentError::Store)?
        .ok_or_else(|| SubagentError::Validation(format!("provider not found: {}", provider_id)))?;
    if !provider.enabled {
        return Err(SubagentError::Validation(format!("provider disabled: {}", provider_id)));
    }
    let model = db
        .get_model_by_provider_and_model_id(provider_id, model_id)
        .map_err(SubagentError::Store)?
        .ok_or_else(|| SubagentError::Validation(format!("model not found in provider: {}", model_id)))?;
    if !model.enabled {
        return Err(SubagentError::Validation(format!("model disabled: {}", model_id)));
    }
    Ok(())
}
```

- [ ] **Step 8: Update `create_subagent` signature + body**

Edit `crates/busytok-subagent/src/resolver.rs:140-173`:

```rust
fn create_subagent(
    db: &busytok_store::Database,
    name: &str,
    cwd: &str,
    repo_hash: &str,
    default_profile: &str,
    bound_provider_id: &str,
    bound_model_id: &str,
) -> Result<LogicalSubagent> {
    let now = busytok_domain::now_ms();
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
        bound_provider_id: bound_provider_id.to_string(),
        bound_model_id: bound_model_id.to_string(),
        status: "cold".to_string(),
        created_at_ms: now,
        updated_at_ms: now,
        last_active_at_ms: None,
    };
    db.subagent_upsert_logical(&row).map_err(SubagentError::Store)?;
    db.subagent_upsert_memory(&SubagentMemoryRow::new_empty(&id)).map_err(SubagentError::Store)?;
    Ok(row_to_model(&row))
}
```

- [ ] **Step 9: Add `SubagentError::Validation` variant + extend `code()` method**

Edit `crates/busytok-subagent/src/error.rs` — add the `Validation` variant:

```rust
#[derive(Debug, thiserror::Error)]
pub enum SubagentError {
    // ... existing variants ...
    #[error("{0}")]
    Validation(String),
}
```

**Important:** also extend the `code()` method (error.rs:68-83) to add a `Validation` branch — otherwise the `match` becomes non-exhaustive and won't compile. Add:

```rust
SubagentError::Validation(_) => "subagent.validation_error",
```

Grep `crates/busytok-subagent/src/manager.rs` for `e.code()` callsites in `delegate()` (e.g. line 190 `reason = e.code()`) — the new variant flows through automatically once `code()` returns the string.

- [ ] **Step 10: Update `delegate` callsite in manager**

Edit `crates/busytok-subagent/src/manager.rs:167-200`. The `delegate()` method now extracts `bound_provider_id` + `bound_model_id` from the request, applies the "both or neither" rule, and calls `resolve_by_name` with the new signature. When reusing (name path hit OR `subagent_id` path), the bound fields are ignored.

```rust
pub async fn delegate(&self, req: DelegateRequest) -> Result<DelegateResult> {
    // Spec §3.3: bound fields are conditional required (create path only).
    let bound_pair = match (&req.bound_provider_id, &req.bound_model_id) {
        (Some(p), Some(m)) => Some((p.clone(), m.clone())),
        (None, None) => None,
        _ => return Err(SubagentError::Validation(
            "bound_provider_id and bound_model_id must be provided together".into(),
        ).into()),
    };
    // INTERIM: Task 3 will delete `profile_model()` and this line; until then
    // we keep it so the post-resolution delegate body (which still references
    // `profile_model` in `model:` fields of `DelegateResult` and `ExecutorInput`)
    // continues to compile. Task 3 Step 11 rewrites those `model:` fields to
    // `req.model_override.clone()` (no `profile_model` fallback) and deletes
    // this line + the `profile_model()` method.
    let profile_model = self.profile_model(&req.profile);
    let Resolved { subagent, created } = {
        let db = self.db.lock().expect("subagent db lock poisoned");
        if let Some(id) = &req.subagent_id {
            // Reuse path: ignore bound fields, resolve by id.
            Resolved {
                subagent: resolve_by_id(&db, id)?,
                created: false,
            }
        } else {
            // name path: pass bound fields; resolver validates only on create.
            let (p, m) = bound_pair.unwrap_or((String::new(), String::new()));
            match resolve_by_name(
                &db,
                &req.subagent_name,
                &req.cwd,
                &req.profile,
                &p,
                &m,
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
    // ── Everything below is the EXISTING delegate() body, unchanged ──
    // (preserve verbatim from the pre-refactor delegate() implementation):
    //   1. The "Unknown profile" early-reject guard (delegate() lines ~133-142)
    //      — KEEP as-is. (Note: `profile_known` check stays; profile is still
    //      a real config concept, just no longer carries provider/model.)
    //   2. The "prompt vs prompt_artifact_ref" mutual-exclusion guard
    //      (delegate() lines ~143-166) — KEEP as-is.
    //   3. The pressure-gate + has_running_task queue-vs-run decision +
    //      `subagent_insert_task` (lines ~199-252) — KEEP as-is.
    //   4. The early-return `DelegateResult { status: Queued, ... }` when
    //      `should_queue` is true (lines ~266-288) — KEEP as-is. The `model:`
    //      field uses `req.model_override.clone().or(profile_model)` for now;
    //      Task 3 Step 11 will drop the `profile_model` fallback (rewrite to
    //      `req.model_override.clone()`).
    //   5. The `SubagentTaskRow` construction (lines ~290-320) — KEEP as-is.
    //   6. The `execute_task(task_row, subagent)` call (line ~321) — KEEP
    //      as-is. Task 5 rewrites `execute_task` internals to use
    //      `subagent.bound_provider_id` + `effective_model_id`.
    //   7. The post-execute `DelegateResult` construction + return — KEEP
    //      as-is.
    //
    // The ONLY change in this step is replacing the resolution block (old
    // lines 167-197) with the new `bound_pair` extraction + new-signature
    // `resolve_by_name` call above. Everything else in delegate() is
    // preserved verbatim. The `profile_model` line is kept as INTERIM so
    // the post-resolution body compiles; Task 3 removes it.
}
```

NOTE: The engineer should NOT rewrite the post-resolution body — only the resolution block (lines 167-197 in the pre-refactor file) is replaced (plus the new `bound_pair` extraction + interim `profile_model` line). Task 3 will delete `profile_model()` and rewrite the `model:` fields in `DelegateResult`/`ExecutorInput` to use `req.model_override.clone()` (no `profile_model` fallback). Until then, the post-resolution body stays as-is.

- [ ] **Step 11: Write the failing test for bound fields round-trip**

Add to `crates/busytok-store/tests/subagent_queries.rs` (or the existing subagent_queries test file):

```rust
#[test]
fn subagent_upsert_logical_persists_bound_fields() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    let row = busytok_store::SubagentLogicalSubagentRow {
        id: "sub-1".into(),
        name: "test-sub".into(),
        project_id: "repo-hash".into(),
        repo_path: "/tmp".into(),
        repo_hash: "repo-hash".into(),
        branch: None,
        intent: None,
        default_profile: "pi/search-cheap".into(),
        bound_provider_id: "prov-1".into(),
        bound_model_id: "gpt-4o".into(),
        status: "cold".into(),
        created_at_ms: 1000,
        updated_at_ms: 1000,
        last_active_at_ms: None,
    };
    db.subagent_upsert_logical(&row).unwrap();
    let fetched = db.subagent_get_logical("sub-1").unwrap().unwrap();
    assert_eq!(fetched.bound_provider_id, "prov-1");
    assert_eq!(fetched.bound_model_id, "gpt-4o");
    // Verify default_model column is gone (no API to read it — just verify no panic).
}
```

- [ ] **Step 12: Write the failing tests for creation-time validation**

Add to `crates/busytok-subagent/src/resolver.rs` tests (or `tests/` integration file if no inline tests exist):

```rust
#[test]
fn resolve_by_name_creates_subagent_with_valid_bound_provider_and_model() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    let provider_id = busytok_store::provider_catalog::create_provider(&db.conn(), busytok_store::provider_catalog::CreateProviderReq {
        name: "P1".into(),
        provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
        base_url: "https://api.test.com".into(),
        enabled: true,
        api_key: Some("sk-test".into()),
    }).unwrap().id;
    let model = busytok_store::provider_catalog::create_model(&db.conn(), busytok_store::provider_catalog::CreateModelReq {
        provider_id: provider_id.clone(),
        model_id: "gpt-4o".into(),
        enabled: true,
        tags: vec![],
        display_name: None,
        reasoning: None,
        context_window: Some(128000),
        max_tokens: Some(16384),
    }).unwrap();
    let resolved = resolve_by_name(&db, "test-sub", "/tmp", "pi/search-cheap", &provider_id, &model.model_id).unwrap();
    assert!(resolved.created);
    assert_eq!(resolved.subagent.bound_provider_id, provider_id);
    assert_eq!(resolved.subagent.bound_model_id, "gpt-4o");
}

#[test]
fn resolve_by_name_rejects_disabled_provider() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    let provider_id = busytok_store::provider_catalog::create_provider(&db.conn(), busytok_store::provider_catalog::CreateProviderReq {
        name: "P1".into(),
        provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
        base_url: "https://api.test.com".into(),
        enabled: false,
        api_key: None,
    }).unwrap().id;
    let result = resolve_by_name(&db, "test-sub", "/tmp", "pi/search-cheap", &provider_id, "gpt-4o");
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("provider disabled"), "got: {}", msg);
}

#[test]
fn resolve_by_name_rejects_missing_model_in_provider() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    let provider_id = busytok_store::provider_catalog::create_provider(&db.conn(), busytok_store::provider_catalog::CreateProviderReq {
        name: "P1".into(),
        provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
        base_url: "https://api.test.com".into(),
        enabled: true,
        api_key: Some("sk-test".into()),
    }).unwrap().id;
    let result = resolve_by_name(&db, "test-sub", "/tmp", "pi/search-cheap", &provider_id, "gpt-4o");
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("model not found in provider"), "got: {}", msg);
}
```

- [ ] **Step 13: Fix downstream construction sites**

Run: `cargo check --workspace 2>&1 | grep -E "error|no field|missing field" | head -50`

Fix each `SubagentLogicalSubagentRow { ... }` literal that's missing `bound_provider_id` / `bound_model_id` or has a stray `default_model` field. Common sites: tests in `crates/busytok-store/tests/subagent_queries.rs`, `crates/busytok-subagent/src/manager.rs` (anywhere `LogicalSubagent { ... }` is constructed in tests). Grep `SubagentLogicalSubagentRow {` and `LogicalSubagent {` across the workspace.

Also grep `resolve_by_name(` callsites across the workspace and update each to pass two more args (`bound_provider_id: &str, bound_model_id: &str`). For tests that exercise the reuse path, pass empty strings (the resolver hits the existing row and ignores the bound fields). For manager.rs test mocks that call `resolve_by_name(`, pass `"test-prov"` / `"test-model"` or empty strings as appropriate.

- [ ] **Step 14: Run tests**

Run: `cargo test -p busytok-store --test subagent_queries subagent_upsert_logical_persists_bound_fields && cargo test -p busytok-subagent --lib resolver && cargo test -p busytok-subagent --lib manager`
Expected: PASS

- [ ] **Step 15: Workspace check + clippy**

Run: `cargo check --workspace && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -30`
Expected: clean (compile errors fixed in Step 13). The runtime may still reference `default_model` via `profile_model()` — that's removed in Task 3. For now `LogicalSubagent.default_model` is gone; `manager.rs` calls to `self.profile_model()` return `Option<String>` and don't touch the struct field, so should compile.

- [ ] **Step 16: Commit**

```bash
git add crates/busytok-store/src/repository.rs \
        crates/busytok-store/src/subagent_queries.rs \
        crates/busytok-store/tests/subagent_queries.rs \
        crates/busytok-subagent/src/models.rs \
        crates/busytok-subagent/src/resolver.rs \
        crates/busytok-subagent/src/manager.rs \
        crates/busytok-subagent/src/error.rs
git commit -m "refactor(subagent): atomic bound fields migration — row/struct/SQL + resolve_by_name validation + delegate wiring"
```

---

## Task 3: Profile downgrade — remove provider_id / model / SubagentModelsConfig

**Files:**
- Modify: `crates/busytok-config/src/lib.rs:342-362` (delete `SubagentModelsConfig` + 4 default fns)
- Modify: `crates/busytok-config/src/lib.rs:376-442` (`SubagentProfileConfig` drop `model` + `provider_id`; `default_profiles()` updated)
- Modify: `crates/busytok-protocol/src/dto.rs:1715-1765` (`ProfileDto` / `ProfileCreateRequestDto` / `ProfileUpdateRequestDto`)
- Modify: `crates/busytok-runtime/src/supervisor.rs:1870-1887` (delete `collect_profile_refs`)
- Modify: `crates/busytok-runtime/src/supervisor.rs:6275` (`profile_create` handler)
- Modify: `crates/busytok-runtime/src/supervisor.rs:6369` (`profile_update` handler)
- Modify: `crates/busytok-runtime/src/supervisor.rs:5409-5470` (`subagent_delegate` profile provider/model validation)
- Modify: `crates/busytok-subagent/src/manager.rs:1053-1061` (delete `profile_model`)
- Modify: `crates/busytok-subagent/src/manager.rs:167-360` (`delegate` + `execute_task` — drop `profile_model` lookups)
- Modify: `crates/busytok-store/src/provider_catalog.rs:109-120` (`delete_provider` drop `profile_refs` param)
- Modify: `crates/busytok-store/src/provider_catalog.rs:192-200` (`delete_model` drop `profile_refs` param)
- Modify: `crates/busytok-store/src/provider_catalog.rs` (delete `provider_has_profile_references` + `model_has_profile_references`)
- Modify: `crates/busytok-domain/src/provider_catalog.rs:99-104` (delete `ProfileModelRef`)
- Test: `crates/busytok-runtime/src/supervisor.rs` inline tests, `crates/busytok-config` tests

**Interfaces:**
- Consumes: Task 1 + Task 2
- Produces:
  - `SubagentProfileConfig { write_access, tools, context_budget_tokens, timeout_seconds }` (no `provider_id` / `model`)
  - `ProfileDto` / `ProfileCreateRequestDto` / `ProfileUpdateRequestDto` without `provider_id` / `model`
  - `delete_provider(conn, id) -> Result<()>` (no `profile_refs`)
  - `delete_model(conn, id) -> Result<()>` (no `profile_refs`)
  - `ProfileModelRef` deleted
  - `collect_profile_refs()` deleted
  - `profile_model()` deleted
  - `SubagentModelsConfig` deleted

- [ ] **Step 1: Delete `SubagentModelsConfig` + helpers**

Edit `crates/busytok-config/src/lib.rs` — delete lines 342-374 (struct + Default impl + 4 `default_*_model` fns). Then grep for `SubagentModelsConfig` references across the workspace and remove them (likely in `BusytokSettings` struct field + any `default()` impls).

- [ ] **Step 2: Strip `SubagentProfileConfig` of `model` + `provider_id`**

Edit `crates/busytok-config/src/lib.rs:376-395`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentProfileConfig {
    #[serde(default = "default_false")]
    pub write_access: bool,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default = "default_budget_tokens")]
    pub context_budget_tokens: u32,
    #[serde(default = "default_task_timeout_seconds")]
    pub timeout_seconds: u64,
}
```

Update `default_profiles()` (lines 398-442) to drop `model:` and `provider_id:` fields from each insert.

- [ ] **Step 3: Write the failing test for stripped profile config**

Add to `crates/busytok-config/src/lib.rs` tests (or wherever config tests live):

```rust
#[test]
fn profile_config_has_no_provider_or_model_fields() {
    let toml = r#"
[subagent.profiles."pi/search-cheap"]
write_access = false
tools = ["read"]
context_budget_tokens = 3000
timeout_seconds = 120
"#;
    let settings: BusytokSettings = toml::from_str(toml).unwrap();
    let p = settings.subagent.profiles.get("pi/search-cheap").unwrap();
    // Compile-time check: if these fields still exist, this won't compile.
    let _write = p.write_access;
    let _tools = &p.tools;
    let _budget = p.context_budget_tokens;
    let _timeout = p.timeout_seconds;
}
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo test -p busytok-config profile_config_has_no_provider_or_model_fields`
Expected: FAIL (compile error — fields still exist) until Step 2 is saved; after Step 2, this should pass.

- [ ] **Step 5: Strip DTOs**

Edit `crates/busytok-protocol/src/dto.rs:1713-1765` — remove `provider_id` and `model` fields from `ProfileDto`, `ProfileCreateRequestDto`, `ProfileUpdateRequestDto`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProfileDto {
    pub id: String,
    pub is_builtin: bool,
    pub tools: Vec<String>,
    pub context_budget_tokens: u32,
    pub timeout_seconds: u64,
    pub write_access: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProfileCreateRequestDto {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_budget_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_access: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProfileUpdateRequestDto {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_budget_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_access: Option<bool>,
}
```

- [ ] **Step 6: Update DTO test literals**

Grep `crates/busytok-protocol/src/dto.rs` for `ProfileDto {` / `ProfileCreateRequestDto {` / `ProfileUpdateRequestDto {` in the tests module and remove `provider_id` / `model` fields from each literal.

- [ ] **Step 7: Delete `ProfileModelRef` + reference-check helpers in store**

Edit `crates/busytok-domain/src/provider_catalog.rs:99-104` — delete the `ProfileModelRef` struct.

Edit `crates/busytok-store/src/provider_catalog.rs`:
- Remove `ProfileModelRef` from the `pub use busytok_domain::{...}` re-export
- Delete `provider_has_profile_references` and `model_has_profile_references` helper fns
- Update `delete_provider` signature: `pub fn delete_provider(conn: &Connection, id: &str) -> Result<()>` (drop `profile_refs` param + the early bail check)
- Update `delete_model` signature: `pub fn delete_model(conn: &Connection, id: &str) -> Result<()>` (drop `profile_refs` param + the early bail check)

- [ ] **Step 8: Delete `collect_profile_refs` in supervisor**

Edit `crates/busytok-runtime/src/supervisor.rs:1870-1887` — delete the `collect_profile_refs` method entirely.

- [ ] **Step 9: Update `provider_delete` + `model_delete` handlers**

Edit `crates/busytok-runtime/src/supervisor.rs:5862` (`provider_delete`):

```rust
async fn provider_delete(&self, req: ProviderDeleteRequestDto) -> Result<()> {
    {
        let db = self.db.lock().unwrap();
        db.delete_provider(&req.id).map_err(|e| {
            tracing::error!(event_code = "provider.sql_write_failed", provider_id = %req.id, error = %e, "delete_provider failed");
            e
        })?;
    }
    tracing::info!(event_code = "provider.deleted", provider_id = %req.id, "provider deleted");
    self.provider_deleted(&req.id).await;
    Ok(())
}
```

Edit `crates/busytok-runtime/src/supervisor.rs:6143` (`model_delete`) similarly — drop the `collect_profile_refs` call and the `profile_refs` argument to `db.delete_model`.

- [ ] **Step 10: Delete `profile_model()` from manager**

Edit `crates/busytok-subagent/src/manager.rs:1053-1061` — delete the `profile_model` method.

- [ ] **Step 11: Strip `profile_model` usages from `delegate` + `execute_task`**

Edit `crates/busytok-subagent/src/manager.rs`:
- Line ~167: `let profile_model = self.profile_model(&req.profile);` — delete this line (was kept as INTERIM in Task 2 Step 10; now safe to remove)
- Line ~184: the `resolve_by_name` call already uses the new signature from Task 2 — no change needed here
- Line ~284: `model: req.model_override.clone().or(profile_model)` → `model: req.model_override.clone()` (interim; Task 5 flips this to read `subagent.bound_model_id`)
- Line ~294: same pattern — drop `profile_model` from the `model:` field
- Line ~360: `.or_else(|| self.profile_model(&task.profile))` — delete this fallback (Task 5 replaces with `subagent.bound_model_id`)

Engineer: grep for `profile_model` across the workspace and remove all references. The `delegate()` body should use `req.model_override.clone()` for the `model` field of `DelegateResult` and `ExecutorInput` (Task 2 Step 10 kept `profile_model` as INTERIM; this step removes it).

- [ ] **Step 12: Strip `subagent_delegate` profile provider/model validation**

Edit `crates/busytok-runtime/src/supervisor.rs:5409-5470` — delete the entire `if self.worker_pool().is_some() { ... }` block that validates `profile_cfg.provider_id` + `profile_cfg.model`. The new validation chain (Task 6) will use `bound_provider_id` + `effective_model_id`. For now, the handler just forwards to `subagent_manager().delegate()`.

```rust
async fn subagent_delegate(
    &self,
    req: busytok_protocol::dto::SubagentDelegateRequestDto,
) -> Result<SubagentDelegateResponseDto> {
    let cwd = req.cwd.clone();
    let r = self
        .subagent_manager()
        .delegate(delegate_request_from_dto(req))
        .await
        .map_err(map_subagent_error)?;
    if r.status != busytok_subagent::models::TaskStatus::Queued {
        if let Err(e) = self.write_subagent_usage_event(&r, &cwd) {
            tracing::warn!(
                event_code = "subagent.usage_write_failed",
                task_id = %r.task_id,
                error = %e,
                "failed to write subagent usage event to unified usage_events"
            );
        }
    }
    Ok(SubagentDelegateResponseDto {
        task_id: r.task_id,
        subagent_id: r.subagent_id,
        subagent_name: r.subagent_name,
        adapter: r.adapter,
        adapter_session_id: r.adapter_session_id,
        session_reused: r.session_reused,
        status: r.status.as_str().to_string(),
        profile: r.profile,
        model: r.model,
        summary: r.summary,
        usage: SubagentUsageDto {
            model: r.usage.model,
            provider: r.usage.provider,
            input_tokens: r.usage.input_tokens,
            output_tokens: r.usage.output_tokens,
            cache_read_tokens: r.usage.cache_read_tokens,
            cache_write_tokens: r.usage.cache_write_tokens,
            cost_usd: r.usage.cost_usd,
        },
    })
}
```

- [ ] **Step 13: Update `profile_create` + `profile_update` handlers**

Edit `crates/busytok-runtime/src/supervisor.rs:6275` (`profile_create`) — drop `model` / `provider_id` from the constructed `SubagentProfileConfig`. Drop the whitelist validation (provider existence / model existence checks).

Edit `crates/busytok-runtime/src/supervisor.rs:6369` (`profile_update`) — drop `model` / `provider_id` patch handling.

- [ ] **Step 14: Update store tests for `delete_provider` / `delete_model`**

Grep `crates/busytok-store/tests/provider_catalog.rs` for `delete_provider(` and `delete_model(` — remove the `&[]` or `&profile_refs` argument. Add a test that delete succeeds even when a subagent references the provider (dangling binding is allowed):

```rust
#[test]
fn delete_provider_succeeds_even_when_subagent_bound() {
    let db = busytok_store::Database::open_in_memory().unwrap();
    let provider_id = create_test_provider(&db, "test");
    // Insert a subagent that binds to this provider (dangling allowed).
    db.subagent_upsert_logical(&busytok_store::SubagentLogicalSubagentRow {
        id: "sub-1".into(),
        name: "bound".into(),
        project_id: "h".into(),
        repo_path: "/tmp".into(),
        repo_hash: "h".into(),
        branch: None,
        intent: None,
        default_profile: "pi/search-cheap".into(),
        bound_provider_id: provider_id.clone(),
        bound_model_id: "gpt-4o".into(),
        status: "cold".into(),
        created_at_ms: 1000,
        updated_at_ms: 1000,
        last_active_at_ms: None,
    }).unwrap();
    // Delete should succeed (dangling binding allowed per spec §7.5).
    db.delete_provider(&provider_id).unwrap();
}
```

- [ ] **Step 15: Run tests**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: PASS (or compile errors to fix at construction sites — grep for `provider_id:` / `model:` near `SubagentProfileConfig {` / `ProfileDto {` literals)

- [ ] **Step 16: Workspace check + clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -30`
Expected: clean

- [ ] **Step 17: Commit**

```bash
git add crates/busytok-config/src/lib.rs \
        crates/busytok-protocol/src/dto.rs \
        crates/busytok-domain/src/provider_catalog.rs \
        crates/busytok-store/src/provider_catalog.rs \
        crates/busytok-store/tests/provider_catalog.rs \
        crates/busytok-runtime/src/supervisor.rs \
        crates/busytok-subagent/src/manager.rs
git commit -m "refactor: profile downgrade to behavior template (drop provider_id/model + SubagentModelsConfig + collect_profile_refs)"
```

---

## Task 4: Protocol DTOs — SubagentDelegateRequestDto + SubagentDetailDto + Model DTOs

**Files:**
- Modify: `crates/busytok-protocol/src/dto.rs:1289-1305` (`SubagentDelegateRequestDto`)
- Modify: `crates/busytok-protocol/src/dto.rs:1369-1387` (`SubagentDetailDto`)
- Modify: `crates/busytok-protocol/src/dto.rs:1577-1589` (`ProviderUpdateRequestDto` + `provider_kind` patch, spec §7.1)
- Modify: `crates/busytok-protocol/src/dto.rs:1593-1623` (`ModelCatalogEntryDto` / `ModelCreateRequestDto` / `ModelUpdateRequestDto`)
- Modify: `crates/busytok-runtime/src/supervisor.rs:2896` (`delegate_request_from_dto` helper)
- Modify: `crates/busytok-runtime/src/supervisor.rs:2924` (`subagent_detail` helper)
- Modify: `crates/busytok-runtime/src/supervisor.rs:5834` (`provider_update` handler — plumb `provider_kind` from DTO to `UpdateProviderPatch`)
- Modify: `crates/busytok-runtime/src/supervisor.rs:7285` / `:7329` (inline tests for the two helpers)
- Test: `crates/busytok-protocol/src/dto.rs` inline tests
- Regenerate: `packages/busytok-protocol-types/src/generated.ts`

**Interfaces:**
- Consumes: Tasks 1-3
- Produces:
  - `SubagentDelegateRequestDto { bound_provider_id: Option<String>, bound_model_id: Option<String> }`
  - `SubagentDetailDto { bound_provider_id: String, bound_model_id: String }` (no `default_model`)
  - `ModelCreateRequestDto { context_window: i64, max_tokens: i64, display_name: Option<String>, reasoning: Option<bool> }`
  - `ModelUpdateRequestDto { display_name: Option<String>, reasoning: Option<bool>, context_window: Option<i64>, max_tokens: Option<i64> }` (existing `enabled` stays)
  - `ModelCatalogEntryDto { display_name: Option<String>, reasoning: bool, context_window: Option<i64>, max_tokens: Option<i64> }`
  - `ProviderUpdateRequestDto { provider_kind: Option<ProviderKind> }` (existing fields stay; spec §7.1)

- [ ] **Step 1: Update `SubagentDelegateRequestDto`**

Edit `crates/busytok-protocol/src/dto.rs:1289`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct SubagentDelegateRequestDto {
    pub subagent_name: String,
    pub subagent_id: Option<String>,
    pub cwd: String,
    pub profile: String,
    pub intent: Option<String>,
    pub prompt: String,
    pub prompt_artifact_ref: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub model_override: Option<String>,
    pub source_harness: Option<String>,
    pub source_session_id: Option<String>,
    /// Spec §3.3: when creating a new subagent, both must be provided
    /// together. Ignored when reusing an existing subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_model_id: Option<String>,
}
```

- [ ] **Step 2: Update `SubagentDetailDto`**

Edit `crates/busytok-protocol/src/dto.rs:1369`:

```rust
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
    pub bound_provider_id: String,
    pub bound_model_id: String,
    pub status: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_active_at_ms: Option<i64>,
}
```

- [ ] **Step 3: Update `ModelCreateRequestDto` + `ModelUpdateRequestDto` + `ModelCatalogEntryDto`**

Edit `crates/busytok-protocol/src/dto.rs:1593-1623`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelCatalogEntryDto {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_kind: ProviderKind,
    pub provider_enabled: bool,
    pub model_db_id: String,
    pub model_id: String,
    pub model_enabled: bool,
    pub tags: Vec<String>,
    pub display_name: Option<String>,
    pub reasoning: bool,
    pub context_window: Option<i64>,
    pub max_tokens: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelCreateRequestDto {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub context_window: i64,
    pub max_tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelUpdateRequestDto {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i64>,
}
```

- [ ] **Step 3b: Add `provider_kind` patch to `ProviderUpdateRequestDto` (spec §7.1)**

Edit `crates/busytok-protocol/src/dto.rs:1577-1589` (`ProviderUpdateRequestDto`). Add a `provider_kind: Option<ProviderKind>` field alongside the existing `name` / `base_url` / `enabled` / `api_key` patch fields:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderUpdateRequestDto {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_kind: Option<ProviderKind>,
    // None=不改, Some(None)=清除, Some(Some(k))=更新
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_some")]
    #[ts(type = "string | null | undefined")]
    pub api_key: Option<Option<String>>,
}
```

`ProviderKind` is already imported in dto.rs (used by `ProviderDto` / `ProviderCreateRequestDto`). Add a TDD test alongside the existing `ProviderUpdateRequestDto` tests:

```rust
#[test]
fn provider_update_request_dto_deserializes_provider_kind_patch() {
    let json = serde_json::json!({
        "id": "p1",
        "provider_kind": "anthropic_compatible",
    });
    let dto: ProviderUpdateRequestDto = serde_json::from_value(json).unwrap();
    assert_eq!(dto.provider_kind, Some(ProviderKind::AnthropicCompatible));
}
```

- [ ] **Step 4: Update DTO conversion helpers (located in supervisor.rs, not dto.rs)**

**File location note:** `delegate_request_from_dto` and `subagent_detail` are NOT in `dto.rs` — they live in `crates/busytok-runtime/src/supervisor.rs:2896` and `:2924` respectively (verified via grep). The existing inline tests `delegate_request_from_dto_forwards_all_fields` (supervisor.rs:7285) and `subagent_detail_maps_status_to_string_and_forwards_fields` (supervisor.rs:7329) are also there and will need updating.

Edit `crates/busytok-runtime/src/supervisor.rs:2896` (`delegate_request_from_dto`) to map the new fields:

```rust
pub fn delegate_request_from_dto(req: SubagentDelegateRequestDto) -> busytok_subagent::models::DelegateRequest {
    busytok_subagent::models::DelegateRequest {
        subagent_name: req.subagent_name,
        subagent_id: req.subagent_id,
        cwd: req.cwd,
        profile: req.profile,
        intent: req.intent,
        prompt: req.prompt,
        prompt_artifact_ref: req.prompt_artifact_ref,
        timeout_seconds: req.timeout_seconds,
        model_override: req.model_override,
        source_harness: req.source_harness,
        source_session_id: req.source_session_id,
        bound_provider_id: req.bound_provider_id,
        bound_model_id: req.bound_model_id,
    }
}
```

(If `DelegateRequest` in `crates/busytok-subagent/src/models.rs` doesn't yet have `bound_provider_id` / `bound_model_id` fields, add them now — `pub bound_provider_id: Option<String>, pub bound_model_id: Option<String>`.)

Update `subagent_detail`:

```rust
pub fn subagent_detail(s: busytok_subagent::models::LogicalSubagent) -> SubagentDetailDto {
    SubagentDetailDto {
        id: s.id,
        name: s.name,
        project_id: s.project_id,
        repo_path: s.repo_path,
        repo_hash: s.repo_hash,
        branch: s.branch,
        intent: s.intent,
        default_profile: s.default_profile,
        bound_provider_id: s.bound_provider_id,
        bound_model_id: s.bound_model_id,
        status: s.status.as_str().to_string(),
        created_at_ms: s.created_at_ms,
        updated_at_ms: s.updated_at_ms,
        last_active_at_ms: s.last_active_at_ms,
    }
}
```

- [ ] **Step 5: Update `model_create` + `model_update` + `provider_update` handler conversions**

Grep `crates/busytok-runtime/src/supervisor.rs` for `model_create` and `model_update` handlers. Update the `CreateModelReq` / `UpdateModelPatch` construction to pass the new metadata fields from the DTO. For `model_create`, validate that `context_window` and `max_tokens` are present (they are required by the DTO now, so the type system enforces this).

Also update `provider_update` (supervisor.rs:5834) to plumb the new `provider_kind` patch from `ProviderUpdateRequestDto` into `UpdateProviderPatch`. The existing handler already calls `self.provider_changed(&provider.id).await` (supervisor.rs:5858) after the update — this kills the worker per spec §7.1, so no new kill logic is needed; just plumb the field:

```rust
db.update_provider(&req.id, busytok_store::UpdateProviderPatch {
    name: req.name,
    base_url: req.base_url,
    enabled: req.enabled,
    provider_kind: req.provider_kind,  // NEW (spec §7.1)
    api_key: req.api_key,
})
```

After this plumbing, the existing `provider_changed` call (line 5858) ensures any `provider_kind` change kills the worker so the next delegate re-spawns it with the new API shape (`openai_completions` vs `anthropic_messages`).

- [ ] **Step 6: Update `ModelCatalogEntryDto` construction**

Grep `crates/busytok-runtime/src/supervisor.rs` for `ModelCatalogEntryDto {` literals and add the new metadata fields (mapping from `ModelCatalogEntry`).

- [ ] **Step 7: Write the failing test for `SubagentDelegateRequestDto` bound fields**

Add to `crates/busytok-protocol/src/dto.rs` tests module:

```rust
#[test]
fn delegate_request_dto_deserializes_bound_fields() {
    let json = serde_json::json!({
        "subagent_name": "test",
        "subagent_id": null,
        "cwd": "/tmp",
        "profile": "pi/search-cheap",
        "intent": null,
        "prompt": "hi",
        "prompt_artifact_ref": null,
        "timeout_seconds": null,
        "model_override": null,
        "source_harness": null,
        "source_session_id": null,
        "bound_provider_id": "prov-1",
        "bound_model_id": "gpt-4o",
    });
    let dto: SubagentDelegateRequestDto = serde_json::from_value(json).unwrap();
    assert_eq!(dto.bound_provider_id.as_deref(), Some("prov-1"));
    assert_eq!(dto.bound_model_id.as_deref(), Some("gpt-4o"));
}

#[test]
fn model_create_request_dto_requires_metadata() {
    // Missing context_window + max_tokens → should fail to deserialize.
    let json = serde_json::json!({
        "provider_id": "p1",
        "model_id": "gpt-4o",
    });
    let result: Result<ModelCreateRequestDto, _> = serde_json::from_value(json);
    assert!(result.is_err());
}
```

- [ ] **Step 8: Run tests**

Run: `cargo test -p busytok-protocol`
Expected: PASS

- [ ] **Step 9: Workspace check + clippy**

Run: `cargo check --workspace && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -30`
Expected: clean

- [ ] **Step 10: Regenerate TS types**

Run: `cargo test -p busytok-protocol --features export-ts && pnpm -F busytok-protocol-types build`
Expected: `packages/busytok-protocol-types/src/generated.ts` updated with new fields.

- [ ] **Step 11: Commit**

```bash
git add crates/busytok-protocol/src/dto.rs \
        crates/busytok-subagent/src/models.rs \
        crates/busytok-runtime/src/supervisor.rs \
        packages/busytok-protocol-types/src/generated.ts
git commit -m "feat(protocol): DTOs for bound fields + model metadata + profile downgrade + provider_kind patch (spec §7.1)"
```

---

## Task 5: `ExecutorInput` + `execute_task` validation chain + sidecar executor turn_auto params + delete `inject_provider_env`

**Files:**
- Modify: `crates/busytok-subagent/src/mock_executor.rs:11-31` (`ExecutorInput`)
- Modify: `crates/busytok-subagent/src/manager.rs:351-400` (`execute_task` validation chain)
- Modify: `crates/busytok-subagent/src/sidecar/executor.rs:65-150` (turn_auto params + drop "profile not bound" branch)
- Modify: `crates/busytok-subagent/src/sidecar/pool.rs:67-72` (delete `inject_provider_env`)
- Modify: `crates/busytok-subagent/src/sidecar/pool.rs:220-230` (remove `inject_provider_env` callsite)
- Modify: `crates/busytok-subagent/src/sidecar/pool.rs` tests (remove `inject_provider_env` test if present)
- Test: `crates/busytok-subagent/src/manager.rs` tests, `crates/busytok-subagent/src/sidecar/pool.rs` tests

**Interfaces:**
- Consumes: Tasks 1-5
- Produces:
  - `ExecutorInput { provider_id: String, provider_kind: ProviderKind, provider_base_url: String, provider_api_key: String, model: String }` (no `Option`)
  - `execute_task` validates bound provider exists + enabled + has api_key + bound model exists + enabled
  - `turn_auto` params include `provider_kind` / `provider_base_url` / `provider_api_key` + model metadata (`model_reasoning` / `model_context_window` / `model_max_tokens` / `model_display_name`)
  - `inject_provider_env` deleted (no `OPENAI_*` env injection)

- [ ] **Step 1: Write the failing test for `execute_task` validation chain**

Add to `crates/busytok-subagent/src/manager.rs` tests module (or `crates/busytok-subagent/tests/` integration test file if no inline tests exist). The test builds a real `SubagentManager` backed by an in-memory store + `MockTaskExecutor`, inserts a bound subagent whose provider is disabled, and asserts `delegate()` returns a `Validation` error mentioning `"bound provider disabled"`:

```rust
#[tokio::test]
async fn execute_task_fails_when_bound_provider_disabled() {
    use busytok_subagent::models::{DelegateRequest, SubagentError};
    use busytok_subagent::SubagentManager;
    use busytok_subagent::mock_executor::MockTaskExecutor;
    use busytok_config::SubagentSettings;
    use std::sync::{Arc, Mutex};

    let db = busytok_store::Database::open_in_memory().unwrap();
    // Insert a provider that is DISABLED (spec §4.3 fail-fast).
    let provider = busytok_store::provider_catalog::create_provider(&db.conn(), busytok_store::provider_catalog::CreateProviderReq {
        name: "P1".into(),
        provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
        base_url: "https://api.test.com".into(),
        enabled: false,  // disabled — execute_task must reject
        api_key: Some("sk-test".into()),
    }).unwrap();
    let model = busytok_store::provider_catalog::create_model(&db.conn(), busytok_store::provider_catalog::CreateModelReq {
        provider_id: provider.id.clone(),
        model_id: "gpt-4o".into(),
        enabled: true,
        tags: vec![],
        display_name: None,
        reasoning: None,
        context_window: Some(128000),
        max_tokens: Some(16384),
    }).unwrap();
    // Insert a subagent bound to the disabled provider.
    db.subagent_upsert_logical(&busytok_store::SubagentLogicalSubagentRow {
        id: "sub-1".into(),
        name: "test-sub".into(),
        project_id: "h".into(),
        repo_path: "/tmp".into(),
        repo_hash: "h".into(),
        branch: None,
        intent: None,
        default_profile: "pi/search-cheap".into(),
        bound_provider_id: provider.id.clone(),
        bound_model_id: model.model_id.clone(),
        status: "cold".into(),
        created_at_ms: 1000,
        updated_at_ms: 1000,
        last_active_at_ms: None,
    }).unwrap();
    db.subagent_upsert_memory(&busytok_store::SubagentMemoryRow::new_empty("sub-1")).unwrap();

    let shared_db = Arc::new(Mutex::new(db));
    let settings = SubagentSettings::default();
    let manager = SubagentManager::new(shared_db.clone(), settings, "mock", Arc::new(MockTaskExecutor));

    let req = DelegateRequest {
        subagent_name: "test-sub".into(),
        subagent_id: Some("sub-1".into()),  // reuse path — bound fields ignored
        cwd: "/tmp".into(),
        profile: "pi/search-cheap".into(),
        intent: None,
        prompt: "hi".into(),
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: None,
        bound_model_id: None,
    };
    let result = manager.delegate(req).await;
    assert!(result.is_err(), "delegate should fail when bound provider is disabled");
    let err = result.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("bound provider disabled"), "got: {msg}");
    // Verify the error downcasts to SubagentError::Validation (not a store error).
    let downcast = err.downcast_ref::<SubagentError>();
    assert!(matches!(downcast, Some(SubagentError::Validation(_))), "expected Validation variant");
}
```

Adjust the `DelegateRequest` field set if Task 4 / Task 2 added more fields; grep `crates/busytok-subagent/src/models.rs` for the current struct definition. If `SubagentManager::new` signature differs (e.g. takes `with_pressure_gate`), use whichever constructor the existing tests use.

- [ ] **Step 2: Update `ExecutorInput` struct (with model metadata fields)**

Edit `crates/busytok-subagent/src/mock_executor.rs:11-31`. The `model` and `provider_id` fields become non-optional `String`. Add `provider_kind` / `provider_base_url` / `provider_api_key` (transient) AND the four model metadata fields needed by `turn_auto` params in Step 5:

```rust
#[derive(Clone)]
pub struct ExecutorInput {
    pub subagent_id: String,
    pub subagent_name: String,
    pub cwd: String,
    pub profile: String,
    pub model: String,
    pub prompt: String,
    /// Spec §4.3: when set, the sidecar resolves this artifact path instead of
    /// the inline `prompt`. Mutually exclusive with `prompt`.
    pub prompt_artifact_ref: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub tools: Vec<String>,
    pub memory: MemorySnapshot,
    pub context: CompactContext,
    pub write_access: bool,
    pub provider_id: String,
    pub provider_kind: busytok_domain::ProviderKind,
    pub provider_base_url: String,
    /// 瞬态执行态数据：不写回 task row，不进日志明文，不进 DTO/response/diagnostic。
    pub provider_api_key: String,
    // Model metadata — threaded to the sidecar so `registerProvider` can
    // build a complete model definition (spec §5.2).
    pub model_reasoning: bool,
    pub model_context_window: i64,
    pub model_max_tokens: i64,
    pub model_display_name: Option<String>,
}
```

- [ ] **Step 3: Update `MockTaskExecutor::execute` to use new fields**

`input.model` is now `String` (not `Option<String>`), so `usage.model` becomes `Some(input.model.clone())`. `input.provider_id` is now `String` — `MockTaskExecutor` doesn't currently set `usage.provider` from it (it hardcodes `"mock"`), so no change needed there. Verify the existing body still compiles; the only edit is the `Some(...)` wrap on `model`:

- [ ] **Step 4: Update `manager.rs::execute_task` with the validation chain**

Edit `crates/busytok-subagent/src/manager.rs:351` (`execute_task`). The current signature is `async fn execute_task(&self, task: &SubagentTaskRow, subagent: &LogicalSubagent) -> Result<DelegateResult>` — KEEP this signature (the plan's earlier sketch showed owned params + `Result<()>` which was wrong). Only the body's resolution block (current lines 356-360) is replaced; the rest of the body (memory_row fetch, context build, executor call, result mapping) is preserved verbatim.

Replace the OLD model resolution block:
```rust
let model = task
    .model_override
    .clone()
    .or_else(|| subagent.default_model.clone())
    .or_else(|| self.profile_model(&task.profile));
```

with the NEW validation + resolution block:
```rust
// Spec §4.3: effective model = task.model_override.unwrap_or(sub.bound_model_id)
let effective_model_id = task.model_override.clone()
    .unwrap_or_else(|| subagent.bound_model_id.clone());
// Spec §4.3 validation chain — fail fast on bound provider/model issues.
let (resolved_provider, resolved_model) = {
    let db = self.db.lock().expect("subagent db lock poisoned");
    let provider = db.get_provider_with_secret(&subagent.bound_provider_id)
        .map_err(SubagentError::Store)?
        .ok_or_else(|| SubagentError::Validation(
            format!("bound provider not found: {}", subagent.bound_provider_id)
        ))?;
    if !provider.enabled {
        return Err(SubagentError::Validation(
            format!("bound provider disabled: {}", subagent.bound_provider_id)
        ).into());
    }
    let api_key = provider.api_key.clone().unwrap_or_default();
    if api_key.is_empty() {
        return Err(SubagentError::Validation(
            format!("bound provider missing api key: {}", subagent.bound_provider_id)
        ).into());
    }
    let model = db.get_model_by_provider_and_model_id(&subagent.bound_provider_id, &effective_model_id)
        .map_err(SubagentError::Store)?
        .ok_or_else(|| SubagentError::Validation(
            format!("bound model not found in provider: {}", effective_model_id)
        ))?;
    if !model.enabled {
        return Err(SubagentError::Validation(
            format!("bound model disabled: {}", effective_model_id)
        ).into());
    }
    (provider, model)
};
// `model` (the local variable used by the rest of execute_task) is now the
// resolved model id (a String, not Option<String>). The rest of the body
// uses `effective_model_id` in its place.
let model: Option<String> = Some(effective_model_id.clone());
```

The `let model: Option<String> = Some(effective_model_id.clone());` shim keeps the rest of the existing body (which uses `model.clone()` and `model.as_deref()` in a few places) compiling without further edits. The engineer may later refactor those usages to use `effective_model_id` directly, but that's a cleanup — not required for this task.

Then, in the existing `ExecutorInput { ... }` literal (current lines 413-441), REPLACE:
- `model: model.clone(),` → `model: effective_model_id.clone(),` (now `String`, not `Option<String>`)
- `provider_id: profile_cfg.and_then(|p| p.provider_id.clone()),` → `provider_id: resolved_provider.id.clone(),` (now `String`, not `Option<String>`)

ADD the new fields (populated from the resolved `resolved_provider` + `resolved_model`):
```rust
provider_kind: resolved_provider.provider_kind.clone(),
provider_base_url: resolved_provider.base_url.clone(),
provider_api_key: resolved_provider.api_key.clone().unwrap_or_default(),  // 瞬态，不进日志明文
model_reasoning: resolved_model.reasoning,
model_context_window: resolved_model.context_window.unwrap_or(0),
model_max_tokens: resolved_model.max_tokens.unwrap_or(0),
model_display_name: resolved_model.display_name.clone(),
```

The `tools` / `write_access` / `memory` / `context` fields of `ExecutorInput` are populated by the EXISTING body (lines 382-403: `profile_cfg.map(|p| p.tools.clone())`, `profile_cfg.map(|p| p.write_access)`, `self.context_builder.build(...)`, `db.subagent_get_memory(...)`). PRESERVE those lines verbatim — do NOT replace them with placeholders.

Engineer: the only edits to `execute_task` are (1) the resolution block replacement above, (2) the two field substitutions in `ExecutorInput`, (3) the new field additions. Everything else (memory fetch, context build, executor.execute call, result mapping, task row update) stays as-is.

- [ ] **Step 5: Update sidecar executor `turn_auto` params**

Edit `crates/busytok-subagent/src/sidecar/executor.rs:65-150`. First, drop the "profile not bound to a provider" error branch (current lines 70-73):
```rust
// DELETE THIS — provider_id is now String (always present):
let provider_id = input.provider_id.as_ref().ok_or_else(|| {
    anyhow::anyhow!("profile not bound to a provider — cannot route execute()")
})?;
```
Replace with:
```rust
let provider_id = input.provider_id.clone();  // now String, always present
```

Then extend the `params` JSON object (current lines 122-149) with the new fields. The new fields come directly from `ExecutorInput` (Step 2 added them as struct fields; Step 4 populated them). Replace the existing `params` construction with:

```rust
let params = serde_json::json!({
    "logical_subagent_id": input.subagent_id,
    "logical_subagent_name": input.subagent_name,
    "cwd": input.cwd,
    "profile": input.profile,
    "model": input.model,
    "tools": input.tools,
    "prompt": input.prompt,
    "prompt_artifact_ref": input.prompt_artifact_ref,
    "memory": memory_json,
    "context": {
        "compact_context": input.context.compact_context,
        "budget_tokens": input.context.budget_tokens,
        "source": input.context.source,
    },
    "timeout_ms": input.timeout_seconds.map(|s| s * 1000),
    "constraints": {
        "write_access": input.write_access,
        "timeout_ms": input.timeout_seconds.map(|s| s * 1000).unwrap_or(180000),
    },
    "output_schema": {
        "format": "json",
        "name": "review_result",
        "version": 1,
    },
    "adapter_options": {},
    "provider_id": provider_id,
    "provider_kind": input.provider_kind,
    "provider_base_url": input.provider_base_url,
    // provider_api_key is sent so the sidecar can register it in AuthStorage.
    // Sidecar must NOT log this field in plaintext (Task 7 enforces).
    "provider_api_key": input.provider_api_key,
    "model_reasoning": input.model_reasoning,
    "model_context_window": input.model_context_window,
    "model_max_tokens": input.model_max_tokens,
    "model_display_name": input.model_display_name,
});
```

The `info!` log call (current line 150-156) currently logs `provider_id = %provider_id` — verify it does NOT log `provider_api_key` (grep for `provider_api_key` in executor.rs to confirm no plaintext logging). The `serde_json::json!` macro serializes the key into the RPC payload but does NOT log it; the `info!` macro only logs the fields explicitly listed.

- [ ] **Step 6: Delete `inject_provider_env` + clean up pool.rs module docstring**

Edit `crates/busytok-subagent/src/sidecar/pool.rs`:
- Delete lines 67-72 (the `inject_provider_env` function definition)
- Delete the callsite at line ~227 (`inject_provider_env(&mut config.env, &entry);`) AND the `info!` log at line ~232 (`"injected OPENAI_API_KEY + OPENAI_BASE_URL into sidecar env"`)
- Delete the test at line ~518-528 (the `inject_provider_env` test that asserts `env.get("OPENAI_API_KEY")`)
- **Clean up the module docstring (lines 1-44)** — multiple lines reference the old env-injection mechanism:
  - Line 4-5: `injecting provider-specific env (\`OPENAI_API_KEY\` + \`OPENAI_BASE_URL\`) into the cloned \`SidecarConfig\` before construction.` → replace with: `lazily creating them via \`ensure_worker\`. Provider credentials are now threaded per-turn via \`turn_auto\` params (Task 5), not via env injection.`
  - Line 7-12: the `**Fixed env injection (Task 6):**` paragraph — delete entirely (no longer applies).
  - Line 67-68: the `/// Inject provider credentials into env using FIXED names. / /// Sidecar only recognizes \`OPENAI_API_KEY\` and \`OPENAI_BASE_URL\`.` doc comment — delete with the function.
  - Line 87-90: the `/// injects \`OPENAI_API_KEY\` / \`OPENAI_BASE_URL\` per provider before` comment on `base_config` → replace with: `/// Produced by \`resolve_base_sidecar_config\`; the pool clones it per provider (no env override — credentials flow via \`turn_auto\` params).`

Engineer: grep `pool.rs` for `OPENAI_API_KEY` and `OPENAI_BASE_URL` — every remaining reference after the deletions must be in a comment that's being rewritten or deleted. No production code should reference these env names.

- [ ] **Step 7: Update `Manager` constructor sites for `ExecutorInput`**

Grep `crates/busytok-subagent/src/manager.rs` and test files for `ExecutorInput {` literals — add the new fields to each. For tests, use sensible defaults (`provider_id: "test-prov".into()`, `provider_kind: ProviderKind::OpenAiCompatible`, `provider_base_url: "https://test".into()`, `provider_api_key: "sk-test".into()`, `model_reasoning: false`, `model_context_window: 8000`, `model_max_tokens: 1000`, `model_display_name: None`).

- [ ] **Step 8: Run tests**

Run: `cargo test -p busytok-subagent`
Expected: PASS (some existing tests may need updates to construct `ExecutorInput` with the new fields)

- [ ] **Step 9: Workspace check + clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -30`
Expected: clean

- [ ] **Step 10: Commit**

```bash
git add crates/busytok-subagent/src/mock_executor.rs \
        crates/busytok-subagent/src/manager.rs \
        crates/busytok-subagent/src/sidecar/executor.rs \
        crates/busytok-subagent/src/sidecar/pool.rs
git commit -m "feat(subagent): ExecutorInput + execute_task validation chain + delete inject_provider_env"
```

---

## Task 6: `provider_test_connection` Anthropic branch

**Files:**
- Modify: `crates/busytok-runtime/src/supervisor.rs:5884-6033` (`provider_test_connection`)
- Test: `crates/busytok-runtime/src/supervisor.rs` inline tests (lines ~6807+)

**Interfaces:**
- Consumes: Task 1 (`ProviderKind::AnthropicCompatible`)
- Produces: `provider_test_connection` dispatches by `provider.provider_kind` — OpenAI path unchanged, Anthropic path uses `POST /v1/messages` with `max_tokens: 1` + `messages: [{role: "user", content: "ping"}]`

- [ ] **Step 1: Write the failing test for Anthropic probe request contract**

The runtime crate has no HTTP mock library (no `wiremock` / `mockito` in `Cargo.toml`). Testing only "which branch is taken" would let an engineer ship a malformed request (wrong path, missing `x-api-key`, wrong body) and still see green. To prevent this, extract a **pure request builder helper** `build_probe_request(kind: ProviderKind, base_url: &str, api_key: &str) -> ProbeRequest` that returns the full request contract (method, path, headers, JSON body) without any network I/O, then test the contract fields directly.

Add `ProbeRequest` + `build_probe_request` to `crates/busytok-runtime/src/supervisor.rs` (outside `#[cfg(test)]` — used by `provider_test_connection` in production):

```rust
/// A probe request contract built from a `ProviderKind`, without network I/O.
/// Extracted so unit tests can assert method / path / headers / body without
/// spinning up an HTTP server.
#[derive(Debug, PartialEq, Eq)]
pub struct ProbeRequest {
    pub method: reqwest::Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: serde_json::Value,
}

/// Build the probe request for a `ProviderKind`. Used by `provider_test_connection`
/// to construct the actual HTTP call; tested directly to lock the contract.
pub fn build_probe_request(
    kind: busytok_domain::ProviderKind,
    base_url: &str,
    api_key: &str,
) -> ProbeRequest {
    let base = base_url.trim_end_matches('/');
    match kind {
        busytok_domain::ProviderKind::OpenAiCompatible => ProbeRequest {
            method: reqwest::Method::GET,
            url: format!("{}/models", base),
            headers: vec![("Authorization".into(), format!("Bearer {}", api_key))],
            body: serde_json::Value::Null,
        },
        busytok_domain::ProviderKind::AnthropicCompatible => ProbeRequest {
            method: reqwest::Method::POST,
            url: format!("{}/v1/messages", base),
            headers: vec![
                ("x-api-key".into(), api_key.to_string()),
                ("anthropic-version".into(), "2023-06-01".into()),
                ("content-type".into(), "application/json".into()),
            ],
            body: serde_json::json!({
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "ping"}],
            }),
        },
    }
}
```

Add the following tests inside the existing `#[cfg(test)]` module (alongside the `provider_test_connection_*` tests at line 6807+):

```rust
#[test]
fn build_probe_request_openai_uses_get_models_with_bearer_auth() {
    let req = build_probe_request(
        busytok_domain::ProviderKind::OpenAiCompatible,
        "https://api.test.com",
        "sk-test",
    );
    assert_eq!(req.method, reqwest::Method::GET);
    assert_eq!(req.url, "https://api.test.com/models");
    assert_eq!(
        req.headers.iter().find(|(k, _)| k == "Authorization"),
        Some(&("Authorization".into(), "Bearer sk-test".into())),
        "OpenAI probe must carry Authorization: Bearer <key>"
    );
    assert_eq!(req.body, serde_json::Value::Null);
}

#[test]
fn build_probe_request_anthropic_uses_post_v1_messages_with_x_api_key_header() {
    let req = build_probe_request(
        busytok_domain::ProviderKind::AnthropicCompatible,
        "https://api.anthropic.com",
        "sk-ant-test",
    );
    assert_eq!(req.method, reqwest::Method::POST);
    assert_eq!(req.url, "https://api.anthropic.com/v1/messages");
    // Required headers — x-api-key (not Bearer), anthropic-version, content-type
    assert_eq!(
        req.headers.iter().find(|(k, _)| k == "x-api-key"),
        Some(&("x-api-key".into(), "sk-ant-test".into())),
        "Anthropic probe must use x-api-key header (not Bearer)"
    );
    assert_eq!(
        req.headers.iter().find(|(k, _)| k == "anthropic-version"),
        Some(&("anthropic-version".into(), "2023-06-01".into())),
        "Anthropic probe must carry anthropic-version header"
    );
    assert_eq!(
        req.headers.iter().find(|(k, _)| k == "content-type"),
        Some(&("content-type".into(), "application/json".into())),
    );
    // Minimal body shape: max_tokens + messages[0].role + messages[0].content
    assert_eq!(req.body["max_tokens"], 1);
    assert_eq!(req.body["messages"][0]["role"], "user");
    assert_eq!(req.body["messages"][0]["content"], "ping");
}

#[test]
fn build_probe_request_trims_trailing_slash_from_base_url() {
    let req = build_probe_request(
        busytok_domain::ProviderKind::AnthropicCompatible,
        "https://api.anthropic.com/",
        "sk-test",
    );
    assert_eq!(req.url, "https://api.anthropic.com/v1/messages");
}
```

- [ ] **Step 2: Refactor `provider_test_connection` to dispatch by kind via `build_probe_request`**

Edit `crates/busytok-runtime/src/supervisor.rs:5884`. The Anthropic branch MUST call `build_probe_request` (the same helper tested in Step 1) so the request contract is locked — no inline header/body duplication. The OpenAI branch stays as-is (it has DB access for the probe model fallback). Add the Anthropic branch:

```rust
async fn provider_test_connection(
    &self,
    req: ProviderTestConnectionRequestDto,
) -> Result<ProviderTestConnectionResponseDto> {
    let provider = {
        let db = self.db.lock().unwrap();
        db.get_provider_with_secret(&req.id)
            .map_err(|e| { tracing::error!(event_code = "provider.sql_read_failed", provider_id = %req.id, error = %e, "get_provider_with_secret failed"); e })?
            .ok_or_else(|| anyhow::anyhow!("provider not found: {}", req.id))?
    };
    let provider_id = provider.id.clone();
    let base_url = provider.base_url.clone();
    if !base_url.starts_with("https://") {
        anyhow::bail!("provider base_url must use HTTPS (got: {})", base_url);
    }
    let api_key = provider.api_key.as_deref()
        .ok_or_else(|| anyhow::anyhow!("provider has no api key"))?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    match provider.provider_kind {
        busytok_domain::ProviderKind::OpenAiCompatible => {
            // ... existing OpenAI logic (GET /models → fallback POST /chat/completions) ...
            // Keep as-is. Engineer: move the existing body into a `test_connection_openai` helper
            // that takes `&self` (for DB access to list_models_filtered) + client + provider_id + base_url + api_key.
            self.test_connection_openai(&client, &provider_id, &base_url, api_key).await
        }
        busytok_domain::ProviderKind::AnthropicCompatible => {
            self.test_connection_anthropic(&client, &provider_id, &base_url, api_key).await
        }
    }
}

async fn test_connection_anthropic(
    &self,
    client: &reqwest::Client,
    provider_id: &str,
    base_url: &str,
    api_key: &str,
) -> Result<ProviderTestConnectionResponseDto> {
    // Build the probe request via the tested helper — DO NOT inline headers/body here.
    // The contract (path, x-api-key, anthropic-version, body shape) is locked by
    // `build_probe_request_anthropic_uses_post_v1_messages_with_x_api_key_header` test.
    let probe = build_probe_request(
        busytok_domain::ProviderKind::AnthropicCompatible,
        base_url,
        api_key,
    );
    tracing::info!(
        event_code = "provider.test_connection",
        provider_id = %provider_id,
        url = %probe.url,
        "testing anthropic provider connection"
    );
    let mut req_builder = client
        .request(probe.method, &probe.url);
    for (k, v) in &probe.headers {
        req_builder = req_builder.header(k, v);
    }
    if probe.body != serde_json::Value::Null {
        req_builder = req_builder.json(&probe.body);
    }
    let resp = req_builder.send().await;
    match resp {
        Ok(r) if r.status().is_success() => {
            tracing::info!(event_code = "provider.test_connection.ok", provider_id = %provider_id, "anthropic connection test succeeded");
            Ok(ProviderTestConnectionResponseDto { ok: true, error: None, models_detected: None })
        }
        Ok(r) => {
            let status = r.status();
            tracing::warn!(event_code = "provider.test_connection.failed", provider_id = %provider_id, status = %status, "anthropic connection test failed");
            Ok(ProviderTestConnectionResponseDto { ok: false, error: Some(format!("HTTP {}", status)), models_detected: None })
        }
        Err(e) => {
            tracing::warn!(event_code = "provider.test_connection.error", provider_id = %provider_id, error = %e, "anthropic connection test error");
            Ok(ProviderTestConnectionResponseDto { ok: false, error: Some(format!("request failed: {}", e)), models_detected: None })
        }
    }
}
```

**Critical:** the Anthropic branch must call `build_probe_request()` — the same helper tested in Step 1. This ensures the request contract (path `/v1/messages`, header `x-api-key`, header `anthropic-version: 2023-06-01`, body `{max_tokens: 1, messages: [{role: "user", content: "ping"}]}`) is verified by tests. Do NOT inline a separate `serde_json::json!({...})` body or `.header("x-api-key", ...)` call in `test_connection_anthropic` — that would bypass the test contract and allow drift.

The OpenAI helper needs DB access (for the probe model fallback). Pass `self` through or extract the probe-model lookup into a separate helper. Engineer: keep the existing DB access pattern (lock + list_models_filtered) inside `test_connection_openai`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p busytok-runtime provider_test_connection`
Expected: PASS (existing OpenAI tests + new Anthropic test)

- [ ] **Step 4: Workspace check + clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -30`
Expected: clean

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-runtime/src/supervisor.rs
git commit -m "feat(runtime): provider_test_connection Anthropic /v1/messages probe"
```

---

## Task 7: Sidecar types + pi_session.ts + turn_auto handler

**Files:**
- Modify: `apps/pi-sidecar/src/types.ts` (`TurnAutoParams`)
- Modify: `apps/pi-sidecar/src/pi_session.ts` (`CreateSessionOpts` + `defaultSessionFactory`)
- Modify: `apps/pi-sidecar/src/handlers/turn_auto.ts` (thread params to `pool.ensure()`)
- Test: `apps/pi-sidecar/src/pi_session.test.ts` (or wherever session tests live)
- Test: `apps/pi-sidecar/src/handlers/turn_auto.test.ts`

**Interfaces:**
- Consumes: Task 5 (Rust sends the new params)
- Produces:
  - `TurnAutoParams { provider_kind, provider_base_url, provider_api_key, model_reasoning, model_context_window, model_max_tokens, model_display_name }`
  - `CreateSessionOpts { model: string, provider_id: string, provider_kind, provider_base_url, provider_api_key, model_reasoning, model_context_window, model_max_tokens, model_display_name }`
  - `defaultSessionFactory` uses `AuthStorage.inMemory` + `ModelRegistry.create` + `registerProvider` (no `cachedRegistry`, no env var)
  - `inject_provider_env` already deleted in Task 5

- [ ] **Step 1: Write the failing test for `defaultSessionFactory` with new params**

Add to `apps/pi-sidecar/tests/model_resolution.test.ts` (the existing test file that already mocks `@earendil-works/pi-coding-agent` — see its `vi.mock(...)` block at line 29). Extend the mock to capture `registerProvider` calls and add a new `describe` block:

```typescript
// At the top of tests/model_resolution.test.ts, EXTEND the existing
// `vi.mock('@earendil-works/pi-coding-agent', ...)` block to add
// `registerProvider` to the fake registry. Replace the existing
// `ModelRegistry.create: () => ({...})` with:
//
//   ModelRegistry: {
//     create: () => ({
//       find: (provider: string, modelId: string) => {
//         if (provider === 'test-provider' && modelId === 'test-model-id') {
//           return hoisted.fakeModel;
//         }
//         return undefined;
//       },
//       getAll: () => [hoisted.fakeModel],
//       registerProvider: hoisted.registerProvider,  // NEW — capture calls
//     }),
//   },
//
// And in the `vi.hoisted(...)` block at the top, add:
//   registerProvider: vi.fn((providerId: string, _config: unknown) => {
//     // Pretend registration succeeded; `find` will return fakeModel for
//     // the registered (provider, model) pair.
//   }),

describe('defaultSessionFactory multi-API provider registration (spec §5.2)', () => {
  beforeEach(() => {
    hoisted.registerProvider.mockClear();
    hoisted.createAgentSessionCalls.length = 0;
  });

  it('registers provider with anthropic-messages api for anthropic_compatible kind', async () => {
    const session = await defaultSessionFactory('sub-1', {
      cwd: '/tmp',
      model: 'test-model-id',
      provider_id: 'test-provider',
      provider_kind: 'anthropic_compatible',
      provider_base_url: 'https://api.anthropic.com',
      provider_api_key: 'sk-ant-test',
      model_reasoning: true,
      model_context_window: 200000,
      model_max_tokens: 8192,
      model_display_name: 'Claude Sonnet 4.5',
    });
    expect(session).toBeDefined();
    expect(hoisted.registerProvider).toHaveBeenCalledTimes(1);
    expect(hoisted.registerProvider).toHaveBeenCalledWith(
      'test-provider',
      expect.objectContaining({
        api: 'anthropic-messages',
        baseUrl: 'https://api.anthropic.com',
      }),
    );
    // The resolved model object must be passed into createAgentSession.
    expect(hoisted.createAgentSessionCalls).toHaveLength(1);
    expect(hoisted.createAgentSessionCalls[0].model).toBeDefined();
  });

  it('registers provider with openai-completions api for openai_compatible kind', async () => {
    await defaultSessionFactory('sub-1', {
      cwd: '/tmp',
      model: 'test-model-id',
      provider_id: 'test-provider',
      provider_kind: 'openai_compatible',
      provider_base_url: 'https://api.openai.com',
      provider_api_key: 'sk-test',
      model_reasoning: false,
      model_context_window: 128000,
      model_max_tokens: 16384,
    });
    expect(hoisted.registerProvider).toHaveBeenCalledWith(
      'test-provider',
      expect.objectContaining({
        api: 'openai-completions',
        baseUrl: 'https://api.openai.com',
      }),
    );
  });

  it('registers model with contextWindow + maxTokens from CreateSessionOpts', async () => {
    await defaultSessionFactory('sub-1', {
      cwd: '/tmp',
      model: 'test-model-id',
      provider_id: 'test-provider',
      provider_kind: 'openai_compatible',
      provider_base_url: 'https://api.test.com',
      provider_api_key: 'sk-test',
      model_reasoning: true,
      model_context_window: 200000,
      model_max_tokens: 8192,
      model_display_name: 'Test Model',
    });
    expect(hoisted.registerProvider).toHaveBeenCalledWith(
      'test-provider',
      expect.objectContaining({
        models: expect.arrayContaining([
          expect.objectContaining({
            id: 'test-model-id',
            contextWindow: 200000,
            maxTokens: 8192,
            reasoning: true,
            name: 'Test Model',
          }),
        ]),
      }),
    );
  });
});
```

The existing `model_resolution.test.ts` already mocks `createAgentSession`, `ModelRegistry`, and `AuthStorage` — only the `registerProvider` capture is new. The hoisted pattern keeps the mock module-level so `vi.mock` can reference it.

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -F pi-sidecar test`
Expected: FAIL (CreateSessionOpts doesn't have the new fields yet)

- [ ] **Step 3: Update `types.ts` `TurnAutoParams`**

Edit `apps/pi-sidecar/src/types.ts`:

```typescript
export type ProviderKind = 'openai_compatible' | 'anthropic_compatible';

export interface TurnAutoParams {
  logical_subagent_id: string;
  logical_subagent_name?: string;
  cwd: string;
  profile: string;
  model?: string;
  /** Provider ID — now REQUIRED (was optional in Phase 3). The Rust side
   *  always sends it since subagent binding makes provider routing explicit. */
  provider_id: string;
  provider_kind: ProviderKind;
  provider_base_url: string;
  /** Transient — sidecar must NOT log this in plaintext. */
  provider_api_key: string;
  model_reasoning: boolean;
  model_context_window: number;
  model_max_tokens: number;
  model_display_name?: string;
  tools?: string[];
  prompt: string;
  prompt_artifact_ref?: string | null;
  timeout_ms?: number;
  memory?: MemoryField;
  context?: CompactContext;
  constraints?: { write_access: boolean; timeout_ms: number };
}
```

NOTE: `provider_id` changes from optional (`provider_id?: string`) to required (`provider_id: string`). Grep `apps/pi-sidecar/src` for `params.provider_id` / `p.provider_id` usages — any code that assumed it could be `undefined` (e.g. `if (params.provider_id) { ... }`) must be updated to use the value directly.

- [ ] **Step 4: Update `CreateSessionOpts` + `defaultSessionFactory` in `pi_session.ts`**

Edit `apps/pi-sidecar/src/pi_session.ts`:

```typescript
export interface CreateSessionOpts {
  cwd: string;
  model: string;
  provider_id: string;
  provider_kind: ProviderKind;
  provider_base_url: string;
  provider_api_key: string;
  model_reasoning: boolean;
  model_context_window: number;
  model_max_tokens: number;
  model_display_name?: string;
  tools?: string[];
}

const PROVIDER_KIND_TO_PI_API: Record<ProviderKind, string> = {
  openai_compatible: 'openai-completions',
  anthropic_compatible: 'anthropic-messages',
};

export const defaultSessionFactory = async (
  logical_subagent_id: string,
  opts: CreateSessionOpts,
): Promise<PiSdkSession> => {
  const { createAgentSession, ModelRegistry, AuthStorage } = await import('@earendil-works/pi-coding-agent');
  // 1. AuthStorage — in-memory, secret sole source.
  const authStorage = AuthStorage.inMemory({
    [opts.provider_id]: { type: 'api_key', key: opts.provider_api_key },
  });
  // 2. ModelRegistry — in-memory, no file I/O.
  const registry = ModelRegistry.create(authStorage);
  // 3. Dynamic provider registration.
  const piApi = PROVIDER_KIND_TO_PI_API[opts.provider_kind];
  registry.registerProvider(opts.provider_id, {
    baseUrl: opts.provider_base_url,
    api: piApi,
    apiKey: '__busytok_runtime__',  // placeholder; real key from authStorage
    models: [{
      id: opts.model,
      name: opts.model_display_name ?? opts.model,
      reasoning: opts.model_reasoning,
      input: ['text'],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      contextWindow: opts.model_context_window,
      maxTokens: opts.model_max_tokens,
    }],
  });
  // 4. Precise model lookup.
  const model = registry.find(opts.provider_id, opts.model);
  if (!model) {
    throw new SidecarError(`model not found in registry after registerProvider: ${opts.model}`, -32603);
  }
  // 5. Create session.
  const sessionOpts: { cwd: string; tools?: string[]; model: unknown; authStorage: unknown; modelRegistry: unknown } = {
    cwd: opts.cwd,
    model,
    authStorage,
    modelRegistry: registry,
    ...(opts.tools ? { tools: opts.tools } : {}),
  };
  const { session } = await createAgentSession(sessionOpts as Parameters<typeof createAgentSession>[0]);
  return new PiSdkSession(
    session as unknown as SdkSession,
    logical_subagent_id,
    session.sessionId,
    opts.provider_id,
  );
};
```

Delete the `cachedRegistry` global + `resolveModelObject` function (lines 117-155).

Update the `PiSdkSession` constructor — the `resolvedProvider` parameter is now required (the `opts.provider_id` is always set). The 4th arg changes from `optional` to `required`:

```typescript
constructor(
  sdk: SdkSession,
  logical_subagent_id: string,
  adapter_session_id: string,
  resolvedProvider: string,
) {
  // ... same body ...
}
```

- [ ] **Step 5: Update `turn_auto.ts` handler to thread params**

Edit `apps/pi-sidecar/src/handlers/turn_auto.ts`. The `turnAutoHandlerWithPool` extracts the new fields from `TurnAutoParams` and constructs `CreateSessionOpts`:

```typescript
const sessionOpts: CreateSessionOpts = {
  cwd: params.cwd,
  model: params.model,
  provider_id: params.provider_id,
  provider_kind: params.provider_kind,
  provider_base_url: params.provider_base_url,
  provider_api_key: params.provider_api_key,
  model_reasoning: params.model_reasoning,
  model_context_window: params.model_context_window,
  model_max_tokens: params.model_max_tokens,
  ...(params.model_display_name ? { model_display_name: params.model_display_name } : {}),
  ...(params.tools ? { tools: params.tools } : {}),
};
const session = await pool.ensure(params.logical_subagent_id, sessionOpts);
```

NOTE: The existing `pool.ensure()` signature takes `(logical_subagent_id, opts)` and calls `defaultSessionFactory` on miss. The hit branch must ignore the new routing params (spec §5.5). Verify the existing `pool.ensure()` implementation does this — if it spreads `opts` into the session cache key, change the key to be only `logical_subagent_id`.

- [ ] **Step 6: Verify session pool key is `logical_subagent_id` only**

Edit `apps/pi-sidecar/src/session_pool.ts` (or wherever `SessionPool` is defined). The `ensure()` method should key on `logical_subagent_id` only:

```typescript
async ensure(id: string, opts: CreateSessionOpts): Promise<PiSdkSession> {
  const existing = this.sessions.get(id);
  if (existing && !existing.isClosed()) {
    existing.last_used_at_ms = Date.now();
    return existing;  // hit: ignore opts
  }
  const session = await this.factory(id, opts);  // miss: use opts
  this.sessions.set(id, session);
  return session;
}
```

Engineer: grep for `SessionPool` / `pool.ensure` and verify the existing implementation matches. If it includes `provider_id` / `model` in the key, change it to use only `logical_subagent_id`.

- [ ] **Step 7: Run tests**

Run: `pnpm -F pi-sidecar test`
Expected: PASS

- [ ] **Step 8: Typecheck**

Run: `pnpm -F pi-sidecar typecheck`
Expected: clean (no `lint` script exists for pi-sidecar — do NOT run one)

- [ ] **Step 9: Commit**

```bash
git add apps/pi-sidecar/src/types.ts \
        apps/pi-sidecar/src/pi_session.ts \
        apps/pi-sidecar/src/handlers/turn_auto.ts \
        apps/pi-sidecar/src/session_pool.ts \
        apps/pi-sidecar/src/pi_session.test.ts
git commit -m "feat(sidecar): defaultSessionFactory via AuthStorage.inMemory + ModelRegistry.registerProvider"
```

---

## Task 8: GUI updates — provider_kind selector + model metadata form

**Files:**
- Modify: `apps/gui/src/pages/ProvidersPage.tsx` (provider_kind selector)
- Modify: `apps/gui/src/components/ModelsSection.tsx` (model metadata form)
- Test: `apps/gui/src/pages/ProvidersPage.test.tsx` (already exists — extend with `provider_kind` selector assertions)
- Test: `apps/gui/src/components/ModelsSection.test.tsx` (already exists — extend with metadata form assertions)

**Interfaces:**
- Consumes: Task 4 (regenerated TS types with `ProviderKind` + model metadata)
- Produces:
  - Provider create/edit form includes a `provider_kind` selector (`openai_compatible` / `anthropic_compatible`)
  - Model create form includes required `context_window` + `max_tokens` inputs + optional `display_name` + `reasoning` checkbox
  - Both test files extended to cover the new UI controls (required for `pnpm coverage:gui` ≥90% threshold)

- [ ] **Step 1: Update `ProvidersPage.tsx` provider_kind selector**

Grep `apps/gui/src/pages/ProvidersPage.tsx` for `provider_kind` — the existing code likely hardcodes `"openai_compatible"`. Replace with a `<select>` that offers both values:

```tsx
<select
  value={form.provider_kind}
  onChange={(e) => setForm({ ...form, provider_kind: e.target.value as ProviderKind })}
>
  <option value="openai_compatible">OpenAI Compatible</option>
  <option value="anthropic_compatible">Anthropic Compatible</option>
</select>
```

Update the form state type to use `ProviderKind` from `@busytok/protocol-types`.

- [ ] **Step 2: Update `ModelsSection.tsx` model metadata form**

Grep `apps/gui/src/components/ModelsSection.tsx` for the create form. Add inputs for `context_window` (required, number), `max_tokens` (required, number), `display_name` (optional, text), `reasoning` (optional, checkbox):

```tsx
<input
  type="number"
  placeholder="Context window (required)"
  value={form.context_window ?? ''}
  onChange={(e) => setForm({ ...form, context_window: e.target.value ? Number(e.target.value) : undefined })}
/>
<input
  type="number"
  placeholder="Max tokens (required)"
  value={form.max_tokens ?? ''}
  onChange={(e) => setForm({ ...form, max_tokens: e.target.value ? Number(e.target.value) : undefined })}
/>
<input
  type="text"
  placeholder="Display name (optional)"
  value={form.display_name ?? ''}
  onChange={(e) => setForm({ ...form, display_name: e.target.value || undefined })}
/>
<label>
  <input
    type="checkbox"
    checked={form.reasoning ?? false}
    onChange={(e) => setForm({ ...form, reasoning: e.target.checked })}
  />
  Reasoning
</label>
```

The form submit handler must validate that `context_window` and `max_tokens` are present before calling the RPC. Show a user-facing error if missing.

- [ ] **Step 3: Write failing tests for `provider_kind` selector in `ProvidersPage.test.tsx`**

Both `ProvidersPage.test.tsx` and `ModelsSection.test.tsx` already exist (see the mock setup pattern at `apps/gui/src/pages/ProvidersPage.test.tsx:1-50`). Extend them with assertions for the new UI controls. Add to `ProvidersPage.test.tsx`:

```tsx
it("renders provider_kind selector with both options", async () => {
  mockUseProviders.mockReturnValue({
    data: { providers: [], total: 0 } as ProviderListResponseDto,
    isLoading: false,
    error: null,
  });
  mockUseProviderMutations.mockReturnValue({
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
    testConnection: vi.fn(),
  });
  mockUseModels.mockReturnValue({ data: { models: [], total: 0 }, isLoading: false, error: null });
  mockUseModelMutations.mockReturnValue({ create: vi.fn(), update: vi.fn(), delete: vi.fn() });
  mockUseSettingsSnapshot.mockReturnValue({ data: null, isLoading: false });
  mockUseProfileMutations.mockReturnValue({ create: vi.fn(), update: vi.fn(), delete: vi.fn() });

  render(
    <QueryClientProvider client={new QueryClient()}>
      <ProvidersPage />
    </QueryClientProvider>
  );

  // Open the create-provider form (adjust selector to match existing UI)
  const addButton = await screen.findByText(/add provider|新增/i);
  fireEvent.click(addButton);

  // Assert the selector exists and offers both kinds
  const selector = await screen.findByRole("combobox", { name: /kind|类型/i });
  expect(selector).toBeTruthy();
  const options = screen.getAllByRole("option");
  const optionValues = options.map((o) => (o as HTMLOptionElement).value);
  expect(optionValues).toContain("openai_compatible");
  expect(optionValues).toContain("anthropic_compatible");
});

it("blocks create when provider_kind is not selected", async () => {
  // Setup same as above...
  // Assert that the submit button is disabled or an error shows when no kind is selected.
});
```

- [ ] **Step 4: Write failing tests for metadata form in `ModelsSection.test.tsx`**

Add to `ModelsSection.test.tsx`:

```tsx
it("renders context_window and max_tokens as required inputs in create form", async () => {
  mockUseProviders.mockReturnValue({
    data: { providers: [{ id: "prov-1", name: "P1", provider_kind: "openai_compatible", base_url: "https://api.test.com", enabled: true } as ProviderDto] } as ProviderListResponseDto,
    isLoading: false,
    error: null,
  });
  mockUseModels.mockReturnValue({ data: { models: [], total: 0 }, isLoading: false, error: null });
  mockUseModelMutations.mockReturnValue({ create: vi.fn(), update: vi.fn(), delete: vi.fn() });

  render(
    <QueryClientProvider client={new QueryClient()}>
      <ModelsSection />
    </QueryClientProvider>
  );

  // Open the create-model form
  const addButton = await screen.findByText(/add model|新增/i);
  fireEvent.click(addButton);

  // Assert required inputs exist
  expect(screen.getByPlaceholderText(/context window/i)).toBeTruthy();
  expect(screen.getByPlaceholderText(/max tokens/i)).toBeTruthy();
  // Assert optional inputs exist
  expect(screen.getByPlaceholderText(/display name/i)).toBeTruthy();
  expect(screen.getByLabelText(/reasoning/i)).toBeTruthy();
});

it("blocks model create when context_window or max_tokens is missing", async () => {
  // Fill form with model_id but leave context_window/max_tokens empty.
  // Assert create mutation is NOT called, or error message shows.
  const createFn = vi.fn();
  mockUseModelMutations.mockReturnValue({ create: createFn, update: vi.fn(), delete: vi.fn() });
  // ... render + open form + fill model_id only + click submit ...
  expect(createFn).not.toHaveBeenCalled();
});
```

- [ ] **Step 5: Run GUI tests + typecheck + coverage**

Run: `pnpm -F gui typecheck && pnpm -F gui test`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add apps/gui/src/pages/ProvidersPage.tsx \
        apps/gui/src/pages/ProvidersPage.test.tsx \
        apps/gui/src/components/ModelsSection.tsx \
        apps/gui/src/components/ModelsSection.test.tsx
git commit -m "feat(gui): provider_kind selector + model metadata form + tests"
```

---

## Task 9: Regression + spec coverage verification + final commit

**Files:**
- Verify: spec test cases 1-40 from `docs/superpowers/specs/2026-07-04-subagent-provider-binding-design.md` §9
- Verify: existing regression tests (`provider_changed_removes_worker_then_respawns`, `e2e_multi_provider_creates_separate_workers`, `e2e_auth_failure_kills_worker`)

**Interfaces:**
- Consumes: Tasks 1-9

- [ ] **Step 1: Run full workspace test suite**

Run: `cargo test --workspace 2>&1 | tail -50`
Expected: PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -30`
Expected: clean

- [ ] **Step 3: Run Rust coverage gate**

Run: `cargo llvm-cov --workspace --fail-under-lines 90 2>&1 | tail -30`
Expected: PASS (Rust coverage ≥ 90%)

- [ ] **Step 4: Run GUI coverage gate**

Run: `pnpm coverage:gui`
Expected: PASS (GUI line/function/branch/statement coverage ≥ 90% — enforced by `vitest.config.ts` thresholds). This is a hard gate because Task 8 modified `ProvidersPage.tsx` + `ModelsSection.tsx`.

- [ ] **Step 5: Run sidecar tests + typecheck**

Run: `pnpm -F pi-sidecar test && pnpm -F pi-sidecar typecheck`
Expected: PASS (no `lint` script exists for pi-sidecar — do NOT run one)

- [ ] **Step 6: Verify spec test cases 1-40 are covered**

For each of the 40 spec test cases (§9.1-9.6), grep the test files to confirm a test exists that exercises the scenario. Add tests for any uncovered cases. The 3 regression tests (§9.6 items 38-40) must still pass — they're existing tests.

Engineer: write a checklist mapping each spec test case to a test function name. If any case is uncovered, add a test in the appropriate crate:

- §9.1 (1-4): `crates/busytok-store/tests/provider_catalog.rs` + `crates/busytok-subagent/src/resolver.rs` tests
- §9.2 (5-12): `crates/busytok-subagent/src/resolver.rs` + `crates/busytok-subagent/src/manager.rs` tests
- §9.3 (13-19): `crates/busytok-subagent/src/manager.rs` tests
- §9.4 (20-30): `crates/busytok-runtime/src/supervisor.rs` tests
- §9.5 (31-37): `apps/pi-sidecar/src/pi_session.test.ts` + integration tests
- §9.6 (38-40): existing regression tests

- [ ] **Step 7: Add integration test for first-session base_url (spec §9.5 item 37, pi#2291)**

Add to `apps/pi-sidecar/tests/model_resolution.test.ts` (same file as Task 7 Step 1's tests — reuses the same `vi.mock` block). The test verifies that the custom `provider_base_url` from `CreateSessionOpts` flows through to the model object passed into `createAgentSession` (i.e. `registerProvider` was called with `baseUrl: <custom base_url>` AND `createAgentSession` received a `model` whose provider config carries that base_url):

```typescript
describe('defaultSessionFactory custom base_url on first session miss (pi#2291)', () => {
  beforeEach(() => {
    hoisted.registerProvider.mockClear();
    hoisted.createAgentSessionCalls.length = 0;
  });

  it('passes custom base_url into registerProvider + createAgentSession model', async () => {
    const customBaseUrl = 'https://api.deepseek.com';
    await defaultSessionFactory('sub-1', {
      cwd: '/tmp',
      model: 'test-model-id',
      provider_id: 'test-provider',
      provider_kind: 'openai_compatible',
      provider_base_url: customBaseUrl,
      provider_api_key: 'sk-test',
      model_reasoning: false,
      model_context_window: 64000,
      model_max_tokens: 4096,
    });

    // 1. registerProvider must receive the custom base_url.
    expect(hoisted.registerProvider).toHaveBeenCalledTimes(1);
    const [_providerId, providerConfig] = hoisted.registerProvider.mock.calls[0];
    expect(providerConfig).toMatchObject({ baseUrl: customBaseUrl });

    // 2. createAgentSession must receive a model object (the registry-
    //    resolved model — which inherits the provider's baseUrl).
    expect(hoisted.createAgentSessionCalls).toHaveLength(1);
    const callOpts = hoisted.createAgentSessionCalls[0];
    expect(callOpts.model).toBeDefined();
    // The fake model's provider matches what registerProvider was called with.
    expect((callOpts.model as { provider?: string }).provider).toBe('test-provider');
    // 3. The session object's resolvedProvider field must equal the provider_id
    //    (the factory passes it to the PiSdkSession constructor).
    // (The existing `defaultSessionFactory` returns a PiSdkSession whose
    // `resolvedProvider` is `opts.provider_id` — Task 7 makes this required.)
  });

  it('does NOT leak provider_api_key into createAgentSession opts (瞬态数据隔离)', async () => {
    await defaultSessionFactory('sub-1', {
      cwd: '/tmp',
      model: 'test-model-id',
      provider_id: 'test-provider',
      provider_kind: 'openai_compatible',
      provider_base_url: 'https://api.test.com',
      provider_api_key: 'sk-secret-do-not-leak',
      model_reasoning: false,
      model_context_window: 8000,
      model_max_tokens: 1000,
    });

    expect(hoisted.createAgentSessionCalls).toHaveLength(1);
    const callOptsJson = JSON.stringify(hoisted.createAgentSessionCalls[0]);
    expect(callOptsJson).not.toContain('sk-secret-do-not-leak');
  });
});
```

- [ ] **Step 8: Final commit (if any test additions)**

```bash
git add crates/ apps/
git commit -m "test: spec coverage for subagent-provider binding + Pi SDK multi-API"
```

- [ ] **Step 9: Final verification**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo llvm-cov --workspace --fail-under-lines 90 && pnpm -r test && pnpm -r typecheck`
Expected: all PASS

---

## Self-Review Notes

**Spec coverage check:**
- §2.1 ProviderKind extension → Task 1 ✓
- §2.2 models metadata → Task 1 ✓
- §2.3 subagent_logical_subagents rebuild → Task 1 (migration) + Task 2 (struct) ✓
- §2.4 LogicalSubagent struct → Task 2 ✓
- §2.5 Profile downgrade → Task 3 ✓
- §3.1 resolve_by_name signature → Task 2 ✓
- §3.2 creation validation chain → Task 2 ✓
- §3.3 SubagentDelegateRequestDto + DelegateRequest → Task 4 + Task 2 ✓
- §3.4 DB write → Task 2 ✓
- §3.5 runtime no profile-derived model/provider → Task 3 (delete profile_model) + Task 5 (execute_task uses bound fields) ✓
- §4.1 model resolution chain → Task 5 ✓
- §4.2 provider resolution chain → Task 5 ✓
- §4.3 validation chain → Task 5 ✓
- §4.4 ExecutorInput → Task 5 ✓
- §4.5 provider_test_connection Anthropic → Task 6 ✓
- §5.1-5.7 sidecar / Pi SDK → Task 7 ✓
- §6 turn_auto / session creation → Task 7 ✓
- §7.1 provider_kind change → kill worker → Task 1 Step 18b (store `UpdateProviderPatch` + `update_provider`) + Task 4 Step 3b (DTO `ProviderUpdateRequestDto` + `provider_kind`) + Task 4 Step 5 (supervisor `provider_update` plumbs `provider_kind` to patch; existing `provider_changed` call kills worker) ✓
- §7.2-7.5 provider update + delete semantics → Task 3 (delete collect_profile_refs) + Task 5 (worker kill via provider_changed, already exists) ✓
- §9 testing → Task 9 ✓

**Placeholder scan:** None — every step has concrete code or exact commands. (Earlier draft had placeholder test stubs in Tasks 5/6/7/9 + `/* from profile config */` / `/* from ExecutorInput — add field */` placeholders in Task 5 + "rest of delegate unchanged" pointer in Task 2; all replaced with concrete code or explicit line-by-line preservation instructions in this revision.)

**Type consistency:**
- `bound_provider_id: String` / `bound_model_id: String` consistent across `LogicalSubagent`, `SubagentLogicalSubagentRow`, `SubagentDetailDto`
- `bound_provider_id: Option<String>` / `bound_model_id: Option<String>` consistent across `SubagentDelegateRequestDto`, `DelegateRequest`
- `provider_kind: ProviderKind` consistent across `ExecutorInput`, `TurnAutoParams`, `CreateSessionOpts`, `UpdateProviderPatch` (`Option<ProviderKind>`), `ProviderUpdateRequestDto` (`Option<ProviderKind>`)
- `provider_api_key: String` consistent (not `Option<String>`) across `ExecutorInput`, `TurnAutoParams`, `CreateSessionOpts`
- `model_reasoning: bool` / `model_context_window: i64` (Rust) / `number` (TS) / `model_max_tokens: i64` (Rust) / `number` (TS) / `model_display_name: Option<String>` (Rust) / `string?` (TS) consistent across `ExecutorInput`, `TurnAutoParams`, `CreateSessionOpts`
- `SubagentError::Validation(String)` added in Task 2 Step 9 + `code()` method extended with `subagent.validation_error` branch (Task 2 Step 9 note)

**Known coupling:** Tasks 2-3 may produce transient compile errors in downstream crates if the engineer doesn't fix all construction sites in the same commit. The plan calls this out at each step ("grep for ... and fix each"). Task 1 Step 18b adds a new field to `UpdateProviderPatch` — every `UpdateProviderPatch { ... }` literal (tests + production) must add `provider_kind: None` to compile; Task 1 Step 18c calls this out.
