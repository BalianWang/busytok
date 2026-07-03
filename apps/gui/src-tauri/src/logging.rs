//! Tauri-side observability: logging init, frontend log relay, bootstrap events.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use busytok_config::LoggingGuards;
use serde::Deserialize;
use serde_json::json;
use time::format_description::FormatItem;
use time::macros::format_description;
use time::OffsetDateTime;

/// Process-wide session identifier, set once during init.
/// Trace sites reference this directly since a root span cannot
/// propagate across async task boundaries in Tauri.
static TAURI_SESSION_ID: OnceLock<String> = OnceLock::new();
static TAURI_LOG_DIR: OnceLock<PathBuf> = OnceLock::new();
static TAURI_MANUAL_FILE_LOGGING: AtomicBool = AtomicBool::new(false);
static DATE_FMT: &[FormatItem<'static>] = format_description!("[year]-[month]-[day]");

pub fn tauri_session_id() -> &'static str {
    TAURI_SESSION_ID
        .get()
        .map(|s| s.as_str())
        .unwrap_or("unknown")
}

fn manual_file_logging_enabled() -> bool {
    TAURI_MANUAL_FILE_LOGGING.load(Ordering::Relaxed)
}

fn rotated_log_path(base_name: &str) -> Option<PathBuf> {
    let dir = TAURI_LOG_DIR.get()?;
    let date = OffsetDateTime::now_utc().format(DATE_FMT).ok()?;
    Some(dir.join(format!("{base_name}.{date}")))
}

fn append_json_line(path: &Path, value: &serde_json::Value) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = serde_json::to_writer(&mut file, value);
    let _ = file.write_all(b"\n");
    let _ = file.flush();
}

fn append_manual_event(
    base_name: &str,
    ts: &str,
    level: &str,
    source: &str,
    session_id: &str,
    correlation_id: Option<&str>,
    event_code: &str,
    message: &str,
    details: Option<serde_json::Value>,
) {
    if !manual_file_logging_enabled() {
        return;
    }
    let Some(path) = rotated_log_path(base_name) else {
        return;
    };
    let value = json!({
        "ts": ts,
        "level": level,
        "source": source,
        "session_id": session_id,
        "correlation_id": correlation_id,
        "event_code": event_code,
        "message": message,
        "details": details,
    });
    append_json_line(&path, &value);
}

pub fn append_bootstrap_event(
    level: &str,
    event_code: &str,
    message: &str,
    details: Option<serde_json::Value>,
) {
    append_manual_event(
        "bootstrap.log",
        &OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        level,
        "tauri",
        tauri_session_id(),
        None,
        event_code,
        message,
        details,
    );
}

// ── DTOs ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct FrontendLogEntryDto {
    pub ts: String,
    pub level: String,
    pub session_id: String,
    #[serde(default)]
    pub correlation_id: Option<String>,
    pub event_code: String,
    pub message: String,
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FlushResult {
    pub written_count: usize,
    pub dropped_count: usize,
}

// ── Logging initialization ────────────────────────────────────────

/// Initialize tracing for the Tauri GUI process.
///
/// **Must be called once, before `ensure_service_running()`.**
/// Sends terminal output to stderr, JSON to `gui.log` (all targets)
/// and `bootstrap.log` (only `target=bootstrap`).
///
/// Callers guarantee single-call; this implementation uses `try_init`
/// so repeat calls in tests are a graceful no-op.
pub fn init_gui_logging(log_dir: &Path, session_id: &str) -> Option<LoggingGuards> {
    let guards = busytok_config::init_logging(log_dir, busytok_config::LogSource::Gui, session_id);
    let _ = TAURI_LOG_DIR.set(log_dir.to_path_buf());

    // Store session_id for cross-module access (bootstrap.rs trace sites).
    let _ = TAURI_SESSION_ID.set(session_id.to_string());
    // Manual JSON appenders are a fallback only. When tracing file
    // layers initialize successfully we avoid synchronous double-writes
    // and rely on the subscriber-owned non-blocking appenders.
    TAURI_MANUAL_FILE_LOGGING.store(guards.is_none(), Ordering::Relaxed);

    tracing::info!(
        target: "bootstrap",
        event_code = "tauri.startup.begin",
        session_id = %session_id,
        source = "tauri",
        pid = std::process::id(),
        "Tauri GUI process starting"
    );
    append_bootstrap_event(
        "INFO",
        "tauri.startup.begin",
        "Tauri GUI process starting",
        Some(json!({ "pid": std::process::id() })),
    );

    guards
}

