//! Service application lifecycle holder.

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use tracing::{error, info, warn};

use busytok_config::BusytokPaths;

use crate::bootstrap;
use crate::BusytokSupervisor;

pub struct ServiceApp {
    startup: Instant,
    supervisor: Arc<BusytokSupervisor>,
    server: Arc<busytok_control::server::ControlServer>,
    server_task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl ServiceApp {
    /// Bootstrap stages 1–4: database, supervisor, status hydration, control server.
    pub async fn boot(paths: BusytokPaths, startup: Instant) -> Result<Self> {
        // Clear any stale marker from a previous run that crashed before
        // removing it. supervisor.rs and the GUI both poll
        // service_marker::exists() to decide whether the service is ready,
        // so a leftover marker would falsely advertise readiness.
        let _ = busytok_config::service_marker::remove(paths.data_dir());

        let (db, _db_report) = bootstrap::open_database(&paths)?;
        let supervisor = bootstrap::create_supervisor(db, paths.clone());
        let _hydrate_report = bootstrap::hydrate_status(&supervisor)?;
        let (server, server_task) =
            bootstrap::bind_control_server(&paths.control_endpoint()?, supervisor.clone()).await?;

        // Write the readiness marker AFTER the control server is bound so
        // supervisor.rs status readers correctly observe service_ready=true.
        // bootstrap::emit_service_ready (called in run()) emits a tracing
        // event; this marker file is the separate signal polled across
        // process boundaries.
        busytok_config::service_marker::write(paths.data_dir())
            .context("writing service.ready marker")?;

        Ok(Self {
            startup,
            supervisor,
            server,
            server_task,
        })
    }

    /// Run stages 5–9 + graceful shutdown.
    ///
    /// Initial scan failure is non-fatal (transitions to ReadyDegraded).
    /// Tailer startup failure IS fatal and triggers full control server
    /// cleanup (shutdown + drain + join) before returning the error.
    ///
    /// The service.ready marker written in `boot()` is removed at every exit
    /// path so supervisor.rs status readers correctly observe service_offline
    /// once the service is no longer running.
    pub async fn run(self) -> Result<()> {
        // Capture the data_dir from paths before destructuring self so we
        // can remove the marker at every exit point.
        let data_dir = self.supervisor.paths().data_dir().to_path_buf();

        let _scan = bootstrap::run_initial_scan_or_register_sources(self.supervisor.clone()).await;

        let tail = match bootstrap::start_tailing(self.supervisor.clone()).await {
            Ok(tail) => tail,
            Err(err) => {
                error!(
                    event_code = "service.tailer.failed",
                    error = %err,
                );
                let ServiceApp {
                    server,
                    server_task,
                    ..
                } = self;
                if let Err(shutdown_err) = shutdown_control_server(server, server_task, false).await
                {
                    warn!(
                        event_code = "service.control_server.shutdown_failed",
                        error = %shutdown_err,
                        "Control server shutdown after tailer failure also failed"
                    );
                }
                let _ = busytok_config::service_marker::remove(&data_dir);
                return Err(err);
            }
        };

        let sampler = bootstrap::start_sampler(&self.supervisor);
        let background_jobs = bootstrap::spawn_background_jobs(self.supervisor.clone());
        bootstrap::emit_service_ready(self.startup);

        let ServiceApp {
            startup,
            supervisor,
            server,
            mut server_task,
        } = self;

        // On Unix (macOS LaunchAgent), launchd sends SIGTERM on stop; tokio's
        // ctrl_c listener catches SIGINT/SIGTERM and enters the graceful path
        // below (marker remove, tailer drain, writer flush, WAL checkpoint).
        //
        // On Windows Task Scheduler, graceful shutdown is NOT guaranteed:
        //   - User logoff → CTRL_SHUTDOWN_EVENT (may or may not reach console handler)
        //   - "End Task" in Task Scheduler → WM_CLOSE (no console → not delivered)
        //   - `schtasks /End` → TerminateProcess (no signal at all)
        //   - RestartOnFailure → old instance killed, new one started
        // In these cases tokio::signal::ctrl_c does NOT fire; the process is
        // terminated without running the cleanup below.
        //
        // This is acceptable because:
        //   1. service_marker::write is called at boot start (stale-marker cleanup
        //      in ServiceApp::boot removes any leftover before writing a new one)
        //   2. SQLite WAL mode is crash-safe — committed data survives abrupt
        //      termination; uncommitted writes are discarded by WAL recovery
        //   3. Writer actor's in-memory queue may lose pending events, but
        //      log files are idempotent to re-parse (tailer resumes from
        //      last checkpoint offset on next boot)
        //   4. RestartOnFailure 3×PT30S provides up to 90s of coverage
        //
        // A SetConsoleCtrlHandler (CTRL_CLOSE_EVENT / CTRL_LOGOFF_EVENT /
        // CTRL_SHUTDOWN_EVENT) could improve this, but adds complexity
        // (static shutdown channel + unsafe extern "system" + multi-thread
        // coordination) for marginal benefit given the crash-recovery design.
        let result_already_read = tokio::select! {
            result = &mut server_task => {
                result??;
                true
            }
            _ = tokio::signal::ctrl_c() => {
                info!(event_code = "service.shutdown.begin");
                false
            }
        };

        shutdown_control_server(server, server_task, result_already_read).await?;

        // --- NEW: shut down the Pi sidecar subprocess (hibernate sessions,
        // kill child). Must come AFTER control server shutdown (no new
        // delegate requests can arrive) and BEFORE the tailer/sampler drain
        // so in-flight sidecar turns don't compete with the writer actor's
        // final flush. No-op when `pi_sidecar.enabled = false`. ---
        supervisor.shutdown_sidecar().await;

        let _ = sampler.send(true);
        let _ = tail.shutdown_tx.send(true);
        let _ = tail.join_handle.await;

        supervisor
            .shutdown_writer()
            .await
            .context("shutdown writer actor")?;

        drop(background_jobs);

        let _ = busytok_config::service_marker::remove(&data_dir);

        info!(
            event_code = "service.shutdown.complete",
            total_elapsed_ms = startup.elapsed().as_millis() as u64,
            "Busytok service shut down gracefully"
        );

        Ok(())
    }
}

/// Shutdown the control server and wait for the server task to exit.
/// Used by both the tailer-failure and normal-shutdown paths.
///
/// `result_already_read`: true when the caller has already extracted the
/// JoinHandle result (e.g. via `result??` in `tokio::select!`). When false,
/// we await the handle here to propagate any stored error.
pub(crate) async fn shutdown_control_server(
    server: Arc<busytok_control::server::ControlServer>,
    mut server_task: tokio::task::JoinHandle<anyhow::Result<()>>,
    result_already_read: bool,
) -> Result<()> {
    server.shutdown();
    server.await_drain().await;
    if !result_already_read {
        (&mut server_task).await??;
    }
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use busytok_config::BusytokPaths;
    use busytok_control::server::ControlServer;
    use serial_test::serial;
    use std::time::Instant;

    /// boot() should complete stages 1-4 and return a ServiceApp with a
    /// running control server that we can shut down cleanly.
    ///
    /// Regression coverage for the P0 marker bug: prior to the fix, boot()
    /// returned Ok(()) without ever writing the service.ready marker, while
    /// supervisor.rs and the GUI polled service_marker::exists() to decide
    /// readiness. This test deliberately does NOT call
    /// `service_marker::write` manually — if boot() regresses and stops
    /// writing the marker, this assertion fails.
    #[tokio::test]
    #[serial]
    async fn boot_creates_operational_service() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = BusytokPaths::for_test(dir.path());
        paths.ensure_dirs_exist().expect("ensure dirs");
        let startup = Instant::now();
        let data_dir = paths.data_dir().to_path_buf();

        let app = ServiceApp::boot(paths, startup).await.expect("boot");

        // boot() must write the service.ready marker so supervisor.rs and the
        // GUI see the service as ready before run() begins.
        assert!(
            busytok_config::service_marker::exists(&data_dir),
            "ServiceApp::boot must write the service.ready marker — supervisor status readers depend on it"
        );

        let ServiceApp {
            server,
            server_task,
            ..
        } = app;
        shutdown_control_server(server, server_task, false)
            .await
            .expect("shutdown");
    }

