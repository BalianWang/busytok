#![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
//! Coverage gap tests for `busytok-tailer`.
//!
//! These tests target previously uncovered code paths in:
//! - `tailer.rs`: the `From<&notify::EventKind>` impl, the watcher
//!   callback closure, `watch_path`, `unwatch_path`, `poll_events`, and
//!   `watched_paths`.
//! - `scanner.rs`: `JsonlLineBuffer::has_pending` and the invalid-UTF-8
//!   warning branch in `read_file_once`.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use busytok_tailer::{
    read_file_once, FileChangeEvent, FileChangeKind, FileWatchService, JsonlLineBuffer,
    ScanFileRequest,
};

// ===========================================================================
// FileChangeKind::From<&notify::EventKind> — tailer.rs lines 37-44
// ===========================================================================

#[test]
fn file_change_kind_from_create_event() {
    let kind = FileChangeKind::from(&notify::EventKind::Create(notify::event::CreateKind::File));
    assert_eq!(kind, FileChangeKind::Created);
}

#[test]
fn file_change_kind_from_modify_event() {
    let kind = FileChangeKind::from(&notify::EventKind::Modify(notify::event::ModifyKind::Any));
    assert_eq!(kind, FileChangeKind::Modified);
}

#[test]
fn file_change_kind_from_remove_event() {
    let kind = FileChangeKind::from(&notify::EventKind::Remove(notify::event::RemoveKind::File));
    assert_eq!(kind, FileChangeKind::Removed);
}

#[test]
fn file_change_kind_from_access_event_maps_to_other() {
    // Access events don't match Create/Modify/Remove, so they fall to Other.
    let kind = FileChangeKind::from(&notify::EventKind::Access(notify::event::AccessKind::Read));
    assert_eq!(kind, FileChangeKind::Other);
}

#[test]
fn file_change_kind_from_any_variant_maps_to_other() {
    let kind = FileChangeKind::from(&notify::EventKind::Any);
    assert_eq!(kind, FileChangeKind::Other);
}

#[test]
fn file_change_kind_from_other_variant_maps_to_other() {
    let kind = FileChangeKind::from(&notify::EventKind::Other);
    assert_eq!(kind, FileChangeKind::Other);
}

// ===========================================================================
// FileWatchService — tailer.rs lines 56-129
// ===========================================================================

#[test]
fn watch_path_succeeds_and_records_path() {
    let temp = tempfile::tempdir().unwrap();
    let mut service = FileWatchService::new().unwrap();
    assert!(service.watch_path(temp.path()).is_ok());
    assert_eq!(service.watched_paths(), [temp.path()]);
}

#[test]
fn watch_path_fails_for_nonexistent_path() {
    let mut service = FileWatchService::new().unwrap();
    let bogus = PathBuf::from("/this/path/does/not/exist/anywhere");
    let result = service.watch_path(&bogus);
    assert!(result.is_err(), "watching a nonexistent path should error");
}

#[test]
fn unwatch_path_removes_from_watched_paths() {
    let temp = tempfile::tempdir().unwrap();
    let mut service = FileWatchService::new().unwrap();
    service.watch_path(temp.path()).unwrap();
    assert_eq!(service.watched_paths().len(), 1);
    assert!(service.unwatch_path(temp.path()).is_ok());
    assert!(
        service.watched_paths().is_empty(),
        "watched_paths should be empty after unwatch"
    );
}

#[test]
#[cfg(unix)]
fn unwatch_unwatched_path_returns_error() {
    let mut service = FileWatchService::new().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let result = service.unwatch_path(temp.path());
    // The notify crate's unwatch behavior is platform-dependent:
    //   - Unix (inotify): returns Err for an unwatched path (no watch descriptor)
    //   - Windows (ReadDirectoryChangesW): returns Ok as a no-op
    assert!(
        result.is_err(),
        "unwatching a path that was never watched should error on Unix (inotify)"
    );
}

