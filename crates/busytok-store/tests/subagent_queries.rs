#![allow(clippy::unwrap_used, clippy::expect_used)]

use busytok_store::repository::{
    SubagentHarnessBindingRow, SubagentLogicalSubagentRow, SubagentTaskRow,
};
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
    let mut mem = busytok_store::repository::SubagentMemoryRow::new_empty("sa-1");
    mem.hot_summary = Some("first".to_string());
    db.subagent_upsert_memory(&mem).unwrap();
    mem.hot_summary = Some("second".to_string());
    db.subagent_upsert_memory(&mem).unwrap();

    let got = db.subagent_get_memory("sa-1").unwrap().unwrap();
    assert_eq!(got.hot_summary.as_deref(), Some("second"));
}

/// Helper: insert a hot binding row for `subagent_id` on the `pi` harness.
fn seed_hot_binding(db: &Database, subagent_id: &str) {
    db.subagent_upsert_binding(&SubagentHarnessBindingRow {
        id: format!("bind_{subagent_id}"),
        subagent_id: subagent_id.to_string(),
        harness: "pi".to_string(),
        adapter_session_id: Some(format!("sess-{subagent_id}")),
        adapter_process_id: Some("12345".to_string()),
        is_hot: 1,
        status: "hot".to_string(),
        created_at_ms: busytok_domain::now_ms(),
        last_used_at_ms: None,
        closed_at_ms: None,
        detail_json: None,
    })
    .unwrap();
}

/// Read the (is_hot, status) of the binding for `subagent_id` on `pi`,
/// or `None` if no binding row exists. Used to verify post-release state.
fn binding_state(db: &Database, subagent_id: &str) -> Option<(i32, String)> {
    db.conn()
        .query_row(
            "SELECT is_hot, status FROM subagent_harness_bindings \
             WHERE subagent_id = ?1 AND harness = 'pi'",
            rusqlite::params![subagent_id],
            |row| Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?)),
        )
        .ok()
}

#[test]
fn release_hot_bindings_for_shutdown_releases_and_rolls_back() {
    // Verifies the graceful-shutdown store contract (spec §3.3):
    //   - releases ALL hot bindings for the harness (is_hot=0, status='closed')
    //     — including bindings on tombstoned subagents (tombstone exclusion
    //     applies to logical STATUS, not to bindings)
    //   - rolls back logical status to warm/cold for affected non-deleted
    //     subagents
    //   - does NOT touch deleted tombstones (Plan 1 deletion semantics)
    //   - does NOT touch in-flight tasks (graceful shutdown lets the sidecar
    //     finish/roll back its own work — the key behavioral difference from
    //     `reconcile_sidecar_crash`, which marks running tasks `failed`)
    let db = db();

    // Subagent A: live, hot, with an in-flight task.
    let mut a = SubagentLogicalSubagentRow::for_test("sa-a", "live");
    a.status = "hot".to_string();
    db.subagent_upsert_logical(&a).unwrap();
    seed_hot_binding(&db, "sa-a");
    let task = SubagentTaskRow::for_test("t-a", "sa-a", "pi/search-cheap", "in-flight");
    db.subagent_insert_task(&task).unwrap();
    // Flip the task to 'running' to simulate an in-flight task mid-shutdown.
    db.subagent_set_task_status("t-a", "running", None, None)
        .unwrap();

    // Subagent B: soft-deleted tombstone, but with a hot binding still in
    // place (simulates a shutdown happening between delegate and a clean
    // delete).
    let mut b = SubagentLogicalSubagentRow::for_test("sa-b", "tomb");
    b.status = "deleted".to_string();
    db.subagent_upsert_logical(&b).unwrap();
    seed_hot_binding(&db, "sa-b");

    // Sanity: both bindings are hot before shutdown; A is hot, B is deleted.
    assert_eq!(binding_state(&db, "sa-a"), Some((1, "hot".to_string())));
    assert_eq!(binding_state(&db, "sa-b"), Some((1, "hot".to_string())));

    let counts = db.subagent_release_hot_bindings_for_shutdown("pi").unwrap();

    // 1. Both hot bindings released (is_hot=0, status='closed') — the
    //    tombstone subagent's binding is ALSO released (bindings are not
    //    tombstone-protected, only logical status is).
    assert_eq!(
        counts.bindings_released, 2,
        "both hot bindings (live + tombstone) must be released"
    );
    assert_eq!(
        binding_state(&db, "sa-a"),
        Some((0, "closed".to_string())),
        "live subagent's binding must be released to is_hot=0, status='closed'"
    );
    assert_eq!(
        binding_state(&db, "sa-b"),
        Some((0, "closed".to_string())),
        "tombstone subagent's binding must also be released (tombstone exclusion applies to logical status only)"
    );
    // No hot bindings remain for the pi harness.
    let hot_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM subagent_harness_bindings WHERE is_hot = 1 AND harness = 'pi'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(hot_count, 0, "no hot bindings should remain after shutdown");

    // 2. The live subagent's status rolled back to warm/cold (no memory
    //    seeded → 'cold'; use the strict spec assertion anyway).
    let a_after = db.subagent_get_logical("sa-a").unwrap().unwrap();
    assert!(
        a_after.status == "warm" || a_after.status == "cold",
        "live subagent status must roll back to warm/cold, got: {:?}",
        a_after.status
    );
    assert_eq!(
        counts.status_rolled_back, 1,
        "only the non-deleted subagent's status is rolled back"
    );

    // 3. The tombstone subagent's status is STILL 'deleted' (tombstone
    //    exclusion — Plan 1 deletion semantics).
    let b_after = db.subagent_get_logical("sa-b").unwrap().unwrap();
    assert_eq!(
        b_after.status, "deleted",
        "soft-deleted subagent must NOT be touched by shutdown reconciliation"
    );

    // 4. The in-flight task is NOT touched — graceful shutdown does not fail
    //    tasks (unlike crash reconciliation, which marks them `failed` /
    //    `SIDECAR_CRASHED`). This is the key behavioral contract difference.
    let task_after = db.subagent_get_task("t-a").unwrap().unwrap();
    assert_eq!(
        task_after.status, "running",
        "in-flight task must NOT be touched by graceful shutdown reconciliation"
    );
    assert_ne!(
        task_after.error.as_deref(),
        Some("SIDECAR_CRASHED"),
        "graceful shutdown must not mark tasks as SIDECAR_CRASHED"
    );
}
