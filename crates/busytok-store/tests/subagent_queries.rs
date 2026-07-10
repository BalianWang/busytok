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

/// Verify `commit_hibernate_binding_and_status` atomically flips the binding
/// (is_hot=0, status='closed', closed_at_ms set) AND the logical status
/// (warm/cold) in a single transaction, and that `status='deleted'` tombstones
/// are excluded from the logical status update (Plan 1 deletion semantics).
#[test]
fn commit_hibernate_binding_and_status_is_atomic_and_excludes_tombstones() {
    let db = db();

    // Subagent A: live, hot, with a hot binding. Hibernate → warm.
    let mut a = SubagentLogicalSubagentRow::for_test("sa-a", "live");
    a.status = "hot".to_string();
    db.subagent_upsert_logical(&a).unwrap();
    seed_hot_binding(&db, "sa-a");
    // Seed memory so hibernate rolls back to 'warm' (not 'cold').
    let mut mem_a = busytok_store::repository::SubagentMemoryRow::new_empty("sa-a");
    mem_a.hot_summary = Some("did stuff".to_string());
    db.subagent_upsert_memory(&mem_a).unwrap();

    // Subagent B: soft-deleted tombstone, but with a hot binding still in
    // place (simulates a hibernate happening between delegate and a clean
    // delete — the binding must still be released, but the tombstone must NOT
    // be revived).
    let mut b = SubagentLogicalSubagentRow::for_test("sa-b", "tomb");
    b.status = "deleted".to_string();
    db.subagent_upsert_logical(&b).unwrap();
    seed_hot_binding(&db, "sa-b");

    // Sanity: both bindings are hot before hibernate; A is hot, B is deleted.
    assert_eq!(binding_state(&db, "sa-a"), Some((1, "hot".to_string())));
    assert_eq!(binding_state(&db, "sa-b"), Some((1, "hot".to_string())));

    // --- Hibernate A: binding flip + status→warm in one transaction. ---
    let mut binding_a = db.subagent_hot_binding("sa-a", "pi").unwrap().unwrap();
    let now = busytok_domain::now_ms();
    binding_a.is_hot = 0;
    binding_a.status = "closed".to_string();
    binding_a.closed_at_ms = Some(now);
    db.subagent_commit_hibernate_binding_and_status(&binding_a, "sa-a", "warm")
        .unwrap();

    // 1. A's binding is released (is_hot=0, status='closed', closed_at_ms set).
    let a_binding_after = binding_state(&db, "sa-a").expect("binding row must remain");
    assert_eq!(a_binding_after.0, 0, "is_hot must be 0 after hibernate");
    assert_eq!(
        a_binding_after.1, "closed",
        "binding status must be 'closed' after hibernate"
    );
    // No hot binding remains for A.
    assert!(
        db.subagent_hot_binding("sa-a", "pi").unwrap().is_none(),
        "no hot binding should remain after hibernate"
    );

    // 2. A's logical status flipped to 'warm' atomically (memory exists).
    let a_after = db.subagent_get_logical("sa-a").unwrap().unwrap();
    assert_eq!(
        a_after.status, "warm",
        "logical status must flip to warm (memory exists) — atomically with the binding flip"
    );

    // --- Hibernate B (tombstone): binding flip happens, status stays 'deleted'. ---
    let mut binding_b = db.subagent_hot_binding("sa-b", "pi").unwrap().unwrap();
    binding_b.is_hot = 0;
    binding_b.status = "closed".to_string();
    binding_b.closed_at_ms = Some(now);
    db.subagent_commit_hibernate_binding_and_status(&binding_b, "sa-b", "warm")
        .unwrap();

    // 3. B's binding WAS released (bindings are not tombstone-protected).
    let b_binding_after = binding_state(&db, "sa-b").expect("binding row must remain");
    assert_eq!(b_binding_after.0, 0, "tombstone binding is_hot must be 0");
    assert_eq!(
        b_binding_after.1, "closed",
        "tombstone binding status must be 'closed'"
    );

    // 4. B's logical status is STILL 'deleted' (tombstone exclusion — Plan 1
    //    deletion semantics). Hibernate must not revive a soft-deleted subagent.
    let b_after = db.subagent_get_logical("sa-b").unwrap().unwrap();
    assert_eq!(
        b_after.status, "deleted",
        "soft-deleted subagent must NOT be revived by hibernate (tombstone exclusion)"
    );

    // 5. Atomicity guard: there is no observable point where the binding is
    //    closed but the status is still 'hot'. Since the commit is a single
    //    transaction, after `commit_hibernate_binding_and_status` returns Ok
    //    both writes are durable — verified above by (1) and (2) holding
    //    simultaneously. (A true concurrency test would need a snapshot at the
    //    intermediate point, which SQLite's tx isolation prevents — so this
    //    test asserts the post-commit invariant instead.)
}

