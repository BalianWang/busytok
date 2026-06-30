# Phase 2: Subagent Monitoring Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a read-only "Subagents" monitoring page to the GUI backed by a new `subagent.runtime_status` RPC that returns a single-snapshot aggregate of pressure state, logical subagents (with task counts + last task), recent tasks (20, all subagents), and sidecar worker state.

**Architecture:** One new RPC (`subagent.runtime_status`) assembled in `BusytokSupervisor` by querying `SubagentManager` (logical subagents + task counts + recent tasks) and `PiSidecarSupervisor` (worker state + pressure + hot sessions). The supervisor caches the latest `ResourceSample` and `hot_session_count` on `SupervisorState` during its existing supervision loop (no new polling thread). The GUI page polls the RPC every 5s using TanStack Query's `refetchInterval`, reusing the ProvidersPage layout shell (SettingsRow/SettingsValue/PageState). No new database migrations — all data comes from existing tables (`subagent_logical_subagents`, `subagent_tasks`, `subagent_memory`) plus in-memory worker state.

**Tech Stack:** Rust (async-trait, tokio, rusqlite, tracing, ts-rs), React + TypeScript + TanStack Query + Vitest, existing `busytok-control`/`busytok-runtime`/`busytok-subagent`/`busytok-protocol` crates.

## Global Constraints

- **Single-snapshot semantics:** all fields in one `subagent.runtime_status` response must come from the same moment — no mixing of stale subagent rows with fresh worker state. Implementation: snapshot the worker state first (single lock), then read DB rows; acceptable because DB reads are fast and the supervision loop updates the cache at most every `monitor_interval_seconds` (default 30s).
- **`tasks_recent`:** fixed limit 20, ordered by `created_at_ms desc`, across ALL subagents (no subagent_id filter).
- **Strictly read-only:** no hibernate/delete/retry buttons. The page must NOT call `subagent.hibernate` / `subagent.delete`. Existing RPCs remain for CLI use.
- **Subagent rows show logical entities only** (no pid). Worker rows show process entities only (no subagent name).
- **5s poll cadence** via `refetchInterval` with `refetchIntervalInBackground: false` (matches `useShellStatus` pattern at `useBusytokData.ts:109-137`).
- **`workers[]` may be empty** pre-Phase-3 (spec line 218). When `sidecar_supervisor` is `None` or not running, return `workers: []`. The page shows "No active sidecar workers."
- **`provider_id` in worker rows:** report `null` for Phase 2 (no provider binding exists yet; Phase 3 adds per-provider workers). The DTO field is `Option<String>`.
- **`"stale"` worker state** is a Phase 3 concept (spec line 252). Phase 2 only reports `"running"` or `"stopped"`.
- **Pressure `level` mapping:** `PressureAction::None` → `"normal"`, `PressureGate` paused state → `"throttled"`, `PressureAction::Hibernate` → `"evicting"`, `PressureAction::GracefulRestart`/`ForceKill` → `"restarting"`. (See `pressure.rs:18` for the `PressureAction` enum.)
- **`memory_used_pct`:** system-wide memory usage percentage = `100 - (system_available_mb / system_total_mb * 100)`, rounded to u32. Derived from the cached `ResourceSample` (which has `system_available_mb`) + `sysinfo::System::total_memory()`.
- **`hot_sessions_limit`:** from `SubagentSettings.max_hot_sessions` (default 3, config/lib.rs:231).
- **Observability:** the RPC handler emits `tracing::debug!(event_code = "subagent.runtime_status_served", ...)` on every call (high frequency, so debug not info). The frontend emits `reportFrontendEventSafely({ level: "INFO", event_code: "subagent.page_viewed", ... })` on page mount.
- **Coverage gate:** new Rust code ≥90% line coverage; new frontend code ≥90% line coverage (matches Phase 1 bar).
- **Rust trait wiring:** adding a trait method touches 6 sites (trait def, `BusytokSupervisor` impl, `TestRuntimeControl` mock, `Arc<T>` blanket impl, `AliasConflictRuntime` test mock at `apps/cli/tests/prompt.rs`, `MethodDispatchErrorRuntime` test mock at `crates/busytok-control/tests/server.rs`).
- **TS type regeneration:** after adding DTOs, run `cargo test -p busytok-protocol generate_typescript_types` to regenerate `packages/busytok-protocol-types/src/generated.ts`.
- **No new Cargo dependencies** unless explicitly stated in a task.

---

## File Structure

**Rust (backend):**
- `crates/busytok-store/src/subagent_queries.rs` — new query functions: `list_recent_tasks_all`, `count_tasks_by_subagent`, `last_task_by_subagent`. (Modify)
- `crates/busytok-store/src/db.rs` — thin wrappers for the new queries. (Modify)
- `crates/busytok-subagent/src/manager.rs` — new methods: `recent_tasks_all`, `task_counts_by_subagent`, `last_task_by_subagent`. (Modify)
- `crates/busytok-subagent/src/sidecar/supervisor.rs` — add `spawned_at: Instant` to `SupervisorState`; cache `latest_sample: Option<ResourceSample>` and `latest_hot_sessions: u32` on `SupervisorState`; add public accessors `worker_snapshot()`. (Modify)
- `crates/busytok-protocol/src/dto.rs` — new DTOs: `SubagentRuntimeStatusRequestDto`, `SubagentRuntimeStatusResponseDto`, `SubagentPressureGateDto`, `SubagentRuntimeSubagentDto`, `SubagentRuntimeTaskDto`, `SubagentWorkerDto`. (Modify)
- `crates/busytok-protocol/src/methods.rs` — add `"subagent.runtime_status"` to manifest. (Modify)
- `crates/busytok-protocol/src/ts.rs` — add `decl()` entries for new DTOs. (Modify)
- `crates/busytok-control/src/dispatch.rs` — add `subagent_runtime_status` to `RuntimeControl` trait + dispatch arm + `TestRuntimeControl` stub + `Arc<T>` forwarding. (Modify)
- `crates/busytok-control/tests/server.rs` — add `subagent_runtime_status` to `MethodDispatchErrorRuntime`. (Modify)
- `apps/cli/tests/prompt.rs` — add `subagent_runtime_status` to `AliasConflictRuntime`. (Modify)
- `crates/busytok-runtime/src/supervisor.rs` — implement `subagent_runtime_status` handler assembling the aggregate snapshot. (Modify)
- `crates/busytok-runtime/tests/supervisor_control.rs` — tests for the new handler. (Modify)
- `crates/busytok-subagent/src/sidecar/supervisor.rs` tests — tests for new accessors. (Modify)

**TypeScript (frontend):**
- `packages/busytok-protocol-types/src/generated.ts` — regenerated. (Auto)
- `apps/gui/src/api/busytokClient.ts` — add `subagentRuntimeStatus()`. (Modify)
- `apps/gui/src/api/queryKeys.ts` — add `subagentRuntimeStatus`. (Modify)
- `apps/gui/src/api/useBusytokData.ts` — add `useSubagentRuntimeStatus()` with 5s poll. (Modify)
- `apps/gui/src/pages/SubagentsPage.tsx` — new monitoring page. (Create)
- `apps/gui/src/pages/SubagentsPage.test.tsx` — tests. (Create)
- `apps/gui/src/components/AppShell.tsx` — add `"subagents"` to `DesktopPage` union. (Modify)
- `apps/gui/src/components/desktop/Sidebar.tsx` — add Subagents entry to Tools group. (Modify)
- `apps/gui/src/App.tsx` — add route + DESKTOP_PAGES entry. (Modify)
- `apps/gui/src/App.test.tsx` — bump sidebar count. (Modify)
- `apps/gui/src/components/desktop/Sidebar.test.tsx` — add Subagents button assertion. (Modify)
- `apps/gui/src/components/AppShell.test.tsx` — bump sidebar count. (Modify)

---

## Task 1: Store-layer queries for aggregate task data

**Files:**
- Modify: `crates/busytok-store/src/subagent_queries.rs`
- Modify: `crates/busytok-store/src/db.rs`
- Test: `crates/busytok-store/src/subagent_queries.rs` (inline `#[cfg(test)]` module)

**Interfaces:**
- Produces: `list_recent_tasks_all(conn, limit) -> Result<Vec<SubagentTaskRow>>`, `count_tasks_by_subagent(conn) -> Result<Vec<(String, u32)>>`, `last_task_by_subagent(conn) -> Result<Vec<(String, i64, String)>>` (subagent_id, created_at_ms, status)

- [ ] **Step 1: Write failing tests for the three new query functions**

Add a `#[cfg(test)] mod phase2_tests` module at the end of `crates/busytok-store/src/subagent_queries.rs`. Use `Database::open_in_memory()` (db.rs:183) and existing `SubagentTaskRow::for_test(...)` (repository.rs:548) fixtures.

