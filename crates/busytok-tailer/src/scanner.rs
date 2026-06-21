//! JSONL line scanner: reads raw bytes from files, splits completed JSONL
//! lines, and tracks exact byte offsets for checkpointing.
//!
//! This module is storage-agnostic and business-agnostic. It does NOT depend
//! on `busytok-store`, `busytok-aggregator`, or `busytok-adapters`. It only
//! reads bytes, splits completed JSONL lines, tracks exact byte offsets,
//! carries `inode` into `ParseContext` when the platform exposes it, and
//! returns a safe checkpoint candidate that excludes incomplete final lines.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use busytok_domain::ParseContext;

/// Buffer that accumulates incomplete JSONL lines and returns completed lines
/// when a newline is encountered.
#[derive(Debug, Default)]
pub struct JsonlLineBuffer {
    /// Bytes accumulated for the current incomplete line.
    pending: Vec<u8>,
}

impl JsonlLineBuffer {
    /// Create a new empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push raw bytes into the buffer. Returns a list of completed lines
    /// (without trailing newlines). Partial lines remain buffered until a
    /// newline arrives.
    pub fn push_bytes(&mut self, data: &[u8]) -> Vec<String> {
        let mut completed = Vec::new();

        // Prepend any pending bytes and process the combined stream.
        let mut combined: Vec<u8> = std::mem::take(&mut self.pending);
        combined.extend_from_slice(data);

        // Normalize \r\n → \n for cross-platform compatibility.
        // Claude Code on Windows writes JSONL files with \r\n line endings,
        // which would leave trailing \r on each line and cause JSON parse
        // failures. Stripping \r before \n avoids that.
        let combined = normalize_crlf(combined);

        let mut start = 0;
        for (i, &byte) in combined.iter().enumerate() {
            if byte == b'\n' {
                let line_bytes = &combined[start..i];
                // Skip empty lines (consecutive newlines)
                if !line_bytes.is_empty() {
                    let line = String::from_utf8_lossy(line_bytes).into_owned();
                    completed.push(line);
                }
                start = i + 1;
            }
        }

        // Anything after the last newline stays pending.
        self.pending = combined[start..].to_vec();

        completed
    }

    /// Returns true if there are pending bytes that haven't been terminated
    /// by a newline.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Take the pending bytes out, clearing the buffer.
    pub fn take_pending(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }
}

/// A request to scan a file and extract completed JSONL lines.
#[derive(Debug, Clone)]
pub struct ScanFileRequest {
    /// Logical source identifier (e.g. "claude-code").
    pub source_id: String,
    /// Unique identifier for this specific file within the source.
    pub source_file_id: String,
    /// Path to the file to scan.
    pub path: PathBuf,
    /// Byte offset to start reading from. Used for resuming after a
    /// checkpoint.
    pub resume_offset: u64,
    /// The inode from the previous scan, if known. Used to detect file
    /// rotation (file deleted and recreated with a different inode).
    /// If the current inode differs, the offset is reset to 0.
    pub previous_inode: Option<String>,
}

impl ScanFileRequest {
    /// Create a request for test use with default (zero) resume offset.
    pub fn for_test(source_id: &str, source_file_id: &str, path: &Path) -> Self {
        Self {
            source_id: source_id.to_string(),
            source_file_id: source_file_id.to_string(),
            path: path.to_path_buf(),
            resume_offset: 0,
            previous_inode: None,
        }
    }

    /// Set the resume offset for incremental reads.
    pub fn with_resume_offset(mut self, offset: u64) -> Self {
        self.resume_offset = offset;
        self
    }

    /// Set the previous inode for rotation detection.
    pub fn with_previous_inode(mut self, inode: Option<String>) -> Self {
        self.previous_inode = inode;
        self
    }
}

/// A single completed line read from a tailed file, along with its
/// parse context.
#[derive(Debug, Clone)]
pub struct TailedLine {
    /// The raw text of the completed line (no trailing newline).
    pub text: String,
    /// Context supplied to adapters for parsing.
    pub context: ParseContext,
}

