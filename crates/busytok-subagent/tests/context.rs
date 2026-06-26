#![allow(clippy::unwrap_used)]
#![allow(unused_imports)]

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
    m.decisions_json =
        Some(serde_json::to_string(&["Focus on read-only analysis".to_string()]).unwrap());
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
    assert!(
        ctx.compact_context.contains("auth-investigator"),
        "includes subagent name"
    );
    assert!(
        ctx.compact_context.contains("Study auth refresh logic"),
        "includes intent"
    );
    assert!(
        ctx.compact_context
            .contains("Check concurrent refresh handling"),
        "includes prompt"
    );
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
    assert!(ctx
        .compact_context
        .contains("Token refresh logic is in src/auth/token.ts."));
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
    assert!(
        ctx.compact_context.contains(&prompt),
        "prompt never trimmed under hard limit"
    );
}

#[test]
fn trims_recent_task_summaries_when_over_budget() {
    let builder = ContextBuilder::new(cfg());
    let tiny_budget = 200u32;
    let tasks: Vec<SubagentTaskRow> = (0..5)
        .map(|i| {
            task_row(
                &format!("task_{i}"),
                &format!("summary_{i} padding ").repeat(20),
                1000 + i,
            )
        })
        .collect();
    let (ctx, _snap) = builder.build(&subagent(), &mem_row(), &tasks, "short prompt", tiny_budget);
    assert!(ctx.compact_context.contains("short prompt"));
    let present_count = (0..5)
        .filter(|i| ctx.compact_context.contains(&format!("summary_{i}")))
        .count();
    assert!(
        present_count < 5,
        "expected trimming, {present_count} summaries present"
    );
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
    //
    // With 1 item per trimmable section, the progressive reduction levels
    // (5→3, 20→10, 10→5) don't apply (counts already below thresholds), so
    // sections are dropped directly in priority order. Progressive reduction
    // is exercised by `progressive_trimming_reduces_recent_summaries_5_to_3_*`
    // and `progressive_trimming_truncates_long_summary_to_fit`.
    let tasks = vec![task_row(
        "t1",
        "recent summary that is long enough to matter here padding",
        1000,
    )];
    // hot_summary (~44 chars) + long_summary (~1650 chars) + prompt (~10) + header (~94) ≈ 1798 chars.
    // Adding recent_summaries (~74) + key_files (~29) + open_questions (~46) → ~1947 chars total.
    // Budget 470 tokens → 1880 chars → tight; drop recent+keyfiles+questions (priority 1,3,4) to fit,
    // keep long_summary (priority 5) + hot_summary (priority 6) + prompt (priority 7).
    let (ctx, _snap) = builder.build(&subagent(), &mem, &tasks, "the prompt", 470);
    assert!(
        ctx.compact_context.contains("the prompt"),
        "prompt kept (priority 7)"
    );
    assert!(
        ctx.compact_context.contains("Token refresh logic"),
        "hot_summary kept (priority 6)"
    );
    // recent_summaries (priority 1) and key_files (priority 3) and open_questions (priority 4) dropped.
    assert!(
        !ctx.compact_context.contains("recent summary that is long"),
        "recent_summaries dropped (priority 1)"
    );
    assert!(
        !ctx.compact_context.contains("Key files:"),
        "key_files dropped (priority 3)"
    );
    assert!(
        !ctx.compact_context.contains("Open questions:"),
        "open_questions dropped (priority 4)"
    );
}

