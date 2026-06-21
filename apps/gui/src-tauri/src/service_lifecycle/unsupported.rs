//! No-op [`ServiceLifecycle`] for platforms that don't have a native
//! ensure_registered/ensure_running/status/stop_for_current_session/uninstall
//! flow (anything other than macOS LaunchAgent and Windows Task Scheduler).
//! Mirrors the `unsupported.rs` pattern from `busytok-platform`.

use anyhow::{bail, Result};

use super::{
    EnsureRunningOutcome, InstallOutcome, LifecycleStatus, ServiceLifecycle,
};

pub struct UnsupportedLifecycle;

impl ServiceLifecycle for UnsupportedLifecycle {
    fn ensure_registered(&self) -> Result<InstallOutcome> {
        bail!("ServiceLifecycle not supported on this platform")
    }

    fn ensure_running(&self) -> Result<EnsureRunningOutcome> {
        bail!("ServiceLifecycle not supported on this platform")
    }

    fn status(&self) -> Result<LifecycleStatus> {
        Ok(LifecycleStatus::NeedsAttention)
    }

    fn stop_for_current_session(&self) -> Result<()> {
        bail!("ServiceLifecycle not supported on this platform")
    }

    fn uninstall(&self) -> Result<()> {
        bail!("ServiceLifecycle not supported on this platform")
    }
}
