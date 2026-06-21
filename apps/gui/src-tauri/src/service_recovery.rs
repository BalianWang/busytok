use std::future::Future;
use std::sync::{Arc, OnceLock};

use anyhow::Result;

use crate::bootstrap_lock::with_bootstrap_file_lock;
use crate::service_lifecycle::{self, EnsureRunningOutcome, ServiceLifecycle};
use busytok_config::BusytokPaths;
use busytok_control::client::ControlClient;
use tokio::sync::Mutex;

pub(crate) fn bootstrap_lock() -> &'static Mutex<()> {
    static BOOTSTRAP_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    BOOTSTRAP_LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) async fn connect_with_service_recovery<F, Fut>(
    socket_path: &str,
    bootstrap_service: F,
) -> Result<ControlClient, String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    match ControlClient::connect(socket_path).await {
        Ok(client) => return Ok(client),
        Err(_first_error) => {}
    }

    // Serialize the bootstrap decision. The lock is held across the entire
    // bootstrap_service call so concurrent invokers do not race. The
    // bootstrap_service passed by callers must NOT itself acquire this
    // same mutex re-entrantly — the coordinator's own internal mutex is a
    // different lock.
    {
        let _guard = bootstrap_lock().lock().await;
        if let Ok(client) = ControlClient::connect(socket_path).await {
            return Ok(client);
        }
        bootstrap_service().await?;
    }

    ControlClient::connect(socket_path)
        .await
        .map_err(|e| format!("service unavailable: {e}"))
}

// ── Trait-driven service recovery ─────────────────────────────────────

/// Runs the recovery loop using the supplied [`ServiceLifecycle`].
///
/// The lifecycle is owned by the [`crate::lifecycle_coordinator::LifecycleCoordinator`]
/// and shared with this function via a reference.
pub fn run_service_recovery(lc: &dyn ServiceLifecycle) -> Result<()> {
    run_recovery_with(lc)
}

/// Serialized ensure-running that routes through the supplied lifecycle.
pub(crate) async fn ensure_service_running_serialized_with(
    lc: Arc<dyn ServiceLifecycle>,
) -> Result<(), String> {
    let _guard = bootstrap_lock().lock().await;
    let paths = BusytokPaths::new();
    let lc_for_blocking = Arc::clone(&lc);
    let result = tauri::async_runtime::spawn_blocking(move || {
        with_bootstrap_file_lock(&paths, || {
            match lc_for_blocking.ensure_running() {
                Ok(service_lifecycle::EnsureRunningOutcome::AlreadyRunning) => {
                    Ok(crate::bootstrap::ServiceBootstrapStatus::AlreadyRunning)
                }
                Ok(service_lifecycle::EnsureRunningOutcome::Started { .. }) => {
                    Ok(crate::bootstrap::ServiceBootstrapStatus::Started)
                }
                Err(e) => Err(e),
            }
        })
    })
    .await
    .map_err(|e| format!("service bootstrap task failed: {e}"))?
    .map(|_| ())
    .map_err(|e| {
        tracing::warn!(
            event_code = "bootstrap.recovery_failed",
            error = %e,
            "service bootstrap failed"
        );
        format!("service bootstrap failed: {e}")
    });
    result
}

/// Core recovery logic, parameterised over any [`ServiceLifecycle`]
/// implementation so it can be unit-tested without touching the system.
pub(crate) fn run_recovery_with(lc: &dyn ServiceLifecycle) -> Result<()> {
    // Log current status for diagnostics
    match lc.status() {
        Ok(status) => {
            tracing::info!(
                event_code = "service_recovery.status_check",
                status = ?status,
                "service status before recovery"
            );
        }
        Err(e) => {
            tracing::warn!(
                event_code = "service_recovery.status_check_failed",
                error = %e,
                "failed to check service status before recovery"
            );
        }
    }

    // Always ensure the service is running (handles upgrades, restarts, etc.)
    match lc.ensure_running() {
        Ok(EnsureRunningOutcome::AlreadyRunning) => {
            tracing::info!(
                event_code = "service_recovery.already_running",
                "service already running"
            );
            Ok(())
        }
        Ok(EnsureRunningOutcome::Started { .. }) => {
            tracing::info!(
                event_code = "service_recovery.started",
                "service started during recovery"
            );
            Ok(())
        }
        Err(e) => {
            tracing::warn!(
                event_code = "service_recovery.failed",
                error = %e,
                "service recovery failed"
            );
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service_lifecycle::{EnsureRunningOutcome, InstallOutcome, LifecycleStatus};
    use std::sync::Mutex;

    struct CountingFake {
        ensure_calls: Mutex<u32>,
    }

    impl ServiceLifecycle for CountingFake {
        fn ensure_registered(&self) -> Result<InstallOutcome> {
            Ok(InstallOutcome::AlreadyPresent)
        }
        fn ensure_running(&self) -> Result<EnsureRunningOutcome> {
            let mut n = self.ensure_calls.lock().unwrap();
            *n += 1;
            Ok(EnsureRunningOutcome::AlreadyRunning)
        }
        fn status(&self) -> Result<LifecycleStatus> {
            Ok(LifecycleStatus::Running)
        }
        fn stop_for_current_session(&self) -> Result<()> {
            Ok(())
        }
        fn uninstall(&self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn recovery_invokes_lifecycle() {
        let fake = CountingFake {
            ensure_calls: Mutex::new(0),
        };
        let _ = run_recovery_with(&fake);
        assert!(
            *fake.ensure_calls.lock().unwrap() >= 1,
            "recovery should call ensure_running at least once"
        );
    }
}
