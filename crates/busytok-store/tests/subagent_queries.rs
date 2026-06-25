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
    // NOTE: anyhow's plain Display only shows the outer context message; use the
    // alternate `{:#}` formatter to traverse the error chain (rusqlite's
    // "UNIQUE constraint failed: ..." lives one level down).
    assert!(format!("{err:#}").to_lowercase().contains("unique"));
}

#[test]
fn insert_task_and_mark_completed_round_trips() {
    let db = db();
    let sa = SubagentLogicalSubagentRow::for_test("sa-1", "r");
    db.subagent_upsert_logical(&sa).unwrap();
    let task = SubagentTaskRow::for_test("t-1", "sa-1", "pi/search-cheap", "go");
    db.subagent_insert_task(&task).unwrap();

    db.subagent_set_task_status("t-1", "completed", Some("done".to_string()), None)
        .unwrap();
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
