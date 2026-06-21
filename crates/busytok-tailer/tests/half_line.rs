use busytok_tailer::JsonlLineBuffer;

#[test]
fn buffers_incomplete_jsonl_line_until_newline() {
    let mut buffer = JsonlLineBuffer::default();
    assert!(buffer.push_bytes(br#"{"a":"#).is_empty());
    let lines = buffer.push_bytes(br#""b"}"#);
    assert!(lines.is_empty());
    let lines = buffer.push_bytes(b"\n");
    assert_eq!(lines, vec![r#"{"a":"b"}"#.to_string()]);
}

#[test]
fn incomplete_final_line_is_not_checkpointed_as_consumed() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("session.jsonl");
    std::fs::write(&log, br#"{"partial":true"#).unwrap();
    let request = busytok_tailer::ScanFileRequest::for_test("source-1", "source-file-1", &log);
    let batch = busytok_tailer::read_file_once(request).unwrap();
    assert!(batch.lines.is_empty());
    assert_eq!(batch.checkpoint_offset, 0);
    assert!(!batch.pending_bytes.is_empty());
}