/// Result of a single scan read operation.
#[derive(Debug, Clone)]
pub struct ScanReadBatch {
    /// The source ID from the request.
    pub source_id: String,
    /// The source file ID from the request.
    pub source_file_id: String,
    /// Completed lines extracted in this read.
    pub lines: Vec<TailedLine>,
    /// Bytes that are pending because the final line has no trailing newline.
    /// These should be preserved for the next read cycle.
    pub pending_bytes: Vec<u8>,
    /// A safe checkpoint offset: the byte position just past the last
    /// completed line's newline. Does NOT include incomplete final lines.
    pub checkpoint_offset: u64,
    /// Total file size in bytes at the time of reading.
    pub file_size_bytes: u64,
    /// Last modification time of the file in milliseconds since epoch, if
    /// available.
    pub last_mtime_ms: Option<i64>,
    /// Whether the file was restarted (inode changed or file truncated).
    /// When true, cumulative snapshots should not use persisted DB baselines.
    pub was_reset: bool,
}

/// Read a file once, extracting completed JSONL lines starting from the
/// resume offset. Incomplete final lines are returned as pending bytes but
/// are NOT included in the checkpoint offset.
pub fn read_file_once(request: ScanFileRequest) -> Result<ScanReadBatch> {
    let path = &request.path;

    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read file metadata for {}", path.display()))?;

    let file_size = metadata.len();
    let last_mtime_ms = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64);

    // Read the inode if available (Unix only).
    let inode = read_inode(path);

    // Detect file truncation or rotation.
    // If the file shrank (resume_offset > file_size) or the inode changed
    // (file was deleted and recreated), reset the offset to 0 so we
    // re-read from the beginning.
    let mut effective_offset = request.resume_offset;
    let inode_changed = request.previous_inode.is_some()
        && inode.is_some()
        && request.previous_inode.as_ref() != inode.as_ref();
    let file_shrank = request.resume_offset > file_size;
    let was_reset = inode_changed || file_shrank;

    if was_reset {
        tracing::info!(
            path = %path.display(),
            old_offset = request.resume_offset,
            file_size,
            inode_changed,
            file_shrank,
            "resetting offset to 0 due to file truncation or rotation"
        );
        effective_offset = 0;
    }

    // If effective offset is at or past file size, there's nothing new.
    if effective_offset >= file_size {
        return Ok(ScanReadBatch {
            source_id: request.source_id,
            source_file_id: request.source_file_id,
            lines: Vec::new(),
            pending_bytes: Vec::new(),
            checkpoint_offset: effective_offset,
            file_size_bytes: file_size,
            last_mtime_ms,
            was_reset,
        });
    }

    // Read the file from the effective offset.
    let mut file =
        fs::File::open(path).with_context(|| format!("failed to open file {}", path.display()))?;
    std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(effective_offset)).with_context(
        || {
            format!(
                "failed to seek to offset {} in {}",
                effective_offset,
                path.display()
            )
        },
    )?;

    let bytes_to_read = (file_size - effective_offset) as usize;
    let mut buf = vec![0u8; bytes_to_read];
    std::io::Read::read_exact(&mut file, &mut buf).with_context(|| {
        format!(
            "failed to read {} bytes from {}",
            bytes_to_read,
            path.display()
        )
    })?;

    // Warn if the raw bytes contain invalid UTF-8 before lossy conversion.
    if std::str::from_utf8(&buf).is_err() {
        tracing::warn!(
            path = %path.display(),
            offset = effective_offset,
            "invalid UTF-8 bytes encountered in log file; JSON parsing may be corrupted"
        );
    }

    // Split into completed lines using the line buffer.
    let mut line_buffer = JsonlLineBuffer::new();
    let completed = line_buffer.push_bytes(&buf);
    let pending_bytes = line_buffer.take_pending();

    // Build TailedLine entries with precise byte offsets.
    // We scan the RAW buffer (buf) for \n positions rather than using
    // line_text.len(), because push_bytes normalizes \r\n -> \n and
    // the normalized length would give wrong byte offsets for files
    // with CRLF line endings.
    let mut lines = Vec::with_capacity(completed.len());
    let mut raw_pos = 0usize;

    for (line_number, line_text) in completed.iter().enumerate() {
        // Find the next \n in the raw buffer starting from raw_pos.
        let newline_pos = match buf[raw_pos..].iter().position(|&b| b == b'\n') {
            Some(p) => raw_pos + p,
            None => {
                // No newline found in raw buffer for this completed line.
                // This should not happen since completed lines are produced
                // from \n positions, but if it does, skip the line.
                break;
            }
        };
        let line_end = newline_pos + 1; // inclusive of newline

        let source_offset_start = effective_offset + raw_pos as u64;
        let source_offset_end = effective_offset + line_end as u64;

        let line_number_u64 = line_number as u64 + 1;

        lines.push(TailedLine {
            text: line_text.clone(),
            context: ParseContext {
                source_file_id: request.source_file_id.clone(),
                source_path: path.to_string_lossy().into_owned(),
                inode: inode.clone(),
                source_line: line_number_u64,
                source_offset_start,
                source_offset_end,
                replay_sequence: line_number_u64,
            },
        });

        raw_pos = line_end;
    }

    // The checkpoint offset is just past the last completed line's newline.
    // It does NOT include incomplete final lines.
    let checkpoint_offset = if lines.is_empty() {
        effective_offset
    } else {
        lines.last().unwrap().context.source_offset_end
    };

    Ok(ScanReadBatch {
        source_id: request.source_id,
        source_file_id: request.source_file_id,
        lines,
        pending_bytes,
        checkpoint_offset,
        file_size_bytes: file_size,
        last_mtime_ms,
        was_reset,
    })
}

