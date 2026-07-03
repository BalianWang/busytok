#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::uninlined_format_args,
    dead_code,
    unused_imports,
    unused_variables
)]

//! Coverage gap tests for `crates/busytok-store/src/subagent_queries.rs`.
//!
//! Each test exercises a previously-uncovered function or branch. Tests are
//! self-contained and use `Database::open_in_memory()`.

use busytok_store::repository::{
    SubagentHarnessBindingRow, SubagentLogicalSubagentRow, SubagentMemoryRow,
    SubagentResourceEventRow, SubagentTaskRow, SubagentUsageRecordRow,
};
use busytok_store::subagent_queries;
use busytok_store::Database;

fn db() -> Database {
    Database::open_in_memory().unwrap()
}

fn seed_subagent(db: &Database, id: &str, name: &str) -> SubagentLogicalSubagentRow {
    let mut row = SubagentLogicalSubagentRow::for_test(id, name);
    row.repo_hash = "repo-h".to_string();
    row.project_id = "proj-h".to_string();
    db.subagent_upsert_logical(&row).unwrap();
    row
}

fn seed_task(db: &Database, id: &str, subagent_id: &str, intent: &str) -> SubagentTaskRow {
    let task = SubagentTaskRow::for_test(id, subagent_id, "pi/search-cheap", intent);
    db.subagent_insert_task(&task).unwrap();
    task
}

fn make_binding(subagent_id: &str, is_hot: i32, status: &str) -> SubagentHarnessBindingRow {
    let now = busytok_domain::now_ms();
    SubagentHarnessBindingRow {
        id: format!("bind_{subagent_id}_{status}"),
        subagent_id: subagent_id.to_string(),
        harness: "pi".to_string(),
        adapter_session_id: Some(format!("sess-{subagent_id}")),
        adapter_process_id: Some("12345".to_string()),
        is_hot,
        status: status.to_string(),
        created_at_ms: now,
        last_used_at_ms: Some(now),
        closed_at_ms: None,
        detail_json: None,
    }
}

// ── find_by_name_in_repo ────────────────────────────────────────────────────

#[test]
fn find_by_name_in_repo_returns_matching_rows_including_deleted() {
    let db = db();
    let _a = seed_subagent(&db, "sa-a", "reviewer");
    // Must set status='deleted' BEFORE upsert to avoid the partial unique
    // index on (project_id, repo_hash, name) WHERE status != 'deleted'.
    let mut b = SubagentLogicalSubagentRow::for_test("sa-b", "reviewer");
    b.repo_hash = "repo-h".to_string();
    b.project_id = "proj-h".to_string();
    b.status = "deleted".to_string();
    db.subagent_upsert_logical(&b).unwrap();

    let rows =
        subagent_queries::find_by_name_in_repo(db.conn(), "proj-h", "repo-h", "reviewer").unwrap();
    // Both active and deleted rows are returned (no status filter at SQL level).
    assert_eq!(rows.len(), 2);
}

#[test]
fn find_by_name_in_repo_returns_empty_when_no_match() {
    let db = db();
    seed_subagent(&db, "sa-a", "reviewer");
    let rows =
        subagent_queries::find_by_name_in_repo(db.conn(), "proj-h", "repo-h", "nonexistent")
            .unwrap();
    assert!(rows.is_empty());
}

// ── list_filtered ───────────────────────────────────────────────────────────

#[test]
fn list_filtered_no_filters_returns_all_non_deleted() {
    let db = db();
    let a = seed_subagent(&db, "sa-a", "a");
    db.subagent_upsert_logical(&a).unwrap();
    let mut b = seed_subagent(&db, "sa-b", "b");
    b.status = "deleted".to_string();
    b.last_active_at_ms = Some(1000);
    db.subagent_upsert_logical(&b).unwrap();

    let rows = subagent_queries::list_filtered(db.conn(), None, None, false).unwrap();
    assert_eq!(rows.len(), 1, "deleted must be excluded");
    assert_eq!(rows[0].id, "sa-a");
}

#[test]
fn list_filtered_include_deleted_returns_all() {
    let db = db();
    let mut a = seed_subagent(&db, "sa-a", "a");
    let mut b = seed_subagent(&db, "sa-b", "b");
    b.status = "deleted".to_string();
    db.subagent_upsert_logical(&a).unwrap();
    db.subagent_upsert_logical(&b).unwrap();

    let rows = subagent_queries::list_filtered(db.conn(), None, None, true).unwrap();
    assert_eq!(rows.len(), 2, "include_deleted=true returns all");
}

#[test]
fn list_filtered_by_status_returns_matching() {
    let db = db();
    let mut a = seed_subagent(&db, "sa-a", "a");
    a.status = "hot".to_string();
    let mut b = seed_subagent(&db, "sa-b", "b");
    b.status = "cold".to_string();
    db.subagent_upsert_logical(&a).unwrap();
    db.subagent_upsert_logical(&b).unwrap();

    let rows = subagent_queries::list_filtered(db.conn(), Some("hot"), None, false).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "hot");
}