```rust
#[cfg(test)]
mod phase2_tests {
    use super::*;
    use crate::db::Database;
    use crate::repository::SubagentTaskRow;

    fn seed_subagent(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO subagent_logical_subagents (id, name, project_id, repo_path, repo_hash, branch, intent, default_profile, default_model, status, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, 'proj', '/repo', 'hash', NULL, NULL, 'pi/review-cheap', NULL, 'warm', 1000, 1000)",
            rusqlite::params![id, id],
        ).unwrap();
    }

    fn seed_task(conn: &Connection, id: &str, subagent_id: &str, status: &str, created_at_ms: i64) {
        let mut row = SubagentTaskRow::for_test(id, subagent_id, "pi");
        row.status = status.to_string();
        row.created_at_ms = created_at_ms;
        // Use the existing insert path — adapt to whatever repository.rs exposes.
        // If no public insert, use raw SQL:
        conn.execute(
            "INSERT INTO subagent_tasks (id, subagent_id, source_harness, source_session_id, intent, profile, prompt, prompt_artifact_ref, output_schema_name, output_schema_version, status, result_summary, result_json, error, created_at_ms, started_at_ms, completed_at_ms, timeout_seconds, model_override)
             VALUES (?1, ?2, 'pi', NULL, NULL, 'pi/review-cheap', 'prompt', NULL, NULL, 0, ?3, NULL, NULL, NULL, ?4, NULL, NULL, NULL, NULL)",
            rusqlite::params![id, subagent_id, status, created_at_ms],
        ).unwrap();
    }

    #[test]
    fn list_recent_tasks_all_returns_across_all_subagents() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_subagent(conn, "sub-a");
        seed_subagent(conn, "sub-b");
        seed_task(conn, "t1", "sub-a", "completed", 1000);
        seed_task(conn, "t2", "sub-b", "failed", 2000);
        seed_task(conn, "t3", "sub-a", "completed", 3000);

        let tasks = list_recent_tasks_all(conn, 20).unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].id, "t3"); // desc order
        assert_eq!(tasks[1].id, "t2");
        assert_eq!(tasks[2].id, "t1");
    }

    #[test]
    fn list_recent_tasks_all_respects_limit() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_subagent(conn, "sub-a");
        for i in 0..10 {
            seed_task(conn, &format!("t{i}"), "sub-a", "completed", 1000 + i);
        }
        let tasks = list_recent_tasks_all(conn, 3).unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].id, "t9");
    }

    #[test]
    fn count_tasks_by_subagent_groups_correctly() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_subagent(conn, "sub-a");
        seed_subagent(conn, "sub-b");
        seed_task(conn, "t1", "sub-a", "completed", 1000);
        seed_task(conn, "t2", "sub-a", "failed", 2000);
        seed_task(conn, "t3", "sub-b", "completed", 3000);

        let counts = count_tasks_by_subagent(conn).unwrap();
        let mut map: std::collections::HashMap<String, u32> = counts.into_iter().collect();
        assert_eq!(map.remove("sub-a").unwrap(), 2);
        assert_eq!(map.remove("sub-b").unwrap(), 1);
    }

    #[test]
    fn last_task_by_subagent_returns_latest() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_subagent(conn, "sub-a");
        seed_task(conn, "t1", "sub-a", "completed", 1000);
        seed_task(conn, "t2", "sub-a", "failed", 2000);

        let lasts = last_task_by_subagent(conn).unwrap();
        assert_eq!(lasts.len(), 1);
        let (sub_id, created_at, status) = &lasts[0];
        assert_eq!(sub_id, "sub-a");
        assert_eq!(*created_at, 2000);
        assert_eq!(status, "failed");
    }

    #[test]
    fn last_task_by_subagent_empty_when_no_tasks() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        seed_subagent(conn, "sub-a");
        let lasts = last_task_by_subagent(conn).unwrap();
        assert!(lasts.is_empty());
    }
}
```

Run: `cargo test -p busytok-store phase2_tests`
Expected: FAIL — functions not defined.

- [ ] **Step 2: Implement `list_recent_tasks_all`**

Add to `crates/busytok-store/src/subagent_queries.rs` after the existing `list_tasks` function (~line 393):

```rust
/// Returns the most recent tasks across ALL subagents, ordered by created_at_ms desc.
/// Spec §4 Phase 2: `tasks_recent` fixed limit 20, no subagent_id filter.
pub fn list_recent_tasks_all(conn: &Connection, limit: i64) -> Result<Vec<SubagentTaskRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, subagent_id, source_harness, source_session_id, intent, profile, prompt,
                prompt_artifact_ref, output_schema_name, output_schema_version, status,
                result_summary, result_json, error, created_at_ms, started_at_ms, completed_at_ms,
                timeout_seconds, model_override
         FROM subagent_tasks
         ORDER BY created_at_ms DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(rusqlite::params![limit], |row| {
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
            timeout_seconds: row.get(17)?,
            model_override: row.get(18)?,
        })
    })?;
    rows.collect::<Result<Vec<_>>>().map_err(Into::into)
}
```

- [ ] **Step 3: Implement `count_tasks_by_subagent`**

Add to `crates/busytok-store/src/subagent_queries.rs`:

```rust
/// Returns (subagent_id, task_count) for every subagent that has at least one task.
pub fn count_tasks_by_subagent(conn: &Connection) -> Result<Vec<(String, u32)>> {
    let mut stmt = conn.prepare(
        "SELECT subagent_id, COUNT(*) as cnt
         FROM subagent_tasks
         GROUP BY subagent_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
    })?;
    rows.collect::<Result<Vec<_>>>().map_err(Into::into)
}
```

- [ ] **Step 4: Implement `last_task_by_subagent`**

Add to `crates/busytok-store/src/subagent_queries.rs`:

```rust
/// Returns (subagent_id, created_at_ms, status) for the most recent task of each subagent.
/// Only includes subagents that have at least one task.
pub fn last_task_by_subagent(conn: &Connection) -> Result<Vec<(String, i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT t.subagent_id, t.created_at_ms, t.status
         FROM subagent_tasks t
         INNER JOIN (
             SELECT subagent_id, MAX(created_at_ms) AS max_created
             FROM subagent_tasks
             GROUP BY subagent_id
         ) latest ON t.subagent_id = latest.subagent_id AND t.created_at_ms = latest.max_created",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?))
    })?;
    rows.collect::<Result<Vec<_>>>().map_err(Into::into)
}
```

- [ ] **Step 5: Add DB wrappers in `db.rs`**

Add to `crates/busytok-store/src/db.rs` after `subagent_list_tasks` (~line 1941):

```rust
/// Recent tasks across all subagents (spec §4 Phase 2 tasks_recent).
pub fn subagent_list_recent_tasks_all(&self, limit: i64) -> Result<Vec<busytok_domain::SubagentTaskRow>> {
    crate::subagent_queries::list_recent_tasks_all(&self.conn, limit)
}

/// Per-subagent task counts (spec §4 Phase 2 subagents[].task_count).
pub fn subagent_count_tasks_by_subagent(&self) -> Result<Vec<(String, u32)>> {
    crate::subagent_queries::count_tasks_by_subagent(&self.conn)
}

/// Per-subagent last task (subagent_id, created_at_ms, status).
pub fn subagent_last_task_by_subagent(&self) -> Result<Vec<(String, i64, String)>> {
    crate::subagent_queries::last_task_by_subagent(&self.conn)
}
```

Note: the `SubagentTaskRow` import path must match the existing `subagent_list_tasks` return type — check db.rs:1941 for the exact path used (`busytok_domain::SubagentTaskRow` or `crate::repository::SubagentTaskRow`).

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p busytok-store phase2_tests`
Expected: PASS (5 tests)

- [ ] **Step 7: Commit**

```bash
git add crates/busytok-store/src/subagent_queries.rs crates/busytok-store/src/db.rs
git commit -m "feat(store): add aggregate task queries for subagent monitoring (list_recent_tasks_all, count_tasks_by_subagent, last_task_by_subagent)"
```

---

## Task 2: SubagentManager methods for aggregate data

**Files:**
- Modify: `crates/busytok-subagent/src/manager.rs`
- Test: `crates/busytok-subagent/src/manager.rs` (inline `#[cfg(test)]` module or `tests/` file — match existing convention)

**Interfaces:**
- Consumes: `Database::subagent_list_recent_tasks_all`, `Database::subagent_count_tasks_by_subagent`, `Database::subagent_last_task_by_subagent` (Task 1)
- Produces: `SubagentManager::recent_tasks_all(limit) -> Result<Vec<SubagentTaskSummary>>`, `SubagentManager::task_counts_by_subagent() -> Result<HashMap<String, u32>>`, `SubagentManager::last_task_by_subagent() -> Result<HashMap<String, (i64, String)>>`
- Cross-task note: Task 6 Step 2 adds `SubagentManager::runtime_status_snapshot(recent_limit) -> Result<RuntimeStatusSnapshot>` to this same file — a combined method that performs all 4 DB reads under one lock to preserve single-snapshot semantics (spec §4 line 213). The 3 individual methods above are still used by the snapshot internally.

- [ ] **Step 1: Write failing tests for the three new manager methods**

Add tests in the existing test module of `crates/busytok-subagent/src/manager.rs` (or `tests/manager_phase2.rs` if the existing tests are in a separate file — check the convention first). The tests should seed an in-memory DB via `Database::open_in_memory()`, construct a `SubagentManager` with a test executor, and verify the three methods.

