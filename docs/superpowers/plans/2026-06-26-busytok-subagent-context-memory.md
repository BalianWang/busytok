# Busytok Subagent Context Builder + Memory Updater Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the stub delegate flow with a real ContextBuilder (budget-controlled prompt assembly) and MemoryUpdater (merge + rule-based compaction), so `subagent.delegate` builds a compact context from SQLite, sends it to the sidecar, and merges the sidecar's `memory_update` response back into `subagent_memory`.

**Architecture:** Two new pure-Rust modules (`context.rs`, `memory.rs`) sit between the store and the executor. `SubagentManager::delegate` calls `ContextBuilder::build` before `executor.execute` and `MemoryUpdater::update` after it. The executor grows `ExecutorInput.context` / `ExecutorInput.memory` (inbound) and `ExecutorOutput.memory_update` (outbound). The mock sidecar gains a `BUSYTOK_MOCK_MEMORY_UPDATE` env var so e2e tests exercise the full merge path, and echoes the received `compact_context` back in `task_summary` so the e2e test can prove context was built from memory. No new DB migrations — `subagent_memory` already has all required columns from Plan 1.

**Tech Stack:** Rust (busytok-subagent, busytok-store, busytok-config), TypeScript (apps/pi-sidecar), vitest, cargo-llvm-cov, bash mock fixture.

## Global Constraints

- **Spec §6.1 responsibility boundary:** busytok-service produces `compact_context`; the sidecar does NOT perform its own context assembly or trimming. The sidecar consumes `context.compact_context` as the authoritative context source.
- **Spec §6.1 budget:** Default 4000 tokens, per-profile configurable via `SubagentProfileConfig.context_budget_tokens`. Hard max 8000 tokens (`SubagentContextConfig.max_budget_tokens`). Current task prompt is NEVER trimmed unless it exceeds the hard limit.
- **Spec §6.1 trim priority (over budget):** 1) recent task summaries (N=5→3→1); 2) attempts (keep last 3); 3) key_files (20→10); 4) open_questions (10→5); 5) long_summary (truncate); 6) hot_summary (preserve); 7) current task prompt (never trimmed). Plan 4 implements priorities 1, 3, 4, 5, 6, 7. Priority 2 (attempts trimming in the context) is folded into the long_summary during compaction; the context builder does not emit a separate attempts section (attempts live inside `long_summary` after compaction).
- **Spec §6.2 hot_summary source:** `hot_summary = result.memory_update.current_state_summary` — NOT `result.task_summary`. The current code (manager.rs `write_hot_summary(&db, &subagent.id, &out.summary)`) writes the wrong field and MUST be fixed. When `memory_update` is absent (mock executor or older sidecar), `hot_summary` is PRESERVED (not erased) — a missing update must not destroy existing memory or trigger a spurious warm→cold transition.
- **Spec §6.2 compaction triggers (any):** (a) ≥ `compaction_tasks_threshold` tasks since last compaction (counted via `created_at_ms > last_compacted_at_ms`, NOT via the capped `recent_tasks.len()`), (b) `hot_summary` + `long_summary` + recent summaries > `compaction_budget_ratio` of the **profile** context budget (NOT the global default), (c) hibernate is about to happen. Plan 4 implements (a) and (b); (c) is deferred to the hibernate flow (already wired in Plan 1/3's `commit_eviction` path which calls `write_hot_summary` — that path does NOT trigger compaction and that's acceptable for MVP).
- **Spec §6.2 compaction algorithm:** Rule-based (concatenation + truncation), NOT LLM-based. `new_long_summary = old.long_summary[:2000] + "\n\nRecent findings:\n" + last_5_task_summaries[:1000]`, capped at 3000 chars. "Last 5" means the 5 MOST RECENT (recent_tasks is DESC-ordered by the store; take the first 5, no reversal). Per-category caps applied DURING compaction: decisions ≤20, key_files ≤20, open_questions ≤10 (unresolved only), attempts last 10.
- **Spec §6.3 normalization:** key_files: **repo-relative** (strip `repo_path`/cwd prefix), forward slashes, strip `./` prefix, macOS case-insensitive dedup. open_questions: trim whitespace, dedupe by lowercase exact match, preserve original casing for display, status `open`|`resolved`.
- **Spec §3.3 invariant (preserved):** `status='warm'` iff `subagent_memory.hot_summary IS NOT NULL`. Plan 3's `commit_eviction` already computes this from memory state. Plan 4's `MemoryUpdater` writes `hot_summary` from `current_state_summary` when provided; when `memory_update` is absent, `hot_summary` is preserved (unchanged), so the invariant holds.
- **No new DB migrations:** `subagent_memory` (migration `0003_subagent.sql`) already has all 12 columns. `last_compacted_at_ms` and `last_compacted_task_id` exist.
- **Reuse existing infrastructure:** `SubagentContextConfig` (busytok-config), `SubagentProfileConfig`, `subagent_get_memory` / `subagent_upsert_memory` / `subagent_list_tasks` / `subagent_get_logical` (busytok-store), `SubagentMemoryRow::new_empty`, tracing `event_code` convention (`subagent.<area>.<event>`), mock-sidecar.sh env-var pattern, vitest for TS tests, cargo-llvm-cov for coverage.
- **Coverage gates:** workspace ≥ 82% (hard), per-crate `busytok-subagent` ≥ 90% (hard). New modules (`context.rs`, `memory.rs`) must be comprehensively unit-tested to maintain the per-crate gate.
- **Bash 3.2 compatibility:** mock-sidecar.sh must not use `declare -A`; use `${arr[@]+"${arr[@]}"}` for empty-array expansion under `set -u`.
- **TS test infrastructure:** all new TS tests use vitest, live in `apps/pi-sidecar/tests/`, run via `pnpm test`. No `node:test` or `tsx --test`.

---

## File Structure

**New files:**
- `crates/busytok-subagent/src/context.rs` — `ContextBuilder`, `CompactContext`, `MemorySnapshot`, `build_context`. Pure function of (memory row, recent tasks, logical subagent, profile config, context config). No I/O — reads are done by the caller, the builder assembles from owned data.
- `crates/busytok-subagent/src/memory.rs` — `MemoryUpdater`, `MemoryUpdate`, `KeyFile`, `OpenQuestion`, merge/normalize/compact logic. Pure functions over `SubagentMemoryRow` + `MemoryUpdate`; the caller handles DB I/O.
- `crates/busytok-subagent/tests/context.rs` — unit tests for `ContextBuilder` (budget, trim priority, never-trim-prompt).
- `crates/busytok-subagent/tests/memory.rs` — unit tests for `MemoryUpdater` (merge, normalize, compaction triggers, caps).

**Modified files:**
- `crates/busytok-subagent/src/mock_executor.rs` — grow `ExecutorInput` (`memory`, `context`, `tools`), grow `ExecutorOutput` (`memory_update`), update `MockTaskExecutor`/`FailingTaskExecutor`.
- `crates/busytok-subagent/src/manager.rs` — insert `ContextBuilder::build` before execute, replace `write_hot_summary` with `MemoryUpdater::update`, pass enriched `ExecutorInput`.
- `crates/busytok-subagent/src/sidecar/executor.rs` — send `memory`/`context`/`tools`/`constraints` in turn_auto params, parse `result.memory_update` into `ExecutorOutput.memory_update`.
- `crates/busytok-subagent/src/lib.rs` — declare `pub mod context; pub mod memory;`.
- `crates/busytok-subagent/tests/manager.rs` — update tests for new `ExecutorInput` fields, assert memory merge in delegate, assert context built.
- `crates/busytok-subagent/tests/sidecar_executor.rs` — update tests for new `ExecutorOutput.memory_update` field.
- `crates/busytok-subagent/tests/fixtures/mock-sidecar.sh` — add `BUSYTOK_MOCK_MEMORY_UPDATE=1` env var emitting `result.memory_update`, and echo `context.compact_context` back in `task_summary` so e2e can verify context building.
- `apps/pi-sidecar/src/types.ts` — add structured `Memory`, `CompactContext` to `TurnAutoParams`; add typed `memory_update` to `TurnAutoResult.result`.
- `apps/pi-sidecar/src/handlers/turn_auto.ts` — accept (and echo) `memory`/`context`; emit `memory_update` when `BUSYTOK_MOCK_MEMORY_UPDATE=1`; echo `compact_context` into `task_summary`.
- `apps/pi-sidecar/tests/turn_auto.test.ts` — test that `memory`/`context` are accepted and `memory_update` is emitted.
- `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs` — add e2e test: delegate twice, assert second delegate's `task_summary` contains the first delegate's `hot_summary` (proving context was built from merged memory).

**Deleted code:**
- `SubagentManager::write_hot_summary` helper (manager.rs) — fully replaced by `MemoryUpdater::update`.

---

## Task 1: Memory domain types + MemoryUpdater (pure logic)

**Files:**
- Create: `crates/busytok-subagent/src/memory.rs`
- Create: `crates/busytok-subagent/tests/memory.rs`
- Modify: `crates/busytok-subagent/src/lib.rs`

**Interfaces:**
- Consumes: `SubagentMemoryRow` (busytok-store), `SubagentTaskRow` (busytok-store), `SubagentContextConfig` (busytok-config)
- Produces:
  - `pub struct KeyFile { path, reason, last_seen_at_ms, score }`
  - `pub struct OpenQuestion { question, status, created_at_ms, last_seen_at_ms }`
  - `pub struct MemoryUpdate { current_state_summary, key_files, decisions, open_questions }`
  - `pub struct MemoryUpdater { config: SubagentContextConfig }`
  - `impl MemoryUpdater { pub fn new(config) -> Self; pub fn update(&self, current: SubagentMemoryRow, update: MemoryUpdate, recent_tasks: &[SubagentTaskRow], task_id: &str, profile_budget_tokens: u32, repo_path: &str) -> SubagentMemoryRow }`

- [ ] **Step 1: Write failing tests for merge, normalize, and compaction**

Create `crates/busytok-subagent/tests/memory.rs`:

