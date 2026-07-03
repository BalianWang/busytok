#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
use std::process;

use anyhow::Result;
use tracing::{error, info};

use busytok_config::{init_logging, BusytokPaths, LogSource};
use busytok_runtime::ServiceApp;

#[tokio::main]
async fn main() {
    let paths = BusytokPaths::new();
    let _ = paths.ensure_dirs_exist();

    let service_session_id = uuid::Uuid::new_v4().to_string();
    let _guards = init_logging(&paths.log_dir(), LogSource::Service, &service_session_id);

    let _root = tracing::info_span!(
        "service_process",
        session_id = %service_session_id,
        source = "service",
        pid = std::process::id(),
    )
    .entered();

    if let Err(err) = run_main(paths).await {
        error!(
            event_code = "service.startup.fatal",
            stage = "service_app",
            error = %err,
            "service fatal error"
        );
        drop(_guards);
        drop(_root);
        process::exit(1);
    }
}

async fn run_main(paths: BusytokPaths) -> Result<()> {
    let startup = std::time::Instant::now();
    info!(
        event_code = "service.startup.begin",
        pid = std::process::id(),
        version = env!("CARGO_PKG_VERSION"),
        "Busytok service starting"
    );

    let app = ServiceApp::boot(paths, startup).await?;
    app.run().await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// `run_main` boots the full service (stages 1–4) and enters the `run()`
    /// select-loop. We verify it reaches the blocking point by checking that
    /// the service.ready marker is written (indicating boot completed).
    ///
    /// `run_main` is `!Send` (ServiceApp::run uses tokio::signal::ctrl_c),
    /// so we use `tokio::select!` to race it against a marker-poll loop
    /// instead of `tokio::spawn`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_main_boots_service_and_enters_run_loop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = BusytokPaths::for_test(dir.path());
        paths.ensure_dirs_exist().expect("ensure dirs");
        let data_dir = paths.data_dir().to_path_buf();

        // Race run_main against a marker-poll loop. When the marker
        // appears, boot() completed and run() entered its blocking select!.
        tokio::select! {
            result = run_main(paths) => {
                panic!(
                    "run_main should block on select! loop, but completed with: {:?}",
                    result
                );
            }
            _ = async {
                let deadline = tokio::time::Instant::now()
                    + std::time::Duration::from_secs(10);
                loop {
                    if busytok_config::service_marker::exists(&data_dir) {
                        return;
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            } => {}
        }

        assert!(
            busytok_config::service_marker::exists(&data_dir),
            "service.ready marker must appear after boot()"
        );
    }
}