/// Try to read the inode of a file. Returns None on non-Unix platforms or
/// if the stat call fails.
#[cfg(unix)]
pub fn read_inode(path: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).ok().map(|m| m.ino().to_string())
}

#[cfg(not(unix))]
pub fn read_inode(_path: &Path) -> Option<String> {
    None
}

/// Normalize CRLF (`\r\n`) line endings to LF (`\n`).
///
/// Iterates through the byte vector once, stripping `\r` when it appears
/// immediately before `\n`. All other bytes pass through unchanged.
fn normalize_crlf(data: Vec<u8>) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\r' && i + 1 < data.len() && data[i + 1] == b'\n' {
            // Skip the \r, keep the \n
            result.push(b'\n');
            i += 2;
        } else {
            result.push(data[i]);
            i += 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_crlf_noop_on_unix_lines() {
        let input: Vec<u8> = b"hello\nworld\n".to_vec();
        let output = normalize_crlf(input.clone());
        assert_eq!(output, input);
    }

    #[test]
    fn normalize_crlf_removes_carriage_returns() {
        let input: Vec<u8> = b"hello\r\nworld\r\n".to_vec();
        let output = normalize_crlf(input);
        assert_eq!(output, b"hello\nworld\n");
    }

    #[test]
    fn normalize_crlf_mixed_endings() {
        let input: Vec<u8> = b"line1\nline2\r\nline3\nline4\r\n".to_vec();
        let output = normalize_crlf(input);
        assert_eq!(output, b"line1\nline2\nline3\nline4\n");
    }

    #[test]
    fn normalize_crlf_stray_cr_unchanged() {
        // A stray \r not followed by \n should be preserved.
        let input: Vec<u8> = b"hello\rworld\n".to_vec();
        let output = normalize_crlf(input);
        assert_eq!(output, b"hello\rworld\n");
    }

    #[test]
    fn normalize_crlf_empty() {
        let input: Vec<u8> = b"".to_vec();
        let output = normalize_crlf(input);
        assert_eq!(output, b"");
    }

    #[test]
    fn push_bytes_handles_crlf_in_data() {
        let mut buf = JsonlLineBuffer::new();
        let data = b"{\"a\":1}\r\n{\"a\":2}\r\n";
        let lines = buf.push_bytes(data);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], r#"{"a":1}"#);
        assert_eq!(lines[1], r#"{"a":2}"#);
    }

    #[test]
    fn push_bytes_handles_mixed_line_endings() {
        let mut buf = JsonlLineBuffer::new();
        let data = b"{\"a\":1}\n{\"a\":2}\r\n{\"a\":3}\n";
        let lines = buf.push_bytes(data);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], r#"{"a":1}"#);
        assert_eq!(lines[1], r#"{"a":2}"#);
        assert_eq!(lines[2], r#"{"a":3}"#);
    }

    #[test]
    fn push_bytes_crlf_across_boundary() {
        // Simulate reading \r and \n in separate push_bytes calls
        // (common with small buffer reads on Windows).
        let mut buf = JsonlLineBuffer::new();
        let lines1 = buf.push_bytes(b"{\"a\":1}\r");
        assert_eq!(lines1.len(), 0, "line without \\n stays pending");
        let lines2 = buf.push_bytes(b"\n{\"a\":2}\n");
        assert_eq!(lines2.len(), 2);
        assert_eq!(lines2[0], r#"{"a":1}"#);
        assert_eq!(lines2[1], r#"{"a":2}"#);
    }

    #[test]
    fn read_file_once_crlf_offsets_are_correct() {
        // Verify that checkpoint_offset accounts for \r bytes,
        // not just the normalized line text length.
        let dir = std::env::temp_dir();
        let path = dir.join("test_crlf_offsets.jsonl");

        // Each line: {"a":1} (7 bytes) + \r\n (2 bytes) = 9 bytes
        // 4 lines => 36 bytes total
        let content = b"{\"a\":1}\r\n{\"a\":2}\r\n{\"a\":3}\r\n{\"a\":4}\r\n";
        let file_size = content.len() as u64;
        std::fs::write(&path, content).unwrap();

        let request = ScanFileRequest::for_test("test", "test-file", &path);
        let batch = read_file_once(request).unwrap();

        std::fs::remove_file(&path).ok();

        assert_eq!(batch.lines.len(), 4);
        assert_eq!(
            batch.checkpoint_offset, file_size,
            "checkpoint_offset should equal file size ({}), got {}",
            file_size, batch.checkpoint_offset
        );

        // Each line has an offset span of 9 bytes (7 JSON + CR + LF).
        for (i, line) in batch.lines.iter().enumerate() {
            let start = (i * 9) as u64;
            let end = start + 9;
            assert_eq!(
                line.context.source_offset_start, start,
                "line {} start offset",
                i
            );
            assert_eq!(line.context.source_offset_end, end, "line {} end offset", i);
        }
    }

    #[test]
    fn read_file_once_mixed_endings_offsets() {
        // CRLF (\r\n) lines are 9 bytes, LF (\n) lines are 8 bytes.
        let dir = std::env::temp_dir();
        let path = dir.join("test_mixed_offsets.jsonl");

        // Line 1: {"a":1} (7 bytes) + \r\n (2 bytes) = 9 bytes
        // Line 2: {"a":2} (7 bytes) + \n   (1 byte)  = 8 bytes
        // Line 3: {"a":3} (7 bytes) + \r\n (2 bytes) = 9 bytes
        // Total: 26 bytes
        let content = b"{\"a\":1}\r\n{\"a\":2}\n{\"a\":3}\r\n";
        let file_size = content.len() as u64;
        std::fs::write(&path, content).unwrap();

        let request = ScanFileRequest::for_test("test", "test-file", &path);
        let batch = read_file_once(request).unwrap();

        std::fs::remove_file(&path).ok();

        assert_eq!(batch.lines.len(), 3);
        assert_eq!(
            batch.checkpoint_offset, file_size,
            "checkpoint_offset should equal file size"
        );

        // Line offset spans: CRLF = 9 bytes, LF = 8 bytes
        assert_eq!(batch.lines[0].context.source_offset_start, 0);
        assert_eq!(batch.lines[0].context.source_offset_end, 9);
        assert_eq!(batch.lines[1].context.source_offset_start, 9);
        assert_eq!(batch.lines[1].context.source_offset_end, 17);
        assert_eq!(batch.lines[2].context.source_offset_start, 17);
        assert_eq!(batch.lines[2].context.source_offset_end, 26);
    }
}