#[test]
fn poll_events_returns_empty_on_timeout_when_idle() {
    let mut service = FileWatchService::new().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let watch_dir = temp.path().canonicalize().unwrap();
    service.watch_path(&watch_dir).unwrap();
    // Give the watcher a moment to settle — on macOS, FSEvents may deliver
    // an initial Created event for the watched directory itself when the
    // watch is first established. Draining these before the idle poll
    // ensures the timeout branch is actually exercised.
    std::thread::sleep(Duration::from_millis(200));
    let _ = service.poll_events(Duration::from_millis(50));
    // No file activity; should return empty within the timeout. This
    // exercises the `RecvTimeoutError::Timeout` arm of `poll_events`.
    let events = service.poll_events(Duration::from_millis(150));
    assert!(
        events.is_empty(),
        "expected no events when no file activity, got {:?}",
        events
    );
}

#[test]
fn poll_events_receives_events_for_file_creation() {
    let temp = tempfile::tempdir().unwrap();
    let watch_dir = temp.path().canonicalize().unwrap();
    let mut service = FileWatchService::new().unwrap();
    service.watch_path(&watch_dir).unwrap();
    // Give the watcher a moment to register the watch.
    std::thread::sleep(Duration::from_millis(150));

    let file_path = watch_dir.join("created.jsonl");
    std::fs::write(&file_path, b"hello\n").unwrap();

    let events = poll_for_events(&service, Duration::from_secs(3));
    assert!(
        !events.is_empty(),
        "expected at least one event after creating a file"
    );
    assert!(
        events.iter().any(|e| e.path == file_path),
        "expected an event for {:?}, got {:?}",
        file_path,
        events
    );
}

#[test]
fn poll_events_receives_modify_event_for_append() {
    let temp = tempfile::tempdir().unwrap();
    let watch_dir = temp.path().canonicalize().unwrap();
    let file_path = watch_dir.join("modified.jsonl");
    std::fs::write(&file_path, b"initial\n").unwrap();

    let mut service = FileWatchService::new().unwrap();
    service.watch_path(&watch_dir).unwrap();
    std::thread::sleep(Duration::from_millis(150));

    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&file_path)
            .unwrap();
        writeln!(f, "appended").unwrap();
    }

    let events = poll_for_events(&service, Duration::from_secs(3));
    assert!(
        events
            .iter()
            .any(|e| e.path == file_path && e.kind == FileChangeKind::Modified),
        "expected a Modified event for {:?}, got {:?}",
        file_path,
        events
    );
}

#[test]
fn poll_events_receives_remove_event_for_deleted_file() {
    let temp = tempfile::tempdir().unwrap();
    let watch_dir = temp.path().canonicalize().unwrap();
    let file_path = watch_dir.join("deleted.jsonl");
    std::fs::write(&file_path, b"temp\n").unwrap();

    let mut service = FileWatchService::new().unwrap();
    service.watch_path(&watch_dir).unwrap();
    std::thread::sleep(Duration::from_millis(150));

    std::fs::remove_file(&file_path).unwrap();

    let events = poll_for_events(&service, Duration::from_secs(3));
    assert!(
        events
            .iter()
            .any(|e| e.path == file_path && e.kind == FileChangeKind::Removed),
        "expected a Removed event for {:?}, got {:?}",
        file_path,
        events
    );
}