```rust
#[tokio::test]
async fn recent_tasks_all_returns_across_all_subagents() {
    // SharedDb = Arc<std::sync::Mutex<Database>> (NOT tokio::sync::Mutex)
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    // seed subagents + tasks using db.lock().unwrap().execute(...)
    // (sync lock, NOT .await — see existing SubagentManager tests for the pattern)
    {
        let conn = db.lock().unwrap();
        // ... seed sub-a, sub-b, t1/t2/t3 using raw SQL (same as Task 1 tests)
    }
    let manager = SubagentManager::new_for_test(db.clone());
    let tasks = manager.recent_tasks_all(20).await.unwrap();
    assert_eq!(tasks.len(), 3);
    assert_eq!(tasks[0].id, "t3"); // desc order
}

#[tokio::test]
async fn task_counts_by_subagent_groups_correctly() {
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    // ... seed sub-a (2 tasks), sub-b (1 task) using db.lock().unwrap()
    let manager = SubagentManager::new_for_test(db.clone());
    let counts = manager.task_counts_by_subagent().await.unwrap();
    assert_eq!(counts.get("sub-a"), Some(&2));
    assert_eq!(counts.get("sub-b"), Some(&1));
}

#[tokio::test]
async fn last_task_by_subagent_returns_latest_per_subagent() {
    let db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory().unwrap()));
    // ... seed sub-a with t1 (1000ms, completed) + t2 (2000ms, failed)
    let manager = SubagentManager::new_for_test(db.clone());
    let lasts = manager.last_task_by_subagent().await.unwrap();
    let (created_at, status) = lasts.get("sub-a").unwrap();
    assert_eq!(*created_at, 2000);
    assert_eq!(status, "failed");
}
```

Note: `SharedDb = Arc<std::sync::Mutex<Database>>` (manager.rs:23) — use `std::sync::Mutex`, NOT `tokio::sync::Mutex`. The manager methods use `self.db.lock().await` because `SharedDb` is wrapped in a way that supports async — check the actual `self.db.lock()` call in the existing `tasks` method (manager.rs:673) for the exact pattern and replicate it. `SubagentManager::new_for_test` may not exist — check the existing test setup in manager.rs. If tests use a different construction helper (e.g. `SubagentManager::with_test_executor`), use that. The key is to reuse the existing test construction pattern, not invent a new one.

Run: `cargo test -p busytok-subagent recent_tasks_all`
Expected: FAIL — methods not defined.

- [ ] **Step 2: Implement `recent_tasks_all`**

Add to `crates/busytok-subagent/src/manager.rs` after the existing `tasks` method (~line 673):

```rust
/// Returns the most recent tasks across ALL subagents (spec §4 Phase 2 tasks_recent).
pub async fn recent_tasks_all(&self, limit: i64) -> Result<Vec<SubagentTaskSummary>> {
    let db = self.db.lock().await;
    let rows = db.subagent_list_recent_tasks_all(limit)?;
    Ok(rows.into_iter().map(task_row_to_summary).collect())
}
```

Note: `task_row_to_summary` is the existing mapping function used by the `tasks` method — find it (likely in manager.rs near the `tasks` impl) and reuse it. If it's a free function, call it directly. If it's a method, extract or reuse.

- [ ] **Step 3: Implement `task_counts_by_subagent`**

Add to `crates/busytok-subagent/src/manager.rs`:

```rust
/// Returns a map of subagent_id → task_count (spec §4 Phase 2 subagents[].task_count).
pub async fn task_counts_by_subagent(&self) -> Result<std::collections::HashMap<String, u32>> {
    let db = self.db.lock().await;
    let counts = db.subagent_count_tasks_by_subagent()?;
    Ok(counts.into_iter().collect())
}
```

- [ ] **Step 4: Implement `last_task_by_subagent`**

Add to `crates/busytok-subagent/src/manager.rs`:

```rust
/// Returns a map of subagent_id → (created_at_ms, status) for each subagent's latest task.
pub async fn last_task_by_subagent(&self) -> Result<std::collections::HashMap<String, (i64, String)>> {
    let db = self.db.lock().await;
    let lasts = db.subagent_last_task_by_subagent()?;
    Ok(lasts.into_iter().collect())
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p busytok-subagent recent_tasks_all && cargo test -p busytok-subagent task_counts_by_subagent && cargo test -p busytok-subagent last_task_by_subagent`
Expected: PASS (3 tests)

- [ ] **Step 6: Commit**

```bash
git add crates/busytok-subagent/src/manager.rs
git commit -m "feat(subagent): add SubagentManager aggregate methods (recent_tasks_all, task_counts_by_subagent, last_task_by_subagent)"
```

---

## Task 3: PiSidecarSupervisor state exposure (spawned_at, latest sample, worker snapshot)

**Files:**
- Modify: `crates/busytok-subagent/src/sidecar/supervisor.rs`
- Test: `crates/busytok-subagent/src/sidecar/supervisor.rs` (inline `#[cfg(test)]` module or `tests/supervisor_phase2.rs`)

**Interfaces:**
- Produces: `PiSidecarSupervisor::worker_snapshot() -> Option<WorkerSnapshot>` where `WorkerSnapshot` is a new struct with `state: WorkerState` (running/stopped), `pid: Option<u32>`, `uptime_seconds: Option<u64>`, `hot_sessions: u32`, `memory_used_pct: Option<u32>`, `pressure_level: PressureLevel`

- [ ] **Step 1: Add `spawned_at` and caches to `SupervisorState`**

In `crates/busytok-subagent/src/sidecar/supervisor.rs`, modify the `SupervisorState` struct (~line 78-104):

```rust
pub struct SupervisorState {
    child: Option<Child>,
    client: Option<Arc<Mutex<SidecarRpcClient>>>,
    last_activity: tokio::time::Instant,
    restart_attempts: u32,
    supervision_started: bool,
    generation: u64,
    resource_pressure_state: ResourcePressureState,
    pub restart_history: VecDeque<tokio::time::Instant>,
    // ── Phase 2 monitoring state ──────────────────────────────
    spawned_at: Option<tokio::time::Instant>,
    latest_sample: Option<ResourceSample>,
    latest_hot_sessions: u32,
}
```

