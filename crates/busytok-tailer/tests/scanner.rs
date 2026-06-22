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
use busytok_tailer::{read_file_once, read_inode, ScanFileRequest};

#[test]
fn read_file_once_returns_completed_lines_and_checkpoint_candidate() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("session.jsonl");
    std::fs::write(
        &log,
        include_str!("../../../fixtures/claude-code/basic.jsonl"),
    )
    .unwrap();
    let request = ScanFileRequest::for_test("source-1", "source-file-1", &log);
    let batch = read_file_once(request).unwrap();
    assert_eq!(batch.lines.len(), 1);
    assert!(batch.checkpoint_offset > 0);
    assert_eq!(batch.lines[0].context.source_file_id, "source-file-1");
    assert_eq!(batch.lines[0].context.source_offset_start, 0);
    assert_eq!(
        batch.lines[0].context.source_offset_end,
        batch.checkpoint_offset
    );
}

#[test]
fn resume_offset_skips_consumed_lines_without_business_parsing() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("session.jsonl");
    std::fs::write(&log, "{}\n{\"next\":true}\n").unwrap();
    let request = ScanFileRequest::for_test("source-1", "source-file-1", &log);
    let first = read_file_once(request).unwrap();
    let request = ScanFileRequest::for_test("source-1", "source-file-1", &log)
        .with_resume_offset(first.lines[0].context.source_offset_end);
    let second = read_file_once(request).unwrap();
    assert_eq!(second.lines.len(), 1);
    assert_eq!(second.lines[0].text, r#"{"next":true}"#);
}

#[test]
fn truncated_file_resets_offset_to_zero() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("session.jsonl");

    // Write initial content.
    std::fs::write(&log, "{\"first\":true}\n{\"second\":true}\n").unwrap();
    let request = ScanFileRequest::for_test("source-1", "source-file-1", &log);
    let first = read_file_once(request).unwrap();
    assert_eq!(first.lines.len(), 2);
    let stale_offset = first.checkpoint_offset;

    // Truncate the file (shrink it).
    std::fs::write(&log, "{\"new\":true}\n").unwrap();

    // Resume with a stale offset that exceeds the new file size.
    let request = ScanFileRequest::for_test("source-1", "source-file-1", &log)
        .with_resume_offset(stale_offset);
    let second = read_file_once(request).unwrap();

    // The file was truncated, so offset should have been reset to 0
    // and we should read the new content from the start.
    assert_eq!(second.lines.len(), 1);
    assert_eq!(second.lines[0].text, r#"{"new":true}"#);
}

#[test]
fn inode_change_resets_offset_to_zero() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("session.jsonl");

    // Write initial content and record the inode.
    std::fs::write(&log, "{\"old\":true}\n").unwrap();
    let old_inode = read_inode(&log);

    // Read the file once to consume it.
    let request = ScanFileRequest::for_test("source-1", "source-file-1", &log);
    let first = read_file_once(request).unwrap();
    assert_eq!(first.lines.len(), 1);
    let stale_offset = first.checkpoint_offset;

    // Delete and recreate the file (new inode).
    std::fs::remove_file(&log).unwrap();
    std::fs::write(&log, "{\"recreated\":true}\n").unwrap();
    let new_inode = read_inode(&log);

    // On macOS, inodes can be reused quickly, so this test only validates
    // the logic when inode actually changed.
    if old_inode != new_inode {
        let request = ScanFileRequest::for_test("source-1", "source-file-1", &log)
            .with_resume_offset(stale_offset)
            .with_previous_inode(old_inode);
        let second = read_file_once(request).unwrap();
        // Inode changed, offset should have been reset to 0.
        assert_eq!(second.lines.len(), 1);
        assert_eq!(second.lines[0].text, r#"{"recreated":true}"#);
    }

    // Also test that providing the *current* inode does NOT reset offset.
    // (Simulate: same inode, stale offset but file still at that size.)
    // This is harder to test without mocking, but we can test that when
    // previous_inode matches current, the offset is NOT reset.
    std::fs::write(&log, "{\"a\":true}\n{\"b\":true}\n").unwrap();
    let curr_inode = read_inode(&log);
    let request = ScanFileRequest::for_test("source-1", "source-file-1", &log);
    let first_read = read_file_once(request).unwrap();
    let first_offset = first_read.checkpoint_offset;

    // Read again with previous_inode = current and offset at the right spot.
    // Since offset == file_size, there should be nothing new.
    let request = ScanFileRequest::for_test("source-1", "source-file-1", &log)
        .with_resume_offset(first_offset)
        .with_previous_inode(curr_inode.clone());
    let second_read = read_file_once(request).unwrap();
    assert!(
        second_read.lines.is_empty(),
        "matching inode + offset at EOF should yield no new lines"
    );
}