#[test]
fn poll_events_drains_multiple_pending_events() {
    let temp = tempfile::tempdir().unwrap();
    let watch_dir = temp.path().canonicalize().unwrap();
    let mut service = FileWatchService::new().unwrap();
    service.watch_path(&watch_dir).unwrap();
    std::thread::sleep(Duration::from_millis(150));

    // Create several files rapidly so multiple events land in the channel.
    for i in 0..5 {
        std::fs::write(watch_dir.join(format!("multi-{i}.jsonl")), b"x\n").unwrap();
    }
    // Allow the watcher to deliver all events to the channel.
    std::thread::sleep(Duration::from_millis(800));

    // Accumulate across polls. Creating 5 files produces at least 5 events
    // (Create + possibly Modify per file). This also exercises the
    // `while let Ok(event) = self.rx.try_recv()` drain loop when 2+
    // events are pending in a single `poll_events` call.
    let mut total = 0;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let batch = service.poll_events(Duration::from_millis(200));
        if batch.is_empty() && total > 0 {
            break;
        }
        total += batch.len();
    }
    assert!(
        total >= 2,
        "expected at least 2 events from creating 5 files, got {}",
        total
    );
}

/// Poll the watcher repeatedly until `timeout` elapses, collecting all
/// successful events. Used to absorb platform-specific event delivery
/// latency (notably macOS FSEvents coalescing/delay).
fn poll_for_events(service: &FileWatchService, timeout: Duration) -> Vec<FileChangeEvent> {
    let deadline = Instant::now() + timeout;
    let mut all = Vec::new();
    while Instant::now() < deadline {
        all.extend(
            service
                .poll_events(Duration::from_millis(200))
                .into_iter()
                .flatten(),
        );
        if !all.is_empty() {
            // Once we have events, do one more short poll to catch any
            // coalesced events that arrive slightly later.
            all.extend(
                service
                    .poll_events(Duration::from_millis(100))
                    .into_iter()
                    .flatten(),
            );
            return all;
        }
    }
    all
}

// ===========================================================================
// JsonlLineBuffer::has_pending — scanner.rs lines 67-69
// ===========================================================================

#[test]
fn has_pending_returns_false_for_empty_buffer() {
    let buf = JsonlLineBuffer::new();
    assert!(!buf.has_pending());
}

#[test]
fn has_pending_returns_true_after_partial_line() {
    let mut buf = JsonlLineBuffer::new();
    assert!(buf.push_bytes(b"partial line without newline").is_empty());
    assert!(buf.has_pending());
}

#[test]
fn has_pending_returns_false_after_complete_line() {
    let mut buf = JsonlLineBuffer::new();
    buf.push_bytes(b"complete line\n");
    assert!(!buf.has_pending());
}

#[test]
fn has_pending_returns_false_after_take_pending() {
    let mut buf = JsonlLineBuffer::new();
    buf.push_bytes(b"partial");
    assert!(buf.has_pending());
    let _ = buf.take_pending();
    assert!(!buf.has_pending());
}

// ===========================================================================
// read_file_once invalid UTF-8 warning path — scanner.rs lines 235-241
// ===========================================================================

#[test]
fn read_file_once_handles_invalid_utf8_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("invalid-utf8.jsonl");
    // Write content with an invalid UTF-8 sequence (0xFF, 0xFE are not
    // valid UTF-8 start bytes). Each line is terminated by \n so the
    // line buffer produces completed (lossy-converted) lines.
    let content: &[u8] = b"valid\n\xff\xfe\n";
    std::fs::write(&path, content).unwrap();

    let request = ScanFileRequest::for_test("src", "file-1", &path);
    let batch = read_file_once(request).unwrap();

    // Both lines should be present (lossy-converted). The invalid UTF-8
    // check in `read_file_once` triggers the `tracing::warn!` branch.
    assert_eq!(
        batch.lines.len(),
        2,
        "both completed lines should be present"
    );
    assert_eq!(batch.lines[0].text, "valid");
    // Invalid bytes are replaced with U+FFFD by from_utf8_lossy.
    assert!(
        batch.lines[1].text.contains('\u{fffd}'),
        "expected replacement character in lossy-converted line, got {:?}",
        batch.lines[1].text
    );
    assert_eq!(
        batch.checkpoint_offset,
        content.len() as u64,
        "checkpoint should cover all bytes including invalid ones"
    );
}