#[test]
fn list_filtered_by_project_returns_matching() {
    let db = db();
    let mut a = seed_subagent(&db, "sa-a", "a");
    a.project_id = "proj-X".to_string();
    let mut b = seed_subagent(&db, "sa-b", "b");
    b.project_id = "proj-Y".to_string();
    db.subagent_upsert_logical(&a).unwrap();
    db.subagent_upsert_logical(&b).unwrap();

    let rows =
        subagent_queries::list_filtered(db.conn(), None, Some("proj-X"), false).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].project_id, "proj-X");
}

#[test]
fn list_filtered_by_status_and_project() {
    let db = db();
    let mut a = seed_subagent(&db, "sa-a", "a");
    a.status = "hot".to_string();
    a.project_id = "proj-X".to_string();
    let mut b = seed_subagent(&db, "sa-b", "b");
    b.status = "cold".to_string();
    b.project_id = "proj-X".to_string();
    db.subagent_upsert_logical(&a).unwrap();
    db.subagent_upsert_logical(&b).unwrap();

    let rows =
        subagent_queries::list_filtered(db.conn(), Some("hot"), Some("proj-X"), false).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "sa-a");
}

// ── hard_delete_logical_subagent ────────────────────────────────────────────

#[test]
fn hard_delete_logical_subagent_cascades_all_dependents() {
    let db = db();
    let sa = seed_subagent(&db, "sa-del", "to-delete");
    seed_task(&db, "t-1", "sa-del", "go");
    // Seed a binding
    let binding = make_binding("sa-del", 1, "hot");
    db.subagent_upsert_binding(&binding).unwrap();
    // Seed memory
    let mem = SubagentMemoryRow::new_empty("sa-del");
    db.subagent_upsert_memory(&mem).unwrap();
    // Seed a usage record
    let usage = SubagentUsageRecordRow {
        id: "ur-1".to_string(),
        task_id: "t-1".to_string(),
        subagent_id: "sa-del".to_string(),
        source_usage_event_id: None,
        harness: "pi".to_string(),
        provider: None,
        model: None,
        input_tokens: Some(100),
        output_tokens: Some(50),
        cache_read_tokens: None,
        cache_write_tokens: None,
        total_cost_usd: None,
        duration_ms: None,
        created_at_ms: busytok_domain::now_ms(),
    };
    db.conn()
        .execute(
            "INSERT INTO subagent_usage_records \
             (id, task_id, subagent_id, source_usage_event_id, harness, provider, model, \
              input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, \
              total_cost_usd, duration_ms, created_at_ms) \
             VALUES (?1, ?2, ?3, NULL, ?4, NULL, NULL, 100, 50, NULL, NULL, NULL, NULL, ?5)",
            rusqlite::params!["ur-1", "t-1", "sa-del", "pi", usage.created_at_ms],
        )
        .unwrap();
    // Seed a resource event
    let re = SubagentResourceEventRow {
        id: "re-1".to_string(),
        event_type: "pressure".to_string(),
        target_id: Some("sa-del".to_string()),
        rss_mb: Some(500.0),
        cpu_percent: Some(10.0),
        detail_json: None,
        created_at_ms: busytok_domain::now_ms(),
    };
    db.conn()
        .execute(
            "INSERT INTO subagent_resource_events \
             (id, event_type, target_id, rss_mb, cpu_percent, detail_json, created_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)",
            rusqlite::params![re.id, re.event_type, re.target_id, re.rss_mb, re.cpu_percent, re.created_at_ms],
        )
        .unwrap();

    // Hard delete
    subagent_queries::hard_delete_logical_subagent(db.conn(), "sa-del").unwrap();

    // All dependents must be gone
    assert!(db.subagent_get_logical("sa-del").unwrap().is_none());
    assert!(db.subagent_get_task("t-1").unwrap().is_none());
    let binding_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM subagent_harness_bindings WHERE subagent_id = 'sa-del'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(binding_count, 0);
    let mem_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM subagent_memory WHERE subagent_id = 'sa-del'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(mem_count, 0);
    let usage_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM subagent_usage_records WHERE subagent_id = 'sa-del'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(usage_count, 0);
    let resource_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM subagent_resource_events WHERE target_id = 'sa-del'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(resource_count, 0);
}

// ── get_memory ──────────────────────────────────────────────────────────────

#[test]
fn get_memory_returns_none_when_no_row() {
    let db = db();
    assert!(db.subagent_get_memory("sa-none").unwrap().is_none());
}

// ── list_tasks ──────────────────────────────────────────────────────────────

#[test]
fn list_tasks_returns_empty_for_unknown_subagent() {
    let db = db();
    let tasks = subagent_queries::list_tasks(db.conn(), "sa-none", 10).unwrap();
    assert!(tasks.is_empty());
}