Update the `Default` impl for `SupervisorState` (or the constructor where it's initialized) to set:
```rust
spawned_at: None,
latest_sample: None,
latest_hot_sessions: 0,
```

- [ ] **Step 2: Set `spawned_at` in `spawn_internal`**

In `spawn_internal` (~line 506-527), after the child is successfully spawned, set `spawned_at`:

```rust
// Inside spawn_internal, after `state.child = Some(child);`
state.spawned_at = Some(tokio::time::Instant::now());
```

And clear it on shutdown (in `shutdown_internal` ~line 225, set `state.spawned_at = None`).

- [ ] **Step 3: Cache `latest_sample` and `latest_hot_sessions` in `maybe_sample_resources`**

In `maybe_sample_resources` (~line 695-839), after the sample is computed, cache it:

```rust
// After `let sample = self.resource_monitor...sample(...)` (existing code)
{
    let mut state = self.state.lock().await;
    state.latest_sample = Some(sample.clone());
    state.latest_hot_sessions = hot_session_count; // the value passed to sample()
}
```

Note: `ResourceSample` must be `Clone` — verify at resource.rs:23 (it derives Clone already per the research).

- [ ] **Step 4: Define `WorkerSnapshot` + `WorkerState` + `PressureLevel` types**

Add near the top of `crates/busytok-subagent/src/sidecar/supervisor.rs` (or in a new `monitoring.rs` submodule if the file is already large — but prefer keeping it in supervisor.rs for simplicity):

```rust
/// Read-only snapshot of a sidecar worker's state (spec §4 Phase 2 workers[]).
#[derive(Debug, Clone)]
pub struct WorkerSnapshot {
    pub state: WorkerState,
    pub pid: Option<u32>,
    pub uptime_seconds: Option<u64>,
    pub hot_sessions: u32,
    pub memory_used_pct: Option<u32>,
    pub pressure_level: PressureLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    Running,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureLevel {
    Normal,
    Throttled,
    Evicting,
    Restarting,
}
```

**IMPORTANT — re-export from `sidecar/mod.rs`:** After defining these types, add re-exports to `crates/busytok-subagent/src/sidecar/mod.rs` (line 11) so they're reachable as `busytok_subagent::sidecar::WorkerSnapshot` etc.:

```rust
pub use supervisor::{PiSidecarSupervisor, SharedDb, SidecarHandle, WorkerSnapshot, WorkerState, PressureLevel};
```

- [ ] **Step 5: Implement `worker_snapshot()`**

Add a public method on `PiSidecarSupervisor`:

```rust
/// Returns a snapshot of the worker's current state for monitoring (spec §4 Phase 2).
/// Returns None if the supervisor was never configured (no resource monitor).
pub async fn worker_snapshot(&self) -> Option<WorkerSnapshot> {
    let state = self.state.lock().await;
    let is_running = state.child.as_ref().map(|c| c.id().is_some()).unwrap_or(false);
    let worker_state = if is_running { WorkerState::Running } else { WorkerState::Stopped };
    let pid = state.child.as_ref().and_then(|c| c.id());
    let uptime_seconds = state.spawned_at.map(|t| t.elapsed().as_secs());
    let hot_sessions = state.latest_hot_sessions;
    let memory_used_pct = state.latest_sample.as_ref().map(|s| {
        // system_available_mb is in MB; total memory comes from sysinfo
        // But we don't have total_memory cached. Use the resource_monitor if available.
        // Fallback: compute from the sample's system_available_mb if we can get total.
        // For Phase 2, if we can't compute, return None.
        // See Step 6 for the full computation.
        0u32 // placeholder, replaced in Step 6
    });
    let pressure_level = match state.resource_pressure_state {
        ResourcePressureState::Normal => PressureLevel::Normal,
        ResourcePressureState::Pressure => {
            // Check if the last pressure action was Hibernate (evicting)
            // vs PauseOnly/GracefulRestart (throttled).
            if let Some(gate) = &self.pressure_gate {
                match gate.last_action() {
                    Some(busytok_subagent::PressureAction::Hibernate) => PressureLevel::Evicting,
                    _ => PressureLevel::Throttled,
                }
            } else {
                PressureLevel::Throttled
            }
        }
        ResourcePressureState::LimitExceeded => PressureLevel::Restarting,
    };
    Some(WorkerSnapshot {
        state: worker_state,
        pid,
        uptime_seconds,
        hot_sessions,
        memory_used_pct,
        pressure_level,
    })
}
```

Note: the pressure level mapping is nuanced. The spec wants `normal/throttled/evicting/restarting`. The `ResourcePressureState` has `Normal/Pressure/LimitExceeded`. The `PressureGate.last_action()` returns the most recent `PressureAction`. Map as:
- `Normal` state → `Normal`
- `Pressure` state → `Throttled` (new tasks paused)
- `LimitExceeded` state + `GracefulRestart`/`ForceKill` action → `Restarting`
- `Pressure` state + `Hibernate` action → `Evicting`

Simplify the implementation based on what's actually reachable in Phase 2 (the sidecar may not even be running). The key is that `Normal` → `Normal` and any pressure → the appropriate non-normal level.

- [ ] **Step 6: Compute `memory_used_pct` properly**

`ResourceSample.system_available_mb` gives available memory. To compute `used_pct = 100 - (available / total * 100)`, we need total memory. The `ResourceMonitor` owns a `sysinfo::System` (resource.rs:108). Add a method to `ResourceMonitor`:

```rust
// In crates/busytok-subagent/src/resource.rs
impl ResourceMonitor {
    /// Returns the system total memory in MB.
    pub fn total_memory_mb(&self) -> f64 {
        bytes_to_mb(self.system.total_memory())
    }

    /// Returns the latest memory usage percentage (0-100).
    pub fn memory_used_pct(&self) -> Option<u32> {
        let total = self.total_memory_mb();
        if total <= 0.0 { return None; }
        let available = self.system.available_memory();
        let available_mb = bytes_to_mb(available);
        let pct = 100.0 - (available_mb / total * 100.0);
        Some(pct.round() as u32)
    }
}
```

Then in `worker_snapshot()`, read it via the resource_monitor field:

```rust
let memory_used_pct = if let Some(ref monitor) = self.resource_monitor {
    let monitor = monitor.lock().ok()?;
    monitor.memory_used_pct()
} else {
    None
};
```

Note: `bytes_to_mb` is already defined in resource.rs — find it and reuse (do not duplicate).

- [ ] **Step 7: Write tests for `worker_snapshot`**

```rust
#[tokio::test]
async fn worker_snapshot_returns_stopped_when_not_started() {
    let sup = PiSidecarSupervisor::with_resource_policy(/* test config */);
    let snap = sup.worker_snapshot().await;
    assert!(snap.is_some());
    let snap = snap.unwrap();
    assert_eq!(snap.state, WorkerState::Stopped);
    assert!(snap.pid.is_none());
    assert!(snap.uptime_seconds.is_none());
    assert_eq!(snap.hot_sessions, 0);
}

#[tokio::test]
async fn worker_snapshot_pressure_level_normal_by_default() {
    let sup = PiSidecarSupervisor::with_resource_policy(/* test config */);
    let snap = sup.worker_snapshot().await.unwrap();
    assert_eq!(snap.pressure_level, PressureLevel::Normal);
}
```

Note: use the existing test construction pattern for `PiSidecarSupervisor` (check existing tests in supervisor.rs or tests/ directory). The `with_resource_policy` constructor (supervisor.rs:148) takes a `ResourcePolicy` — use the default.

- [ ] **Step 8: Run tests + clippy**

Run: `cargo test -p busytok-subagent worker_snapshot && cargo clippy -p busytok-subagent -- -D warnings`
Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add crates/busytok-subagent/src/sidecar/supervisor.rs crates/busytok-subagent/src/resource.rs
git commit -m "feat(subagent): expose PiSidecarSupervisor worker snapshot (spawned_at, latest_sample, memory_used_pct, pressure_level)"
```

---

## Task 4: Protocol DTOs for `subagent.runtime_status`

**Files:**
- Modify: `crates/busytok-protocol/src/dto.rs`
- Modify: `crates/busytok-protocol/src/methods.rs`
- Modify: `crates/busytok-protocol/src/ts.rs`
- Regenerate: `packages/busytok-protocol-types/src/generated.ts`

**Interfaces:**
- Produces: `SubagentRuntimeStatusRequestDto`, `SubagentRuntimeStatusResponseDto`, `SubagentPressureGateDto`, `SubagentRuntimeSubagentDto`, `SubagentRuntimeTaskDto`, `SubagentWorkerDto`

- [ ] **Step 1: Define the DTOs in `dto.rs`**

Add to `crates/busytok-protocol/src/dto.rs` after the existing subagent DTOs (~line 1387):

```rust
// ---------------------------------------------------------------------------
// Subagent runtime status DTOs (spec §4 Phase 2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize, Serialize, TS)]
pub struct SubagentRuntimeStatusRequestDto {
    /// Reserved for future filtering; Phase 2 ignores this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentPressureGateDto {
    pub level: String,           // "normal" | "throttled" | "evicting" | "restarting"
    pub memory_used_pct: u32,
    pub hot_sessions_total: u32,
    pub hot_sessions_limit: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentRuntimeSubagentDto {
    pub name: String,
    pub status: String,          // "hot" | "warm" | "cold" | "deleted"
    pub task_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_task_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_task_status: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentRuntimeTaskDto {
    pub task_id: String,
    pub subagent_name: String,
    pub status: String,          // "queued" | "running" | "completed" | "failed" | "cancelled"
    pub created_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentWorkerDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub state: String,           // "running" | "stopped" (Phase 3 adds "stale")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<u64>,
    pub hot_sessions: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentRuntimeStatusResponseDto {
    pub pressure_gate: SubagentPressureGateDto,
    pub subagents: Vec<SubagentRuntimeSubagentDto>,
    pub tasks_recent: Vec<SubagentRuntimeTaskDto>,
    pub workers: Vec<SubagentWorkerDto>,
}
```

- [ ] **Step 2: Add `subagent.runtime_status` to method_manifest()**

In `crates/busytok-protocol/src/methods.rs`, add to the subagent block (~line 43-49):

```rust
"subagent.runtime_status".to_string(),
```

Also update the test at methods.rs:59-99 that asserts presence of methods — add `"subagent.runtime_status"` to the expected list.

- [ ] **Step 3: Add `decl()` entries in `ts.rs`**

In `crates/busytok-protocol/src/ts.rs`, add to the `type_defs` vector (~line 192, after the last provider DTO):

```rust
dto::SubagentRuntimeStatusRequestDto::decl(),
dto::SubagentRuntimeStatusResponseDto::decl(),
dto::SubagentPressureGateDto::decl(),
dto::SubagentRuntimeSubagentDto::decl(),
dto::SubagentRuntimeTaskDto::decl(),
dto::SubagentWorkerDto::decl(),
```

- [ ] **Step 4: Regenerate `generated.ts`**

Run: `cargo test -p busytok-protocol generate_typescript_types`
Expected: PASS, `packages/busytok-protocol-types/src/generated.ts` updated with 6 new types.

- [ ] **Step 5: Verify generated types look correct**

Read the end of `packages/busytok-protocol-types/src/generated.ts` and confirm the 6 new types appear with correct field names (snake_case) and Option fields marked optional.

- [ ] **Step 6: Run protocol tests**

Run: `cargo test -p busytok-protocol`
Expected: PASS (including the methods manifest test + ts generation test)

- [ ] **Step 7: Commit**

```bash
git add crates/busytok-protocol/src/dto.rs crates/busytok-protocol/src/methods.rs crates/busytok-protocol/src/ts.rs packages/busytok-protocol-types/src/generated.ts
git commit -m "feat(protocol): add subagent.runtime_status DTOs (SubagentRuntimeStatusResponseDto + 5 nested DTOs)"
```

---

## Task 5: RuntimeControl trait wiring (6 sites)

**Files:**
- Modify: `crates/busytok-control/src/dispatch.rs` (4 sites: trait def, dispatch arm, TestRuntimeControl, Arc<T> blanket)
- Modify: `crates/busytok-control/tests/server.rs` (MethodDispatchErrorRuntime)
- Modify: `apps/cli/tests/prompt.rs` (AliasConflictRuntime)

**Interfaces:**
- Consumes: `SubagentRuntimeStatusRequestDto`, `SubagentRuntimeStatusResponseDto` (Task 4)
- Produces: `RuntimeControl::subagent_runtime_status` trait method available for implementation

- [ ] **Step 1: Add trait method to `RuntimeControl`**

In `crates/busytok-control/src/dispatch.rs` ~line 195 (after `subagent_delete`), add:

```rust
async fn subagent_runtime_status(
    &self,
    req: busytok_protocol::dto::SubagentRuntimeStatusRequestDto,
) -> Result<busytok_protocol::dto::SubagentRuntimeStatusResponseDto>;
```

- [ ] **Step 2: Add dispatch arm**

In `crates/busytok-control/src/dispatch.rs` ~line 503 (after the `subagent.delete` arm), add:

```rust
"subagent.runtime_status" => {
    let req: SubagentRuntimeStatusRequestDto = serde_json::from_value(request.params)
        .map_err(|e| anyhow::anyhow!("invalid params for subagent.runtime_status: {e}"))?;
    let dto = self.runtime.subagent_runtime_status(req).await?;
    ControlResponse::ok(serde_json::to_value(dto)?)
}
```

Note: import `SubagentRuntimeStatusRequestDto` at the top of dispatch.rs alongside the other DTO imports.

- [ ] **Step 3: Add stub to `TestRuntimeControl`**

In `crates/busytok-control/src/dispatch.rs` ~line 1166 (after the `provider_test_connection` stub), add:

```rust
async fn subagent_runtime_status(
    &self,
    _req: busytok_protocol::dto::SubagentRuntimeStatusRequestDto,
) -> Result<busytok_protocol::dto::SubagentRuntimeStatusResponseDto> {
    Ok(busytok_protocol::dto::SubagentRuntimeStatusResponseDto {
        pressure_gate: busytok_protocol::dto::SubagentPressureGateDto {
            level: "normal".to_string(),
            memory_used_pct: 0,
            hot_sessions_total: 0,
            hot_sessions_limit: 3,
        },
        subagents: Vec::new(),
        tasks_recent: Vec::new(),
        workers: Vec::new(),
    })
}
```

- [ ] **Step 4: Add forwarding to `Arc<T>` blanket impl**

In `crates/busytok-control/src/dispatch.rs` ~line 1370 (after the `provider_test_connection` forwarding), add:

```rust
async fn subagent_runtime_status(
    &self,
    req: busytok_protocol::dto::SubagentRuntimeStatusRequestDto,
) -> Result<busytok_protocol::dto::SubagentRuntimeStatusResponseDto> {
    (**self).subagent_runtime_status(req).await
}
```

- [ ] **Step 5: Add to `MethodDispatchErrorRuntime`**

In `crates/busytok-control/tests/server.rs` ~line 87, add to the `RuntimeControl` impl:

```rust
async fn subagent_runtime_status(
    &self,
    _req: busytok_protocol::dto::SubagentRuntimeStatusRequestDto,
) -> Result<busytok_protocol::dto::SubagentRuntimeStatusResponseDto> {
    self.inner.subagent_runtime_status(_req).await
}
```

Note: match the existing forwarding pattern used by other methods in this impl — check how `provider_test_connection` forwards.

- [ ] **Step 6: Add to `AliasConflictRuntime`**

In `apps/cli/tests/prompt.rs` ~line 297, add to the `RuntimeControl` impl:

```rust
async fn subagent_runtime_status(
    &self,
    _req: busytok_protocol::dto::SubagentRuntimeStatusRequestDto,
) -> Result<busytok_protocol::dto::SubagentRuntimeStatusResponseDto> {
    self.inner.subagent_runtime_status(_req).await
}
```

Note: match the existing forwarding pattern. If `AliasConflictRuntime` doesn't forward to `inner`, adapt to whatever pattern it uses.

- [ ] **Step 7: Verify compilation**

Run: `cargo build --workspace --exclude busytok-gui`
Expected: compiles (the BusytokSupervisor impl is added in Task 6; for now it will fail because BusytokSupervisor doesn't implement the new method yet — see Task 6 Step 1 for the real impl. To unblock compilation here, add a temporary `todo!()` stub in supervisor.rs, OR do Task 6 Step 1 first. Recommended: implement the handler in Task 6 immediately after this task, without committing in between.)

**IMPORTANT:** Tasks 5 and 6 must be done together to keep the tree compiling. Do Task 5 steps 1-6, then Task 6 Step 1 (handler stub), verify compilation, then complete Task 6 implementation, then commit once for both.

- [ ] **Step 8: Commit (after Task 6 Step 1 makes it compile)**

This commit will be combined with Task 6.

---

## Task 6: BusytokSupervisor `subagent_runtime_status` handler implementation

**Files:**
- Modify: `crates/busytok-runtime/src/supervisor.rs`
- Test: `crates/busytok-runtime/tests/supervisor_control.rs`

**Interfaces:**
- Consumes: `SubagentManager::list`, `recent_tasks_all`, `task_counts_by_subagent`, `last_task_by_subagent` (Tasks 1-2); `PiSidecarSupervisor::worker_snapshot` (Task 3); `SubagentSettings.max_hot_sessions` (config)
- Produces: working `subagent_runtime_status` RPC handler

- [ ] **Step 1: Add handler stub (to unblock compilation from Task 5)**

In `crates/busytok-runtime/src/supervisor.rs` ~line 4656 (after `subagent_delete`), add:

```rust
async fn subagent_runtime_status(
    &self,
    _req: SubagentRuntimeStatusRequestDto,
) -> Result<SubagentRuntimeStatusResponseDto> {
    todo!("implemented in Step 2")
}
```

Run: `cargo build --workspace --exclude busytok-gui`
Expected: compiles. Now proceed to Step 2 to replace the todo!.

- [ ] **Step 2: Implement the handler**

Replace the stub with the full implementation. **Single-snapshot semantics (spec line 213):** all DB reads occur under one `SubagentManager` lock acquisition — add a `runtime_status_snapshot()` method to `SubagentManager` (Task 2) that performs all 4 reads under a single DB lock and returns a combined struct. This avoids 4 separate lock acquisitions that could observe inconsistent state.

First, add a combined snapshot method to `SubagentManager` (in `crates/busytok-subagent/src/manager.rs`, Task 2):

```rust
/// Combined snapshot for subagent.runtime_status — all reads under one DB lock
/// to preserve single-snapshot semantics (spec §4 line 213).
pub async fn runtime_status_snapshot(&self, recent_limit: i64) -> Result<RuntimeStatusSnapshot> {
    let db = self.db.lock().await;
    let subs = db.subagent_list_filtered(None, None, false)?;
    let task_counts = db.subagent_count_tasks_by_subagent()?;
    let last_tasks = db.subagent_last_task_by_subagent()?;
    let recent_tasks = db.subagent_list_recent_tasks_all(recent_limit)?;
    Ok(RuntimeStatusSnapshot {
        subagents: subs,
        task_counts: task_counts.into_iter().collect(),
        last_tasks: last_tasks.into_iter().collect(),
        recent_tasks: recent_tasks.into_iter().map(task_row_to_summary).collect(),
    })
}

pub struct RuntimeStatusSnapshot {
    pub subagents: Vec<LogicalSubagent>,
    pub task_counts: std::collections::HashMap<String, u32>,
    pub last_tasks: std::collections::HashMap<String, (i64, String)>,
    pub recent_tasks: Vec<SubagentTaskSummary>,
}
```

Note: the existing `list()` method calls `db.subagent_list_filtered(...)` then maps rows to `LogicalSubagent` via `row_to_model`. For the snapshot, you may need to call the row-mapping inline (or expose a `list_raw` method). The simplest approach: call the existing `subagent_list_filtered` + `row_to_model` within `runtime_status_snapshot`. Check how `list()` (manager.rs:655) does it and replicate the mapping.

Then the handler in `BusytokSupervisor`:

```rust
async fn subagent_runtime_status(
    &self,
    _req: SubagentRuntimeStatusRequestDto,
) -> Result<SubagentRuntimeStatusResponseDto> {
    // 1. Snapshot worker state (single SupervisorState lock, fast, in-memory)
    let worker_opt = if let Some(sup) = &self.sidecar_supervisor {
        sup.worker_snapshot().await
    } else {
        None
    };

    // 2. Read settings for hot_sessions_limit
    let settings = self.settings.lock().unwrap();
    let hot_sessions_limit = settings.subagent.pi_sidecar.max_hot_sessions;
    drop(settings);

    // 3. Build pressure_gate DTO
    let pressure_gate = if let Some(ref snap) = worker_opt {
        let level = match snap.pressure_level {
            busytok_subagent::sidecar::PressureLevel::Normal => "normal",
            busytok_subagent::sidecar::PressureLevel::Throttled => "throttled",
            busytok_subagent::sidecar::PressureLevel::Evicting => "evicting",
            busytok_subagent::sidecar::PressureLevel::Restarting => "restarting",
        };
        SubagentPressureGateDto {
            level: level.to_string(),
            memory_used_pct: snap.memory_used_pct.unwrap_or(0),
            hot_sessions_total: snap.hot_sessions,
            hot_sessions_limit,
        }
    } else {
        SubagentPressureGateDto {
            level: "normal".to_string(),
            memory_used_pct: 0,
            hot_sessions_total: 0,
            hot_sessions_limit,
        }
    };

    // 4. Build workers DTO (spec line 218: workers[] may be empty pre-Phase-3)
    let workers: Vec<SubagentWorkerDto> = if let Some(snap) = worker_opt {
        vec![SubagentWorkerDto {
            provider_id: None, // Phase 2: no provider binding
            state: match snap.state {
                busytok_subagent::sidecar::WorkerState::Running => "running",
                busytok_subagent::sidecar::WorkerState::Stopped => "stopped",
            }.to_string(),
            pid: snap.pid,
            uptime_seconds: snap.uptime_seconds,
            hot_sessions: snap.hot_sessions,
        }]
    } else {
        Vec::new()
    };

    // 5. Single-snapshot DB read (one lock, all 4 queries — spec §4 line 213)
    let snapshot = self.subagent_manager
        .runtime_status_snapshot(20)
        .await
        .map_err(map_subagent_error)?;

    // 6. Build subagents DTO (exclude deleted — list() with include_deleted=false already does this)
    let subagents: Vec<SubagentRuntimeSubagentDto> = snapshot.subagents.iter()
        .map(|s| {
            let task_count = snapshot.task_counts.get(&s.id).copied().unwrap_or(0);
            let last_task = snapshot.last_tasks.get(&s.id);
            SubagentRuntimeSubagentDto {
                name: s.name.clone(),
                status: s.status.as_str().to_string(),
                task_count,
                last_task_at_ms: last_task.map(|(ts, _)| *ts),
                last_task_status: last_task.map(|(_, st)| st.clone()),
            }
        })
        .collect();

    // 7. Build tasks_recent DTO (subagent_name resolved via name lookup)
    let name_lookup: std::collections::HashMap<String, String> = snapshot.subagents.iter()
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();

    let tasks_recent: Vec<SubagentRuntimeTaskDto> = snapshot.recent_tasks.iter()
        .map(|t| SubagentRuntimeTaskDto {
            task_id: t.id.clone(),
            subagent_name: name_lookup.get(&t.subagent_id).cloned().unwrap_or_else(|| t.subagent_id.clone()),
            status: t.status.as_str().to_string(), // TaskStatus enum → &str → String
            created_at_ms: t.created_at_ms,
            error: t.error.clone(),
        })
        .collect();

    tracing::debug!(
        event_code = "subagent.runtime_status_served",
        subagent_count = subagents.len(),
        task_count = tasks_recent.len(),
        worker_count = workers.len(),
        "served subagent.runtime_status"
    );

    Ok(SubagentRuntimeStatusResponseDto {
        pressure_gate,
        subagents,
        tasks_recent,
        workers,
    })
}
```

Note: `SubagentTaskSummary` (models.rs:113-123) has `status: TaskStatus` (enum), NOT `String`. Use `t.status.as_str().to_string()` to convert. The `as_str()` method is at models.rs:57.

- [ ] **Step 3: Write tests for the handler**

Add to `crates/busytok-runtime/tests/supervisor_control.rs`:

```rust
#[tokio::test]
async fn subagent_runtime_status_returns_empty_when_no_data() {
    let sup = test_supervisor().await;
    let result = sup.subagent_runtime_status(SubagentRuntimeStatusRequestDto::default()).await;
    assert!(result.is_ok());
    let resp = result.unwrap();
    assert_eq!(resp.pressure_gate.level, "normal");
    assert_eq!(resp.pressure_gate.hot_sessions_limit, 3); // default
    assert!(resp.subagents.is_empty());
    assert!(resp.tasks_recent.is_empty());
    // workers may be empty if no sidecar configured in test env
}

#[tokio::test]
async fn subagent_runtime_status_includes_subagents_with_task_counts() {
    let sup = test_supervisor().await;
    // Seed: sub-a with 2 tasks, sub-b with 1 task
    // Use the existing test seeding pattern (check test_supervisor helper)
    seed_subagent(&sup, "sub-a").await;
    seed_subagent(&sup, "sub-b").await;
    seed_task(&sup, "t1", "sub-a", "completed", 1000).await;
    seed_task(&sup, "t2", "sub-a", "failed", 2000).await;
    seed_task(&sup, "t3", "sub-b", "completed", 3000).await;

    let resp = sup.subagent_runtime_status(SubagentRuntimeStatusRequestDto::default()).await.unwrap();
    assert_eq!(resp.subagents.len(), 2);
    let sub_a = resp.subagents.iter().find(|s| s.name == "sub-a").unwrap();
    assert_eq!(sub_a.task_count, 2);
    assert_eq!(sub_a.last_task_status.as_deref(), Some("failed"));
    let sub_b = resp.subagents.iter().find(|s| s.name == "sub-b").unwrap();
    assert_eq!(sub_b.task_count, 1);
}

#[tokio::test]
async fn subagent_runtime_status_tasks_recent_ordered_desc() {
    let sup = test_supervisor().await;
    seed_subagent(&sup, "sub-a").await;
    seed_task(&sup, "t1", "sub-a", "completed", 1000).await;
    seed_task(&sup, "t2", "sub-a", "completed", 3000).await;
    seed_task(&sup, "t3", "sub-a", "completed", 2000).await;

    let resp = sup.subagent_runtime_status(SubagentRuntimeStatusRequestDto::default()).await.unwrap();
    assert_eq!(resp.tasks_recent.len(), 3);
    assert_eq!(resp.tasks_recent[0].task_id, "t2"); // 3000ms
    assert_eq!(resp.tasks_recent[1].task_id, "t3"); // 2000ms
    assert_eq!(resp.tasks_recent[2].task_id, "t1"); // 1000ms
}

#[tokio::test]
async fn subagent_runtime_status_excludes_deleted_subagents() {
    let sup = test_supervisor().await;
    seed_subagent_with_status(&sup, "sub-a", "warm").await;
    seed_subagent_with_status(&sup, "sub-deleted", "deleted").await;

    let resp = sup.subagent_runtime_status(SubagentRuntimeStatusRequestDto::default()).await.unwrap();
    assert_eq!(resp.subagents.len(), 1);
    assert_eq!(resp.subagents[0].name, "sub-a");
}

#[tokio::test]
async fn subagent_runtime_status_tasks_recent_includes_subagent_name() {
    let sup = test_supervisor().await;
    seed_subagent(&sup, "my-agent").await;
    seed_task(&sup, "t1", "my-agent", "completed", 1000).await;

    let resp = sup.subagent_runtime_status(SubagentRuntimeStatusRequestDto::default()).await.unwrap();
    assert_eq!(resp.tasks_recent.len(), 1);
    assert_eq!(resp.tasks_recent[0].subagent_name, "my-agent");
}
```

Note: `test_supervisor()`, `seed_subagent()`, `seed_task()` are test helpers — check the existing tests in supervisor_control.rs for the established pattern and reuse/adapt. If they don't exist, build minimal versions using the DB handle.

- [ ] **Step 4: Run tests**

Run: `cargo test -p busytok-runtime --test supervisor_control subagent_runtime_status`
Expected: PASS (5 tests)

- [ ] **Step 5: Commit (combines Tasks 5 + 6)**

```bash
git add crates/busytok-control/src/dispatch.rs crates/busytok-control/tests/server.rs apps/cli/tests/prompt.rs crates/busytok-runtime/src/supervisor.rs crates/busytok-runtime/tests/supervisor_control.rs
git commit -m "feat(runtime): implement subagent.runtime_status RPC (trait wiring 6 sites + handler + tests)"
```

---

## Task 7: Frontend client method + query hook

**Files:**
- Modify: `apps/gui/src/api/busytokClient.ts`
- Modify: `apps/gui/src/api/queryKeys.ts`
- Modify: `apps/gui/src/api/useBusytokData.ts`

**Interfaces:**
- Produces: `subagentRuntimeStatus()` client method, `useSubagentRuntimeStatus()` hook with 5s polling

- [ ] **Step 1: Add client method**

In `apps/gui/src/api/busytokClient.ts`, add after the provider methods (~line 182):

```typescript
subagentRuntimeStatus: () =>
  call<SubagentRuntimeStatusResponseDto>("subagent.runtime_status"),
```

Add the type import at the top of the file (alongside existing `@busytok/protocol-types` imports):

```typescript
import type {
  // ... existing imports ...
  SubagentRuntimeStatusResponseDto,
} from "@busytok/protocol-types";
```

- [ ] **Step 2: Add query key**

In `apps/gui/src/api/queryKeys.ts`, add after `providers` (~line 49):

```typescript
subagentRuntimeStatus: () => ["subagents", "runtime_status"] as const,
```

- [ ] **Step 3: Add `useSubagentRuntimeStatus` hook with 5s polling**

In `apps/gui/src/api/useBusytokData.ts`, add after `useProviders` (~line 377):

```typescript
const SUBAGENT_REFETCH_MS = 5_000;

export function useSubagentRuntimeStatus() {
  const client = useBusytokClient();
  return useQuery({
    queryKey: queryKeys.subagentRuntimeStatus(),
    queryFn: () => client.subagentRuntimeStatus(),
    refetchInterval: SUBAGENT_REFETCH_MS,
    refetchIntervalInBackground: false,
  });
}
```

Note: import `useQuery` (already imported) and `SubagentRuntimeStatusResponseDto` type if needed for explicit typing (TanStack Query infers it from the queryFn).

- [ ] **Step 4: Run typecheck**

Run: `cd apps/gui && pnpm typecheck`
Expected: clean

- [ ] **Step 5: Commit**

```bash
git add apps/gui/src/api/busytokClient.ts apps/gui/src/api/queryKeys.ts apps/gui/src/api/useBusytokData.ts
git commit -m "feat(gui): add subagentRuntimeStatus client method + useSubagentRuntimeStatus hook (5s poll)"
```

---

## Task 8: SubagentsPage component

**Files:**
- Create: `apps/gui/src/pages/SubagentsPage.tsx`
- Create: `apps/gui/src/pages/SubagentsPage.test.tsx`

**Interfaces:**
- Consumes: `useSubagentRuntimeStatus` (Task 7), `PageState`, `SettingsRow`, `SettingsValue`, `SettingsActionGroup`, `reportFrontendEventSafely`
- Produces: read-only monitoring page with pressure summary + subagents table + task history + workers section

- [ ] **Step 1: Write failing tests**

Create `apps/gui/src/pages/SubagentsPage.test.tsx`:

```typescript
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { SubagentRuntimeStatusResponseDto } from "@busytok/protocol-types";

vi.mock("../api/useBusytokData", () => ({
  useSubagentRuntimeStatus: vi.fn(),
}));

import { useSubagentRuntimeStatus } from "../api/useBusytokData";
import { SubagentsPage } from "./SubagentsPage";

const mockUseStatus = vi.mocked(useSubagentRuntimeStatus);

function makeResponse(overrides: Partial<SubagentRuntimeStatusResponseDto> = {}): SubagentRuntimeStatusResponseDto {
  return {
    pressure_gate: { level: "normal", memory_used_pct: 30, hot_sessions_total: 1, hot_sessions_limit: 3 },
    subagents: [],
    tasks_recent: [],
    workers: [],
    ...overrides,
  };
}

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <SubagentsPage />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mockUseStatus.mockReturnValue({ data: makeResponse(), isLoading: false, isError: false, isFetching: false });
});

afterEach(() => cleanup());

describe("SubagentsPage", () => {
  it("renders pressure summary", () => {
    renderPage();
    expect(screen.getByText(/pressure/i)).toBeTruthy();
    expect(screen.getByText(/normal/i)).toBeTruthy();
    expect(screen.getByText(/30%/)).toBeTruthy();
  });

  it("renders subagents section", () => {
    mockUseStatus.mockReturnValue({
      data: makeResponse({
        subagents: [{
          name: "my-agent",
          status: "warm",
          task_count: 5,
          last_task_at_ms: 1000,
          last_task_status: "completed",
        }],
      }),
      isLoading: false, isError: false, isFetching: false,
    });
    renderPage();
    expect(screen.getByText("my-agent")).toBeTruthy();
    expect(screen.getByText("warm")).toBeTruthy();
    expect(screen.getByText("5")).toBeTruthy();
  });

  it("renders empty state when no subagents", () => {
    renderPage();
    expect(screen.getByText(/no subagents/i)).toBeTruthy();
  });

  it("renders task history section", () => {
    mockUseStatus.mockReturnValue({
      data: makeResponse({
        tasks_recent: [{
          task_id: "t1",
          subagent_name: "my-agent",
          status: "completed",
          created_at_ms: 1000,
          error: null,
        }],
      }),
      isLoading: false, isError: false, isFetching: false,
    });
    renderPage();
    expect(screen.getByText("t1")).toBeTruthy();
    expect(screen.getByText("my-agent")).toBeTruthy();
    expect(screen.getByText("completed")).toBeTruthy();
  });

  it("renders workers section with running worker", () => {
    mockUseStatus.mockReturnValue({
      data: makeResponse({
        workers: [{
          provider_id: null,
          state: "running",
          pid: 12345,
          uptime_seconds: 60,
          hot_sessions: 2,
        }],
      }),
      isLoading: false, isError: false, isFetching: false,
    });
    renderPage();
    expect(screen.getByText(/12345/)).toBeTruthy();
    expect(screen.getByText("running")).toBeTruthy();
  });

  it("renders empty workers state", () => {
    renderPage();
    expect(screen.getByText(/no active sidecar workers/i)).toBeTruthy();
  });

  it("shows loading state", () => {
    mockUseStatus.mockReturnValue({ data: undefined, isLoading: true, isError: false, isFetching: false });
    renderPage();
    expect(screen.getByText(/loading/i)).toBeTruthy();
  });

  it("shows error state", () => {
    mockUseStatus.mockReturnValue({ data: undefined, isLoading: false, isError: true, isFetching: false });
    renderPage();
    expect(screen.getByText(/error/i)).toBeTruthy();
  });

  it("shows pressure warning when throttled", () => {
    mockUseStatus.mockReturnValue({
      data: makeResponse({
        pressure_gate: { level: "throttled", memory_used_pct: 85, hot_sessions_total: 3, hot_sessions_limit: 3 },
      }),
      isLoading: false, isError: false, isFetching: false,
    });
    renderPage();
    expect(screen.getByText("throttled")).toBeTruthy();
  });

  it("does NOT render any action buttons (read-only)", () => {
    mockUseStatus.mockReturnValue({
      data: makeResponse({
        subagents: [{ name: "a", status: "warm", task_count: 0, last_task_at_ms: null, last_task_status: null }],
      }),
      isLoading: false, isError: false, isFetching: false,
    });
    renderPage();
    // No hibernate/delete/retry buttons
    expect(screen.queryByRole("button", { name: /hibernate/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /delete/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /retry/i })).toBeNull();
  });
});
```

Run: `cd apps/gui && pnpm exec vitest run src/pages/SubagentsPage.test.tsx`
Expected: FAIL (component not defined)

- [ ] **Step 2: Implement `SubagentsPage`**

Create `apps/gui/src/pages/SubagentsPage.tsx`:

```typescript
import { useEffect } from "react";
import type { SubagentRuntimeStatusResponseDto } from "@busytok/protocol-types";
import { useSubagentRuntimeStatus } from "../api/useBusytokData";
import { PageState } from "../components/PageState";
import { SettingsRow } from "../components/desktop/SettingsRow";
import { SettingsValue } from "../components/desktop/SettingsValue";
import { SettingsActionGroup } from "../components/desktop/SettingsActionGroup";
import { reportFrontendEventSafely } from "../logging/safeReporter";

function formatUptime(seconds: number | null | undefined): string {
  if (seconds == null) return "—";
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  if (m < 60) return `${m}m ${s}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

function formatTimestamp(ms: number | null | undefined): string {
  if (ms == null) return "—";
  return new Date(ms).toLocaleTimeString();
}

export function SubagentsPage() {
  const { data, isLoading, isError } = useSubagentRuntimeStatus();

  useEffect(() => {
    reportFrontendEventSafely({
      level: "INFO",
      event_code: "subagent.page_viewed",
      message: "Subagents monitoring page viewed",
    });
  }, []);

  if (isLoading) {
    return <PageState kind="loading" title="Loading" message="Fetching subagent runtime status…" />;
  }
  if (isError || !data) {
    return <PageState kind="error" title="Error" message="Failed to load subagent runtime status." />;
  }

  const pressure = data.pressure_gate;
  const pressureTone = pressure.level === "normal" ? "default" : "warning";

  return (
    <div className="settings-page">
      <div className="settings-pane">
        {/* Pressure summary */}
        <section className="settings-section">
          <h2>Pressure Summary</h2>
          <div className="settings-panel">
            <SettingsRow
              label="Pressure level"
              description="System-wide resource pressure state"
              control={<SettingsValue value={pressure.level} tone={pressureTone} />}
            />
            <SettingsRow
              label="Memory used"
              description="System-wide memory usage"
              control={<SettingsValue value={`${pressure.memory_used_pct}%`} />}
            />
            <SettingsRow
              label="Hot sessions"
              description={`${pressure.hot_sessions_total} / ${pressure.hot_sessions_limit} limit`}
              control={<SettingsValue value={`${pressure.hot_sessions_total}`} />}
            />
          </div>
        </section>

        {/* Active subagents */}
        <section className="settings-section">
          <h2>Subagents</h2>
          <div className="settings-panel">
            {data.subagents.length === 0 ? (
              <SettingsRow
                label="No subagents"
                description="No logical subagents have been created yet."
                control={<SettingsValue value="—" tone="muted" />}
              />
            ) : (
              data.subagents.map((s) => (
                <SettingsRow
                  key={s.name}
                  label={s.name}
                  description={`Last task: ${formatTimestamp(s.last_task_at_ms)}${s.last_task_status ? ` (${s.last_task_status})` : ""}`}
                  control={
                    <SettingsActionGroup>
                      <SettingsValue value={s.status} />
                      <SettingsValue value={`${s.task_count} tasks`} tone="muted" />
                    </SettingsActionGroup>
                  }
                />
              ))
            )}
          </div>
        </section>

        {/* Task history */}
        <section className="settings-section">
          <h2>Task History (recent 20)</h2>
          <div className="settings-panel">
            {data.tasks_recent.length === 0 ? (
              <SettingsRow
                label="No tasks"
                description="No subagent tasks have been run yet."
                control={<SettingsValue value="—" tone="muted" />}
              />
            ) : (
              data.tasks_recent.map((t) => (
                <SettingsRow
                  key={t.task_id}
                  label={t.task_id}
                  description={`${t.subagent_name} • ${formatTimestamp(t.created_at_ms)}${t.error ? ` • ${t.error}` : ""}`}
                  control={<SettingsValue value={t.status} />}
                />
              ))
            )}
          </div>
        </section>

        {/* Sidecar workers */}
        <section className="settings-section">
          <h2>Sidecar Workers</h2>
          <div className="settings-panel">
            {data.workers.length === 0 ? (
              <SettingsRow
                label="No active sidecar workers"
                description="The sidecar is not running or not configured."
                control={<SettingsValue value="—" tone="muted" />}
              />
            ) : (
              data.workers.map((w, i) => (
                <SettingsRow
                  key={i}
                  label={w.state}
                  description={`PID: ${w.pid ?? "—"} • Uptime: ${formatUptime(w.uptime_seconds)} • Hot sessions: ${w.hot_sessions}`}
                  control={<SettingsValue value={w.state} />}
                />
              ))
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Run tests + iterate**

Run: `cd apps/gui && pnpm exec vitest run src/pages/SubagentsPage.test.tsx`
Expected: PASS (10 tests)

- [ ] **Step 4: Check coverage**

Run: `cd apps/gui && pnpm exec vitest run src/pages/SubagentsPage.test.tsx --coverage`
Expected: SubagentsPage.tsx coverage ≥90% lines. Add tests for any uncovered branches (e.g., uptime formatting edge cases, error display in task history).

- [ ] **Step 5: Commit**

```bash
git add apps/gui/src/pages/SubagentsPage.tsx apps/gui/src/pages/SubagentsPage.test.tsx
git commit -m "feat(gui): SubagentsPage component (read-only monitoring, pressure/subagents/tasks/workers)"
```

---

## Task 9: Sidebar + routing integration

**Files:**
- Modify: `apps/gui/src/components/AppShell.tsx`
- Modify: `apps/gui/src/components/desktop/Sidebar.tsx`
- Modify: `apps/gui/src/App.tsx`
- Modify: `apps/gui/src/App.test.tsx`
- Modify: `apps/gui/src/components/desktop/Sidebar.test.tsx`
- Modify: `apps/gui/src/components/AppShell.test.tsx`

- [ ] **Step 1: Add `"subagents"` to `DesktopPage` union**

In `apps/gui/src/components/AppShell.tsx` (~line 23-28):

```typescript
export type DesktopPage =
  | "overview" | "usage" | "prompt_palette" | "providers" | "subagents" | "settings";
```

- [ ] **Step 2: Add Subagents to sidebar (Tools group)**

In `apps/gui/src/components/desktop/Sidebar.tsx`, add to the "Tools" group items (spec §4 line 188: "TOOLS group"):

```typescript
import { Activity, BarChart3, Bot, Command, Plug, Settings, type LucideIcon } from "lucide-react";
// ...
const GROUPS: SidebarGroup[] = [
  {
    label: "Monitoring",
    items: [
      { id: "overview", label: "Overview", icon: BarChart3 },
      { id: "usage", label: "Usage", icon: Activity },
    ],
  },
  {
    label: "Tools",
    items: [
      { id: "prompt_palette", label: "Prompt Palette", icon: Command },
      { id: "providers", label: "Providers", icon: Plug },
      { id: "subagents", label: "Subagents", icon: Bot },  // ← new (spec §4: TOOLS group)
    ],
  },
  // ... rest unchanged
];
```

Note: `Bot` is a lucide-react icon that fits "subagent" semantics. Verify it exists in lucide-react (it does).

- [ ] **Step 3: Add route to `App.tsx`**

In `apps/gui/src/App.tsx`:

Add import: `import { SubagentsPage } from "./pages/SubagentsPage";`

Add to `DESKTOP_PAGES`:
```typescript
const DESKTOP_PAGES: readonly DesktopPage[] = [
  "overview", "usage", "prompt_palette", "providers", "subagents", "settings",
];
```

Add to the switch:
```typescript
case "subagents":
  pageContent = <SubagentsPage />;
  break;
```

- [ ] **Step 4: Update test assertions**

In `apps/gui/src/App.test.tsx`: bump sidebar count from 5 to 6, add "Subagents" to labels assertion.
In `apps/gui/src/components/desktop/Sidebar.test.tsx`: add Subagents button assertion.
In `apps/gui/src/components/AppShell.test.tsx`: bump sidebar count from 5 to 6.

- [ ] **Step 5: Run all GUI tests + typecheck**

Run: `cd apps/gui && pnpm exec vitest run && pnpm typecheck`
Expected: all pass

- [ ] **Step 6: Commit**

```bash
git add apps/gui/src/components/AppShell.tsx apps/gui/src/components/desktop/Sidebar.tsx apps/gui/src/App.tsx apps/gui/src/App.test.tsx apps/gui/src/components/desktop/Sidebar.test.tsx apps/gui/src/components/AppShell.test.tsx
git commit -m "feat(gui): add Subagents page to sidebar + routing (Tools group)"
```

---

## Task 10: Full verification gate

**Files:**
- No code changes (verification only, unless a gate fails)

- [ ] **Step 1: Rust verification**

```bash
cargo fmt --all --check
cargo clippy --workspace --exclude busytok-gui --all-targets -- -D warnings
cargo test --workspace --exclude busytok-gui
```

Expected: fmt clean, clippy clean, all tests pass.

- [ ] **Step 2: Frontend verification**

```bash
cd apps/gui
pnpm typecheck
pnpm exec vitest run --coverage
pnpm build
```

Expected: typecheck clean, all tests pass, coverage ≥90% on new files (SubagentsPage.tsx), build succeeds.

- [ ] **Step 3: Verify spec coverage checklist**

- [ ] `subagent.runtime_status` RPC returns single-snapshot aggregate
- [ ] `pressure_gate` has level/memory_used_pct/hot_sessions_total/hot_sessions_limit
- [ ] `subagents[]` has name/status/task_count/last_task_at_ms/last_task_status
- [ ] `tasks_recent` has 20 most recent across ALL subagents, ordered desc, with subagent_name
- [ ] `workers[]` has provider_id (null)/state/pid/uptime_seconds/hot_sessions
- [ ] `workers[]` is empty when sidecar not running (graceful)
- [ ] Page is read-only (no hibernate/delete/retry buttons)
- [ ] Page polls every 5s
- [ ] Subagent rows show logical entities (no pid); worker rows show process entities (no subagent name)
- [ ] Observability: `tracing::debug!` on RPC serve, `reportFrontendEventSafely` on page view

- [ ] **Step 4: Manual smoke test (if possible)**

- Launch the GUI
- Verify "Subagents" appears in the sidebar under Tools
- Click it — page renders with 4 sections (Pressure, Subagents, Tasks, Workers)
- Verify empty states render correctly when no subagents/tasks/workers

- [ ] **Step 5: Final commit (if any verification fixes were needed)**

If all gates pass without fixes, no commit needed. If fixes were made, commit them with a descriptive message.

---

## Self-Review Notes

### Spec coverage
- §4 Phase 2 `subagent.runtime_status` → Tasks 4-6
- §4 Phase 2 GUI page → Tasks 7-9
- §4 Phase 2 constraints (single-snapshot, 20 limit, read-only, empty workers) → Global Constraints + Task 6 implementation
- §2.4 CONTRIBUTING.md → NOT updated in Phase 2 (no new credential invariant; the Phase 1 update stands)

### Architecture decisions
1. **No new polling thread:** the supervision loop already runs every `monitor_interval_seconds` (default 30s) and caches `latest_sample` + `latest_hot_sessions`. The 5s GUI poll reads the cache — no extra work per poll. Trade-off: the cache may be up to 30s stale, acceptable for monitoring.
2. **Worker state is a single lock:** `worker_snapshot()` takes the `SupervisorState` lock once, reads all fields, releases. No async I/O under the lock.
3. **DB reads are separate from worker snapshot:** the handler reads the worker snapshot first (in-memory, fast), then reads DB rows. Single-snapshot semantics are preserved because the DB reads are fast and the worker cache updates infrequently.
4. **No new DB migration:** all data comes from existing tables + in-memory state.
5. **`provider_id: null` in worker DTO:** honest representation — no provider binding exists until Phase 3.

### Implementation risk points
1. **`SubagentTaskSummary` field names:** verify the exact field names in models.rs:113-123 when mapping to `SubagentRuntimeTaskDto` in Task 6.
2. **`task_row_to_summary` mapping function:** the existing `tasks` method uses a mapping function — find and reuse it in `recent_tasks_all` (Task 2). Don't duplicate the mapping logic.
3. **`SubagentManager` test construction:** the existing tests use a specific construction helper — reuse it, don't invent a new one.
4. **`bytes_to_mb` helper:** already exists in resource.rs — reuse, don't duplicate.
5. **Pressure level mapping:** the spec's 4 levels (normal/throttled/evicting/restarting) don't map 1:1 to `ResourcePressureState` (3 states). The mapping uses `PressureGate.last_action()` to disambiguate. In practice, Phase 2's sidecar may never reach pressure states, so the mapping is mostly theoretical — but it must be correct.
6. **Tasks 5+6 must be committed together** to keep the tree compiling.