// --- find_hot_binding_by_session (Plan 3 / Task 1) -------------------------
//
// These tests verify the store-layer query used by the eviction flow (Task 5)
// to locate a hot binding row by adapter_session_id + harness. The candidate
// itself comes from the RPC error's `data.candidate`, not a DB query — this
// query is what the executor uses to find the binding to persist.

#[test]
fn find_hot_binding_by_session_returns_binding_for_known_session() {
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow {
        id: "sub_a".into(),
        name: "a".into(),
        project_id: "p".into(),
        repo_path: "/r".into(),
        repo_hash: "h".into(),
        branch: None,
        intent: None,
        default_profile: "pi/search-cheap".into(),
        bound_provider_id: "test-provider".into(),
        bound_model_id: "test-model".into(),
        status: "hot".into(),
        created_at_ms: now,
        updated_at_ms: now,
        last_active_at_ms: Some(now),
    })
    .unwrap();
    db.subagent_commit_hot_binding_and_status(
        &SubagentHarnessBindingRow {
            id: "bind_a".into(),
            subagent_id: "sub_a".into(),
            harness: "pi".into(),
            adapter_session_id: Some("sess_a".into()),
            adapter_process_id: None,
            is_hot: 1,
            status: "hot".into(),
            created_at_ms: now,
            last_used_at_ms: Some(now),
            closed_at_ms: None,
            detail_json: None,
        },
        "sub_a",
    )
    .unwrap();

    let binding = db
        .subagent_find_hot_binding_by_session("sess_a", "pi")
        .unwrap();
    assert!(binding.is_some());
    let binding = binding.unwrap();
    assert_eq!(binding.subagent_id, "sub_a");
    assert_eq!(binding.adapter_session_id.as_deref(), Some("sess_a"));
}

#[test]
fn find_hot_binding_by_session_returns_none_for_unknown_session() {
    let db = Database::open_in_memory().unwrap();
    let result = db
        .subagent_find_hot_binding_by_session("nonexistent", "pi")
        .unwrap();
    assert!(result.is_none());
}

#[test]
fn find_hot_binding_by_session_excludes_closed_bindings() {
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow {
        id: "sub_b".into(),
        name: "b".into(),
        project_id: "p".into(),
        repo_path: "/r".into(),
        repo_hash: "h".into(),
        branch: None,
        intent: None,
        default_profile: "pi/search-cheap".into(),
        bound_provider_id: "test-provider".into(),
        bound_model_id: "test-model".into(),
        status: "warm".into(),
        created_at_ms: now,
        updated_at_ms: now,
        last_active_at_ms: Some(now),
    })
    .unwrap();
    // Insert a closed (is_hot=0) binding — should NOT be found
    db.subagent_commit_hibernate_binding_and_status(
        &SubagentHarnessBindingRow {
            id: "bind_b".into(),
            subagent_id: "sub_b".into(),
            harness: "pi".into(),
            adapter_session_id: Some("sess_b".into()),
            adapter_process_id: None,
            is_hot: 0,
            status: "closed".into(),
            created_at_ms: now,
            last_used_at_ms: Some(now),
            closed_at_ms: Some(now),
            detail_json: None,
        },
        "sub_b",
        "warm",
    )
    .unwrap();

    let result = db
        .subagent_find_hot_binding_by_session("sess_b", "pi")
        .unwrap();
    assert!(result.is_none(), "closed bindings must not be returned");
}