#[test]
fn list_tasks_returns_tasks_ordered_by_created_desc() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    let mut t1 = SubagentTaskRow::for_test("t-1", "sa-1", "pi/search-cheap", "first");
    t1.created_at_ms = 1000;
    let mut t2 = SubagentTaskRow::for_test("t-2", "sa-1", "pi/search-cheap", "second");
    t2.created_at_ms = 2000;
    db.subagent_insert_task(&t1).unwrap();
    db.subagent_insert_task(&t2).unwrap();

    let tasks = subagent_queries::list_tasks(db.conn(), "sa-1", 10).unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].id, "t-2", "newest first");
    assert_eq!(tasks[1].id, "t-1");
}

#[test]
fn list_tasks_respects_limit() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    for i in 0..5 {
        let mut t = SubagentTaskRow::for_test(&format!("t-{i}"), "sa-1", "pi/search-cheap", "go");
        t.created_at_ms = 1000 + i as i64;
        db.subagent_insert_task(&t).unwrap();
    }
    let tasks = subagent_queries::list_tasks(db.conn(), "sa-1", 2).unwrap();
    assert_eq!(tasks.len(), 2);
}

// ── set_task_status (cancelled/failed arms) ─────────────────────────────────

#[test]
fn set_task_status_cancelled_sets_completed_at() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    seed_task(&db, "t-1", "sa-1", "go");
    subagent_queries::set_task_status(db.conn(), "t-1", "cancelled", Some("cancelled reason".into()), Some("err".into())).unwrap();
    let got = db.subagent_get_task("t-1").unwrap().unwrap();
    assert_eq!(got.status, "cancelled");
    assert!(got.completed_at_ms.is_some());
    assert_eq!(got.result_summary.as_deref(), Some("cancelled reason"));
    assert_eq!(got.error.as_deref(), Some("err"));
}

#[test]
fn set_task_status_failed_sets_completed_at() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    seed_task(&db, "t-1", "sa-1", "go");
    subagent_queries::set_task_status(db.conn(), "t-1", "failed", None, Some("boom".into())).unwrap();
    let got = db.subagent_get_task("t-1").unwrap().unwrap();
    assert_eq!(got.status, "failed");
    assert!(got.completed_at_ms.is_some());
    assert_eq!(got.error.as_deref(), Some("boom"));
}

#[test]
fn set_task_status_running_does_not_set_completed_at() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    seed_task(&db, "t-1", "sa-1", "go");
    subagent_queries::set_task_status(db.conn(), "t-1", "running", None, None).unwrap();
    let got = db.subagent_get_task("t-1").unwrap().unwrap();
    assert_eq!(got.status, "running");
    assert!(got.completed_at_ms.is_none());
}

// ── set_task_error_kind ─────────────────────────────────────────────────────

#[test]
fn set_task_error_kind_sets_and_clears() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    seed_task(&db, "t-1", "sa-1", "go");
    // Set
    db.subagent_set_task_error_kind("t-1", Some("SIDECAR_CRASHED")).unwrap();
    let got = db.subagent_get_task("t-1").unwrap().unwrap();
    assert_eq!(got.error_kind.as_deref(), Some("SIDECAR_CRASHED"));
    // Clear
    db.subagent_set_task_error_kind("t-1", None).unwrap();
    let got = db.subagent_get_task("t-1").unwrap().unwrap();
    assert!(got.error_kind.is_none());
}

// ── pick_oldest_queued_task ─────────────────────────────────────────────────

#[test]
fn pick_oldest_queued_task_returns_none_when_no_queued() {
    let db = db();
    let got = db.subagent_pick_oldest_queued_task().unwrap();
    assert!(got.is_none());
}

#[test]
fn pick_oldest_queued_task_picks_oldest_and_flips_to_running() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    let mut t1 = SubagentTaskRow::for_test("t-1", "sa-1", "pi/search-cheap", "first");
    t1.created_at_ms = 1000;
    t1.status = "queued".to_string();
    let mut t2 = SubagentTaskRow::for_test("t-2", "sa-1", "pi/search-cheap", "second");
    t2.created_at_ms = 2000;
    t2.status = "queued".to_string();
    db.subagent_insert_task(&t1).unwrap();
    db.subagent_insert_task(&t2).unwrap();

    let picked = db.subagent_pick_oldest_queued_task().unwrap();
    assert!(picked.is_some());
    let picked = picked.unwrap();
    assert_eq!(picked.id, "t-1", "oldest queued task must be picked");
    assert_eq!(picked.status, "running");
    assert!(picked.started_at_ms.is_some());
}

#[test]
fn pick_oldest_queued_task_skips_subagent_with_running_task() {
    // Per-subagent FIFO: if a subagent already has a running task, its queued
    // tasks must NOT be picked.
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    seed_subagent(&db, "sa-2", "r2");

    // sa-1 has a running task + a queued task
    let mut t_running = SubagentTaskRow::for_test("t-run", "sa-1", "pi/search-cheap", "running");
    t_running.status = "running".to_string();
    t_running.created_at_ms = 1000;
    db.subagent_insert_task(&t_running).unwrap();
    let mut t_queued = SubagentTaskRow::for_test("t-q", "sa-1", "pi/search-cheap", "queued");
    t_queued.status = "queued".to_string();
    t_queued.created_at_ms = 2000;
    db.subagent_insert_task(&t_queued).unwrap();

    // sa-2 has only a queued task (created later)
    let mut t2 = SubagentTaskRow::for_test("t-2", "sa-2", "pi/search-cheap", "queued2");
    t2.status = "queued".to_string();
    t2.created_at_ms = 3000;
    db.subagent_insert_task(&t2).unwrap();

    let picked = db.subagent_pick_oldest_queued_task().unwrap();
    assert!(picked.is_some());
    let picked = picked.unwrap();
    // Should pick sa-2's task, NOT sa-1's (sa-1 has a running task)
    assert_eq!(picked.id, "t-2");
}