    /// The tailer-failure cleanup path: shutdown + drain + await server_task.
    /// Tests `shutdown_control_server` directly since making `start_tailing`
    /// fail in a unit test is impractical.
    #[tokio::test]
    async fn shutdown_control_server_awaits_task() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = BusytokPaths::for_test(dir.path());
        paths.ensure_dirs_exist().expect("ensure dirs");

        let db = busytok_store::Database::open_in_memory().expect("db");
        let supervisor = Arc::new(BusytokSupervisor::new(db, paths));

        let (server, _endpoint) = ControlServer::spawn_for_test(supervisor)
            .await
            .expect("spawn_for_test");
        let server = Arc::new(server);
        let server_task = tokio::spawn({
            let server = Arc::clone(&server);
            async move { server.run().await }
        });

        // Shutdown should complete without hanging.
        shutdown_control_server(Arc::clone(&server), server_task, false)
            .await
            .expect("shutdown");
    }

    /// Regression: if the server task already exited with an error before
    /// shutdown_control_server is called, the error must propagate (not be
    /// silently swallowed). This models the tailer-failure path where the
    /// result_already_read flag is false.
    #[tokio::test]
    async fn shutdown_propagates_already_errored_task() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = BusytokPaths::for_test(dir.path());
        paths.ensure_dirs_exist().expect("ensure dirs");

        let db = busytok_store::Database::open_in_memory().expect("db");
        let supervisor = Arc::new(BusytokSupervisor::new(db, paths));

        let (server, _endpoint) = ControlServer::spawn_for_test(supervisor)
            .await
            .expect("spawn_for_test");
        let server = Arc::new(server);

        // Spawn a task that immediately errors (simulating server crash).
        let server_task: tokio::task::JoinHandle<anyhow::Result<()>> =
            tokio::spawn(async { Err(anyhow::anyhow!("simulated server crash")) });

        // Wait for the task to finish WITHOUT consuming its result — poll
        // is_finished() instead of awaiting the handle.
        while !server_task.is_finished() {
            tokio::task::yield_now().await;
        }

        let result = shutdown_control_server(Arc::clone(&server), server_task, false).await;
        assert!(
            result.is_err(),
            "shutdown should propagate the already-stored task error"
        );
    }

    /// `shutdown_control_server` with `result_already_read=true` skips the
    /// server_task await (the caller already consumed the JoinHandle result
    /// via `result??` in the `tokio::select!` block of `run()`). This test
    /// verifies the skip path does not hang or panic.
    #[tokio::test]
    async fn shutdown_control_server_skips_task_when_already_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = BusytokPaths::for_test(dir.path());
        paths.ensure_dirs_exist().expect("ensure dirs");

        let db = busytok_store::Database::open_in_memory().expect("db");
        let supervisor = Arc::new(BusytokSupervisor::new(db, paths));

        let (server, _endpoint) = ControlServer::spawn_for_test(supervisor)
            .await
            .expect("spawn_for_test");
        let server = Arc::new(server);
        let server_task = tokio::spawn({
            let server = Arc::clone(&server);
            async move { server.run().await }
        });

        // With result_already_read=true, the function must NOT await the
        // server_task — it just shuts down the server and drains.
        let result = shutdown_control_server(Arc::clone(&server), server_task, true).await;
        assert!(
            result.is_ok(),
            "shutdown with already_read=true must succeed"
        );
    }

    /// Full `run()` lifecycle: boot, reach the `select!` block, then trigger
    /// shutdown by calling `server.shutdown()` from a separate task. This
    /// exercises the `server_task` completion branch of the `select!` and
    /// the entire graceful-shutdown cleanup sequence (sampler, tailer,
    /// writer, sidecar, marker removal).
    ///
    /// The `server` field is private, but this test is in a child module of
    /// `service_app`, so it can access private fields.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    async fn run_completes_gracefully_on_server_shutdown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = BusytokPaths::for_test(dir.path());
        paths.ensure_dirs_exist().expect("ensure dirs");
        let data_dir = paths.data_dir().to_path_buf();

        // Keep this lifecycle test hermetic: default discovery would scan the
        // developer/CI user's real Claude and Codex logs, making the 10s
        // shutdown assertion depend on host data volume.
        let mut settings = busytok_config::BusytokSettings::default();
        settings.discovery.claude_code_default_paths = false;
        settings.discovery.codex_default_paths = false;
        settings.save(&paths).expect("save test settings");

        let app = ServiceApp::boot(paths, Instant::now()).await.expect("boot");

        // Clone the server Arc BEFORE run() takes ownership of app.
        // Child modules can access private parent fields.
        let server = Arc::clone(&app.server);

        // Spawn a task that waits for run() to reach the select! block,
        // then shuts down the server to trigger the server_task completion
        // path (the first branch of the tokio::select!).
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            server.shutdown();
        });

        // run() should complete when server_task completes.
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), app.run()).await;

        // The timeout should NOT fire — run() should complete.
        assert!(
            result.is_ok(),
            "run() should complete within 10s after server shutdown"
        );
        let run_result = result.unwrap();
        assert!(
            run_result.is_ok(),
            "run() should return Ok(()) after graceful shutdown"
        );

        // The marker must be removed by run()'s cleanup path.
        assert!(
            !busytok_config::service_marker::exists(&data_dir),
            "service.ready marker must be removed after run() completes"
        );
    }
}
