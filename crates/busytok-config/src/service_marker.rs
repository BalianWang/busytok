//! Marker-file readiness signal for the Busytok service.
//! Replaces `control_socket().exists()` for Windows parity.
//! Lives in busytok-config because it's a path operation, and both
//! busytok-runtime (writer) and apps/gui (reader) already depend on config.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn marker_path(data_dir: &Path) -> PathBuf {
    data_dir.join("service.ready")
}

pub fn write(data_dir: &Path) -> Result<PathBuf> {
    let path = marker_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("ready.tmp");
    std::fs::write(&tmp, "")?;
    std::fs::rename(&tmp, &path)?;
    tracing::info!(event_code = "service.marker.written", path = %path.display());
    Ok(path)
}

pub fn remove(data_dir: &Path) -> Result<()> {
    let path = marker_path(data_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            tracing::info!(event_code = "service.marker.removed", path = %path.display());
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing marker file {}", path.display())),
    }
}

pub fn exists(data_dir: &Path) -> bool {
    marker_path(data_dir).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn write_then_exists_then_remove() {
        let dir = tempdir().unwrap();
        assert!(!exists(dir.path()));
        write(dir.path()).unwrap();
        assert!(exists(dir.path()));
        remove(dir.path()).unwrap();
        assert!(!exists(dir.path()));
    }
    #[test]
    fn remove_when_missing_is_ok() {
        let dir = tempdir().unwrap();
        remove(dir.path()).unwrap();
    }
    #[test]
    fn write_idempotent() {
        let dir = tempdir().unwrap();
        write(dir.path()).unwrap();
        write(dir.path()).unwrap();
        assert!(exists(dir.path()));
    }
}