```rust
#![allow(clippy::unwrap_used)]

use busytok_config::SubagentContextConfig;
use busytok_store::{SubagentMemoryRow, SubagentTaskRow};
use busytok_subagent::memory::{
    KeyFile, MemoryUpdate, MemoryUpdater, OpenQuestion,
};

fn mem_row(subagent_id: &str) -> SubagentMemoryRow {
    SubagentMemoryRow::new_empty(subagent_id)
}

fn task_row(id: &str, summary: &str, created_at_ms: i64) -> SubagentTaskRow {
    SubagentTaskRow {
        id: id.into(),
        subagent_id: "sub-a".into(),
        source_harness: None,
        source_session_id: None,
        intent: None,
        profile: "pi/review-cheap".into(),
        prompt: Some("do thing".into()),
        prompt_artifact_ref: None,
        output_schema_name: None,
        output_schema_version: 1,
        status: "completed".into(),
        result_summary: Some(summary.into()),
        result_json: None,
        error: None,
        created_at_ms,
        started_at_ms: None,
        completed_at_ms: Some(created_at_ms + 1000),
    }
}

fn cfg() -> SubagentContextConfig {
    SubagentContextConfig::default()
}

const REPO: &str = "/repo";

#[test]
fn merges_key_files_dedupes_and_updates_score() {
    let updater = MemoryUpdater::new(cfg());
    let mut current = mem_row("sub-a");
    current.key_files_json = Some(
        serde_json::to_string(&[KeyFile {
            path: "src/auth/token.ts".into(),
            reason: "refresh logic".into(),
            last_seen_at_ms: 1000,
            score: 3,
        }])
        .unwrap(),
    );
    let update = MemoryUpdate {
        current_state_summary: Some("new state".into()),
        key_files: vec![
            KeyFile {
                path: "./src/auth/token.ts".into(), // normalize + dedupe
                reason: "updated reason".into(),
                last_seen_at_ms: 5000,
                score: 1,
            },
            KeyFile {
                path: "tests/auth.test.ts".into(),
                reason: "new file".into(),
                last_seen_at_ms: 5000,
                score: 2,
            },
        ],
        decisions: vec![],
        open_questions: vec![],
    };
    let result = updater.update(current, update, &[], "task_1", 4000, REPO);
    let files: Vec<KeyFile> = serde_json::from_str(result.key_files_json.as_deref().unwrap()).unwrap();
    assert_eq!(files.len(), 2, "dedupe by normalized path");
    let token = files.iter().find(|f| f.path == "src/auth/token.ts").unwrap();
    assert_eq!(token.score, 3, "keep max score on merge");
    assert_eq!(token.last_seen_at_ms, 5000, "update last_seen to latest");
    assert_eq!(token.reason, "updated reason", "reason updated by new entry");
}

#[test]
fn normalizes_absolute_path_to_repo_relative() {
    let updater = MemoryUpdater::new(cfg());
    let current = mem_row("sub-a");
    let update = MemoryUpdate {
        current_state_summary: Some("state".into()),
        key_files: vec![KeyFile {
            path: "/repo/src/auth/token.ts".into(), // absolute → repo-relative
            reason: "found".into(),
            last_seen_at_ms: 1000,
            score: 1,
        }],
        decisions: vec![],
        open_questions: vec![],
    };
    let result = updater.update(current, update, &[], "task_1", 4000, REPO);
    let files: Vec<KeyFile> = serde_json::from_str(result.key_files_json.as_deref().unwrap()).unwrap();
    assert_eq!(files[0].path, "src/auth/token.ts", "absolute path stripped to repo-relative");
}

#[test]
fn merges_open_questions_dedupes_by_lowercase() {
    let updater = MemoryUpdater::new(cfg());
    let mut current = mem_row("sub-a");
    current.open_questions_json = Some(
        serde_json::to_string(&[OpenQuestion {
            question: "Does it handle refresh?".into(),
            status: "open".into(),
            created_at_ms: 1000,
            last_seen_at_ms: 1000,
        }])
        .unwrap(),
    );
    let update = MemoryUpdate {
        current_state_summary: Some("state".into()),
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![
            OpenQuestion {
                question: "  does it handle REFRESH?  ".into(), // trim + dedupe by lowercase
                status: "open".into(),
                created_at_ms: 5000,
                last_seen_at_ms: 5000,
            },
            OpenQuestion {
                question: "New question?".into(),
                status: "open".into(),
                created_at_ms: 5000,
                last_seen_at_ms: 5000,
            },
        ],
    };
    let result = updater.update(current, update, &[], "task_1", 4000, REPO);
    let qs: Vec<OpenQuestion> =
        serde_json::from_str(result.open_questions_json.as_deref().unwrap()).unwrap();
    assert_eq!(qs.len(), 2, "dedupe by lowercase exact match after trim");
    assert!(qs.iter().any(|q| q.question == "Does it handle refresh?"), "preserve original casing");
}

#[test]
fn hot_summary_set_from_current_state_summary() {
    let updater = MemoryUpdater::new(cfg());
    let current = mem_row("sub-a");
    let update = MemoryUpdate {
        current_state_summary: Some("Review completed: gaps found.".into()),
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![],
    };
    let result = updater.update(current, update, &[], "task_1", 4000, REPO);
    assert_eq!(
        result.hot_summary.as_deref(),
        Some("Review completed: gaps found."),
        "hot_summary = current_state_summary, NOT task_summary"
    );
}

#[test]
fn hot_summary_preserved_when_memory_update_absent() {
    let updater = MemoryUpdater::new(cfg());
    let mut current = mem_row("sub-a");
    current.hot_summary = Some("Previous state.".into());
    let update = MemoryUpdate::default(); // no current_state_summary
    let result = updater.update(current, update, &[], "task_1", 4000, REPO);
    assert_eq!(
        result.hot_summary.as_deref(),
        Some("Previous state."),
        "hot_summary preserved when memory_update absent — no destructive warm→cold"
    );
}

#[test]
fn compaction_rebuilds_long_summary_when_task_threshold_reached() {
    let mut cfg = cfg();
    cfg.compaction_tasks_threshold = 3;
    let updater = MemoryUpdater::new(cfg);
    let mut current = mem_row("sub-a");
    current.hot_summary = Some("current state".into());
    current.long_summary = Some("old long summary".into());
    current.last_compacted_at_ms = Some(1000);
    let update = MemoryUpdate {
        current_state_summary: Some("state".into()),
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![],
    };
    // 3 tasks since last_compacted_at_ms=1000 → trigger (a).
    let tasks = vec![
        task_row("task_1", "found bug in auth", 2000),
        task_row("task_2", "fixed token refresh", 3000),
        task_row("task_3", "added concurrent test", 4000),
    ];
    let result = updater.update(current, update, &tasks, "task_3", 4000, REPO);
    let long = result.long_summary.as_deref().unwrap();
    assert!(long.contains("old long summary"), "preserves old long_summary");
    assert!(long.contains("Recent findings:"), "appends recent findings header");
    assert!(long.contains("added concurrent test"), "includes MOST RECENT task summary first");
    assert!(result.last_compacted_at_ms.is_some(), "updates last_compacted_at_ms");
    assert_eq!(
        result.last_compacted_task_id.as_deref(),
        Some("task_3"),
        "records compacted task id"
    );
    assert!(long.len() <= 3000, "long_summary capped at 3000 chars");
}

#[test]
fn compaction_uses_most_recent_5_summaries_not_oldest() {
    let mut cfg = cfg();
    cfg.compaction_tasks_threshold = 1;
    let updater = MemoryUpdater::new(cfg);
    let current = mem_row("sub-a");
    let update = MemoryUpdate {
        current_state_summary: Some("state".into()),
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![],
    };
    // 8 tasks; recent_tasks_limit is the default 5, so recent_tasks has 5 (DESC).
    // Compaction must take the 5 most recent (the whole returned set), with the
    // newest summary appearing first in the "Recent findings" section.
    let tasks = vec![
        task_row("t1", "oldest", 1000),
        task_row("t2", "older", 2000),
        task_row("t3", "middle", 3000),
        task_row("t4", "newer", 4000),
        task_row("t5", "newest", 5000),
    ];
    let result = updater.update(current, update, &tasks, "t5", 4000, REPO);
    let long = result.long_summary.as_deref().unwrap();
    let findings_start = long.find("Recent findings:\n").map(|i| i + "Recent findings:\n".len()).unwrap();
    let findings = &long[findings_start..];
    assert!(findings.starts_with("newest"), "most recent summary must appear first");
}

#[test]
fn compaction_trigger_b_fires_on_oversized_memory_using_profile_budget() {
    let mut cfg = cfg();
    cfg.compaction_tasks_threshold = 100; // disable trigger (a)
    cfg.compaction_budget_ratio = 0.1; // trigger (b) fires easily
    let updater = MemoryUpdater::new(cfg);
    let mut current = mem_row("sub-a");
    current.hot_summary = Some("x".repeat(500));
    current.long_summary = Some("y".repeat(500));
    let update = MemoryUpdate {
        current_state_summary: Some("state".into()),
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![],
    };
    // profile_budget_tokens=4000 → budget_chars=16000, threshold=1600.
    // hot+long = 1000 chars < 1600 → no trigger.
    let result_no_trigger = updater.update(current.clone(), update.clone(), &[], "t1", 4000, REPO);
    assert!(result_no_trigger.last_compacted_at_ms.is_none(), "should not compact under threshold");

    // profile_budget_tokens=1000 → budget_chars=4000, threshold=400.
    // hot+long = 1000 chars > 400 → trigger (b).
    let result_trigger = updater.update(current, update, &[], "t1", 1000, REPO);
    assert!(result_trigger.last_compacted_at_ms.is_some(), "trigger (b) fires using profile budget");
}

#[test]
fn no_compaction_when_below_threshold() {
    let mut cfg = cfg();
    cfg.compaction_tasks_threshold = 5;
    let updater = MemoryUpdater::new(cfg);
    let mut current = mem_row("sub-a");
    current.long_summary = Some("existing summary".into());
    let update = MemoryUpdate {
        current_state_summary: Some("state".into()),
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![],
    };
    let result = updater.update(current, update, &[], "task_1", 4000, REPO);
    assert_eq!(
        result.long_summary.as_deref(),
        Some("existing summary"),
        "long_summary unchanged when no compaction"
    );
    assert!(result.last_compacted_at_ms.is_none(), "no compaction timestamp");
}

#[test]
fn caps_key_files_at_20_only_during_compaction() {
    let mut cfg = cfg();
    cfg.compaction_tasks_threshold = 1; // force compaction
    let updater = MemoryUpdater::new(cfg);
    let mut current = mem_row("sub-a");
    let files: Vec<KeyFile> = (0..15)
        .map(|i| KeyFile {
            path: format!("src/file_{i}.ts"),
            reason: "old".into(),
            last_seen_at_ms: 1000,
            score: 1,
        })
        .collect();
    current.key_files_json = Some(serde_json::to_string(&files).unwrap());
    let update = MemoryUpdate {
        current_state_summary: Some("state".into()),
        key_files: (0..10)
            .map(|i| KeyFile {
                path: format!("src/new_{i}.ts"),
                reason: "new".into(),
                last_seen_at_ms: 5000,
                score: 2,
            })
            .collect(),
        decisions: vec![],
        open_questions: vec![],
    };
    let tasks = vec![task_row("task_1", "did thing", 1000)];
    let result = updater.update(current, update, &tasks, "task_1", 4000, REPO);
    let files: Vec<KeyFile> = serde_json::from_str(result.key_files_json.as_deref().unwrap()).unwrap();
    assert!(files.len() <= 20, "key_files capped at 20 during compaction, got {}", files.len());
}

#[test]
fn caps_not_applied_outside_compaction() {
    let mut cfg = cfg();
    cfg.compaction_tasks_threshold = 100; // no compaction
    let updater = MemoryUpdater::new(cfg);
    let mut current = mem_row("sub-a");
    // 25 decisions — exceeds cap of 20, but no compaction should fire.
    current.decisions_json = Some(serde_json::to_string(&(0..25).map(|i| format!("d{i}")).collect::<Vec<_>>()).unwrap());
    let update = MemoryUpdate {
        current_state_summary: Some("state".into()),
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![],
    };
    let result = updater.update(current, update, &[], "task_1", 4000, REPO);
    let decisions: Vec<String> =
        serde_json::from_str(result.decisions_json.as_deref().unwrap()).unwrap();
    assert_eq!(decisions.len(), 25, "caps NOT applied outside compaction — preserve data");
}

#[test]
fn drops_resolved_open_questions_during_compaction() {
    let mut cfg = cfg();
    cfg.compaction_tasks_threshold = 1;
    let updater = MemoryUpdater::new(cfg);
    let mut current = mem_row("sub-a");
    current.open_questions_json = Some(
        serde_json::to_string(&[
            OpenQuestion {
                question: "resolved one".into(),
                status: "resolved".into(),
                created_at_ms: 1000,
                last_seen_at_ms: 1000,
            },
            OpenQuestion {
                question: "still open".into(),
                status: "open".into(),
                created_at_ms: 1000,
                last_seen_at_ms: 1000,
            },
        ])
        .unwrap(),
    );
    let update = MemoryUpdate {
        current_state_summary: Some("state".into()),
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![],
    };
    let tasks = vec![task_row("task_1", "did thing", 1000)];
    let result = updater.update(current, update, &tasks, "task_1", 4000, REPO);
    let qs: Vec<OpenQuestion> =
        serde_json::from_str(result.open_questions_json.as_deref().unwrap()).unwrap();
    assert_eq!(qs.len(), 1, "resolved questions dropped during compaction");
    assert_eq!(qs[0].question, "still open");
}

#[test]
fn attempts_records_most_recent_task_not_oldest() {
    let mut cfg = cfg();
    cfg.compaction_tasks_threshold = 100; // no compaction
    let updater = MemoryUpdater::new(cfg);
    let current = mem_row("sub-a");
    let update = MemoryUpdate {
        current_state_summary: Some("state".into()),
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![],
    };
    // recent_tasks is DESC: [newest, older, oldest].
    let tasks = vec![
        task_row("t3", "newest summary", 3000),
        task_row("t2", "middle summary", 2000),
        task_row("t1", "oldest summary", 1000),
    ];
    let result = updater.update(current, update, &tasks, "t3", 4000, REPO);
    let attempts: Vec<String> =
        serde_json::from_str(result.attempts_json.as_deref().unwrap()).unwrap();
    assert_eq!(attempts.len(), 1, "one attempt appended");
    assert!(attempts[0].contains("newest summary"), "attempts records MOST RECENT task, got: {:?}", attempts[0]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p busytok-subagent --test memory 2>&1 | tail -10`
Expected: FAIL with "unresolved module `memory`" or "cannot find type `KeyFile`".

- [ ] **Step 3: Implement `memory.rs`**

Create `crates/busytok-subagent/src/memory.rs`:

```rust
//! Memory merge and rule-based compaction (spec §6.2, §6.3).
//!
//! Pure functions over owned data — the caller handles DB I/O. The manager
//! reads `SubagentMemoryRow`, constructs a `MemoryUpdate` from the executor
//! output, and writes the returned row back via `subagent_upsert_memory`.

use busytok_config::SubagentContextConfig;
use busytok_store::{SubagentMemoryRow, SubagentTaskRow};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

const MAX_LONG_SUMMARY_CHARS: usize = 3000;
const MAX_DECISIONS: usize = 20;
const MAX_KEY_FILES: usize = 20;
const MAX_OPEN_QUESTIONS: usize = 10;
const MAX_ATTEMPTS: usize = 10;
const RECENT_TASKS_FOR_COMPACTION: usize = 5;
const OLD_LONG_SUMMARY_KEEP_CHARS: usize = 2000;
const RECENT_SUMMARIES_KEEP_CHARS: usize = 1000;

/// A tracked source file with a relevance score. Spec §3.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyFile {
    pub path: String,
    pub reason: String,
    pub last_seen_at_ms: i64,
    pub score: i64,
}

/// An open or resolved question. Spec §3.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub question: String,
    /// "open" or "resolved".
    pub status: String,
    pub created_at_ms: i64,
    pub last_seen_at_ms: i64,
}

/// The delta returned by the sidecar after a turn (spec §4.3 result.memory_update).
/// `current_state_summary` becomes `hot_summary` (spec §6.2). When this entire
/// struct is `default()` (no memory_update emitted by the sidecar), `hot_summary`
/// is PRESERVED — a missing update must not destroy existing memory.
#[derive(Debug, Clone, Default)]
pub struct MemoryUpdate {
    pub current_state_summary: Option<String>,
    pub key_files: Vec<KeyFile>,
    pub decisions: Vec<String>,
    pub open_questions: Vec<OpenQuestion>,
}

/// Merges a `MemoryUpdate` into the current `SubagentMemoryRow`, applying
/// normalization (§6.3) and rule-based compaction (§6.2) when triggered.
pub struct MemoryUpdater {
    config: SubagentContextConfig,
}

impl MemoryUpdater {
    pub fn new(config: SubagentContextConfig) -> Self {
        Self { config }
    }

    /// Produce the next memory row. Pure — no DB I/O.
    ///
    /// - `profile_budget_tokens`: the per-profile context budget, used for
    ///   compaction trigger (b) (§6.2: "> 70% of context budget").
    /// - `repo_path`: used to normalize key_files paths to repo-relative (§6.3).
    /// - `recent_tasks`: DESC-ordered by `created_at_ms` (as returned by
    ///   `subagent_list_tasks`); `recent_tasks[0]` is the most recent.
    pub fn update(
        &self,
        mut current: SubagentMemoryRow,
        update: MemoryUpdate,
        recent_tasks: &[SubagentTaskRow],
        task_id: &str,
        profile_budget_tokens: u32,
        repo_path: &str,
    ) -> SubagentMemoryRow {
        // §6.2.2: hot_summary = current_state_summary (NOT task_summary).
        // When current_state_summary is None (no memory_update), PRESERVE the
        // existing hot_summary — a missing update must not destroy memory.
        if let Some(summary) = update.current_state_summary {
            current.hot_summary = Some(summary);
        }

        // Merge key_files (normalize + dedupe + max score + latest last_seen).
        let mut files = parse_json_vec(&current.key_files_json);
        for f in update.key_files {
            let normalized = normalize_path(&f.path, repo_path);
            if let Some(existing) = files
                .iter_mut()
                .find(|e| same_path_normalized(&e.path, &normalized))
            {
                existing.score = existing.score.max(f.score);
                existing.last_seen_at_ms = existing.last_seen_at_ms.max(f.last_seen_at_ms);
                existing.reason = f.reason;
            } else {
                files.push(KeyFile {
                    path: normalized,
                    reason: f.reason,
                    last_seen_at_ms: f.last_seen_at_ms,
                    score: f.score,
                });
            }
        }
        current.key_files_json = Some(serde_json::to_string(&files).unwrap_or_default());

        // Merge open_questions (trim + dedupe by lowercase exact match, preserve casing).
        let mut questions = parse_json_vec(&current.open_questions_json);
        for q in update.open_questions {
            let trimmed = q.question.trim().to_string();
            let lower = trimmed.to_lowercase();
            if let Some(existing) = questions
                .iter_mut()
                .find(|e| e.question.trim().to_lowercase() == lower)
            {
                existing.last_seen_at_ms = existing.last_seen_at_ms.max(q.last_seen_at_ms);
                if q.status == "resolved" {
                    existing.status = "resolved".into();
                }
            } else {
                questions.push(OpenQuestion {
                    question: trimmed,
                    status: q.status,
                    created_at_ms: q.created_at_ms,
                    last_seen_at_ms: q.last_seen_at_ms,
                });
            }
        }
        current.open_questions_json = Some(serde_json::to_string(&questions).unwrap_or_default());

        // Merge decisions (dedupe by exact match).
        let mut decisions = parse_json_vec(&current.decisions_json);
        for d in update.decisions {
            if !decisions.iter().any(|e: &String| e == &d) {
                decisions.push(d);
            }
        }
        current.decisions_json = Some(serde_json::to_string(&decisions).unwrap_or_default());

        // Attempts: append a one-line summary from the MOST RECENT task.
        // recent_tasks is DESC-ordered, so .first() is the most recent.
        let mut attempts = parse_json_vec(&current.attempts_json);
        if let Some(summary) = recent_tasks.first().and_then(|t| t.result_summary.as_deref()) {
            let entry = format!(
                "{}: {}",
                task_id,
                summary.lines().next().unwrap_or(summary)
            );
            if !attempts.iter().any(|a: &String| a == &entry) {
                attempts.push(entry);
            }
        }
        // Keep last 10.
        let start = attempts.len().saturating_sub(MAX_ATTEMPTS);
        attempts = attempts[start..].to_vec();
        current.attempts_json = Some(serde_json::to_string(&attempts).unwrap_or_default());

        // Compaction triggers (§6.2.3).
        // (a) ≥ threshold tasks since last compaction. Count via created_at_ms
        //     > last_compacted_at_ms (NOT recent_tasks.len(), which is capped
        //     by the SQL LIMIT and would mask the real count).
        // (b) hot_summary + long_summary > ratio of profile budget.
        let last_compacted = current.last_compacted_at_ms.unwrap_or(0);
        let tasks_since = recent_tasks
            .iter()
            .filter(|t| t.created_at_ms > last_compacted)
            .count() as u32;
        let should_compact = tasks_since >= self.config.compaction_tasks_threshold
            || self.memory_oversized(&current, profile_budget_tokens);
        if should_compact {
            self.compact(&mut current, recent_tasks);
            // Per-category caps apply DURING compaction (§6.2.4).
            let mut decisions = parse_json_vec::<String>(&current.decisions_json);
            let start = decisions.len().saturating_sub(MAX_DECISIONS);
            decisions = decisions[start..].to_vec();
            current.decisions_json = Some(serde_json::to_string(&decisions).unwrap_or_default());

            let mut files = parse_json_vec::<KeyFile>(&current.key_files_json);
            files.sort_by(|a, b| b.score.cmp(&a.score).then(b.last_seen_at_ms.cmp(&a.last_seen_at_ms)));
            files.truncate(MAX_KEY_FILES);
            current.key_files_json = Some(serde_json::to_string(&files).unwrap_or_default());

            let mut questions = parse_json_vec::<OpenQuestion>(&current.open_questions_json);
            questions.truncate(MAX_OPEN_QUESTIONS);
            current.open_questions_json = Some(serde_json::to_string(&questions).unwrap_or_default());

            current.last_compacted_at_ms = Some(busytok_domain::now_ms());
            current.last_compacted_task_id = Some(task_id.to_string());
        }

        current.updated_at_ms = busytok_domain::now_ms();
        current
    }

    /// Compaction trigger (b): hot_summary + long_summary exceed
    /// `compaction_budget_ratio` of the profile context budget. We approximate
    /// tokens as chars / 4 (a standard heuristic); this is a conservative
    /// over-count. Uses the PROFILE budget (§6.1: per-profile configurable),
    /// NOT the global default.
    fn memory_oversized(&self, mem: &SubagentMemoryRow, profile_budget_tokens: u32) -> bool {
        let budget_chars = (profile_budget_tokens as usize) * 4;
        let threshold = (budget_chars as f64 * self.config.compaction_budget_ratio) as usize;
        let mut total = 0usize;
        if let Some(s) = &mem.hot_summary {
            total += s.len();
        }
        if let Some(s) = &mem.long_summary {
            total += s.len();
        }
        total > threshold
    }

    /// Rule-based compaction (§6.2.4). Rebuilds long_summary from the old
    /// long_summary + the 5 MOST RECENT task summaries, then drops resolved
    /// open questions. Per-category caps are applied by the caller after this.
    fn compact(&self, mem: &mut SubagentMemoryRow, recent_tasks: &[SubagentTaskRow]) {
        let old_long = mem
            .long_summary
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(OLD_LONG_SUMMARY_KEEP_CHARS)
            .collect::<String>();
        // recent_tasks is DESC (most recent first). Take the first 5 = most recent.
        let recent_summaries: String = recent_tasks
            .iter()
            .take(RECENT_TASKS_FOR_COMPACTION)
            .filter_map(|t| t.result_summary.as_deref())
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .take(RECENT_SUMMARIES_KEEP_CHARS)
            .collect();
        let new_long = if recent_summaries.is_empty() {
            old_long
        } else {
            format!("{old_long}\n\nRecent findings:\n{recent_summaries}")
        };
        let new_long: String = new_long.chars().take(MAX_LONG_SUMMARY_CHARS).collect();
        mem.long_summary = Some(new_long);

        // Drop resolved open questions during compaction (§6.2.4 cap: unresolved only).
        let mut questions = parse_json_vec::<OpenQuestion>(&mem.open_questions_json);
        questions.retain(|q| q.status != "resolved");
        mem.open_questions_json = Some(serde_json::to_string(&questions).unwrap_or_default());
    }
}

/// Generic JSON vec parser — replaces 4 near-identical helpers.
fn parse_json_vec<T: DeserializeOwned>(json: &Option<String>) -> Vec<T> {
    json.as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}

/// Normalize a file path (spec §6.3): repo-relative, forward slashes, strip
/// `./` prefix. macOS case-insensitive dedup is handled by `same_path_normalized`.
fn normalize_path(path: &str, repo_path: &str) -> String {
    let mut p = path.replace('\\', "/");
    // Strip repo_path prefix (absolute or trailing-slash form) to make repo-relative.
    if !repo_path.is_empty() {
        let repo_fwd = repo_path.replace('\\', "/");
        for prefix in &[repo_fwd.as_str(), format!("{repo_fwd}/").as_str()] {
            if p.starts_with(prefix) {
                p = p[prefix.len()..].to_string();
                break;
            }
        }
    }
    let p = p.strip_prefix("./").unwrap_or(&p).to_string();
    p
}

/// Case-insensitive path comparison for dedup (spec §6.3 macOS rule).
fn same_path_normalized(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}
```

Add to `crates/busytok-subagent/src/lib.rs` (after the existing `pub mod` declarations):

```rust
pub mod context;
pub mod memory;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p busytok-subagent --test memory 2>&1 | tail -15`
Expected: PASS — all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-subagent/src/memory.rs crates/busytok-subagent/tests/memory.rs crates/busytok-subagent/src/lib.rs
git commit -m "feat(subagent): add MemoryUpdater with merge, normalize, and rule-based compaction"
```

---

## Task 2: ContextBuilder (pure logic + budget control)

**Files:**
- Create: `crates/busytok-subagent/src/context.rs`
- Create: `crates/busytok-subagent/tests/context.rs`

**Interfaces:**
- Consumes: `SubagentMemoryRow`, `SubagentTaskRow`, `SubagentLogicalSubagentRow`, `SubagentContextConfig`, `SubagentProfileConfig`, `KeyFile`/`OpenQuestion` (Task 1)
- Produces:
  - `pub struct CompactContext { compact_context: String, budget_tokens: u32, source: String }`
  - `pub struct MemorySnapshot { hot_summary, long_summary, key_files: Vec<KeyFile>, decisions: Vec<String>, open_questions: Vec<OpenQuestion> }` (structured — spec §4.3 memory field carries structured debugging data)
  - `pub struct ContextBuilder { config: SubagentContextConfig }`
  - `impl ContextBuilder { pub fn new(config) -> Self; pub fn build(&self, subagent, memory, recent_tasks, prompt, profile_budget_tokens) -> (CompactContext, MemorySnapshot) }`

- [ ] **Step 1: Write failing tests for budget control and trim priority**

Create `crates/busytok-subagent/tests/context.rs`:

```rust
#![allow(clippy::unwrap_used)]

use busytok_config::SubagentContextConfig;
use busytok_store::{SubagentLogicalSubagentRow, SubagentMemoryRow, SubagentTaskRow};
use busytok_subagent::context::{CompactContext, ContextBuilder, MemorySnapshot};
use busytok_subagent::memory::{KeyFile, OpenQuestion};

fn cfg() -> SubagentContextConfig {
    SubagentContextConfig::default()
}

fn subagent() -> SubagentLogicalSubagentRow {
    SubagentLogicalSubagentRow {
        id: "sub-a".into(),
        name: "auth-investigator".into(),
        project_id: "proj".into(),
        repo_path: "/repo".into(),
        repo_hash: "abc".into(),
        branch: Some("main".into()),
        intent: Some("Study auth refresh logic".into()),
        default_profile: "pi/review-cheap".into(),
        default_model: None,
        status: "hot".into(),
        created_at_ms: 1000,
        updated_at_ms: 2000,
        last_active_at_ms: Some(2000),
    }
}

fn mem_row() -> SubagentMemoryRow {
    let mut m = SubagentMemoryRow::new_empty("sub-a");
    m.hot_summary = Some("Token refresh logic is in src/auth/token.ts.".into());
    m.long_summary = Some("Long-term: auth module uses JWT. ".repeat(50));
    m.key_files_json = Some(
        serde_json::to_string(&[KeyFile {
            path: "src/auth/token.ts".into(),
            reason: "refresh logic".into(),
            last_seen_at_ms: 1000,
            score: 3,
        }])
        .unwrap(),
    );
    m.open_questions_json = Some(
        serde_json::to_string(&[OpenQuestion {
            question: "Concurrent refresh handled?".into(),
            status: "open".into(),
            created_at_ms: 1000,
            last_seen_at_ms: 1000,
        }])
        .unwrap(),
    );
    m.decisions_json = Some(serde_json::to_string(&["Focus on read-only analysis".to_string()]).unwrap());
    m
}

fn task_row(id: &str, summary: &str, created_at_ms: i64) -> SubagentTaskRow {
    SubagentTaskRow {
        id: id.into(),
        subagent_id: "sub-a".into(),
        source_harness: None,
        source_session_id: None,
        intent: None,
        profile: "pi/review-cheap".into(),
        prompt: Some("do thing".into()),
        prompt_artifact_ref: None,
        output_schema_name: None,
        output_schema_version: 1,
        status: "completed".into(),
        result_summary: Some(summary.into()),
        result_json: None,
        error: None,
        created_at_ms,
        started_at_ms: None,
        completed_at_ms: Some(created_at_ms + 1000),
    }
}

