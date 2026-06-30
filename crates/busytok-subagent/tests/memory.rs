#![allow(clippy::unwrap_used)]

use busytok_config::SubagentContextConfig;
use busytok_store::{SubagentMemoryRow, SubagentTaskRow};
use busytok_subagent::memory::{KeyFile, MemoryUpdate, MemoryUpdater, OpenQuestion};

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
        timeout_seconds: None,
        model_override: None,
        error_kind: None,
    }
}

/// Like `task_row` but with `result_summary = None` — mirrors the production
/// data shape for a task that has been inserted but not yet completed (the
/// pre-execution snapshot fetched by `delegate`).
fn task_row_no_summary(id: &str, created_at_ms: i64) -> SubagentTaskRow {
    let mut r = task_row(id, "", created_at_ms);
    r.result_summary = None;
    r.status = "queued".into();
    r.completed_at_ms = None;
    r
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
    let result = updater.update(current, update, &[], 0, "task_1", 4000, REPO);
    let files: Vec<KeyFile> =
        serde_json::from_str(result.key_files_json.as_deref().unwrap()).unwrap();
    assert_eq!(files.len(), 2, "dedupe by normalized path");
    let token = files
        .iter()
        .find(|f| f.path == "src/auth/token.ts")
        .unwrap();
    assert_eq!(token.score, 3, "keep max score on merge");
    assert_eq!(token.last_seen_at_ms, 5000, "update last_seen to latest");
    assert_eq!(
        token.reason, "updated reason",
        "reason updated by new entry"
    );
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
    let result = updater.update(current, update, &[], 0, "task_1", 4000, REPO);
    let files: Vec<KeyFile> =
        serde_json::from_str(result.key_files_json.as_deref().unwrap()).unwrap();
    assert_eq!(
        files[0].path, "src/auth/token.ts",
        "absolute path stripped to repo-relative"
    );
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
    let result = updater.update(current, update, &[], 0, "task_1", 4000, REPO);
    let qs: Vec<OpenQuestion> =
        serde_json::from_str(result.open_questions_json.as_deref().unwrap()).unwrap();
    assert_eq!(qs.len(), 2, "dedupe by lowercase exact match after trim");
    assert!(
        qs.iter().any(|q| q.question == "Does it handle refresh?"),
        "preserve original casing"
    );
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
    let result = updater.update(current, update, &[], 0, "task_1", 4000, REPO);
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
    let result = updater.update(current, update, &[], 0, "task_1", 4000, REPO);
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
    let result = updater.update(current, update, &tasks, 3, "task_3", 4000, REPO);
    let long = result.long_summary.as_deref().unwrap();
    assert!(
        long.contains("old long summary"),
        "preserves old long_summary"
    );
    assert!(
        long.contains("Recent findings:"),
        "appends recent findings header"
    );
    assert!(
        long.contains("added concurrent test"),
        "includes MOST RECENT task summary first"
    );
    assert!(
        result.last_compacted_at_ms.is_some(),
        "updates last_compacted_at_ms"
    );
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
    // 5 tasks (all returned, DESC); compaction must take the 5 most recent,
    // with the newest summary appearing first in the "Recent findings" section.
    let tasks = vec![
        task_row("t5", "newest", 5000),
        task_row("t4", "newer", 4000),
        task_row("t3", "middle", 3000),
        task_row("t2", "older", 2000),
        task_row("t1", "oldest", 1000),
    ];
    let result = updater.update(current, update, &tasks, 5, "t5", 4000, REPO);
    let long = result.long_summary.as_deref().unwrap();
    let findings_start = long
        .find("Recent findings:\n")
        .map(|i| i + "Recent findings:\n".len())
        .unwrap();
    let findings = &long[findings_start..];
    assert!(
        findings.starts_with("newest"),
        "most recent summary must appear first"
    );
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
    let result_no_trigger =
        updater.update(current.clone(), update.clone(), &[], 0, "t1", 4000, REPO);
    assert!(
        result_no_trigger.last_compacted_at_ms.is_none(),
        "should not compact under threshold"
    );

    // profile_budget_tokens=1000 → budget_chars=4000, threshold=400.
    // hot+long = 1000 chars > 400 → trigger (b).
    let result_trigger = updater.update(current, update, &[], 0, "t1", 1000, REPO);
    assert!(
        result_trigger.last_compacted_at_ms.is_some(),
        "trigger (b) fires using profile budget"
    );
}

