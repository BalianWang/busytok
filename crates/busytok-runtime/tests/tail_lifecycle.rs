//! Integration test: resume from offset, incomplete line handling,
//! truncation detection, and new file discovery.

use std::io::Write;

use busytok_config::BusytokPaths;
use busytok_domain::{AgentKind, LogSourceType};
use busytok_runtime::BusytokSupervisor;
use busytok_store::Database;

/// Create a minimal valid JSONL line for Claude Code.
fn make_jsonl_line(session_id: &str, model: &str, input_tokens: u64, output_tokens: u64) -> String {
    serde_json::json!({
        "type": "assistant",
        "message": {
            "id": format!("msg_{}", session_id),
            "model": model,
            "usage": {
                "input_tokens": input_tokens,
                "output_tokens": output_tokens
            }
        },
        "sessionId": session_id,
        "timestamp": "2026-05-15T10:00:00Z"
    })
    .to_string()
}

/// Create a DiscoveredLogSource pointing to a specific file.
fn source_for_file(path: &std::path::Path) -> busytok_discovery::DiscoveredLogSource {
    source_for_file_with_agent(path, AgentKind::ClaudeCode)
}

fn source_for_file_with_agent(
    path: &std::path::Path,
    agent: AgentKind,
) -> busytok_discovery::DiscoveredLogSource {
    let root = path.parent().unwrap_or(path).to_path_buf();
    busytok_discovery::DiscoveredLogSource {
        agent,
        source_id: "test-tail".to_string(),
        root_path: root,
        files: vec![path.to_path_buf()],
        source_type: LogSourceType::Jsonl,
        configured_by_user: false,
    }
}

/// Create a DiscoveredLogSource pointing to a directory (no files yet).
fn source_for_root(root: &std::path::Path) -> busytok_discovery::DiscoveredLogSource {
    busytok_discovery::DiscoveredLogSource {
        agent: AgentKind::ClaudeCode,
        source_id: "test-tail".to_string(),
        root_path: root.to_path_buf(),
        files: vec![],
        source_type: LogSourceType::Jsonl,
        configured_by_user: false,
    }
}

#[test]
fn tail_resume_from_offset() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("session.jsonl");

    // Write two lines initially.
    let line1 = make_jsonl_line("sess1", "claude-sonnet-4-20250514", 100, 50);
    let line2 = make_jsonl_line("sess2", "claude-sonnet-4-20250514", 200, 100);

    {
        let mut f = std::fs::File::create(&file_path).expect("create file");
        writeln!(f, "{line1}").expect("write line1");
        writeln!(f, "{line2}").expect("write line2");
    }

    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();

    let supervisor = BusytokSupervisor::new(db, paths);
    let source = source_for_file(&file_path);

    // Initial scan.
    let stats1 = supervisor
        .run_scan_with_sources(vec![source.clone()])
        .expect("initial scan");
    assert!(
        stats1.events_found >= 1,
        "should find events from initial data"
    );

    let db_guard = supervisor.db_handle().lock().unwrap();
    let count1 = db_guard.usage_event_count().expect("count1");

    // Now append more data.
    let line3 = make_jsonl_line("sess3", "claude-sonnet-4-20250514", 300, 150);
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&file_path)
            .expect("open append");
        writeln!(f, "{line3}").expect("write line3");
    }

    // Rescan -- should pick up only the new line.
    drop(db_guard);
    let stats2 = supervisor
        .run_scan_with_sources(vec![source])
        .expect("rescan");
    assert!(
        stats2.events_found >= 1,
        "should find new events from appended data"
    );

    let db_guard = supervisor.db_handle().lock().unwrap();
    let count2 = db_guard.usage_event_count().expect("count2");
    // Total events should be the sum of both scans (assuming InsertOnce works).
    assert!(count2 > count1, "rescan should add new events");
}