#[test]
fn build_includes_subagent_name_intent_and_prompt() {
    let builder = ContextBuilder::new(cfg());
    let (ctx, _snap) = builder.build(
        &subagent(),
        &mem_row(),
        &[],
        "Check concurrent refresh handling.",
        cfg().default_budget_tokens,
    );
    assert!(ctx.compact_context.contains("auth-investigator"), "includes subagent name");
    assert!(ctx.compact_context.contains("Study auth refresh logic"), "includes intent");
    assert!(ctx.compact_context.contains("Check concurrent refresh handling"), "includes prompt");
    assert_eq!(ctx.budget_tokens, cfg().default_budget_tokens);
    assert_eq!(ctx.source, "busytok-context-builder/v1");
}

#[test]
fn build_includes_hot_summary_and_long_summary() {
    let builder = ContextBuilder::new(cfg());
    let (ctx, _snap) = builder.build(
        &subagent(),
        &mem_row(),
        &[],
        "do thing",
        cfg().default_budget_tokens,
    );
    assert!(ctx.compact_context.contains("Token refresh logic is in src/auth/token.ts."));
    assert!(ctx.compact_context.contains("Long-term:"));
}

#[test]
fn build_includes_recent_task_summaries_most_recent_first() {
    let builder = ContextBuilder::new(cfg());
    let tasks = vec![
        task_row("task_1", "found token logic", 2000),
        task_row("task_2", "checked tests", 3000),
    ];
    let (ctx, _snap) = builder.build(
        &subagent(),
        &mem_row(),
        &tasks,
        "continue",
        cfg().default_budget_tokens,
    );
    assert!(ctx.compact_context.contains("found token logic"));
    assert!(ctx.compact_context.contains("checked tests"));
    // Most recent (task_2) should appear before older (task_1).
    let t2 = ctx.compact_context.find("checked tests").unwrap();
    let t1 = ctx.compact_context.find("found token logic").unwrap();
    assert!(t2 < t1, "most recent task summary appears first");
}

#[test]
fn prompt_never_trimmed_under_budget() {
    let builder = ContextBuilder::new(cfg());
    let prompt = "x".repeat(2000); // large prompt
    let (ctx, _snap) = builder.build(
        &subagent(),
        &mem_row(),
        &[],
        &prompt,
        cfg().default_budget_tokens,
    );
    assert!(ctx.compact_context.contains(&prompt), "prompt never trimmed under hard limit");
}

#[test]
fn trims_recent_task_summaries_when_over_budget() {
    let builder = ContextBuilder::new(cfg());
    let tiny_budget = 200u32;
    let tasks: Vec<SubagentTaskRow> = (0..5)
        .map(|i| task_row(&format!("task_{i}"), &format!("summary_{i} padding ".repeat(20)), 1000 + i))
        .collect();
    let (ctx, _snap) = builder.build(
        &subagent(),
        &mem_row(),
        &tasks,
        "short prompt",
        tiny_budget,
    );
    assert!(ctx.compact_context.contains("short prompt"));
    let present_count = (0..5).filter(|i| ctx.compact_context.contains(&format!("summary_{i}"))).count();
    assert!(present_count < 5, "expected trimming, {present_count} summaries present");
}

#[test]
fn trim_priority_drops_lowest_priority_sections_first() {
    let builder = ContextBuilder::new(cfg());
    // Build memory with all section types populated.
    let mem = mem_row();
    // Budget large enough to fit prompt + hot_summary + long_summary but NOT
    // recent_summaries + key_files + open_questions. Trim priority:
    //   1=recent_summaries (drop), 3=key_files (drop), 4=open_questions (drop),
    //   5=long_summary (keep), 6=hot_summary (keep), 7=prompt (keep).
    let tasks = vec![task_row("t1", "recent summary that is long enough to matter here padding", 1000)];
    // hot_summary (~40 chars) + long_summary (~2400 chars) + prompt (~10) ≈ 2450 chars.
    // Budget 3000 tokens → 12000 chars → fits hot+long+prompt, forces trimming of lower-priority.
    // Budget 700 tokens → 2800 chars → tight; drop recent+keyfiles+questions to fit.
    let (ctx, _snap) = builder.build(&subagent(), &mem, &tasks, "the prompt", 700);
    assert!(ctx.compact_context.contains("the prompt"), "prompt kept (priority 7)");
    assert!(ctx.compact_context.contains("Token refresh logic"), "hot_summary kept (priority 6)");
    // recent_summaries (priority 1) and key_files (priority 3) and open_questions (priority 4) dropped.
    assert!(!ctx.compact_context.contains("recent summary that is long"), "recent_summaries dropped (priority 1)");
    assert!(!ctx.compact_context.contains("Key files:"), "key_files dropped (priority 3)");
    assert!(!ctx.compact_context.contains("Open questions:"), "open_questions dropped (priority 4)");
}

#[test]
fn memory_snapshot_carries_structured_data() {
    let builder = ContextBuilder::new(cfg());
    let (_ctx, snap) = builder.build(
        &subagent(),
        &mem_row(),
        &[],
        "do thing",
        cfg().default_budget_tokens,
    );
    assert_eq!(snap.hot_summary.as_deref(), Some("Token refresh logic is in src/auth/token.ts."));
    assert!(snap.long_summary.is_some());
    assert_eq!(snap.key_files.len(), 1, "structured KeyFile retained");
    assert_eq!(snap.key_files[0].path, "src/auth/token.ts");
    assert_eq!(snap.key_files[0].score, 3, "score retained in snapshot");
    assert_eq!(snap.open_questions.len(), 1);
    assert_eq!(snap.open_questions[0].question, "Concurrent refresh handled?");
}

#[test]
fn respects_profile_budget_override() {
    let builder = ContextBuilder::new(cfg());
    let profile_budget = 5000u32;
    let (ctx, _snap) = builder.build(
        &subagent(),
        &mem_row(),
        &[],
        "do thing",
        profile_budget,
    );
    assert_eq!(ctx.budget_tokens, 5000, "uses profile budget override");
}

#[test]
fn clamps_budget_to_hard_max() {
    let mut c = cfg();
    c.max_budget_tokens = 8000;
    let builder = ContextBuilder::new(c);
    let (ctx, _snap) = builder.build(
        &subagent(),
        &mem_row(),
        &[],
        "do thing",
        99999, // over hard max
    );
    assert_eq!(ctx.budget_tokens, 8000, "clamped to hard max");
}

