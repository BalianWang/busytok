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
//! Integration tests for queue types: ScanStats, FileScanResult, TailWorkItem.
//!
//! Covers construction, default values, partition logic, and debug formatting.

use std::path::PathBuf;

use busytok_domain::{
    AgentKind, NormalizedEvent, NormalizedUsageEvent, OperationalDiagnosticEvent,
};
use busytok_runtime::{FileScanResult, ScanStats, TailWorkItem};

// ── ScanStats ─────────────────────────────────────────────────────────

#[test]
fn scan_stats_default() {
    let stats = ScanStats::default();
    assert_eq!(stats.sources, 0);
    assert_eq!(stats.files_scanned, 0);
    assert_eq!(stats.events_found, 0);
    assert_eq!(stats.diagnostics_found, 0);
}

#[test]
fn scan_stats_construction() {
    let stats = ScanStats {
        sources: 3,
        files_scanned: 10,
        events_found: 42,
        diagnostics_found: 2,
    };
    assert_eq!(stats.sources, 3);
    assert_eq!(stats.files_scanned, 10);
    assert_eq!(stats.events_found, 42);
    assert_eq!(stats.diagnostics_found, 2);
}

#[test]
fn scan_stats_clone() {
    let stats = ScanStats {
        sources: 1,
        files_scanned: 5,
        events_found: 20,
        diagnostics_found: 1,
    };
    let cloned = stats.clone();
    assert_eq!(cloned.sources, stats.sources);
    assert_eq!(cloned.files_scanned, stats.files_scanned);
    assert_eq!(cloned.events_found, stats.events_found);
    assert_eq!(cloned.diagnostics_found, stats.diagnostics_found);
}

#[test]
fn scan_stats_debug_format() {
    let stats = ScanStats {
        sources: 2,
        files_scanned: 7,
        events_found: 15,
        diagnostics_found: 3,
    };
    let debug_str = format!("{stats:?}");
    assert!(debug_str.contains("sources: 2"));
    assert!(debug_str.contains("files_scanned: 7"));
    assert!(debug_str.contains("events_found: 15"));
    assert!(debug_str.contains("diagnostics_found: 3"));
}

// ── FileScanResult ────────────────────────────────────────────────────

#[test]
fn file_scan_result_construction() {
    let result = FileScanResult {
        path: PathBuf::from("/tmp/test.jsonl"),
        source_id: "src-1".to_string(),
        events: vec![],
        new_offset: 1024,
        bytes_read: 512,
        reached_eof: true,
    };
    assert_eq!(result.path, PathBuf::from("/tmp/test.jsonl"));
    assert_eq!(result.source_id, "src-1");
    assert!(result.events.is_empty());
    assert_eq!(result.new_offset, 1024);
    assert_eq!(result.bytes_read, 512);
    assert!(result.reached_eof);
}

#[test]
fn file_scan_result_partition_empty() {
    let result = FileScanResult {
        path: PathBuf::from("/tmp/test.jsonl"),
        source_id: "src-1".to_string(),
        events: vec![],
        new_offset: 0,
        bytes_read: 0,
        reached_eof: true,
    };
    let (usage, diagnostics) = result.partition();
    assert!(usage.is_empty());
    assert!(diagnostics.is_empty());
}

#[test]
fn file_scan_result_partition_usage_only() {
    let event = NormalizedEvent::Usage(Box::new(NormalizedUsageEvent::minimal_for_test(
        "evt-1",
        AgentKind::ClaudeCode,
    )));
    let result = FileScanResult {
        path: PathBuf::from("/tmp/test.jsonl"),
        source_id: "src-1".to_string(),
        events: vec![event],
        new_offset: 0,
        bytes_read: 0,
        reached_eof: true,
    };
    let (usage, diagnostics) = result.partition();
    assert_eq!(usage.len(), 1);
    assert!(diagnostics.is_empty());
    assert_eq!(usage[0].id, "evt-1");
}

#[test]
fn file_scan_result_partition_diagnostic_only() {
    let event =
        NormalizedEvent::OperationalDiagnostic(OperationalDiagnosticEvent::for_test("diag-1"));
    let result = FileScanResult {
        path: PathBuf::from("/tmp/test.jsonl"),
        source_id: "src-1".to_string(),
        events: vec![event],
        new_offset: 0,
        bytes_read: 0,
        reached_eof: true,
    };
    let (usage, diagnostics) = result.partition();
    assert!(usage.is_empty());
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].id, "diag-1");
}

#[test]
fn file_scan_result_partition_tool_event_filtered() {
    let event = NormalizedEvent::tool_for_test("tool-1");
    let result = FileScanResult {
        path: PathBuf::from("/tmp/test.jsonl"),
        source_id: "src-1".to_string(),
        events: vec![event],
        new_offset: 0,
        bytes_read: 0,
        reached_eof: true,
    };
    let (usage, diagnostics) = result.partition();
    // Tool events are filtered out by partition.
    assert!(usage.is_empty());
    assert!(diagnostics.is_empty());
}

#[test]
fn file_scan_result_partition_mixed_events() {
    let usage_event = NormalizedEvent::Usage(Box::new(NormalizedUsageEvent::minimal_for_test(
        "evt-1",
        AgentKind::ClaudeCode,
    )));
    let diag_event =
        NormalizedEvent::OperationalDiagnostic(OperationalDiagnosticEvent::for_test("diag-1"));
    let tool_event = NormalizedEvent::tool_for_test("tool-1");
    let result = FileScanResult {
        path: PathBuf::from("/tmp/test.jsonl"),
        source_id: "src-1".to_string(),
        events: vec![usage_event, diag_event, tool_event],
        new_offset: 0,
        bytes_read: 0,
        reached_eof: true,
    };
    let (usage, diagnostics) = result.partition();
    assert_eq!(usage.len(), 1);
    assert_eq!(diagnostics.len(), 1);
    // Tool event is filtered out.
}

// ── TailWorkItem ──────────────────────────────────────────────────────

#[test]
fn tail_work_item_file_changed() {
    let item = TailWorkItem::FileChanged {
        path: PathBuf::from("/tmp/test.jsonl"),
        source_id: "src-1".to_string(),
    };
    let debug_str = format!("{item:?}");
    assert!(debug_str.contains("FileChanged"));
    assert!(debug_str.contains("/tmp/test.jsonl"));
    assert!(debug_str.contains("src-1"));
}

#[test]
fn tail_work_item_file_created() {
    let item = TailWorkItem::FileCreated {
        path: PathBuf::from("/tmp/new.jsonl"),
        source_id: "src-2".to_string(),
    };
    let debug_str = format!("{item:?}");
    assert!(debug_str.contains("FileCreated"));
    assert!(debug_str.contains("/tmp/new.jsonl"));
    assert!(debug_str.contains("src-2"));
}

#[test]
fn tail_work_item_debug_format() {
    let items = vec![
        TailWorkItem::FileChanged {
            path: PathBuf::from("/a.jsonl"),
            source_id: "s1".to_string(),
        },
        TailWorkItem::FileCreated {
            path: PathBuf::from("/b.jsonl"),
            source_id: "s2".to_string(),
        },
    ];
    let debug_str = format!("{items:?}");
    assert!(debug_str.contains("FileChanged"));
    assert!(debug_str.contains("FileCreated"));
}