#[test]
fn compaction_trigger_a_fires_when_tasks_since_exceeds_recent_tasks_len() {
    // P1-2 regression: the authoritative `tasks_since_last_compaction` count
    // comes from a dedicated store query, NOT from `recent_tasks.len()`.
    // When `compaction_tasks_threshold > recent_tasks_limit`, the capped
    // recent_tasks slice would undercount and trigger (a) would never fire.
    // This test proves the authoritative count drives the trigger even when
    // recent_tasks is shorter than the threshold.
    let mut cfg = cfg();
    cfg.compaction_tasks_threshold = 10;
    cfg.compaction_budget_ratio = 0.99; // disable trigger (b)
    let updater = MemoryUpdater::new(cfg);
    let current = mem_row("sub-a");
    let update = MemoryUpdate {
        current_state_summary: Some("state".into()),
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![],
    };
    // recent_tasks has only 3 entries (capped by recent_tasks_limit in real
    // usage), but the store reports 10 tasks since last compaction.
    let tasks = vec![
        task_row("t1", "old", 1000),
        task_row("t2", "mid", 2000),
        task_row("t3", "new", 3000),
    ];
    let result = updater.update(current, update, &tasks, 10, "t3", 4000, REPO);
    assert!(
        result.last_compacted_at_ms.is_some(),
        "trigger (a) must fire using authoritative count (10 >= threshold 10), \
         even though recent_tasks.len() == 3 < threshold"
    );
}

#[test]
fn compaction_trigger_b_fires_when_recent_summaries_large_but_memory_small() {
    // P2-3 regression: trigger (b) must include recent task summaries in the
    // size estimate (§6.2: "hot_summary + long_summary + recent summaries").
    // When memory itself is small but recent summaries are large, compaction
    // must still fire.
    let mut cfg = cfg();
    cfg.compaction_tasks_threshold = 100; // disable trigger (a)
    cfg.compaction_budget_ratio = 0.1; // trigger (b) fires easily
    let updater = MemoryUpdater::new(cfg);
    let mut current = mem_row("sub-a");
    current.hot_summary = Some("tiny".into()); // 4 chars
    current.long_summary = Some("tiny".into()); // 4 chars
    let update = MemoryUpdate {
        current_state_summary: Some("state".into()),
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![],
    };
    // profile_budget_tokens=1000 → budget_chars=4000, threshold=400.
    // hot+long = 8 chars < 400 → no trigger WITHOUT recent summaries.
    // But recent summaries add 500 chars → total = 508 > 400 → trigger (b).
    let tasks = vec![task_row("t1", &"x".repeat(500), 1000)];
    let result = updater.update(current, update, &tasks, 0, "t1", 1000, REPO);
    assert!(
        result.last_compacted_at_ms.is_some(),
        "trigger (b) must fire when recent summaries push total over threshold, \
         even if hot_summary + long_summary alone are small"
    );
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
    let result = updater.update(current, update, &[], 0, "task_1", 4000, REPO);
    assert_eq!(
        result.long_summary.as_deref(),
        Some("existing summary"),
        "long_summary unchanged when no compaction"
    );
    assert!(
        result.last_compacted_at_ms.is_none(),
        "no compaction timestamp"
    );
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
    let result = updater.update(current, update, &tasks, 1, "task_1", 4000, REPO);
    let files: Vec<KeyFile> =
        serde_json::from_str(result.key_files_json.as_deref().unwrap()).unwrap();
    assert!(
        files.len() <= 20,
        "key_files capped at 20 during compaction, got {}",
        files.len()
    );
}

