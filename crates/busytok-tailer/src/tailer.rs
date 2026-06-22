//! File watch service built on the `notify` crate.
//!
//! Provides a simple API for watching directories and polling for file change
//! events. This module is not wired to the GUI; Task 15 wires the runtime
//! pipeline.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

/// A file change event from the watcher.
#[derive(Debug, Clone)]
pub struct FileChangeEvent {
    /// The path that changed.
    pub path: PathBuf,
    /// The kind of event (create, modify, remove, etc.).
    pub kind: FileChangeKind,
}

/// Simplified event kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeKind {
    /// File was created.
    Created,
    /// File was modified.
    Modified,
    /// File was deleted.
    Removed,
    /// Other/unknown event kind.
    Other,
}

impl From<&notify::EventKind> for FileChangeKind {
    fn from(kind: &notify::EventKind) -> Self {
        match kind {
            notify::EventKind::Create(_) => FileChangeKind::Created,
            notify::EventKind::Modify(_) => FileChangeKind::Modified,
            notify::EventKind::Remove(_) => FileChangeKind::Removed,
            _ => FileChangeKind::Other,
        }
    }
}

/// A file watch service that monitors directories for changes.
pub struct FileWatchService {
    watcher: RecommendedWatcher,
    rx: mpsc::Receiver<Result<FileChangeEvent>>,
    watched_paths: Vec<PathBuf>,
}

impl FileWatchService {
    /// Create a new file watch service.
    pub fn new() -> Result<Self> {
        let (tx, rx) = mpsc::channel();

        let watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            let event = match res {
                Ok(e) => e,
                Err(err) => {
                    // Forward errors as Err results.
                    let _ = tx.send(Err(anyhow::anyhow!("watch error: {err}")));
                    return;
                }
            };

            let kind = FileChangeKind::from(&event.kind);
            for path in event.paths {
                let change = FileChangeEvent { path, kind };
                let _ = tx.send(Ok(change));
            }
        })
        .context("failed to create file watcher")?;

        Ok(Self {
            watcher,
            rx,
            watched_paths: Vec::new(),
        })
    }

    /// Start watching a directory for file changes.
    pub fn watch_path(&mut self, path: &Path) -> Result<()> {
        self.watcher
            .watch(path, RecursiveMode::Recursive)
            .with_context(|| format!("failed to watch path {}", path.display()))?;
        self.watched_paths.push(path.to_path_buf());
        Ok(())
    }

    /// Stop watching a path.
    pub fn unwatch_path(&mut self, path: &Path) -> Result<()> {
        self.watcher
            .unwatch(path)
            .with_context(|| format!("failed to unwatch path {}", path.display()))?;
        self.watched_paths.retain(|p| p != path);
        Ok(())
    }

    /// Poll for pending file change events with an optional timeout.
    ///
    /// Returns immediately if there are pending events. If no events are
    /// pending, waits up to `timeout` for the first event.
    pub fn poll_events(&self, timeout: Duration) -> Vec<Result<FileChangeEvent>> {
        let mut events = Vec::new();

        // Try to receive the first event with the specified timeout.
        let first = match self.rx.recv_timeout(timeout) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => return events,
            Err(mpsc::RecvTimeoutError::Disconnected) => return events,
        };

        events.push(first);

        // Drain any additional pending events without blocking.
        while let Ok(event) = self.rx.try_recv() {
            events.push(event);
        }

        events
    }

    /// Returns the list of currently watched paths.
    pub fn watched_paths(&self) -> &[PathBuf] {
        &self.watched_paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_watch_service_can_be_created() {
        let service = FileWatchService::new();
        assert!(service.is_ok());
    }
}