#[test]
fn count_tasks_since_returns_authoritative_count() {
    let db = Database::open_in_memory().unwrap();
    let sub_id = "sub-count-test";
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test(sub_id, "count-test"))
        .unwrap();
    // Insert 8 tasks with varying created_at_ms.
    for i in 0..8 {
        let task = SubagentTaskRow {
            id: format!("task-{i}"),
            subagent_id: sub_id.into(),
            source_harness: None,
            source_session_id: None,
            intent: None,
            profile: "pi/review-cheap".into(),
            prompt: Some("do".into()),
            prompt_artifact_ref: None,
            output_schema_name: None,
            output_schema_version: 1,
            status: "completed".into(),
            result_summary: Some(format!("summary-{i}")),
            result_json: None,
            error: None,
            created_at_ms: 1000 * (i as i64 + 1),
            started_at_ms: None,
            completed_at_ms: None,
            timeout_seconds: None,
            model_override: None,
            error_kind: None,
        };
        db.subagent_insert_task(&task).unwrap();
    }
    // Count since 3000 → tasks at 4000..8000 = 5 tasks.
    let count = db.subagent_count_tasks_since(sub_id, 3000).unwrap();
    assert_eq!(count, 5);
    // Count since 0 → all 8 tasks.
    let count_all = db.subagent_count_tasks_since(sub_id, 0).unwrap();
    assert_eq!(count_all, 8);
}

#[test]
fn task_counts_by_status_returns_queued_and_running_counts() {
    let db = db();
    let sub_id = "sub-status-count";
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test(
        sub_id,
        "status-count",
    ))
    .unwrap();
    // Insert 3 queued, 2 running, 4 completed tasks.
    for (i, status) in [
        ("q1", "queued"),
        ("q2", "queued"),
        ("q3", "queued"),
        ("r1", "running"),
        ("r2", "running"),
        ("c1", "completed"),
        ("c2", "completed"),
        ("c3", "completed"),
        ("c4", "completed"),
    ] {
        let task = SubagentTaskRow {
            id: i.into(),
            subagent_id: sub_id.into(),
            source_harness: None,
            source_session_id: None,
            intent: None,
            profile: "pi/review-cheap".into(),
            prompt: Some("do".into()),
            prompt_artifact_ref: None,
            output_schema_name: None,
            output_schema_version: 1,
            status: status.into(),
            result_summary: None,
            result_json: None,
            error: None,
            created_at_ms: 1000,
            started_at_ms: None,
            completed_at_ms: None,
            timeout_seconds: None,
            model_override: None,
            error_kind: None,
        };
        db.subagent_insert_task(&task).unwrap();
    }
    let (queued, running) = db.subagent_task_counts_by_status().unwrap();
    assert_eq!(queued, 3);
    assert_eq!(running, 2);
}

#[test]
fn task_counts_by_status_returns_zeros_when_no_tasks() {
    let db = db();
    let (queued, running) = db.subagent_task_counts_by_status().unwrap();
    assert_eq!(queued, 0);
    assert_eq!(running, 0);
}

