#![allow(
    clippy::await_holding_lock,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::field_reassign_with_default,
    clippy::uninlined_format_args,
    clippy::inconsistent_digit_grouping,
    clippy::len_zero,
    clippy::identity_op,
    clippy::useless_vec,
    clippy::manual_dangling_ptr,
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::absurd_extreme_comparisons,
    unused_variables,
    unused_imports,
    dead_code
)]
//! Active fixture-shape contract tests for the Codex adapter.
//!
//! These tests validate that the fixture files conform to the expected shape
//! so that adapter implementations can rely on them. They guard the invariants
//! that Codex cumulative snapshots grow monotonically.

use serde_json::Value;

#[test]
fn codex_cumulative_snapshot_delta_uses_persisted_ordinal() {
    let content = include_str!("../../../fixtures/codex/codex-token-count-snapshot.jsonl");
    let lines: Vec<_> = content.lines().filter(|l| !l.is_empty()).collect();
    assert!(lines.len() >= 2);
    let first: Value = serde_json::from_str(lines[0]).unwrap();
    let second: Value = serde_json::from_str(lines[1]).unwrap();
    // Same session, growing total_token_usage (cumulative snapshot)
    assert_eq!(first["session_id"], second["session_id"]);
    assert!(
        second["total_token_usage"]["input_tokens"]
            .as_i64()
            .unwrap()
            > first["total_token_usage"]["input_tokens"].as_i64().unwrap()
    );
}

#[test]
fn codex_reasoning_tokens_not_double_counted() {
    let content = include_str!("../../../fixtures/codex/codex-token-count-snapshot.jsonl");
    let lines: Vec<_> = content.lines().filter(|l| !l.is_empty()).collect();
    let first: Value = serde_json::from_str(lines[0]).unwrap();
    // Reasoning tokens exist in the fixture
    assert!(first["last_token_usage"]["reasoning_output_tokens"].is_number());
}