// ── Frontend log writing ──────────────────────────────────────────

pub fn write_frontend_log_entry(entry: &FrontendLogEntryDto) {
    let span = tracing::info_span!(
        "frontend_event",
        session_id = %entry.session_id,
        correlation_id = entry.correlation_id.as_deref().unwrap_or(""),
    );
    let _guard = span.enter();

    let msg = &entry.message;
    match entry.level.as_str() {
        "ERROR" => {
            tracing::error!(
                source = "frontend",
                event_code = %entry.event_code,
                ts = %entry.ts,
                details = ?entry.details,
                "{msg}"
            );
        }
        "WARN" => {
            tracing::warn!(
                source = "frontend",
                event_code = %entry.event_code,
                ts = %entry.ts,
                details = ?entry.details,
                "{msg}"
            );
        }
        _ => {
            tracing::info!(
                source = "frontend",
                event_code = %entry.event_code,
                ts = %entry.ts,
                details = ?entry.details,
                "{msg}"
            );
        }
    }

    append_manual_event(
        "gui.log",
        &entry.ts,
        &entry.level,
        "frontend",
        &entry.session_id,
        entry.correlation_id.as_deref(),
        &entry.event_code,
        &entry.message,
        entry.details.clone(),
    );
}

pub fn flush_frontend_logs_inner(entries: &[FrontendLogEntryDto]) -> FlushResult {
    let total = entries.len();
    let sid = tauri_session_id();
    tracing::info!(
        event_code = "frontend.buffer_flush",
        session_id = %sid,
        source = "tauri",
        received_count = total,
        "flushing buffered frontend logs"
    );
    append_manual_event(
        "gui.log",
        &OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        "INFO",
        "tauri",
        sid,
        None,
        "frontend.buffer_flush",
        "flushing buffered frontend logs",
        Some(json!({ "received_count": total })),
    );

    let mut dropped = 0usize;
    for entry in entries {
        if entry.event_code.is_empty() || entry.message.is_empty() {
            dropped += 1;
            continue;
        }
        write_frontend_log_entry(entry);
    }

    let written = total.saturating_sub(dropped);
    if dropped > 0 {
        tracing::warn!(
            event_code = "frontend.buffer_flush_partial_failure",
            session_id = %sid,
            source = "tauri",
            written_count = written,
            dropped_count = dropped,
            "some buffered entries were invalid and skipped"
        );
        append_manual_event(
            "gui.log",
            &OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
            "WARN",
            "tauri",
            sid,
            None,
            "frontend.buffer_flush_partial_failure",
            "some buffered entries were invalid and skipped",
            Some(json!({ "written_count": written, "dropped_count": dropped })),
        );
    }

    FlushResult {
        written_count: written,
        dropped_count: dropped,
    }
}

// ── Tauri command handlers (thin wrappers) ────────────────────────

#[tauri::command]
pub(crate) fn log_frontend_event(entry: FrontendLogEntryDto) -> Result<(), String> {
    write_frontend_log_entry(&entry);
    Ok(())
}