// ── has_running_task ────────────────────────────────────────────────────────

#[test]
fn has_running_task_returns_true_when_running_exists() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    let mut t = SubagentTaskRow::for_test("t-1", "sa-1", "pi/search-cheap", "go");
    t.status = "running".to_string();
    db.subagent_insert_task(&t).unwrap();
    assert!(db.subagent_has_running_task("sa-1").unwrap());
}

#[test]
fn has_running_task_returns_false_when_no_running() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    seed_task(&db, "t-1", "sa-1", "go");
    assert!(!db.subagent_has_running_task("sa-1").unwrap());
}

#[test]
fn has_running_task_returns_false_for_unknown_subagent() {
    let db = db();
    assert!(!db.subagent_has_running_task("sa-none").unwrap());
}

// ── commit_hot_binding_and_status ───────────────────────────────────────────

#[test]
fn commit_hot_binding_and_status_sets_logical_status_to_hot() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    let binding = make_binding("sa-1", 1, "hot");
    subagent_queries::commit_hot_binding_and_status(db.conn(), &binding, "sa-1").unwrap();

    let sa = db.subagent_get_logical("sa-1").unwrap().unwrap();
    assert_eq!(sa.status, "hot");
    let got = db.subagent_hot_binding("sa-1", "pi").unwrap();
    assert!(got.is_some());
    assert_eq!(got.unwrap().is_hot, 1);
}

// ── commit_eviction ─────────────────────────────────────────────────────────

#[test]
fn commit_eviction_with_hot_summary_returns_warm() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    // First make it hot
    let hot_binding = make_binding("sa-1", 1, "hot");
    subagent_queries::commit_hot_binding_and_status(db.conn(), &hot_binding, "sa-1").unwrap();

    // Now evict with a hot_summary
    let mut closed_binding = make_binding("sa-1", 0, "closed");
    closed_binding.id = "bind-closed".to_string();
    closed_binding.closed_at_ms = Some(busytok_domain::now_ms());
    let new_status = subagent_queries::commit_eviction(
        db.conn(),
        &closed_binding,
        "sa-1",
        Some("eviction summary"),
    )
    .unwrap();
    assert_eq!(new_status, "warm");
    let sa = db.subagent_get_logical("sa-1").unwrap().unwrap();
    assert_eq!(sa.status, "warm");
    let mem = db.subagent_get_memory("sa-1").unwrap().unwrap();
    assert_eq!(mem.hot_summary.as_deref(), Some("eviction summary"));
}

#[test]
fn commit_eviction_without_hot_summary_returns_cold() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    // No memory row exists → cold
    let closed_binding = make_binding("sa-1", 0, "closed");
    let new_status =
        subagent_queries::commit_eviction(db.conn(), &closed_binding, "sa-1", None).unwrap();
    assert_eq!(new_status, "cold");
    let sa = db.subagent_get_logical("sa-1").unwrap().unwrap();
    assert_eq!(sa.status, "cold");
}

#[test]
fn commit_eviction_skips_deleted_tombstones() {
    let db = db();
    let mut sa = seed_subagent(&db, "sa-1", "r");
    sa.status = "deleted".to_string();
    db.subagent_upsert_logical(&sa).unwrap();
    let closed_binding = make_binding("sa-1", 0, "closed");
    let new_status =
        subagent_queries::commit_eviction(db.conn(), &closed_binding, "sa-1", Some("summary"))
            .unwrap();
    // The binding is still upserted, but the logical status stays 'deleted'
    assert_eq!(new_status, "warm"); // computed from memory state
    let sa_after = db.subagent_get_logical("sa-1").unwrap().unwrap();
    assert_eq!(sa_after.status, "deleted", "tombstone must not be revived");
}

// ── find_lru_hot_binding ────────────────────────────────────────────────────

#[test]
fn find_lru_hot_binding_returns_none_when_no_hot_bindings() {
    let db = db();
    let got = subagent_queries::find_lru_hot_binding(db.conn(), "pi").unwrap();
    assert!(got.is_none());
}