#[test]
fn caps_not_applied_outside_compaction() {
    let mut cfg = cfg();
    cfg.compaction_tasks_threshold = 100; // no compaction
    let updater = MemoryUpdater::new(cfg);
    let mut current = mem_row("sub-a");
    // 25 decisions — exceeds cap of 20, but no compaction should fire.
    current.decisions_json =
        Some(serde_json::to_string(&(0..25).map(|i| format!("d{i}")).collect::<Vec<_>>()).unwrap());
    let update = MemoryUpdate {
        current_state_summary: Some("state".into()),
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![],
    };
    let result = updater.update(current, update, &[], 0, "task_1", 4000, REPO);
    let decisions: Vec<String> =
        serde_json::from_str(result.decisions_json.as_deref().unwrap()).unwrap();
    assert_eq!(
        decisions.len(),
        25,
        "caps NOT applied outside compaction — preserve data"
    );
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
    let result = updater.update(current, update, &tasks, 1, "task_1", 4000, REPO);
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
    let result = updater.update(current, update, &tasks, 3, "t3", 4000, REPO);
    let attempts: Vec<String> =
        serde_json::from_str(result.attempts_json.as_deref().unwrap()).unwrap();
    assert_eq!(attempts.len(), 1, "one attempt appended");
    assert!(
        attempts[0].contains("newest summary"),
        "attempts records MOST RECENT task, got: {:?}",
        attempts[0]
    );
}

#[test]
fn attempts_skips_when_most_recent_summary_is_none() {
    // C-1 regression: in production, the pre-execution `recent_tasks` snapshot
    // fetched by `delegate` has result_summary=None for the current (just-
    // inserted, still-queued) task. The attempts logic must handle this
    // gracefully — skip the entry rather than panicking — and must NOT fall
    // back to an older task's summary. The C-1 fix in manager.rs re-fetches
    // recent_tasks after the result is persisted, so this None shape should
    // never reach memory_updater in the delegate hot path; this test pins the
    // defensive behavior so a future regression (stale snapshot) degrades
    // gracefully instead of panicking.
    let mut cfg = cfg();
    cfg.compaction_tasks_threshold = 100; // no compaction
    let updater = MemoryUpdater::new(cfg);
    let mut current = mem_row("sub-a");
    current.attempts_json = Some(serde_json::to_string(&["prior: old run"]).unwrap());
    let update = MemoryUpdate {
        current_state_summary: Some("state".into()),
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![],
    };
    // Most recent task (first in DESC order) has result_summary=None — the
    // production shape at fetch time before the result is written.
    let tasks = vec![
        task_row_no_summary("t3", 3000),
        task_row("t2", "middle summary", 2000),
        task_row("t1", "oldest summary", 1000),
    ];
    let result = updater.update(current, update, &tasks, 3, "t3", 4000, REPO);
    let attempts: Vec<String> =
        serde_json::from_str(result.attempts_json.as_deref().unwrap()).unwrap();
    assert_eq!(
        attempts.len(),
        1,
        "no new attempt appended when most recent task has no summary; \
         existing entry preserved. got: {attempts:?}"
    );
    assert!(
        attempts[0] == "prior: old run",
        "must NOT fall back to an older task's summary, got: {:?}",
        attempts[0]
    );
}

#[test]
fn compaction_long_summary_is_none_when_empty() {
    // m-1 regression: after compaction, long_summary must be None (not
    // Some("")) when the rebuilt long summary is empty. Some("") would be
    // semantically wrong (claims a summary exists) and would skew the
    // compaction trigger (b) size estimate on the next turn.
    let mut cfg = cfg();
    cfg.compaction_tasks_threshold = 1; // force compaction
    let updater = MemoryUpdater::new(cfg);
    let current = mem_row("sub-a"); // no long_summary, no hot_summary
    let update = MemoryUpdate {
        current_state_summary: None,
        key_files: vec![],
        decisions: vec![],
        open_questions: vec![],
    };
    // No recent task summaries → rebuilt long_summary is empty → must be None.
    let result = updater.update(current, update, &[], 1, "t1", 4000, REPO);
    assert!(
        result.long_summary.is_none(),
        "long_summary must be None (not Some(\"\")) when compaction produced \
         an empty summary, got: {:?}",
        result.long_summary.as_deref()
    );
}