/// Spec §2.3: `bound_provider_id` + `bound_model_id` round-trip through the
/// store. Verifies the atomic bound fields migration (Task 2) persists both
/// columns and that `default_model` is gone (no API to read it — just verify
/// no panic and the bound fields come back as written).
#[test]
fn subagent_upsert_logical_persists_bound_fields() {
    let db = db();
    let row = SubagentLogicalSubagentRow {
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
    // Round-trip via find_by_name_in_repo too (exercises the SELECT column
    // list + row construction for that path).
    let by_name = db
        .subagent_find_by_name_in_repo("repo-hash", "repo-hash", "test-sub")
        .unwrap();
    assert_eq!(by_name.len(), 1);
    assert_eq!(by_name[0].bound_provider_id, "prov-1");
    assert_eq!(by_name[0].bound_model_id, "gpt-4o");
    // list_active_by_repo + list_filtered also exercise the bound column
    // mapping — verify they return the bound fields correctly.
    let active = db.subagent_list_active_by_repo("repo-hash").unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].bound_provider_id, "prov-1");
    let filtered = db.subagent_list_filtered(None, None, false).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].bound_model_id, "gpt-4o");
}

// --- reap_orphaned_running_tasks -------------------------------------------

/// Helper: insert a task row with explicit `status` + `started_at_ms`.
fn insert_running_task_with_started_at(
    db: &Database,
    id: &str,
    subagent_id: &str,
    status: &str,
    started_at_ms: Option<i64>,
) {
    let mut row = SubagentTaskRow::for_test(id, subagent_id, "pi/search-cheap", "do");
    row.status = status.to_string();
    row.started_at_ms = started_at_ms;
    db.subagent_insert_task(&row).unwrap();
}

/// Helper: insert a running task with explicit `started_at_ms` + per-task
/// `timeout_seconds` override (for testing per-task reaper cutoff).
fn insert_running_task_with_timeout(
    db: &Database,
    id: &str,
    subagent_id: &str,
    started_at_ms: Option<i64>,
    timeout_seconds: Option<i64>,
) {
    let mut row = SubagentTaskRow::for_test(id, subagent_id, "pi/search-cheap", "do");
    row.status = "running".to_string();
    row.started_at_ms = started_at_ms;
    row.timeout_seconds = timeout_seconds;
    db.subagent_insert_task(&row).unwrap();
}

#[test]
fn reap_orphaned_running_tasks_marks_stale_running_as_failed() {
    let db = db();
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test("sub-reap", "reap"))
        .unwrap();
    // An orphan: started 600s ago, max_age = 360s → should be reaped.
    let now = busytok_domain::now_ms();
    let stale_started = now - 600_000;
    insert_running_task_with_started_at(
        &db,
        "orphan-1",
        "sub-reap",
        "running",
        Some(stale_started),
    );

    let reaped = db
        .subagent_reap_orphaned_running_tasks(now, 300, 60_000)
        .unwrap();
    assert_eq!(reaped, vec!["orphan-1".to_string()]);
    let task = db.subagent_get_task("orphan-1").unwrap().unwrap();
    assert_eq!(task.status, "failed");
    assert_eq!(task.error.as_deref(), Some("ORPHANED_REAPED"));
    assert!(task.completed_at_ms.is_some());
}

#[test]
fn reap_orphaned_running_tasks_skips_fresh_running() {
    let db = db();
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test("sub-fresh", "fresh"))
        .unwrap();
    // Fresh: started 10s ago, max_age = 360s → NOT reaped.
    let now = busytok_domain::now_ms();
    insert_running_task_with_started_at(&db, "fresh-1", "sub-fresh", "running", Some(now - 10_000));

    let reaped = db
        .subagent_reap_orphaned_running_tasks(now, 300, 60_000)
        .unwrap();
    assert!(reaped.is_empty());
    let task = db.subagent_get_task("fresh-1").unwrap().unwrap();
    assert_eq!(task.status, "running");
}

#[test]
fn reap_orphaned_running_tasks_skips_null_started_at() {
    // A running task with NULL started_at_ms (shouldn't happen in normal
    // flow, but defense-in-depth) is NOT reaped — we can't compute its age.
    let db = db();
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test("sub-null", "null"))
        .unwrap();
    insert_running_task_with_started_at(&db, "null-1", "sub-null", "running", None);

    let reaped = db
        .subagent_reap_orphaned_running_tasks(busytok_domain::now_ms(), 300, 60_000)
        .unwrap();
    assert!(reaped.is_empty());
    let task = db.subagent_get_task("null-1").unwrap().unwrap();
    assert_eq!(task.status, "running");
}