#[test]
fn find_lru_hot_binding_returns_oldest_last_used() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    seed_subagent(&db, "sa-2", "r2");

    let mut b1 = make_binding("sa-1", 1, "hot");
    b1.last_used_at_ms = Some(1000);
    b1.id = "bind-1".to_string();
    let mut b2 = make_binding("sa-2", 1, "hot");
    b2.last_used_at_ms = Some(2000);
    b2.id = "bind-2".to_string();
    db.subagent_upsert_hot_binding(&b1).unwrap();
    db.subagent_upsert_hot_binding(&b2).unwrap();

    let lru = subagent_queries::find_lru_hot_binding(db.conn(), "pi").unwrap();
    assert!(lru.is_some());
    let lru = lru.unwrap();
    assert_eq!(lru.id, "bind-1", "LRU = oldest last_used_at_ms");
}

// ── write_hot_summary (insert vs update paths) ──────────────────────────────

#[test]
fn write_hot_summary_inserts_when_no_memory_row() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    subagent_queries::write_hot_summary(db.conn(), "sa-1", "new summary").unwrap();
    let mem = db.subagent_get_memory("sa-1").unwrap().unwrap();
    assert_eq!(mem.hot_summary.as_deref(), Some("new summary"));
}

#[test]
fn write_hot_summary_updates_existing_row() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    let mut mem = SubagentMemoryRow::new_empty("sa-1");
    mem.hot_summary = Some("old".to_string());
    db.subagent_upsert_memory(&mem).unwrap();

    subagent_queries::write_hot_summary(db.conn(), "sa-1", "updated").unwrap();
    let mem = db.subagent_get_memory("sa-1").unwrap().unwrap();
    assert_eq!(mem.hot_summary.as_deref(), Some("updated"));
}

// ── reconcile_sidecar_crash ─────────────────────────────────────────────────

#[test]
fn reconcile_sidecar_crash_no_hot_bindings_returns_default() {
    let db = db();
    let counts = subagent_queries::reconcile_sidecar_crash(db.conn(), "pi").unwrap();
    assert_eq!(counts.tasks_failed, 0);
    assert_eq!(counts.bindings_released, 0);
    assert_eq!(counts.status_rolled_back, 0);
}

#[test]
fn reconcile_sidecar_crash_fails_running_tasks_and_releases_bindings() {
    let db = db();
    // Subagent A: hot, with a running task
    let mut a = seed_subagent(&db, "sa-a", "live");
    a.status = "hot".to_string();
    db.subagent_upsert_logical(&a).unwrap();
    let hot_binding = make_binding("sa-a", 1, "hot");
    db.subagent_upsert_hot_binding(&hot_binding).unwrap();
    let mut task = SubagentTaskRow::for_test("t-a", "sa-a", "pi/search-cheap", "in-flight");
    task.status = "running".to_string();
    db.subagent_insert_task(&task).unwrap();

    // Subagent B: soft-deleted tombstone with a hot binding
    let mut b = seed_subagent(&db, "sa-b", "tomb");
    b.status = "deleted".to_string();
    db.subagent_upsert_logical(&b).unwrap();
    let mut hot_binding_b = make_binding("sa-b", 1, "hot");
    hot_binding_b.id = "bind-b".to_string();
    db.subagent_upsert_hot_binding(&hot_binding_b).unwrap();

    let counts = subagent_queries::reconcile_sidecar_crash(db.conn(), "pi").unwrap();

    // 1. Running task → failed
    assert_eq!(counts.tasks_failed, 1);
    let task_after = db.subagent_get_task("t-a").unwrap().unwrap();
    assert_eq!(task_after.status, "failed");
    assert_eq!(task_after.error.as_deref(), Some("SIDECAR_CRASHED"));

    // 2. Both hot bindings released (is_hot=0, status='crashed')
    assert_eq!(counts.bindings_released, 2);

    // 3. Live subagent status rolled back (warm/cold, not hot)
    assert_eq!(counts.status_rolled_back, 1, "only non-deleted subagent rolled back");
    let a_after = db.subagent_get_logical("sa-a").unwrap().unwrap();
    assert!(a_after.status == "warm" || a_after.status == "cold");

    // 4. Tombstone stays deleted
    let b_after = db.subagent_get_logical("sa-b").unwrap().unwrap();
    assert_eq!(b_after.status, "deleted");
}

// ── insert_usage_record ─────────────────────────────────────────────────────

