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
//! Multi-agent scan integration tests.
//!
//! Verifies that the supervisor registers all adapters (Claude Code,
//! Codex) and that an empty scan returns zero stats.

use busytok_config::BusytokPaths;
use busytok_runtime::BusytokSupervisor;
use busytok_store::Database;

#[test]
fn supervisor_registers_claude_and_codex_adapters() {
    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::new();
    let supervisor = BusytokSupervisor::new(db, paths);
    let agents = supervisor.debug_registered_agents();
    assert!(agents.iter().any(|a| a == "claude_code"));
    assert!(agents.iter().any(|a| a == "codex"));
}

#[test]
fn empty_scan_with_no_sources_returns_zero_stats() {
    let db = Database::open_in_memory().unwrap();
    let paths = BusytokPaths::new();
    let supervisor = BusytokSupervisor::new(db, paths);
    let stats = supervisor.run_scan_with_sources(vec![]).unwrap();
    assert_eq!(stats.events_found, 0);
    assert_eq!(stats.files_scanned, 0);
}