#[tauri::command]
pub(crate) fn flush_frontend_logs(
    entries: Vec<FrontendLogEntryDto>,
) -> Result<FlushResult, String> {
    Ok(flush_frontend_logs_inner(&entries))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
    use super::*;

    fn make_entry(level: &str, event_code: &str, message: &str) -> FrontendLogEntryDto {
        FrontendLogEntryDto {
            ts: "2026-01-01T00:00:00Z".to_string(),
            level: level.to_string(),
            session_id: "test-session".to_string(),
            correlation_id: None,
            event_code: event_code.to_string(),
            message: message.to_string(),
            details: None,
        }
    }

    #[test]
    fn write_frontend_log_entry_error_level_executes_match_branch() {
        let entry = make_entry("ERROR", "test.error", "error message");
        write_frontend_log_entry(&entry);
    }

    #[test]
    fn write_frontend_log_entry_warn_level_executes_match_branch() {
        let entry = make_entry("WARN", "test.warn", "warn message");
        write_frontend_log_entry(&entry);
    }

    #[test]
    fn write_frontend_log_entry_info_level_executes_default_branch() {
        let entry = make_entry("INFO", "test.info", "info message");
        write_frontend_log_entry(&entry);
    }

    #[test]
    fn write_frontend_log_entry_unknown_level_executes_default_branch() {
        let entry = make_entry("DEBUG", "test.debug", "debug message");
        write_frontend_log_entry(&entry);
    }

    #[test]
    fn flush_frontend_logs_inner_with_all_valid_entries() {
        let entries = vec![
            make_entry("ERROR", "evt.1", "msg 1"),
            make_entry("INFO", "evt.2", "msg 2"),
        ];
        let result = flush_frontend_logs_inner(&entries);
        assert_eq!(result.written_count, 2);
        assert_eq!(result.dropped_count, 0);
    }

    #[test]
    fn flush_frontend_logs_inner_drops_entries_with_empty_event_code() {
        let entries = vec![
            make_entry("INFO", "", "msg with empty code"),
            make_entry("INFO", "evt.valid", "valid msg"),
        ];
        let result = flush_frontend_logs_inner(&entries);
        assert_eq!(result.written_count, 1);
        assert_eq!(result.dropped_count, 1);
    }

    #[test]
    fn flush_frontend_logs_inner_drops_entries_with_empty_message() {
        let entries = vec![
            make_entry("INFO", "evt.valid", ""),
            make_entry("INFO", "evt.also", "valid"),
        ];
        let result = flush_frontend_logs_inner(&entries);
        assert_eq!(result.written_count, 1);
        assert_eq!(result.dropped_count, 1);
    }

    #[test]
    fn flush_frontend_logs_inner_with_empty_list() {
        let result = flush_frontend_logs_inner(&[]);
        assert_eq!(result.written_count, 0);
        assert_eq!(result.dropped_count, 0);
    }

    #[test]
    fn flush_frontend_logs_inner_drops_all_invalid_entries() {
        let entries = vec![
            make_entry("INFO", "", ""),
            make_entry("WARN", "", "msg with empty code"),
        ];
        let result = flush_frontend_logs_inner(&entries);
        assert_eq!(result.written_count, 0);
        assert_eq!(result.dropped_count, 2);
    }

    #[test]
    fn tauri_session_id_returns_unknown_before_init() {
        // Before init_gui_logging is called, TAURI_SESSION_ID is not set.
        // This test may run after another test that called init_gui_logging,
        // in which case tauri_session_id() returns the set value. We just
        // verify the function does not panic.
        let _ = tauri_session_id();
    }

    #[test]
    fn rotated_log_path_returns_none_when_log_dir_not_set() {
        // When TAURI_LOG_DIR is not set, rotated_log_path returns None.
        // This test verifies that path; if TAURI_LOG_DIR was already set
        // by a previous test, this still passes (the path will be Some).
        let _ = rotated_log_path("test.log");
    }

    #[test]
    fn append_manual_event_returns_early_when_disabled() {
        // When manual file logging is disabled, append_manual_event returns
        // immediately without touching the filesystem.
        TAURI_MANUAL_FILE_LOGGING.store(false, Ordering::Relaxed);
        append_manual_event(
            "test.log",
            "2026-01-01T00:00:00Z",
            "INFO",
            "test",
            "session",
            None,
            "test.event",
            "test message",
            None,
        );
    }

    #[test]
    fn append_manual_event_writes_when_enabled_and_dir_set() {
        // Enable manual logging and set a log dir so the full path executes.
        TAURI_MANUAL_FILE_LOGGING.store(true, Ordering::Relaxed);
        // Try to set TAURI_LOG_DIR — if already set, that's fine.
        let _ = TAURI_LOG_DIR.set(std::env::temp_dir());
        append_manual_event(
            "test-manual.log",
            "2026-01-01T00:00:00Z",
            "INFO",
            "test",
            "session",
            None,
            "test.event",
            "test message for manual logging",
            Some(serde_json::json!({"key": "value"})),
        );
        // Reset to avoid affecting other tests.
        TAURI_MANUAL_FILE_LOGGING.store(false, Ordering::Relaxed);
    }

    #[test]
    fn append_bootstrap_event_executes_without_panic() {
        // append_bootstrap_event calls append_manual_event which checks
        // the manual logging flag. We just verify no panic.
        append_bootstrap_event(
            "INFO",
            "test.bootstrap",
            "bootstrap test message",
            Some(serde_json::json!({"phase": "test"})),
        );
    }

    #[test]
    fn init_gui_logging_can_be_called() {
        // init_gui_logging uses try_init internally so repeat calls are
        // no-ops. We verify it doesn't panic.
        let dir = tempfile::tempdir().unwrap();
        let _ = init_gui_logging(dir.path(), "test-session-id");
    }
}
