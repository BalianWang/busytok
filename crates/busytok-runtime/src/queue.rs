//! Internal work queue types for the Busytok runtime.
//!
//! These types represent the items flowing through the scan and tail pipelines,
//! tracking per-file scan state and parsed results.

use std::path::PathBuf;

use busytok_domain::events::{NormalizedEvent, NormalizedUsageEvent, OperationalDiagnosticEvent};

/// Result of scanning a single file: parsed events and diagnostics.
#[doc(hidden)]
pub struct FileScanResult {
    /// Path of the scanned file.
    pub path: PathBuf,
    /// Source ID this file belongs to.
    pub source_id: String,
    /// Parsed events (usage + diagnostics).
    pub events: Vec<NormalizedEvent>,
    /// New offset after scanning (only advanced past complete lines).
    pub new_offset: u64,
    /// Number of bytes read during this scan.
    pub bytes_read: u64,
    /// Whether we reached EOF.
    pub reached_eof: bool,
}

impl FileScanResult {
    /// Split events into usage events and diagnostics.
    pub fn partition(self) -> (Vec<NormalizedUsageEvent>, Vec<OperationalDiagnosticEvent>) {
        let mut usage = Vec::new();
        let mut diagnostics = Vec::new();
        for event in self.events {
            match event {
                NormalizedEvent::Usage(u) => usage.push(*u),
                NormalizedEvent::OperationalDiagnostic(d) => diagnostics.push(d),
                NormalizedEvent::Tool(_) => {}
            }
        }
        (usage, diagnostics)
    }
}

/// Internal message sent from the file watcher to the tail processor.
#[derive(Debug)]
pub enum TailWorkItem {
    /// A file was modified; re-read from its last known offset.
    FileChanged { path: PathBuf, source_id: String },
    /// A new file was discovered in a watched directory.
    FileCreated { path: PathBuf, source_id: String },
}

/// Summary statistics from a scan or tail pass.
#[derive(Debug, Default, Clone)]
pub struct ScanStats {
    /// Number of sources discovered.
    pub sources: usize,
    /// Number of files scanned.
    pub files_scanned: usize,
    /// Number of usage events found.
    pub events_found: usize,
    /// Number of diagnostic events found.
    pub diagnostics_found: usize,
}