#[test]
fn insert_usage_record_round_trips() {
    let db = db();
    seed_subagent(&db, "sa-1", "r");
    seed_task(&db, "t-1", "sa-1", "go");
    let row = SubagentUsageRecordRow {
        id: "ur-1".to_string(),
        task_id: "t-1".to_string(),
        subagent_id: "sa-1".to_string(),
        source_usage_event_id: None,
        harness: "pi".to_string(),
        provider: Some("openai".to_string()),
        model: Some("gpt-4o".to_string()),
        input_tokens: Some(100),
        output_tokens: Some(50),
        cache_read_tokens: Some(10),
        cache_write_tokens: Some(5),
        total_cost_usd: Some(0.01),
        duration_ms: Some(5000),
        created_at_ms: busytok_domain::now_ms(),
    };
    subagent_queries::insert_usage_record(db.conn(), &row).unwrap();

    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM subagent_usage_records WHERE id = 'ur-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

// ── insert_resource_event / list_resource_events ────────────────────────────

#[test]
fn insert_resource_event_round_trips() {
    let db = db();
    let row = SubagentResourceEventRow {
        id: "re-1".to_string(),
        event_type: "pressure".to_string(),
        target_id: Some("sa-1".to_string()),
        rss_mb: Some(500.0),
        cpu_percent: Some(12.5),
        detail_json: Some(r#"{"k":"v"}"#.to_string()),
        created_at_ms: 1000,
    };
    subagent_queries::insert_resource_event(db.conn(), &row).unwrap();

    let events = subagent_queries::list_resource_events(db.conn(), None, 10).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, "re-1");
    assert_eq!(events[0].event_type, "pressure");
    assert_eq!(events[0].target_id.as_deref(), Some("sa-1"));
}

#[test]
fn list_resource_events_filters_by_target_id() {
    let db = db();
    for i in 0..3 {
        let row = SubagentResourceEventRow {
            id: format!("re-{i}"),
            event_type: "pressure".to_string(),
            target_id: Some(format!("sa-{i}")),
            rss_mb: Some(100.0 * i as f64),
            cpu_percent: None,
            detail_json: None,
            created_at_ms: i as i64 * 1000,
        };
        subagent_queries::insert_resource_event(db.conn(), &row).unwrap();
    }

    let filtered = subagent_queries::list_resource_events(db.conn(), Some("sa-1"), 10).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].target_id.as_deref(), Some("sa-1"));
}

#[test]
fn list_resource_events_respects_limit() {
    let db = db();
    for i in 0..5 {
        let row = SubagentResourceEventRow {
            id: format!("re-{i}"),
            event_type: "pressure".to_string(),
            target_id: None,
            rss_mb: None,
            cpu_percent: None,
            detail_json: None,
            created_at_ms: i as i64 * 1000,
        };
        subagent_queries::insert_resource_event(db.conn(), &row).unwrap();
    }

    let events = subagent_queries::list_resource_events(db.conn(), None, 2).unwrap();
    assert_eq!(events.len(), 2);
    // Newest first (ORDER BY created_at_ms DESC)
    assert_eq!(events[0].id, "re-4");
    assert_eq!(events[1].id, "re-3");
}

#[test]
fn list_resource_events_returns_empty_when_no_data() {
    let db = db();
    let events = subagent_queries::list_resource_events(db.conn(), None, 10).unwrap();
    assert!(events.is_empty());
}

// ── get_logical_subagent (L48) ──────────────────────────────────────────────

#[test]
fn get_logical_subagent_returns_row_when_exists() {
    let db = db();
    let seeded = seed_subagent(&db, "sa-get", "fetcher");
    let row = subagent_queries::get_logical_subagent(db.conn(), "sa-get")
        .unwrap()
        .expect("subagent should exist");
    assert_eq!(row.id, seeded.id);
    assert_eq!(row.name, "fetcher");
}

#[test]
fn get_logical_subagent_returns_none_when_not_found() {
    let db = db();
    let row = subagent_queries::get_logical_subagent(db.conn(), "nonexistent").unwrap();
    assert!(row.is_none());
}

// ── list_active_by_repo (L79) ───────────────────────────────────────────────

#[test]
fn list_active_by_repo_returns_non_deleted_and_excludes_deleted() {
    let db = db();
    let _active = seed_subagent(&db, "sa-active-r", "active-reviewer");
    let mut deleted = SubagentLogicalSubagentRow::for_test("sa-deleted-r", "deleted-reviewer");
    deleted.repo_hash = "repo-h".to_string();
    deleted.project_id = "proj-h".to_string();
    deleted.status = "deleted".to_string();
    db.subagent_upsert_logical(&deleted).unwrap();

    let rows = subagent_queries::list_active_by_repo(db.conn(), "repo-h").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "sa-active-r");
}

#[test]
fn list_active_by_repo_returns_empty_for_unknown_repo() {
    let db = db();
    let rows = subagent_queries::list_active_by_repo(db.conn(), "no-such-repo").unwrap();
    assert!(rows.is_empty());
}

// ── get_task (L348) ─────────────────────────────────────────────────────────

#[test]
fn get_task_returns_row_when_exists() {
    let db = db();
    let _sa = seed_subagent(&db, "sa-task", "worker");
    let seeded = seed_task(&db, "task-get", "sa-task", "do work");
    let row = subagent_queries::get_task(db.conn(), "task-get")
        .unwrap()
        .expect("task should exist");
    assert_eq!(row.id, seeded.id);
    assert_eq!(row.subagent_id, "sa-task");
    assert_eq!(row.prompt.as_deref(), Some("do work"));
}

#[test]
fn get_task_returns_none_when_not_found() {
    let db = db();
    let row = subagent_queries::get_task(db.conn(), "nonexistent").unwrap();
    assert!(row.is_none());
}

// ── count_tasks_since (L463) ────────────────────────────────────────────────