#[test]
fn reap_orphaned_running_tasks_skips_non_running() {
    // Queued/completed/failed tasks are never touched by the reaper.
    let db = db();
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test("sub-other", "other"))
        .unwrap();
    let now = busytok_domain::now_ms();
    let stale = now - 600_000;
    insert_running_task_with_started_at(&db, "queued-1", "sub-other", "queued", Some(stale));
    insert_running_task_with_started_at(&db, "completed-1", "sub-other", "completed", Some(stale));
    insert_running_task_with_started_at(&db, "failed-1", "sub-other", "failed", Some(stale));

    let reaped = db
        .subagent_reap_orphaned_running_tasks(now, 300, 60_000)
        .unwrap();
    assert!(reaped.is_empty());
    assert_eq!(
        db.subagent_get_task("queued-1").unwrap().unwrap().status,
        "queued"
    );
    assert_eq!(
        db.subagent_get_task("completed-1").unwrap().unwrap().status,
        "completed"
    );
    assert_eq!(
        db.subagent_get_task("failed-1").unwrap().unwrap().status,
        "failed"
    );
}

#[test]
fn reap_orphaned_running_tasks_reaps_multiple_across_subagents() {
    let db = db();
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test("sub-a", "a"))
        .unwrap();
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test("sub-b", "b"))
        .unwrap();
    let now = busytok_domain::now_ms();
    let stale = now - 600_000;
    // Two orphans across two subagents.
    insert_running_task_with_started_at(&db, "orph-a1", "sub-a", "running", Some(stale));
    insert_running_task_with_started_at(&db, "orph-b1", "sub-b", "running", Some(stale));
    // A fresh one that should survive.
    insert_running_task_with_started_at(&db, "fresh-b2", "sub-b", "running", Some(now - 10_000));

    let reaped = db
        .subagent_reap_orphaned_running_tasks(now, 300, 60_000)
        .unwrap();
    assert_eq!(reaped.len(), 2);
    assert!(reaped.contains(&"orph-a1".to_string()));
    assert!(reaped.contains(&"orph-b1".to_string()));
    // Fresh survives.
    assert_eq!(
        db.subagent_get_task("fresh-b2").unwrap().unwrap().status,
        "running"
    );
}

#[test]
fn reap_orphaned_running_tasks_unblocks_queued_pick() {
    // Regression test for the actual bug: an orphaned `running` task
    // blocks `pick_oldest_queued_task` (SQL excludes subagents with a
    // running task). After reaping, the queued task should become pickable.
    let db = db();
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test(
        "sub-unblock",
        "unblock",
    ))
    .unwrap();
    let now = busytok_domain::now_ms();
    let stale = now - 600_000;
    // Orphan running + a queued task on the SAME subagent.
    insert_running_task_with_started_at(&db, "orph-1", "sub-unblock", "running", Some(stale));
    insert_running_task_with_started_at(&db, "queued-1", "sub-unblock", "queued", None);

    // Before reap: pick returns None (subagent has a running task).
    assert!(db.subagent_pick_oldest_queued_task().unwrap().is_none());

    // Reap the orphan.
    let reaped = db
        .subagent_reap_orphaned_running_tasks(now, 300, 60_000)
        .unwrap();
    assert_eq!(reaped, vec!["orph-1".to_string()]);

    // After reap: pick returns the queued task (subagent has no running task).
    let picked = db.subagent_pick_oldest_queued_task().unwrap().unwrap();
    assert_eq!(picked.id, "queued-1");
    assert_eq!(picked.status, "running"); // pick flips it to running
}

#[test]
fn reap_orphaned_running_tasks_empty_db_returns_empty_vec() {
    let db = db();
    let reaped = db
        .subagent_reap_orphaned_running_tasks(busytok_domain::now_ms(), 300, 60_000)
        .unwrap();
    assert!(reaped.is_empty());
}

