use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;

use anyhow::{Context, Result};
use busytok_config::BusytokPaths;
use fs2::FileExt;

pub fn bootstrap_lock_path(paths: &BusytokPaths) -> PathBuf {
    paths.data_dir().join("bootstrap.lock")
}

pub fn with_bootstrap_file_lock<F, T>(paths: &BusytokPaths, f: F) -> Result<T>
where
    F: FnOnce() -> Result<T>,
{
    fs::create_dir_all(paths.data_dir())?;
    let path = bootstrap_lock_path(paths);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening bootstrap lock {}", path.display()))?;
    acquire_lock(&file, &path)?;
    let result = f();
    file.unlock().ok();
    result
}

fn acquire_lock(file: &File, path: &std::path::Path) -> Result<()> {
    tracing::info!(
        event_code = "bootstrap.lock_wait",
        path = %path.display(),
        "waiting for bootstrap file lock"
    );
    file.lock_exclusive()
        .with_context(|| format!("locking {}", path.display()))?;
    tracing::info!(
        event_code = "bootstrap.lock_acquired",
        path = %path.display(),
        "acquired bootstrap file lock"
    );
    Ok(())
}