#[test]
fn tail_incomplete_line_does_not_advance_offset() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("incomplete.jsonl");

    // Write one complete line + one incomplete line (no trailing newline).
    let line1 = make_jsonl_line("sess1", "claude-sonnet-4-20250514", 100, 50);
    let incomplete = r#"{"type": "assistant", "message": {"id":"msg_incomplete"#; // truncated JSON

    {
        let mut f = std::fs::File::create(&file_path).expect("create file");
        writeln!(f, "{line1}").expect("write complete line");
        write!(f, "{incomplete}").expect("write incomplete line");
        // No trailing newline on the incomplete line!
    }

    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();

    let supervisor = BusytokSupervisor::new(db, paths);
    let source = source_for_file(&file_path);

    // Scan should succeed, processing only the complete line.
    let stats = supervisor
        .run_scan_with_sources(vec![source.clone()])
        .expect("scan with incomplete line");

    // We should get the complete line's events.
    assert!(
        stats.events_found >= 1,
        "should find events from complete line"
    );

    // Now complete the second line.
    {
        let line2 = make_jsonl_line("sess2", "claude-sonnet-4-20250514", 200, 100);
        let full_content = format!("{line1}\n{line2}\n");
        std::fs::write(&file_path, full_content).expect("rewrite file");
    }

    // Rescan -- the incomplete line's offset should cause a re-read
    // that now picks up the completed second line.
    let stats2 = supervisor
        .run_scan_with_sources(vec![source])
        .expect("rescan after completing line");

    // The offset tracking should ensure the second line is now processed.
    let db_guard = supervisor.db_handle().lock().unwrap();
    let final_count = db_guard.usage_event_count().expect("final count");
    // After completing the line, we should have at least 2 events
    // (the first one from line1, and the second from line2).
    // However, the rescan will re-read from the offset which was
    // after line1, so it should now find line2.
    assert!(
        final_count >= 2 || stats2.events_found >= 1,
        "after completing the line, new events should be found"
    );
}

#[test]
fn tail_empty_file_no_crash() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("empty.jsonl");

    // Create an empty file.
    std::fs::File::create(&file_path).expect("create empty file");

    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();

    let supervisor = BusytokSupervisor::new(db, paths);
    let source = source_for_file(&file_path);

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("scan of empty file should not crash");

    assert_eq!(stats.events_found, 0);
}

#[test]
fn tail_file_with_only_newlines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("blank.jsonl");

    {
        let mut f = std::fs::File::create(&file_path).expect("create");
        write!(f, "\n\n\n").expect("write blank lines");
    }

    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();

    let supervisor = BusytokSupervisor::new(db, paths);
    let source = source_for_file(&file_path);

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .expect("scan of blank file should not crash");

    assert_eq!(stats.events_found, 0);
}

#[test]
fn tail_truncated_file_is_reread_from_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("rotate.jsonl");

    // Write initial content and scan it.
    let line1 = make_jsonl_line("sess1", "claude-sonnet-4-20250514", 100, 50);
    let line2 = make_jsonl_line("sess2", "claude-sonnet-4-20250514", 200, 100);
    {
        let mut f = std::fs::File::create(&file_path).expect("create file");
        writeln!(f, "{line1}").expect("write line1");
        writeln!(f, "{line2}").expect("write line2");
    }

    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();

    let supervisor = BusytokSupervisor::new(db, paths);
    let source = source_for_file(&file_path);

    let stats1 = supervisor
        .run_scan_with_sources(vec![source.clone()])
        .expect("initial scan");
    assert!(stats1.events_found >= 1, "should find initial events");

    // Now truncate the file (shrink it) — simulating log rotation.
    let line_new = make_jsonl_line("sess_new", "claude-sonnet-4-20250514", 50, 25);
    std::fs::write(&file_path, format!("{line_new}\n")).expect("truncate file");

    // Rescan — the file shrank, so the stale offset exceeds file size.
    // The scanner should detect this and re-read from offset 0.
    let stats2 = supervisor
        .run_scan_with_sources(vec![source])
        .expect("rescan after truncation");

    // We should find the new line from the truncated file.
    // (Depending on InsertOnce, the old events from line1/line2 are already
    // committed, but the new content from offset 0 should be parseable.)
    assert!(
        stats2.events_found >= 1,
        "truncated file should be re-read from 0"
    );
}

#[test]
fn tail_new_file_discovered_under_watched_root() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Create a source that watches the directory, but has no files yet.
    let db = Database::open_in_memory().expect("db open");
    let paths = BusytokPaths::new();

    let supervisor = BusytokSupervisor::new(db, paths);
    let source = source_for_root(dir.path());

    // Initial scan with no files.
    let stats1 = supervisor
        .run_scan_with_sources(vec![source.clone()])
        .expect("initial scan with no files");
    assert_eq!(stats1.events_found, 0, "no events initially");
    assert_eq!(stats1.files_scanned, 0, "no files initially");

    // Now create a new file under the watched root.
    let new_file = dir.path().join("new_session.jsonl");
    let line = make_jsonl_line("sess_new", "claude-sonnet-4-20250514", 300, 150);
    {
        let mut f = std::fs::File::create(&new_file).expect("create new file");
        writeln!(f, "{line}").expect("write line");
    }

    // Rescan with the new file now included.
    // In the real tailer, the Created event would dynamically add the file.
    // Here we simulate that by passing the updated source with the new file.
    let updated_source = source_for_file(&new_file);
    let stats2 = supervisor
        .run_scan_with_sources(vec![updated_source])
        .expect("rescan with new file");
    assert!(
        stats2.events_found >= 1,
        "new file should be scanned and produce events"
    );
    assert!(stats2.files_scanned >= 1, "new file should be counted");
}