#[test]
fn progressive_trimming_reduces_recent_summaries_5_to_3_before_dropping() {
    let builder = ContextBuilder::new(cfg());
    // 5 recent summaries (each 57 chars). recent_summaries section: 314 chars
    // at N=5, 194 chars at N=3. Other sections are small so reducing recent
    // from 5 to 3 is enough to fit.
    let tasks: Vec<SubagentTaskRow> = (0..5)
        .map(|i| {
            task_row(
                &format!("task_{i}"),
                &format!("summary_{i} padding ").repeat(3),
                1000 + i,
            )
        })
        .collect();
    // Use a small long_summary so it doesn't dominate the budget.
    let mut mem = mem_row();
    mem.long_summary = Some("Short long summary.".into());
    // Total with 5 summaries ≈ 607 chars; with 3 summaries ≈ 487 chars.
    // Budget 140 tokens = 560 chars: 5 doesn't fit (607 > 560), 3 fits (487 ≤ 560).
    let (ctx, _snap) = builder.build(&subagent(), &mem, &tasks, "do thing", 140);
    // Tasks are DESC-sorted by created_at_ms (1000+i). Take 3 = task_4, task_3,
    // task_2 (most recent). Dropped = task_1, task_0 (oldest).
    assert!(
        ctx.compact_context.contains("summary_4"),
        "most recent summary kept (task_4)"
    );
    assert!(
        ctx.compact_context.contains("summary_3"),
        "2nd most recent summary kept (task_3)"
    );
    assert!(
        ctx.compact_context.contains("summary_2"),
        "3rd most recent summary kept (task_2)"
    );
    assert!(
        !ctx.compact_context.contains("summary_1"),
        "4th most recent summary dropped (progressive reduction 5→3)"
    );
    assert!(
        !ctx.compact_context.contains("summary_0"),
        "5th most recent summary dropped (progressive reduction 5→3)"
    );
    // Lower-priority sections are NOT yet trimmed (reduction stopped at priority 1).
    assert!(
        ctx.compact_context.contains("Key files:"),
        "key_files not yet trimmed (priority 3)"
    );
    assert!(
        ctx.compact_context.contains("Open questions:"),
        "open_questions not yet trimmed (priority 4)"
    );
    assert!(
        ctx.compact_context.contains("Long-term findings:"),
        "long_summary not yet trimmed (priority 5)"
    );
    assert!(
        ctx.compact_context.contains("Current state:"),
        "hot_summary preserved (priority 6)"
    );
    assert!(
        ctx.compact_context.contains("do thing"),
        "prompt preserved (priority 7)"
    );
}

#[test]
fn progressive_trimming_truncates_long_summary_to_fit() {
    let builder = ContextBuilder::new(cfg());
    // No recent tasks, no key_files, no open_questions — only long_summary
    // is trimmable. long_summary is 1000 'X' chars; budget forces truncation
    // (not dropping) of the long_summary section.
    let mut mem = SubagentMemoryRow::new_empty("sub-a");
    mem.hot_summary = Some("Hot.".into());
    mem.long_summary = Some("X".repeat(1000));
    // Sections: header (~95) + long_summary (~1021) + hot_summary (~20) + prompt (~23) ≈ 1159 chars.
    // Budget 200 tokens = 800 chars: full (1159) > 800, so trim.
    // No recent/key_files/open_questions to trim (all empty). Reach priority 5:
    //   without_long = 95 + 20 + 23 = 138 chars. remaining = 800 - 138 = 662.
    //   overhead = 21 ("Long-term findings:\n" + "\n"). long_chars = 662 - 21 = 641.
    //   Truncated total = 138 + 20 + 641 + 1 = 800 ≤ 800. Fits!
    // So long_summary is truncated to 641 'X' chars (not dropped).
    let (ctx, _snap) = builder.build(&subagent(), &mem, &[], "do thing", 200);
    assert!(
        ctx.compact_context.contains("Long-term findings:"),
        "long_summary section present (truncated, not dropped)"
    );
    let x_count = ctx.compact_context.matches('X').count();
    assert!(
        x_count > 0,
        "long_summary content present (truncated, not empty): {x_count} Xs"
    );
    assert!(
        x_count < 1000,
        "long_summary truncated (not full 1000): {x_count} Xs"
    );
    // Protected sections preserved.
    assert!(
        ctx.compact_context.contains("Current state:"),
        "hot_summary preserved (priority 6)"
    );
    assert!(
        ctx.compact_context.contains("do thing"),
        "prompt preserved (priority 7)"
    );
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
    assert_eq!(
        snap.hot_summary.as_deref(),
        Some("Token refresh logic is in src/auth/token.ts.")
    );
    assert!(snap.long_summary.is_some());
    assert_eq!(snap.key_files.len(), 1, "structured KeyFile retained");
    assert_eq!(snap.key_files[0].path, "src/auth/token.ts");
    assert_eq!(snap.key_files[0].score, 3, "score retained in snapshot");
    assert_eq!(snap.open_questions.len(), 1);
    assert_eq!(
        snap.open_questions[0].question,
        "Concurrent refresh handled?"
    );
}

#[test]
fn respects_profile_budget_override() {
    let builder = ContextBuilder::new(cfg());
    let profile_budget = 5000u32;
    let (ctx, _snap) = builder.build(&subagent(), &mem_row(), &[], "do thing", profile_budget);
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
    assert_eq!(
        ctx.budget_tokens, default,
        "zero budget falls back to default"
    );
}