#[test]
fn count_tasks_since_counts_only_after_threshold() {
    let db = db();
    let _sa = seed_subagent(&db, "sa-cts", "counter");
    // Manually insert tasks with specific created_at_ms values.
    for (id, ts) in [("t-old", 100), ("t-new1", 200), ("t-new2", 300)] {
        let mut task = SubagentTaskRow::for_test(id, "sa-cts", "pi/search", "intent");
        task.created_at_ms = ts;
        db.subagent_insert_task(&task).unwrap();
    }
    let count = subagent_queries::count_tasks_since(db.conn(), "sa-cts", 150).unwrap();
    assert_eq!(count, 2);
}

#[test]
fn count_tasks_since_returns_zero_when_none_after_threshold() {
    let db = db();
    let _sa = seed_subagent(&db, "sa-cts2", "counter2");
    let count = subagent_queries::count_tasks_since(db.conn(), "sa-cts2", 999_999).unwrap();
    assert_eq!(count, 0);
}

// ── task_counts_by_status (L489) ────────────────────────────────────────────

#[test]
fn task_counts_by_status_counts_queued_and_running() {
    let db = db();
    let _sa1 = seed_subagent(&db, "sa-tc1", "tc-worker1");
    let _sa2 = seed_subagent(&db, "sa-tc2", "tc-worker2");
    // Insert and manually set statuses.
    let t1 = SubagentTaskRow::for_test("tc-1", "sa-tc1", "pi/search", "a");
    db.subagent_insert_task(&t1).unwrap();
    subagent_queries::set_task_status(db.conn(), "tc-1", "running", None, None).unwrap();

    let t2 = SubagentTaskRow::for_test("tc-2", "sa-tc2", "pi/search", "b");
    db.subagent_insert_task(&t2).unwrap();
    // tc-2 stays "queued" (default from for_test)

    let (queued, running) = subagent_queries::task_counts_by_status(db.conn()).unwrap();
    assert_eq!(queued, 1);
    assert_eq!(running, 1);
}

// ── hot_binding (L771) ──────────────────────────────────────────────────────

#[test]
fn hot_binding_returns_hot_binding_for_subagent_and_harness() {
    let db = db();
    let _sa = seed_subagent(&db, "sa-hb", "hb-worker");
    let binding = make_binding("sa-hb", 1, "hot");
    db.subagent_upsert_binding(&binding).unwrap();

    let row = subagent_queries::hot_binding(db.conn(), "sa-hb", "pi")
        .unwrap()
        .expect("hot binding should exist");
    assert_eq!(row.subagent_id, "sa-hb");
    assert_eq!(row.harness, "pi");
    assert_eq!(row.is_hot, 1);
}

#[test]
fn hot_binding_returns_none_when_no_hot_binding() {
    let db = db();
    let _sa = seed_subagent(&db, "sa-hb2", "hb-worker2");
    let row = subagent_queries::hot_binding(db.conn(), "sa-hb2", "pi").unwrap();
    assert!(row.is_none());
}

// ── find_hot_binding_by_session (L839) ──────────────────────────────────────

#[test]
fn find_hot_binding_by_session_returns_matching_binding() {
    let db = db();
    let _sa = seed_subagent(&db, "sa-fhbs", "fhbs-worker");
    let mut binding = make_binding("sa-fhbs", 1, "hot");
    binding.adapter_session_id = Some("sess-fhbs".to_string());
    db.subagent_upsert_binding(&binding).unwrap();

    let row = subagent_queries::find_hot_binding_by_session(db.conn(), "sess-fhbs", "pi")
        .unwrap()
        .expect("binding should exist");
    assert_eq!(row.subagent_id, "sa-fhbs");
    assert_eq!(row.adapter_session_id.as_deref(), Some("sess-fhbs"));
}

#[test]
fn find_hot_binding_by_session_returns_none_when_not_found() {
    let db = db();
    let row = subagent_queries::find_hot_binding_by_session(db.conn(), "no-such-sess", "pi").unwrap();
    assert!(row.is_none());
}

// ── upsert_binding (L589) ───────────────────────────────────────────────────

#[test]
fn upsert_binding_inserts_and_updates_binding() {
    let db = db();
    let _sa = seed_subagent(&db, "sa-ub", "ub-worker");
    let mut binding = make_binding("sa-ub", 0, "closed");
    db.subagent_upsert_binding(&binding).unwrap();

    // Upsert again with changed status.
    binding.status = "hot".to_string();
    binding.is_hot = 1;
    db.subagent_upsert_binding(&binding).unwrap();

    let row = subagent_queries::hot_binding(db.conn(), "sa-ub", "pi")
        .unwrap()
        .expect("binding should exist after upsert");
    assert_eq!(row.status, "hot");
    assert_eq!(row.is_hot, 1);
}

// ── upsert_hot_binding (L622) ───────────────────────────────────────────────