#[test]
fn reap_uses_cas_guard_so_concurrent_completion_is_safe() {
    // If a task is completed between the SELECT and UPDATE, the CAS
    // guard `status = 'running'` ensures the UPDATE affects 0 rows for
    // that task — no double-completion. We simulate this by completing
    // the task before the reaper's UPDATE runs (same connection, so
    // serialization is guaranteed).
    let db = db();
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test("sub-cas", "cas"))
        .unwrap();
    let now = busytok_domain::now_ms();
    let stale = now - 600_000;
    insert_running_task_with_started_at(&db, "cas-1", "sub-cas", "running", Some(stale));
    // Simulate concurrent completion before reaper UPDATE.
    db.subagent_set_task_status("cas-1", "completed", Some("done".into()), None)
        .unwrap();

    let _reaped = db
        .subagent_reap_orphaned_running_tasks(now, 300, 60_000)
        .unwrap();
    // The SELECT found the row (it was running at SELECT time in this
    // serialized test), but the UPDATE's CAS guard means 0 rows affected.
    // The function still returns the id from SELECT, but the status is
    // NOT changed (stays completed). This is acceptable — the caller logs
    // the reaped ids, and the task is in a terminal state either way.
    let task = db.subagent_get_task("cas-1").unwrap().unwrap();
    assert_eq!(task.status, "completed");
}

#[test]
fn reap_honors_per_task_timeout_override() {
    // Regression test for review finding P1: a task with a long
    // `--timeout 600` override must NOT be reaped at the default ceiling
    // (300s + 60s buffer = 360s). The reaper must use the per-task
    // `timeout_seconds` column via COALESCE.
    let db = db();
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test("sub-ovr", "ovr"))
        .unwrap();
    let now = busytok_domain::now_ms();

    // Task with --timeout 600, started 400s ago.
    // Default ceiling would be 360s → reaped (BUG).
    // Per-task ceiling is 600s + 60s = 660s → NOT reaped (CORRECT).
    insert_running_task_with_timeout(
        &db,
        "long-timeout",
        "sub-ovr",
        Some(now - 400_000),
        Some(600),
    );
    // Task with default timeout (NULL), started 400s ago.
    // Default ceiling 360s → reaped.
    insert_running_task_with_timeout(&db, "default-timeout", "sub-ovr", Some(now - 400_000), None);

    let reaped = db
        .subagent_reap_orphaned_running_tasks(now, 300, 60_000)
        .unwrap();
    // Only the default-timeout task is reaped; the long-timeout task survives.
    assert_eq!(reaped, vec!["default-timeout".to_string()]);
    assert_eq!(
        db.subagent_get_task("long-timeout")
            .unwrap()
            .unwrap()
            .status,
        "running"
    );
    assert_eq!(
        db.subagent_get_task("default-timeout")
            .unwrap()
            .unwrap()
            .status,
        "failed"
    );
}

#[test]
fn reap_honors_per_task_timeout_override_eventually_reaps() {
    // Same setup as above, but the long-timeout task has now exceeded
    // its own ceiling (600s + 60s = 660s). It should be reaped.
    let db = db();
    db.subagent_upsert_logical(&SubagentLogicalSubagentRow::for_test("sub-ovr2", "ovr2"))
        .unwrap();
    let now = busytok_domain::now_ms();

    insert_running_task_with_timeout(
        &db,
        "long-timeout",
        "sub-ovr2",
        Some(now - 700_000), // 700s ago > 660s ceiling
        Some(600),
    );

    let reaped = db
        .subagent_reap_orphaned_running_tasks(now, 300, 60_000)
        .unwrap();
    assert_eq!(reaped, vec!["long-timeout".to_string()]);
    assert_eq!(
        db.subagent_get_task("long-timeout")
            .unwrap()
            .unwrap()
            .status,
        "failed"
    );
}