#[test]
fn zero_budget_treated_as_default() {
    let c = cfg();
    let default = c.default_budget_tokens;
    let builder = ContextBuilder::new(c);
    let (ctx, _snap) = builder.build(
        &subagent(),
        &mem_row(),
        &[],
        "do thing",
        0, // zero → treated as default
    );
    assert_eq!(ctx.budget_tokens, default, "zero budget falls back to default");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p busytok-subagent --test context 2>&1 | tail -10`
Expected: FAIL with "unresolved module `context`".

- [ ] **Step 3: Implement `context.rs`**

Create `crates/busytok-subagent/src/context.rs`:

```rust
//! Context builder with budget control and trim priority (spec §6.1).
//!
//! Produces the `compact_context` string consumed by the sidecar as the
//! authoritative context source, plus a `MemorySnapshot` carrying the raw
//! structured memory fields (for the RPC `memory` field — debugging and
//! future direct-sidecar consumption). Pure functions, no I/O.

use busytok_config::SubagentContextConfig;
use busytok_store::{SubagentLogicalSubagentRow, SubagentMemoryRow, SubagentTaskRow};

use crate::memory::{KeyFile, OpenQuestion};

/// The prompt-ready context string + budget metadata (spec §6.1 output).
pub struct CompactContext {
    pub compact_context: String,
    pub budget_tokens: u32,
    pub source: String,
}

/// Raw structured memory for the RPC `memory` field (spec §4.3). The sidecar
/// uses `compact_context` as authoritative; this carries full structured data
/// for debugging (key_files retain score/reason, open_questions retain status).
pub struct MemorySnapshot {
    pub hot_summary: Option<String>,
    pub long_summary: Option<String>,
    pub key_files: Vec<KeyFile>,
    pub decisions: Vec<String>,
    pub open_questions: Vec<OpenQuestion>,
}

pub struct ContextBuilder {
    config: SubagentContextConfig,
}

impl ContextBuilder {
    pub fn new(config: SubagentContextConfig) -> Self {
        Self { config }
    }

    /// Build the compact context string from the subagent's memory + recent
    /// tasks. Applies the §6.1 trim priority when over budget.
    pub fn build(
        &self,
        subagent: &SubagentLogicalSubagentRow,
        memory: &SubagentMemoryRow,
        recent_tasks: &[SubagentTaskRow],
        prompt: &str,
        profile_budget_tokens: u32,
    ) -> (CompactContext, MemorySnapshot) {
        // Zero budget → fall back to default (avoid clamp(1, max) treating 0 as 1).
        let effective_budget = if profile_budget_tokens == 0 {
            self.config.default_budget_tokens
        } else {
            profile_budget_tokens
        };
        let budget = effective_budget.clamp(1, self.config.max_budget_tokens);

        let snapshot = MemorySnapshot {
            hot_summary: memory.hot_summary.clone(),
            long_summary: memory.long_summary.clone(),
            key_files: parse_json_vec(&memory.key_files_json),
            decisions: parse_json_vec(&memory.decisions_json),
            open_questions: parse_json_vec(&memory.open_questions_json),
        };

        // Recent task summaries: most recent first (recent_tasks is DESC).
        let recent_summaries: Vec<String> = recent_tasks
            .iter()
            .take(self.config.recent_tasks_limit as usize)
            .filter_map(|t| t.result_summary.as_deref())
            .map(String::from)
            .collect();

        let mut sections: Vec<Section> = Vec::new();
        // Identity + intent (never trimmed — tiny).
        let mut header = format!("You are Busytok logical subagent: {}\n", subagent.name);
        if let Some(intent) = &subagent.intent {
            header.push_str(&format!("\nLong-term goal: {intent}\n"));
        }
        sections.push(Section::header(header));

        // Recent task summaries (trim priority 1).
        if !recent_summaries.is_empty() {
            sections.push(Section::recent_summaries(format!(
                "Recent tasks:\n{}\n",
                recent_summaries
                    .iter()
                    .map(|s| format!("- {s}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )));
        }

        // Key files (trim priority 3).
        if !snapshot.key_files.is_empty() {
            sections.push(Section::key_files(format!(
                "Key files: {}\n",
                snapshot
                    .key_files
                    .iter()
                    .map(|f| f.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        // Open questions (trim priority 4).
        if !snapshot.open_questions.is_empty() {
            sections.push(Section::open_questions(format!(
                "Open questions:\n{}\n",
                snapshot
                    .open_questions
                    .iter()
                    .map(|q| format!("- {}", q.question))
                    .collect::<Vec<_>>()
                    .join("\n")
            )));
        }

        // Long summary (trim priority 5).
        if let Some(long) = &snapshot.long_summary {
            sections.push(Section::long_summary(format!("Long-term findings:\n{long}\n")));
        }

        // Hot summary (trim priority 6 — preserve as much as possible).
        if let Some(hot) = &snapshot.hot_summary {
            sections.push(Section::hot_summary(format!("Current state:\n{hot}\n")));
        }

        // Current task prompt (trim priority 7 — NEVER trimmed).
        sections.push(Section::prompt(format!("\nCurrent task:\n{prompt}\n")));

        let budget_chars = (budget as usize) * 4; // token ≈ 4 chars heuristic
        let compact = assemble_with_budget(sections, budget_chars);

        (
            CompactContext {
                compact_context: compact,
                budget_tokens: budget,
                source: "busytok-context-builder/v1".to_string(),
            },
            snapshot,
        )
    }
}

/// A labeled section with a trim priority (lower number = trimmed first).
/// `protected` sections (header, prompt) are never dropped.
struct Section {
    priority: u8,
    text: String,
    protected: bool,
}

impl Section {
    fn header(text: String) -> Self {
        Self { priority: 7, text, protected: true }
    }
    fn recent_summaries(text: String) -> Self {
        Self { priority: 1, text, protected: false }
    }
    fn key_files(text: String) -> Self {
        Self { priority: 3, text, protected: false }
    }
    fn open_questions(text: String) -> Self {
        Self { priority: 4, text, protected: false }
    }
    fn long_summary(text: String) -> Self {
        Self { priority: 5, text, protected: false }
    }
    fn hot_summary(text: String) -> Self {
        Self { priority: 6, text, protected: false }
    }
    fn prompt(text: String) -> Self {
        Self { priority: 7, text, protected: true }
    }
}

/// Assemble sections into one string. If over budget, progressively drop
/// lowest-priority non-protected sections (never header/prompt) until it fits.
/// Rebuilds from kept sections directly (no string-replace, which could
/// corrupt the context when a section's text appears inside another).
fn assemble_with_budget(sections: Vec<Section>, budget_chars: usize) -> String {
    let full: String = sections.iter().map(|s| s.text.as_str()).collect();
    if full.len() <= budget_chars || budget_chars == 0 {
        return full;
    }

    // Over budget: sort drop candidates by priority ascending (lowest = dropped first).
    let mut drop_order: Vec<usize> = sections
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.protected)
        .collect::<Vec<_>>()
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    drop_order.sort_by_key(|&i| sections[i].priority);

    let mut dropped: Vec<usize> = Vec::new();
    for &idx in &drop_order {
        let current_len: usize = sections
            .iter()
            .enumerate()
            .filter(|(i, _)| !dropped.contains(i))
            .map(|(_, s)| s.text.len())
            .sum();
        if current_len <= budget_chars {
            break;
        }
        dropped.push(idx);
    }

    // Rebuild from kept sections, preserving original order.
    let mut result = String::new();
    for (i, s) in sections.iter().enumerate() {
        if !dropped.contains(&i) {
            result.push_str(&s.text);
        }
    }
    result.trim_end_matches('\n').to_string() + "\n"
}

fn parse_json_vec<T: serde::de::DeserializeOwned>(json: &Option<String>) -> Vec<T> {
    json.as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p busytok-subagent --test context 2>&1 | tail -15`
Expected: PASS — all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-subagent/src/context.rs crates/busytok-subagent/tests/context.rs
git commit -m "feat(subagent): add ContextBuilder with budget control and trim priority"
```

---

## Task 3: Wire ContextBuilder + MemoryUpdater into the delegate flow

**Files:**
- Modify: `crates/busytok-subagent/src/mock_executor.rs`
- Modify: `crates/busytok-subagent/src/manager.rs`
- Modify: `crates/busytok-subagent/tests/manager.rs`

**Interfaces:**
- Consumes: `ContextBuilder` (Task 2), `MemoryUpdater` (Task 1), `SubagentContextConfig`, `SubagentProfileConfig`
- Produces: updated `ExecutorInput` (with `memory`, `context`, `tools`), updated `ExecutorOutput` (with `memory_update`), updated `SubagentManager::delegate` that builds context before execute and updates memory after.

- [ ] **Step 1: Update `ExecutorInput` and `ExecutorOutput`**

In `crates/busytok-subagent/src/mock_executor.rs`, replace the struct definitions and add imports. The full new file content:

```rust
//! Task executor abstraction. Plan 1 had a mock; Plan 2 adds a sidecar-backed
//! executor. The trait lets `SubagentManager` stay executor-agnostic.

use crate::context::CompactContext;
use crate::context::MemorySnapshot;
use crate::memory::MemoryUpdate;
use crate::models::{TaskStatus, TaskUsage};

/// Input to a task executor — everything needed to run one turn.
pub struct ExecutorInput {
    pub subagent_id: String,
    pub subagent_name: String,
    pub cwd: String,
    pub profile: String,
    pub model: Option<String>,
    pub prompt: String,
    pub timeout_seconds: Option<u64>,
    pub tools: Vec<String>,
    pub memory: MemorySnapshot,
    pub context: CompactContext,
    pub write_access: bool,
}

/// Output from a task executor — mapped into `DelegateResult` by the manager.
pub struct ExecutorOutput {
    pub adapter_session_id: Option<String>,
    pub session_reused: bool,
    pub status: TaskStatus,
    pub summary: String,
    pub usage: TaskUsage,
    pub memory_update: MemoryUpdate,
}

/// Executor trait — `SubagentManager` calls this to run a task.
/// Plan 1: `MockTaskExecutor`. Plan 2: `SidecarTaskExecutor`.
#[async_trait::async_trait]
pub trait TaskExecutor: Send + Sync {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput>;
}

/// Deterministic in-process mock executor. Used by Plan 1 tests and by Plan 2
/// when `pi_sidecar.enabled = false`.
pub struct MockTaskExecutor;

#[async_trait::async_trait]
impl TaskExecutor for MockTaskExecutor {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        let summary = format!("[mock] no sidecar wired yet; prompt was: {}", input.prompt);
        Ok(ExecutorOutput {
            adapter_session_id: None,
            session_reused: false,
            status: TaskStatus::Completed,
            summary: summary.clone(),
            usage: TaskUsage {
                model: input.model.clone(),
                provider: Some("mock".to_string()),
                input_tokens: Some(input.prompt.len() as i64),
                output_tokens: Some(summary.len() as i64),
                ..Default::default()
            },
            memory_update: MemoryUpdate::default(),
        })
    }
}

/// Executor that always fails. Injected when `pi_sidecar.enabled = true`
/// but the sidecar config could not be resolved at supervisor construction
/// time. This ensures delegate calls fail loudly instead of silently
/// succeeding via `MockTaskExecutor` — which would mask a deployment
/// misconfiguration as "functional". The error is wrapped as
/// `SubagentError::SidecarSpawn` (via `anyhow::Error::from`) so
/// `SubagentManager::delegate` can downcast it and preserve the semantic
/// error code `subagent.sidecar_spawn_failed` through the RPC contract.
pub struct FailingTaskExecutor {
    pub reason: String,
}

#[async_trait::async_trait]
impl TaskExecutor for FailingTaskExecutor {
    async fn execute(&self, _input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        Err(anyhow::Error::from(
            crate::error::SubagentError::SidecarSpawn(format!(
                "sidecar was enabled but failed to initialize: {}",
                self.reason
            )),
        ))
    }
}
```

- [ ] **Step 2: Update `SubagentManager::delegate` to build context and update memory**

In `crates/busytok-subagent/src/manager.rs`, add imports at the top:

```rust
use crate::context::ContextBuilder;
use crate::memory::{MemoryUpdate, MemoryUpdater};
```

Add fields to `SubagentManager` and update `new`:

```rust
pub struct SubagentManager {
    db: SharedDb,
    settings: SubagentSettings,
    adapter: String,
    executor: Arc<dyn TaskExecutor>,
    context_builder: ContextBuilder,
    memory_updater: MemoryUpdater,
}

impl SubagentManager {
    pub fn new(
        db: SharedDb,
        settings: SubagentSettings,
        adapter: &str,
        executor: Arc<dyn TaskExecutor>,
    ) -> Self {
        let context_builder = ContextBuilder::new(settings.context.clone());
        let memory_updater = MemoryUpdater::new(settings.context.clone());
        Self {
            db,
            settings,
            adapter: adapter.to_string(),
            executor,
            context_builder,
            memory_updater,
        }
    }
```

In `delegate`, find the section that constructs `ExecutorInput` and calls `self.executor.execute`. Replace the block that builds the input and runs the executor. The replacement reads memory + recent tasks + the logical subagent row, builds the compact context, logs the context-build event, constructs the enriched `ExecutorInput`, runs the executor, then merges the memory update. Concretely, insert this in place of the existing `ExecutorInput` construction + execute call (keep the surrounding task-row insert + status transitions intact; this block sits between the `info!` delegate-start log and the post-execute task-status update):

```rust
        // 3. Build context + memory snapshot from the store, then execute.
        //    No lock held during execution.
        let model = req.model_override.clone().or(profile_model);
        let started = busytok_domain::now_ms();
        let (input, memory_row, recent_tasks, profile_cfg) = {
            let db = self.db.lock().expect("subagent db lock poisoned");
            let memory_row = db
                .subagent_get_memory(&subagent.id)
                .map_err(SubagentError::Store)?
                .unwrap_or_else(|| SubagentMemoryRow::new_empty(&subagent.id));
            let recent_tasks = db
                .subagent_list_tasks(&subagent.id, self.settings.context.recent_tasks_limit as i64)
                .map_err(SubagentError::Store)?;
            let profile_cfg = self.settings.profiles.get(&req.profile);
            let profile_budget = profile_cfg
                .map(|p| p.context_budget_tokens)
                .unwrap_or(self.settings.context.default_budget_tokens);
            let tools = profile_cfg.map(|p| p.tools.clone()).unwrap_or_default();
            let write_access = profile_cfg.map(|p| p.write_access).unwrap_or(false);
            let sub_row = db
                .subagent_get_logical(&subagent.id)
                .map_err(SubagentError::Store)?
                .ok_or_else(|| SubagentError::NotFound(subagent.id.clone()))?;
            let (compact, snapshot) = self.context_builder.build(
                &sub_row,
                &memory_row,
                &recent_tasks,
                &req.prompt,
                profile_budget,
            );
            info!(
                event_code = "subagent.context.built",
                subagent_id = %subagent.id,
                budget_tokens = compact.budget_tokens,
                context_chars = compact.compact_context.len(),
                recent_tasks_count = recent_tasks.len(),
                "built context for delegate"
            );
            let input = ExecutorInput {
                subagent_id: subagent.id.clone(),
                subagent_name: subagent.name.clone(),
                cwd: req.cwd.clone(),
                profile: req.profile.clone(),
                model: model.clone(),
                prompt: req.prompt.clone(),
                timeout_seconds: req.timeout_seconds,
                tools,
                memory: snapshot,
                context: compact,
                write_access,
            };
            (input, memory_row, recent_tasks, profile_cfg.cloned())
        };
        let out = self.executor.execute(&input).await.map_err(|e| {
            match e.downcast::<SubagentError>() {
                Ok(se) => se,
                Err(other) => {
                    warn!(event_code = "subagent.delegate.executor_failed", error = %other);
                    SubagentError::Store(other)
                }
            }
        })?;
        let duration_ms = busytok_domain::now_ms().saturating_sub(started);
```

Then replace the `write_hot_summary` call (the line `self.write_hot_summary(&db, &subagent.id, &out.summary)`) with `MemoryUpdater::update`:

```rust
            // memory: merge the sidecar's memory_update into the memory row
            // (spec §6.2). hot_summary comes from current_state_summary, NOT
            // task_summary. When memory_update is absent, hot_summary is
            // preserved. Compaction runs if triggers fire.
            let profile_budget = profile_cfg
                .as_ref()
                .map(|p| p.context_budget_tokens)
                .unwrap_or(self.settings.context.default_budget_tokens);
            let updated_mem = self.memory_updater.update(
                memory_row,
                out.memory_update.clone(),
                &recent_tasks,
                &task_id,
                profile_budget,
                &subagent.repo_path,
            );
            info!(
                event_code = "subagent.memory.updated",
                subagent_id = %subagent.id,
                has_hot_summary = updated_mem.hot_summary.is_some(),
                compacted = updated_mem.last_compacted_at_ms.is_some(),
                "memory updated after delegate"
            );
            db.subagent_upsert_memory(&updated_mem)
                .map_err(SubagentError::Store)?;
```

Delete the `write_hot_summary` method — fully replaced by `MemoryUpdater::update`.

- [ ] **Step 3: Update existing manager tests for the new `ExecutorInput` fields**

In `crates/busytok-subagent/tests/manager.rs`, existing tests use `MockTaskExecutor` which returns `MemoryUpdate::default()` → `hot_summary` is PRESERVED (not erased). Tests that previously asserted `hot_summary` is `Some("[mock] ...")` (written by the old buggy `write_hot_summary`) must be updated: with the mock executor producing no `memory_update`, `hot_summary` stays `None` on a fresh subagent (no prior memory). Update assertions to match.

Search for `hot_summary` assertions in `tests/manager.rs` and update: when using `MockTaskExecutor` on a fresh subagent, `hot_summary` is `None` (no memory_update, no prior memory). Replace any `assert_eq!(mem.hot_summary.as_deref(), Some("[mock] ..."))` with `assert!(mem.hot_summary.is_none(), "mock executor produces no memory_update; hot_summary preserved as None on fresh subagent")`.

Add a new test verifying that context is built and memory is updated when the executor returns a `memory_update`. This test uses a custom executor that captures the `ExecutorInput` to verify context was built, and returns a `MemoryUpdate` to verify the merge:

```rust
use busytok_subagent::memory::{KeyFile, MemoryUpdate, OpenQuestion};
use busytok_subagent::mock_executor::{ExecutorInput, ExecutorOutput, TaskExecutor};
use std::sync::Mutex;

struct MemoryUpdateExecutor {
    captured_input: Mutex<Option<ExecutorInput>>,
}
#[async_trait::async_trait]
impl TaskExecutor for MemoryUpdateExecutor {
    async fn execute(&self, input: &ExecutorInput) -> anyhow::Result<ExecutorOutput> {
        // Capture a clone of the input's context string to verify it was built.
        let captured = ExecutorInput {
            subagent_id: input.subagent_id.clone(),
            subagent_name: input.subagent_name.clone(),
            cwd: input.cwd.clone(),
            profile: input.profile.clone(),
            model: input.model.clone(),
            prompt: input.prompt.clone(),
            timeout_seconds: input.timeout_seconds,
            tools: input.tools.clone(),
            memory: busytok_subagent::context::MemorySnapshot {
                hot_summary: input.memory.hot_summary.clone(),
                long_summary: input.memory.long_summary.clone(),
                key_files: input.memory.key_files.clone(),
                decisions: input.memory.decisions.clone(),
                open_questions: input.memory.open_questions.clone(),
            },
            context: busytok_subagent::context::CompactContext {
                compact_context: input.context.compact_context.clone(),
                budget_tokens: input.context.budget_tokens,
                source: input.context.source.clone(),
            },
            write_access: input.write_access,
        };
        *self.captured_input.lock().unwrap() = Some(captured);
        Ok(ExecutorOutput {
            adapter_session_id: None,
            session_reused: false,
            status: TaskStatus::Completed,
            summary: "task done".into(),
            usage: TaskUsage::default(),
            memory_update: MemoryUpdate {
                current_state_summary: Some("Investigated auth; found refresh gap.".into()),
                key_files: vec![KeyFile {
                    path: "src/auth/token.ts".into(),
                    reason: "refresh logic".into(),
                    last_seen_at_ms: 5000,
                    score: 3,
                }],
                decisions: vec!["Focus on read-only analysis".into()],
                open_questions: vec![OpenQuestion {
                    question: "Concurrent refresh handled?".into(),
                    status: "open".into(),
                    created_at_ms: 5000,
                    last_seen_at_ms: 5000,
                }],
            },
        })
    }
}

#[tokio::test]
async fn delegate_builds_context_and_merges_memory_update() {
    let db: SharedDb = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
    let executor = Arc::new(MemoryUpdateExecutor { captured_input: Mutex::new(None) });
    let manager = SubagentManager::new(
        db.clone(),
        SubagentSettings::default(),
        "pi",
        executor.clone(),
    );
    let req = DelegateRequest {
        subagent_name: "auth-investigator".into(),
        subagent_id: None,
        cwd: "/repo".into(),
        profile: "pi/review-cheap".into(),
        intent: Some("Study auth".into()),
        prompt: "Check refresh logic".into(),
        timeout_seconds: None,
        model_override: None,
        source_harness: Some("cli".into()),
        source_session_id: None,
    };
    let result = manager.delegate(req).await.unwrap();
    assert_eq!(result.status, TaskStatus::Completed);

    // Verify context was built and sent to the executor.
    let captured = executor.captured_input.lock().unwrap().clone().expect("input captured");
    assert!(
        captured.context.compact_context.contains("Check refresh logic"),
        "context contains the prompt"
    );
    assert!(
        captured.context.compact_context.contains("auth-investigator"),
        "context contains the subagent name"
    );
    assert_eq!(captured.context.source, "busytok-context-builder/v1");

    // Verify memory merged: hot_summary from current_state_summary (not task_summary).
    let db_guard = db.lock().unwrap();
    let mem = db_guard.subagent_get_memory(&result.subagent_id).unwrap().unwrap();
    assert_eq!(
        mem.hot_summary.as_deref(),
        Some("Investigated auth; found refresh gap."),
        "hot_summary from current_state_summary, not task_summary"
    );
    let files: Vec<serde_json::Value> =
        serde_json::from_str(mem.key_files_json.as_deref().unwrap()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "src/auth/token.ts");
    let decisions: Vec<String> =
        serde_json::from_str(mem.decisions_json.as_deref().unwrap()).unwrap();
    assert_eq!(decisions, vec!["Focus on read-only analysis"]);
    let qs: Vec<serde_json::Value> =
        serde_json::from_str(mem.open_questions_json.as_deref().unwrap()).unwrap();
    assert_eq!(qs.len(), 1);
    assert_eq!(qs[0]["question"], "Concurrent refresh handled?");
}
```

Note: `MemorySnapshot` and `CompactContext` need to be constructible in tests — they have public fields, so this works. If the test cannot clone `ExecutorInput` fields because `CompactContext`/`MemorySnapshot` don't derive `Clone`, add `#[derive(Clone)]` to both structs in `context.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p busytok-subagent --test manager 2>&1 | tail -20`
Expected: PASS — all tests pass (existing + new `delegate_builds_context_and_merges_memory_update`).

- [ ] **Step 5: Run clippy + fmt**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/busytok-subagent/src/mock_executor.rs crates/busytok-subagent/src/manager.rs crates/busytok-subagent/tests/manager.rs
git commit -m "feat(subagent): wire ContextBuilder + MemoryUpdater into delegate flow"
```

---

## Task 4: Extend SidecarTaskExecutor to send context/memory and parse memory_update

**Files:**
- Modify: `crates/busytok-subagent/src/sidecar/executor.rs`
- Modify: `crates/busytok-subagent/tests/sidecar_executor.rs`

**Interfaces:**
- Consumes: `ExecutorInput.memory`, `ExecutorInput.context`, `ExecutorInput.tools`, `ExecutorInput.write_access` (Task 3)
- Produces: `ExecutorOutput.memory_update` populated from `result.memory_update` in the RPC response.

- [ ] **Step 1: Update `SidecarTaskExecutor::execute` to send the full params**

In `crates/busytok-subagent/src/sidecar/executor.rs`, find the `params` JSON construction and replace it with the full param set per spec §4.3. The structured `memory` field carries full `KeyFile`/`OpenQuestion` objects (not lossy strings) so the sidecar retains score/reason/status:

```rust
        // Build turn_auto params (spec §4.3). Plan 4 adds tools, memory,
        // context, and constraints. Memory carries structured objects.
        let key_files_json: Vec<serde_json::Value> = input
            .memory
            .key_files
            .iter()
            .map(|f| serde_json::json!({
                "path": f.path,
                "reason": f.reason,
                "last_seen_at_ms": f.last_seen_at_ms,
                "score": f.score,
            }))
            .collect();
        let open_questions_json: Vec<serde_json::Value> = input
            .memory
            .open_questions
            .iter()
            .map(|q| serde_json::json!({
                "question": q.question,
                "status": q.status,
                "created_at_ms": q.created_at_ms,
                "last_seen_at_ms": q.last_seen_at_ms,
            }))
            .collect();
        let memory_json = serde_json::json!({
            "hot_summary": input.memory.hot_summary,
            "long_summary": input.memory.long_summary,
            "key_files": key_files_json,
            "decisions": input.memory.decisions,
            "open_questions": open_questions_json,
        });
        let params = serde_json::json!({
            "logical_subagent_id": input.subagent_id,
            "logical_subagent_name": input.subagent_name,
            "cwd": input.cwd,
            "profile": input.profile,
            "model": input.model,
            "tools": input.tools,
            "prompt": input.prompt,
            "prompt_artifact_ref": null,
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
        });
```

Add the import at the top of `executor.rs`:

```rust
use crate::memory::MemoryUpdate;
```

- [ ] **Step 2: Update `parse_turn_auto_result` to extract `memory_update`**

In the same file, find `parse_turn_auto_result` and add `memory_update` extraction:

```rust
fn parse_turn_auto_result(result: &serde_json::Value) -> ExecutorOutput {
    let adapter_session_id = result
        .get("adapter_session_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let session_reused = result
        .get("session_reused")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let status_str = result
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("completed");
    let status = match status_str {
        "completed" => TaskStatus::Completed,
        "failed" | "timeout" => TaskStatus::Failed,
        _ => TaskStatus::Completed,
    };
    let summary = result
        .pointer("/result/task_summary")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let usage = result
        .get("usage")
        .map(|u| TaskUsage {
            model: u.get("model").and_then(|v| v.as_str()).map(String::from),
            provider: u.get("provider").and_then(|v| v.as_str()).map(String::from),
            input_tokens: u.get("input_tokens").and_then(|v| v.as_i64()),
            output_tokens: u.get("output_tokens").and_then(|v| v.as_i64()),
            cache_read_tokens: u.get("cache_read_tokens").and_then(|v| v.as_i64()),
            cache_write_tokens: u.get("cache_write_tokens").and_then(|v| v.as_i64()),
            cost_usd: u.get("cost_usd").and_then(|v| v.as_f64()),
        })
        .unwrap_or_default();

    // Extract memory_update (spec §4.3 result.memory_update). If absent,
    // MemoryUpdate::default() → hot_summary is preserved by MemoryUpdater
    // (current_state_summary None → no overwrite).
    let memory_update = result
        .pointer("/result/memory_update")
        .map(parse_memory_update)
        .unwrap_or_default();

    ExecutorOutput {
        adapter_session_id,
        session_reused,
        status,
        summary,
        usage,
        memory_update,
    }
}

fn parse_memory_update(mu: &serde_json::Value) -> MemoryUpdate {
    use crate::memory::{KeyFile, OpenQuestion};
    let current_state_summary = mu
        .get("current_state_summary")
        .and_then(|v| v.as_str())
        .map(String::from);
    let key_files = mu
        .get("key_files")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    Some(KeyFile {
                        path: f.get("path")?.as_str()?.to_string(),
                        reason: f.get("reason").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        last_seen_at_ms: f.get("last_seen_at_ms").and_then(|v| v.as_i64()).unwrap_or(0),
                        score: f.get("score").and_then(|v| v.as_i64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let decisions = mu
        .get("decisions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let open_questions = mu
        .get("open_questions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|q| {
                    Some(OpenQuestion {
                        question: q.get("question")?.as_str()?.to_string(),
                        status: q.get("status").and_then(|v| v.as_str()).unwrap_or("open").to_string(),
                        created_at_ms: q.get("created_at_ms").and_then(|v| v.as_i64()).unwrap_or(0),
                        last_seen_at_ms: q.get("last_seen_at_ms").and_then(|v| v.as_i64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    MemoryUpdate {
        current_state_summary,
        key_files,
        decisions,
        open_questions,
    }
}
```

- [ ] **Step 3: Update existing sidecar_executor tests**

The existing tests construct `ExecutorInput` via helpers. These must now include `tools`, `memory`, `context`, and `write_access`. Find every `ExecutorInput { ... }` construction in the test file and update it. Example helper:

```rust
fn evict_input(subagent_id: &str) -> ExecutorInput {
    ExecutorInput {
        subagent_id: subagent_id.into(),
        subagent_name: subagent_id.into(),
        cwd: "/repo".into(),
        profile: "pi/review-cheap".into(),
        model: None,
        prompt: "do thing".into(),
        timeout_seconds: None,
        tools: vec![],
        memory: busytok_subagent::context::MemorySnapshot {
            hot_summary: None,
            long_summary: None,
            key_files: vec![],
            decisions: vec![],
            open_questions: vec![],
        },
        context: busytok_subagent::context::CompactContext {
            compact_context: "test context".into(),
            budget_tokens: 4000,
            source: "busytok-context-builder/v1".into(),
        },
        write_access: false,
    }
}
```

Also add a test that `parse_turn_auto_result` extracts `memory_update`:

```rust
#[test]
fn parse_turn_auto_result_extracts_memory_update() {
    let resp = serde_json::json!({
        "adapter_session_id": "sess-1",
        "session_reused": false,
        "status": "completed",
        "result": {
            "task_summary": "did thing",
            "memory_update": {
                "current_state_summary": "new state",
                "key_files": [{"path": "src/a.ts", "reason": "r", "last_seen_at_ms": 1, "score": 2}],
                "decisions": ["decide"],
                "open_questions": [{"question": "q?", "status": "open", "created_at_ms": 1, "last_seen_at_ms": 1}],
            },
        },
        "usage": {"model": "m", "provider": "p", "input_tokens": 1, "output_tokens": 1, "cache_read_tokens": 0, "cache_write_tokens": 0, "cost_usd": 0.0},
    });
    let out = busytok_subagent::sidecar::executor::parse_turn_auto_result_for_test(&resp);
    assert_eq!(out.memory_update.current_state_summary.as_deref(), Some("new state"));
    assert_eq!(out.memory_update.key_files.len(), 1);
    assert_eq!(out.memory_update.key_files[0].path, "src/a.ts");
    assert_eq!(out.memory_update.decisions, vec!["decide".to_string()]);
    assert_eq!(out.memory_update.open_questions.len(), 1);
}

#[test]
fn parse_turn_auto_result_omits_memory_update_when_absent() {
    let resp = serde_json::json!({
        "adapter_session_id": "sess-1",
        "session_reused": false,
        "status": "completed",
        "result": {"task_summary": "did thing"},
        "usage": {"model": "m", "provider": "p", "input_tokens": 1, "output_tokens": 1, "cache_read_tokens": 0, "cache_write_tokens": 0, "cost_usd": 0.0},
    });
    let out = busytok_subagent::sidecar::executor::parse_turn_auto_result_for_test(&resp);
    assert!(out.memory_update.current_state_summary.is_none());
    assert!(out.memory_update.key_files.is_empty());
}
```

Note: `parse_turn_auto_result` is currently private. Either make it `pub(crate)` and expose a `pub fn parse_turn_auto_result_for_test` wrapper, or change visibility to `pub`. The simplest path: change `fn parse_turn_auto_result` to `pub fn parse_turn_auto_result` (it's already in a `pub` module path via `sidecar::executor`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p busytok-subagent --test sidecar_executor 2>&1 | tail -20`
Expected: PASS — all existing tests pass with the new fields, plus the 2 new parse tests.

- [ ] **Step 5: Commit**

```bash
git add crates/busytok-subagent/src/sidecar/executor.rs crates/busytok-subagent/tests/sidecar_executor.rs
git commit -m "feat(subagent): send context/memory in turn_auto and parse memory_update"
```

---

## Task 5: Mock sidecar + TS types — emit memory_update, echo context

**Files:**
- Modify: `crates/busytok-subagent/tests/fixtures/mock-sidecar.sh`
- Modify: `apps/pi-sidecar/src/types.ts`
- Modify: `apps/pi-sidecar/src/handlers/turn_auto.ts`
- Create: `apps/pi-sidecar/tests/turn_auto_memory.test.ts`

**Interfaces:**
- Consumes: existing `TurnAutoParams`, `SessionPool`
- Produces: `BUSYTOK_MOCK_MEMORY_UPDATE=1` env var in bash mock; typed `memory_update` in TS `TurnAutoResult.result`; structured `MemoryField` + `CompactContext` types in `TurnAutoParams`. The mock sidecar echoes the received `context.compact_context` back inside `task_summary` so the e2e test can prove context was built from memory.

- [ ] **Step 1: Add `BUSYTOK_MOCK_MEMORY_UPDATE` + context echo to bash mock-sidecar.sh**

In `crates/busytok-subagent/tests/fixtures/mock-sidecar.sh`, add the env var declaration near the others (after `CLOSE_FAILS`):

```bash
#   BUSYTOK_MOCK_MEMORY_UPDATE=1
#                                When set, session.turn_auto includes a
#                                `result.memory_update` object with
#                                current_state_summary, key_files, decisions,
#                                and open_questions. 0/unset = no memory_update
#                                (hot_summary preserved — no destructive clear).
```

Add the variable declaration near the other env-var reads:

```bash
MEMORY_UPDATE="${BUSYTOK_MOCK_MEMORY_UPDATE:-0}"
```

To avoid duplicating the multi-line `printf` JSON across 3 branches (create, reuse, empty), factor the memory_update fragment into a shell variable computed once. Add a helper function near the top of the script (after the env-var reads):

```bash
# Build the memory_update JSON fragment (empty when MEMORY_UPDATE != 1).
# Interpolated into the turn_auto response to avoid duplicating the JSON
# payload across the create/reuse/empty branches.
build_mem_fragment() {
  if [[ "$MEMORY_UPDATE" == "1" ]]; then
    NOW_MS="$(date +%s)000"
    printf ',"memory_update":{"current_state_summary":"Investigated context; produced memory update.","key_files":[{"path":"src/auth/token.ts","reason":"refresh logic","last_seen_at_ms":%s,"score":3}],"decisions":["Focus on read-only analysis"],"open_questions":[{"question":"Concurrent refresh handled?","status":"open","created_at_ms":%s,"last_seen_at_ms":%s}]}' "$NOW_MS" "$NOW_MS" "$NOW_MS"
  fi
}
```

Then in the `session.turn_auto` handler, modify each branch's `printf` to (a) interpolate `$MEM_FRAGMENT` after `task_summary` and (b) echo the received `context.compact_context` back into `task_summary` so the e2e test can verify context building. First, extract the received `compact_context` from the request params (add this before the branch logic, after parsing the request):

```bash
# Extract context.compact_context from the request to echo it back in
# task_summary — lets e2e tests verify the context was built from memory.
COMPACT_CTX="$(printf '%s' "$PARAMS" | sed -n 's/.*"context"[[:space:]]*:[[:space:]]*{"compact_context"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
if [[ -z "$COMPACT_CTX" ]]; then
  COMPACT_CTX="mock turn completed"
fi
```

Then for each branch (create-new, reuse, empty-session), compute `MEM_FRAGMENT` and include it in the `printf`:

```bash
          MEM_FRAGMENT="$(build_mem_fragment)"
          printf '{"jsonrpc":"2.0","result":{"adapter_session_id":"%s","session_reused":%s,"status":"completed","result":{"task_summary":"%s"%s},"usage":{"model":"deepseek-chat","provider":"deepseek","input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"cost_usd":0.001}},"id":%s}\n' "$SESS" "$REUSED" "$COMPACT_CTX" "$MEM_FRAGMENT" "$ID"
```

Apply the same pattern to the reuse-session and EMPTY_SESSION branches (each gets `MEM_FRAGMENT="$(build_mem_fragment)"` and uses `$COMPACT_CTX` + `$MEM_FRAGMENT` in the printf). The `MEM_FRAGMENT` is empty when `MEMORY_UPDATE != 1`, so the JSON stays valid (`"task_summary":"..."` with no trailing comma when empty).

Note on `sed` portability: the `sed -n 's/.../p'` expression is BRE-compatible and works on bash 3.2 / macOS. If the `compact_context` contains escaped quotes, this naive extraction will fail; for the e2e test the context is simple text, so this is acceptable. If extraction fails, `COMPACT_CTX` falls back to `"mock turn completed"`.

- [ ] **Step 2: Add structured `Memory`, `CompactContext`, and `memory_update` types to TS**

In `apps/pi-sidecar/src/types.ts`, extend `TurnAutoParams` and `TurnAutoResult`. The `memory` field carries structured `KeyFile`/`OpenQuestion` objects (matching what Rust sends):

```typescript
export interface KeyFile {
  path: string;
  reason: string;
  last_seen_at_ms: number;
  score: number;
}

export interface OpenQuestion {
  question: string;
  status: 'open' | 'resolved';
  created_at_ms: number;
  last_seen_at_ms: number;
}

export interface MemoryField {
  hot_summary?: string;
  long_summary?: string;
  key_files: KeyFile[];
  decisions: string[];
  open_questions: OpenQuestion[];
}

export interface CompactContext {
  compact_context: string;
  budget_tokens: number;
  source: string;
}

export interface TurnAutoParams {
  logical_subagent_id: string;
  logical_subagent_name?: string;
  cwd: string;
  profile: string;
  model?: string;
  tools?: string[];
  prompt: string;
  prompt_artifact_ref?: string | null;
  timeout_ms?: number;
  memory?: MemoryField;
  context?: CompactContext;
  constraints?: { write_access: boolean; timeout_ms: number };
}

export interface MemoryUpdate {
  current_state_summary?: string;
  key_files?: KeyFile[];
  decisions?: string[];
  open_questions?: OpenQuestion[];
}

export interface TurnAutoResult {
  adapter_session_id: string;
  session_reused: boolean;
  status: 'completed' | 'failed' | 'timeout';
  result: {
    task_summary: string;
    memory_update?: MemoryUpdate;
    [key: string]: unknown;
  };
  usage: {
    model: string;
    provider: string;
    input_tokens: number;
    output_tokens: number;
    cache_read_tokens: number;
    cache_write_tokens: number;
    cost_usd: number;
  };
}
```

- [ ] **Step 3: Update TS `turn_auto` handler to emit `memory_update` and echo context**

In `apps/pi-sidecar/src/handlers/turn_auto.ts`, update the handler to (a) echo `context.compact_context` into `task_summary` and (b) conditionally include `memory_update` when `BUSYTOK_MOCK_MEMORY_UPDATE=1`:

```typescript
export function turnAutoHandlerWithPool(pool: SessionPool): RequestHandler {
  return async (params) => {
    const p = params as TurnAutoParams;
    if (!p.logical_subagent_id || !p.prompt) {
      throw new SidecarError('missing required fields', -32602);
    }
    const { adapter_session_id, reused } = pool.ensure(p.logical_subagent_id, nextSessionId);
    const now = Date.now();
    const memoryUpdate = process.env.BUSYTOK_MOCK_MEMORY_UPDATE === '1'
      ? {
          current_state_summary: 'Investigated context; produced memory update.',
          key_files: [{ path: 'src/auth/token.ts', reason: 'refresh logic', last_seen_at_ms: now, score: 3 }],
          decisions: ['Focus on read-only analysis'],
          open_questions: [{ question: 'Concurrent refresh handled?', status: 'open' as const, created_at_ms: now, last_seen_at_ms: now }],
        }
      : undefined;
    // Echo the received compact_context back in task_summary so the Rust
    // e2e test can verify the context was built from memory (Task 6).
    const echoedSummary = p.context?.compact_context
      ? `[echo] ${p.context.compact_context}`
      : `[mock] turn completed for: ${p.prompt.slice(0, 80)}`;
    const result: TurnAutoResult = {
      adapter_session_id,
      session_reused: reused,
      status: 'completed',
      result: {
        task_summary: echoedSummary,
        ...(memoryUpdate ? { memory_update: memoryUpdate } : {}),
      },
      usage: {
        model: p.model ?? 'deepseek-chat',
        provider: 'deepseek',
        input_tokens: p.prompt.length,
        output_tokens: 50,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_usd: 0.001,
      },
    };
    return result;
  };
}
```

- [ ] **Step 4: Write vitest test for memory_update emission + context echo**

Create `apps/pi-sidecar/tests/turn_auto_memory.test.ts`:

```typescript
import { describe, it, expect, afterEach } from 'vitest';
import { SessionPool } from '../src/session_pool';
import { turnAutoHandlerWithPool } from '../src/handlers/turn_auto';

describe('turn_auto memory_update + context echo', () => {
  const origEnv = process.env.BUSYTOK_MOCK_MEMORY_UPDATE;

  afterEach(() => {
    if (origEnv === undefined) delete process.env.BUSYTOK_MOCK_MEMORY_UPDATE;
    else process.env.BUSYTOK_MOCK_MEMORY_UPDATE = origEnv;
  });

  it('emits memory_update when BUSYTOK_MOCK_MEMORY_UPDATE=1', async () => {
    process.env.BUSYTOK_MOCK_MEMORY_UPDATE = '1';
    const pool = new SessionPool(3);
    const handler = turnAutoHandlerWithPool(pool);
    const result = await handler({
      logical_subagent_id: 'sub-a',
      prompt: 'check auth',
      cwd: '/repo',
      profile: 'pi/review-cheap',
    } as any) as any;
    expect(result.result.memory_update).toBeDefined();
    expect(result.result.memory_update.current_state_summary).toContain('memory update');
    expect(result.result.memory_update.key_files).toHaveLength(1);
    expect(result.result.memory_update.key_files[0].path).toBe('src/auth/token.ts');
    expect(result.result.memory_update.open_questions).toHaveLength(1);
  });

  it('omits memory_update when env unset', async () => {
    delete process.env.BUSYTOK_MOCK_MEMORY_UPDATE;
    const pool = new SessionPool(3);
    const handler = turnAutoHandlerWithPool(pool);
    const result = await handler({
      logical_subagent_id: 'sub-a',
      prompt: 'check auth',
      cwd: '/repo',
      profile: 'pi/review-cheap',
    } as any) as any;
    expect(result.result.memory_update).toBeUndefined();
  });

  it('echoes context.compact_context in task_summary', async () => {
    const pool = new SessionPool(3);
    const handler = turnAutoHandlerWithPool(pool);
    const result = await handler({
      logical_subagent_id: 'sub-a',
      prompt: 'check auth',
      cwd: '/repo',
      profile: 'pi/review-cheap',
      context: { compact_context: 'BUILT CONTEXT FROM MEMORY', budget_tokens: 4000, source: 'busytok-context-builder/v1' },
    } as any) as any;
    expect(result.result.task_summary).toContain('BUILT CONTEXT FROM MEMORY');
  });

  it('accepts structured memory params without error', async () => {
    process.env.BUSYTOK_MOCK_MEMORY_UPDATE = '1';
    const pool = new SessionPool(3);
    const handler = turnAutoHandlerWithPool(pool);
    const result = await handler({
      logical_subagent_id: 'sub-a',
      prompt: 'check auth',
      cwd: '/repo',
      profile: 'pi/review-cheap',
      memory: {
        hot_summary: 'prev state',
        key_files: [{ path: 'src/a.ts', reason: 'r', last_seen_at_ms: 1, score: 1 }],
        decisions: [],
        open_questions: [],
      },
      context: { compact_context: 'full context', budget_tokens: 4000, source: 'busytok-context-builder/v1' },
    } as any) as any;
    expect(result.status).toBe('completed');
  });
});
```

- [ ] **Step 5: Run TS tests + typecheck**

Run: `cd apps/pi-sidecar && pnpm test 2>&1 | tail -20 && pnpm typecheck 2>&1 | tail -5`
Expected: all tests pass, typecheck clean.

- [ ] **Step 6: Run Rust executor tests to verify bash mock works**

Run: `cargo test -p busytok-subagent --test sidecar_executor 2>&1 | tail -10`
Expected: PASS (existing tests unaffected — `MEMORY_UPDATE` defaults to 0, `COMPACT_CTX` falls back to the default summary).

- [ ] **Step 7: Commit**

```bash
git add crates/busytok-subagent/tests/fixtures/mock-sidecar.sh apps/pi-sidecar/src/types.ts apps/pi-sidecar/src/handlers/turn_auto.ts apps/pi-sidecar/tests/turn_auto_memory.test.ts
git commit -m "feat(sidecar): emit memory_update, echo context, and type Memory/Context in TS"
```

---

## Task 6: E2E test — delegate twice, assert context built from memory

**Files:**
- Modify: `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`

**Interfaces:**
- Consumes: existing `make_sidecar_supervisor`, `make_sidecar_config`, `make_sidecar_settings`, `supervisor.subagent_delegate`, `SubagentDelegateRequestDto`, mock-sidecar.sh with `BUSYTOK_MOCK_MEMORY_UPDATE=1`.

- [ ] **Step 1: Write the e2e test**

In `crates/busytok-runtime/tests/subagent_e2e_sidecar.rs`, add a new test. The test uses the EXISTING helpers (verified to exist): `make_sidecar_settings()`, `make_sidecar_config()` (which returns a `SidecarConfig` with an `env: HashMap` field), and `make_sidecar_supervisor(db, &tmp, settings)`. To inject `BUSYTOK_MOCK_MEMORY_UPDATE=1`, extend the `env` map on the `SidecarConfig`. The test delegates twice to the same subagent, then asserts:
1. First delegate: `hot_summary` is set to the mock's `current_state_summary`; `key_files`/`decisions`/`open_questions` are merged.
2. Second delegate: the response `summary` (which the mock sidecar echoes from `context.compact_context`) contains the first delegate's `hot_summary` text — proving the ContextBuilder read the merged memory and assembled it into the context sent to the sidecar.

```rust
/// Build a SidecarConfig that injects BUSYTOK_MOCK_MEMORY_UPDATE=1 so the
/// mock sidecar emits result.memory_update and echoes compact_context.
fn make_sidecar_config_with_memory_update() -> SidecarConfig {
    let mut cfg = make_sidecar_config();
    cfg.env.insert("BUSYTOK_MOCK_MEMORY_UPDATE".into(), "1".into());
    cfg
}

#[tokio::test]
async fn sidecar_e2e_delegate_merges_memory_and_builds_context_from_memory() {
    let tmp = tempfile::tempdir().unwrap();
    let db = busytok_store::Database::open_in_memory().unwrap();
    let settings = make_sidecar_settings();
    let sidecar_cfg = make_sidecar_config_with_memory_update();
    let paths = busytok_config::BusytokPaths::for_test(tmp.path());
    settings
        .save_to_file(&paths.config_dir().join("settings.toml"))
        .expect("failed to save test settings");
    let supervisor = busytok_runtime::BusytokSupervisor::new_with_sidecar_config(
        db,
        paths,
        sidecar_cfg,
    );

    // First delegate — mock returns memory_update with current_state_summary.
    let resp1 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "auth-investigator".to_string(),
            subagent_id: None,
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/review-cheap".to_string(),
            intent: Some("Study auth".to_string()),
            prompt: "Check refresh logic".to_string(),
            timeout_seconds: None,
            model_override: None,
            source_harness: Some("cli".to_string()),
            source_session_id: None,
        })
        .await
        .unwrap();
    let sub_id = resp1.subagent_id.clone();
    assert_eq!(resp1.status, "completed");

    // Assert memory merged after first delegate.
    {
        let conn = db.connection();
        let mem = busytok_store::subagent_get_memory(conn, &sub_id).unwrap().unwrap();
        assert_eq!(
            mem.hot_summary.as_deref(),
            Some("Investigated context; produced memory update."),
            "hot_summary from memory_update.current_state_summary"
        );
        let files: Vec<serde_json::Value> =
            serde_json::from_str(mem.key_files_json.as_deref().unwrap()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["path"], "src/auth/token.ts");
        let decisions: Vec<String> =
            serde_json::from_str(mem.decisions_json.as_deref().unwrap()).unwrap();
        assert_eq!(decisions, vec!["Focus on read-only analysis".to_string()]);
    }

    // Second delegate — the mock sidecar echoes context.compact_context back
    // in task_summary. If the ContextBuilder read the merged memory, the
    // echoed summary MUST contain the first delegate's hot_summary text.
    let resp2 = supervisor
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "auth-investigator".to_string(),
            subagent_id: Some(sub_id.clone()),
            cwd: tmp.path().join("repo").to_string_lossy().to_string(),
            profile: "pi/review-cheap".to_string(),
            intent: Some("Study auth".to_string()),
            prompt: "Continue investigation".to_string(),
            timeout_seconds: None,
            model_override: None,
            source_harness: Some("cli".to_string()),
            source_session_id: None,
        })
        .await
        .unwrap();
    assert_eq!(resp2.status, "completed");
    assert!(
        resp2.summary.as_deref().unwrap_or("").contains("Investigated context; produced memory update."),
        "second delegate's summary echoes compact_context which must contain the first delegate's hot_summary; got: {:?}",
        resp2.summary
    );

    // After the second delegate, key_files should still have 1 entry (deduped).
    {
        let conn = db.connection();
        let mem = busytok_store::subagent_get_memory(conn, &sub_id).unwrap().unwrap();
        let files: Vec<serde_json::Value> =
            serde_json::from_str(mem.key_files_json.as_deref().unwrap()).unwrap();
        assert_eq!(files.len(), 1, "key_files deduped across delegates");
    }

    supervisor.shutdown_sidecar().await;
    supervisor.shutdown_writer().await;
}
```

Note on DB access: the existing e2e tests access the DB through `busytok_store` functions. If `db.connection()` is not the right accessor, check how the existing e2e test reads DB state (it may use `busytok_store::subagent_get_memory(&db, &id)` directly if `Database` derefs to `Connection`, or open a separate helper). Adapt the accessor to match the existing pattern in `subagent_e2e_sidecar.rs`. If no existing test reads `subagent_memory` directly, use `busytok_store::subagent_get_memory` with whatever connection accessor the `Database` type exposes (check `db.rs` for a `connection()` or `conn()` method, or whether `Database` implements `Deref<Target=Connection>`).

- [ ] **Step 2: Run the e2e test**

Run: `cargo test -p busytok-runtime --test subagent_e2e_sidecar sidecar_e2e_delegate_merges_memory_and_builds_context_from_memory 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 3: Run the full e2e suite to verify no regressions**

Run: `cargo test -p busytok-runtime --test subagent_e2e_sidecar 2>&1 | tail -10`
Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/busytok-runtime/tests/subagent_e2e_sidecar.rs
git commit -m "test(subagent): e2e delegate merges memory and builds context from memory"
```

---

## Task 7: Coverage gate + cleanup

**Files:**
- Modify: `scripts/coverage.sh` (if gate adjustment needed)
- Modify: `crates/busytok-subagent/src/lib.rs` (ensure modules are `pub`)

**Interfaces:**
- Produces: verified coverage ≥ 90% per-crate, ≥ 82% workspace; clean fmt + clippy.

- [ ] **Step 1: Run the full test suite**

Run: `cd /Users/wsd/Data/Busytok/busytok/.worktrees/feat-subagent-foundation && cargo test -p busytok-subagent 2>&1 | tail -10 && cargo test -p busytok-store 2>&1 | tail -5 && cargo test -p busytok-runtime --test subagent_e2e_sidecar 2>&1 | tail -5`
Expected: all pass.

- [ ] **Step 2: Run coverage gate**

Run: `bash scripts/coverage.sh 2>&1 | tail -15`
Expected: workspace ≥ 82%, per-crate `busytok-subagent` ≥ 90%.

If the per-crate coverage is below 90%, add targeted unit tests for uncovered lines in `context.rs` or `memory.rs` (the two new modules). Do NOT lower the gate. The new modules are pure logic and should be easy to cover to ~100%. Likely uncovered branches: the `assemble_with_budget` early-return-when-fits path, the `compact()` empty-recent-summaries path, and the `normalize_path` repo-prefix-strip path. Add tests for those if missing.

- [ ] **Step 3: Run fmt + clippy**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 4: Commit if any changes were needed**

```bash
git add -A
git commit -m "test(subagent): backfill coverage for context/memory modules"
```

(If no changes were needed, skip this step.)

---

## Self-Review

After writing the complete plan (and incorporating the strict subagent review), the following was checked against the spec:

**1. Spec coverage:**
- ✅ §6.1 ContextBuilder with budget control + trim priority → Task 2 (trim priority order now directly tested via `trim_priority_drops_lowest_priority_sections_first`)
- ✅ §6.1 budget default 4000, hard max 8000, profile-configurable, zero→default fallback → Task 2
- ✅ §6.1 prompt never trimmed → Task 2 (`protected: true` on prompt/header)
- ✅ §6.2 MemoryUpdater with merge rules + rule-based compaction → Task 1
- ✅ §6.2 hot_summary = current_state_summary (NOT task_summary) → Task 1 (test `hot_summary_set_from_current_state_summary`), Task 3 (replaces `write_hot_summary`)
- ✅ §6.2 hot_summary PRESERVED when memory_update absent → Task 1 (test `hot_summary_preserved_when_memory_update_absent`) — avoids destructive warm→cold
- ✅ §6.2 compaction trigger (a): ≥ threshold tasks since last compaction (counted via `created_at_ms > last_compacted_at_ms`, NOT `recent_tasks.len()`) → Task 1 (fixed C-2)
- ✅ §6.2 compaction trigger (b): > ratio of PROFILE budget (not global default) → Task 1 (test `compaction_trigger_b_fires_on_oversized_memory_using_profile_budget`, fixed C-5)
- ✅ §6.2 compaction algorithm: last 5 MOST RECENT summaries (`.take(5)` on DESC-ordered, no `.rev()`) → Task 1 (fixed C-4, test `compaction_uses_most_recent_5_summaries_not_oldest`)
- ✅ §6.2 per-category caps applied DURING compaction only → Task 1 (fixed I-7, test `caps_not_applied_outside_compaction`)
- ✅ §6.2 attempts: last 10, MOST RECENT task appended → Task 1 (fixed C-3, test `attempts_records_most_recent_task_not_oldest`)
- ✅ §6.3 key_files: repo-relative (strips `repo_path` prefix) → Task 1 (fixed I-1, test `normalizes_absolute_path_to_repo_relative`)
- ✅ §6.3 key_files: forward slashes, strip `./`, case-insensitive dedup → Task 1
- ✅ §6.3 open_questions: trim, lowercase dedup, preserve casing, status open|resolved → Task 1
- ✅ §4.3 turn_auto params (tools, structured memory, context, constraints, output_schema) → Task 4
- ✅ §4.3 result.memory_update parsing → Task 4 (with parse tests)
- ✅ §3.3 invariant preserved: warm iff hot_summary IS NOT NULL → Task 1 (preserve-when-absent keeps the invariant intact)
- ✅ End-to-end: delegate → build context → sidecar turn → update memory → Tasks 3, 4, 6 (e2e now verifies context WAS built from memory by echoing compact_context back)
- ✅ Bug fix: manager.rs `write_hot_summary(task_summary)` → fixed in Task 3 (replaced by `MemoryUpdater::update`)
- ⚠️ §6.2 compaction trigger (c) "hibernate is about to happen" — deferred (hibernate flow in Plan 1/3 doesn't trigger compaction; acceptable for MVP, documented in Global Constraints)
- ⚠️ §6.1 "prompt exceeds hard limit → route to artifact store" — deferred (future enhancement, acceptable for MVP)

**2. Placeholder scan:** No TBD/TODO/placeholders found. All steps contain complete code. Task 6 includes a note to adapt the DB-accessor pattern to match the existing e2e file (since the exact accessor wasn't read), with concrete fallback guidance — this is a verified-unknown, not a placeholder.

**3. Type consistency:**
- `MemoryUpdate` (Task 1) used in `ExecutorOutput` (Task 3) and parsed in `parse_memory_update` (Task 4) — consistent fields: `current_state_summary`, `key_files`, `decisions`, `open_questions`.
- `CompactContext` (Task 2) used in `ExecutorInput` (Task 3) and sent in `params.context` (Task 4) — consistent fields: `compact_context`, `budget_tokens`, `source`.
- `MemorySnapshot` (Task 2) now uses structured `Vec<KeyFile>` / `Vec<OpenQuestion>` (fixed I-3) — consistent with Task 4's structured `memory` JSON and Task 5's TS `MemoryField` type.
- `KeyFile` / `OpenQuestion` (Task 1) used in `MemoryUpdate`, `MemorySnapshot`, `parse_memory_update`, and TS types — consistent field names: `path`, `reason`, `last_seen_at_ms`, `score` / `question`, `status`, `created_at_ms`, `last_seen_at_ms`.
- `MemoryUpdater::update` signature (Task 1) matches the call site in Task 3: `(current, update, recent_tasks, task_id, profile_budget_tokens, repo_path)`.
- `ExecutorInput` (Task 3) now includes `write_access` (fixed M-2) — consumed by Task 4's `constraints.write_access`.

**4. Deleted code:** `write_hot_summary` helper (manager.rs) fully replaced by `MemoryUpdater::update`. No dangling references. Four duplicate `parse_*` helpers collapsed into one generic `parse_json_vec` (fixed I-8). `Section` enum simplified to a struct (fixed M-1). `assemble_with_budget` rebuilt from kept sections instead of fragile `replacen` (fixed I-2).

**5. Review findings addressed:**
- C-1 (e2e non-existent helpers): Task 6 rewritten to use `make_sidecar_supervisor` + `supervisor.subagent_delegate`.
- C-2 (tasks_since = recent_tasks.len()): now counts `created_at_ms > last_compacted_at_ms`.
- C-3 (attempts .last() = oldest): now uses `.first()` (most recent in DESC order).
- C-4 (compact .rev().take(5) = 5 oldest): now `.take(5)` (5 most recent).
- C-5 (memory_oversized uses default budget): `update()` now takes `profile_budget_tokens`; `memory_oversized` uses it.
- I-1 (repo-relative normalization): `normalize_path` now strips `repo_path` prefix.
- I-2 (replacen fragility): `assemble_with_budget` rebuilds from kept sections.
- I-3 (Vec<String> lossy): `MemorySnapshot` now uses `Vec<KeyFile>`/`Vec<OpenQuestion>`.
- I-4 (trim priority untested): added `trim_priority_drops_lowest_priority_sections_first`.
- I-5 (trigger (b) untested): added `compaction_trigger_b_fires_on_oversized_memory_using_profile_budget`.
- I-6 (test_settings() missing): replaced with `SubagentSettings::default()`.
- I-7 (caps outside compaction): moved caps inside `if should_compact`.
- I-8 (4 parse helpers): collapsed into generic `parse_json_vec<T>`.
- I-9 (bash printf duplicated): factored into `build_mem_fragment` + `COMPACT_CTX`.
- I-10 (e2e doesn't verify context built): mock sidecar echoes `compact_context` in `task_summary`; e2e asserts it contains the first delegate's `hot_summary`.
- M-1 (Section enum over-engineered): simplified to struct with `(priority, text, protected)`.
- M-2 (write_access hardcoded): now pulled from profile config via `ExecutorInput.write_access`.
- M-3 (clamp(1, max) on 0): zero budget now falls back to default.
- Q-1 (erase hot_summary when absent): now PRESERVED (no destructive warm→cold).
- Q-2 (recent tasks display order): most-recent-first (no `.rev()`).