#[test]
fn upsert_hot_binding_replaces_existing_hot_binding() {
    let db = db();
    let _sa = seed_subagent(&db, "sa-uhb", "uhb-worker");
    let b1 = make_binding("sa-uhb", 1, "hot");
    db.subagent_upsert_hot_binding(&b1).unwrap();

    // Upsert a second hot binding for the same subagent+harness — replaces the first.
    let mut b2 = make_binding("sa-uhb", 1, "hot");
    b2.id = "bind_sa-uhb_v2".to_string();
    b2.adapter_session_id = Some("sess-uhb-v2".to_string());
    db.subagent_upsert_hot_binding(&b2).unwrap();

    // Only one hot binding should exist (partial unique index).
    let row = subagent_queries::hot_binding(db.conn(), "sa-uhb", "pi")
        .unwrap()
        .expect("hot binding should exist");
    // upsert_hot_binding's ON CONFLICT DO UPDATE does not touch `id`; verify
    // the fields that ARE updated (adapter_session_id) reflect the new binding.
    assert_eq!(row.adapter_session_id.as_deref(), Some("sess-uhb-v2"));
}

// ── commit_hibernate_binding_and_status (L692) ──────────────────────────────

#[test]
fn commit_hibernate_binding_and_status_sets_warm_and_releases_hot() {
    let db = db();
    let _sa = seed_subagent(&db, "sa-chb", "chb-worker");
    let hot_binding = make_binding("sa-chb", 1, "hot");
    db.subagent_upsert_hot_binding(&hot_binding).unwrap();

    // Hibernate: caller must pre-populate the binding with is_hot=0,
    // status='closed', closed_at_ms=Some(now) per the function's docstring.
    let now = busytok_domain::now_ms();
    let mut closed_binding = hot_binding.clone();
    closed_binding.is_hot = 0;
    closed_binding.status = "closed".to_string();
    closed_binding.closed_at_ms = Some(now);

    // commit_hibernate upserts the closed binding (same id → overwrites the
    // hot row) and rolls the logical subagent status to 'warm'.
    db.subagent_commit_hibernate_binding_and_status(&closed_binding, "sa-chb", "warm").unwrap();

    // Hot binding should no longer exist (the row was flipped to is_hot=0).
    let hot = subagent_queries::hot_binding(db.conn(), "sa-chb", "pi").unwrap();
    assert!(hot.is_none());

    // Logical subagent status should be 'warm'.
    let logical = subagent_queries::get_logical_subagent(db.conn(), "sa-chb")
        .unwrap()
        .expect("subagent should exist");
    assert_eq!(logical.status, "warm");
}

// ── release_hot_bindings_for_shutdown (L1163) ───────────────────────────────

#[test]
fn release_hot_bindings_for_shutdown_returns_default_when_no_bindings() {
    let db = db();
    let counts = subagent_queries::release_hot_bindings_for_shutdown(db.conn(), "pi").unwrap();
    assert_eq!(counts.bindings_released, 0);
    assert_eq!(counts.status_rolled_back, 0);
}

#[test]
fn release_hot_bindings_for_shutdown_releases_bindings_and_rolls_back_status() {
    let db = db();
    let _sa1 = seed_subagent(&db, "sa-shut1", "shut-worker1");
    let _sa2 = seed_subagent(&db, "sa-shut2", "shut-worker2");
    let b1 = make_binding("sa-shut1", 1, "hot");
    let b2 = make_binding("sa-shut2", 1, "hot");
    db.subagent_upsert_hot_binding(&b1).unwrap();
    db.subagent_upsert_hot_binding(&b2).unwrap();

    let counts = subagent_queries::release_hot_bindings_for_shutdown(db.conn(), "pi").unwrap();
    assert_eq!(counts.bindings_released, 2);

    // Both subagents should roll back to 'cold' (no memory/hot_summary).
    let l1 = subagent_queries::get_logical_subagent(db.conn(), "sa-shut1").unwrap().unwrap();
    let l2 = subagent_queries::get_logical_subagent(db.conn(), "sa-shut2").unwrap().unwrap();
    assert_eq!(l1.status, "cold");
    assert_eq!(l2.status, "cold");

    // Hot bindings should be released.
    assert!(subagent_queries::hot_binding(db.conn(), "sa-shut1", "pi").unwrap().is_none());
    assert!(subagent_queries::hot_binding(db.conn(), "sa-shut2", "pi").unwrap().is_none());
}

#[test]
fn release_hot_bindings_for_shutdown_skips_deleted_tombstones() {
    let db = db();
    let mut sa = SubagentLogicalSubagentRow::for_test("sa-tomb", "tomb-worker");
    sa.repo_hash = "repo-h".to_string();
    sa.project_id = "proj-h".to_string();
    sa.status = "deleted".to_string();
    db.subagent_upsert_logical(&sa).unwrap();
    let binding = make_binding("sa-tomb", 1, "hot");
    db.subagent_upsert_hot_binding(&binding).unwrap();

    let counts = subagent_queries::release_hot_bindings_for_shutdown(db.conn(), "pi").unwrap();
    assert_eq!(counts.bindings_released, 1);

    // Tombstone status should remain 'deleted' (not rolled back).
    let logical = subagent_queries::get_logical_subagent(db.conn(), "sa-tomb").unwrap().unwrap();
    assert_eq!(logical.status, "deleted");
}
