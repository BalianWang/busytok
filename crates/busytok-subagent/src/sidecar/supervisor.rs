//! Pi sidecar supervisor — owns the sidecar process lifecycle.
//!
//! Responsibilities (spec §5.4):
//! - Lazy spawn on first `ensure_started` (initialize handshake)
//! - Background supervision loop: crash detection (try_wait), idle-exit timer,
//!   health pinger
//! - Crash recovery: exponential backoff, restart-attempt cap, DB state
//!   reconciliation (tasks→failed, bindings→crashed, logical status→warm/cold)
//! - Graceful shutdown: prepare_hibernate all → adapter.shutdown → 10s grace
//!   → SIGKILL
//! - Resource event writes (`sidecar_start` / `sidecar_stop` / `sidecar_crash`
//!   / `sidecar_restart`) when a DB handle is attached
//!
//! The supervisor is constructed as `Arc<Self>` so that `SidecarHandle` and
//! the background supervision task share ownership. RPC calls lock the state
//! mutex only long enough to clone the client `Arc` and bump `last_activity`;
//! the actual RPC runs with the state lock released, serialized on the
//! client's own `tokio::Mutex`.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{error, info, instrument, warn};

use busytok_store::{Database, SubagentResourceEventRow};

use crate::sidecar::client::SidecarRpcClient;
use crate::sidecar::config::SidecarConfig;
use crate::sidecar::protocol::PROTOCOL_VERSION;
use crate::sidecar::SidecarError;

/// Shared DB handle — `std::sync::Mutex` because the supervisor writes
/// resource events and crash-reconciliation synchronously (no `.await` held
/// across the lock). Mirrors the `SubagentManager` pattern.
pub type SharedDb = Arc<std::sync::Mutex<Database>>;

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

pub struct PiSidecarSupervisor {
    config: SidecarConfig,
    state: Mutex<SupervisorState>,
    db: Option<SharedDb>,
}

struct SupervisorState {
    child: Option<Child>,
    /// The RPC client is wrapped in `Arc<Mutex<…>>` so `call_rpc` can clone
    /// the Arc and release the state lock before performing the (potentially
    /// long) RPC call — avoids holding the state mutex across `.await`.
    client: Option<Arc<Mutex<SidecarRpcClient>>>,
    last_activity: tokio::time::Instant,
    restart_attempts: u32,
    /// Set true when the supervision loop is running; prevents double-spawn
    /// of the loop across concurrent `ensure_started` calls.
    supervision_started: bool,
}

impl PiSidecarSupervisor {
    pub fn new(config: SidecarConfig, db: Option<SharedDb>) -> Arc<Self> {
        Arc::new(Self {
            config,
            state: Mutex::new(SupervisorState {
                child: None,
                client: None,
                last_activity: tokio::time::Instant::now(),
                restart_attempts: 0,
                supervision_started: false,
            }),
            db,
        })
    }

    /// Lazy-spawn the sidecar if not running, then return a handle.
    /// If the sidecar crashed previously, applies exponential backoff before
    /// respawning (capped at `max_restart_attempts`).
    #[instrument(skip(self), fields(event_code = "subagent.sidecar.ensure_started"))]
    pub async fn ensure_started(self: &Arc<Self>) -> Result<SidecarHandle, SidecarError> {
        let needs_spawn = {
            let state = self.state.lock().await;
            state.client.is_none()
                || state
                    .child
                    .as_ref()
                    .map(|c| c.id().is_none())
                    .unwrap_or(true)
        };
        if needs_spawn {
            self.spawn_internal().await?;
        }
        Ok(SidecarHandle {
            supervisor: Arc::clone(self),
        })
    }

    async fn spawn_internal(self: &Arc<Self>) -> Result<(), SidecarError> {
        // Exponential backoff if this is a restart after a crash.
        let backoff = {
            let state = self.state.lock().await;
            // Double-checked locking: `ensure_started` checks `needs_spawn`
            // without holding the lock, so two concurrent callers can both see
            // `needs_spawn=true` and both call `spawn_internal`. Re-check here
            // (under the lock) so only the first caller actually spawns — the
            // second sees the child/client the first installed and returns
            // early. `child.id().is_some()` guards against the case where a
            // child was set but has since exited (id() returns None after
            // wait()/kill()).
            if state.client.is_some()
                && state
                    .child
                    .as_ref()
                    .map(|c| c.id().is_some())
                    .unwrap_or(false)
            {
                return Ok(());
            }
            if state.restart_attempts > self.config.max_restart_attempts {
                return Err(SidecarError::Crashed(format!(
                    "max restart attempts ({}) exceeded",
                    self.config.max_restart_attempts
                )));
            }
            if state.restart_attempts > 0 {
                let exp = 2u32.pow(state.restart_attempts - 1);
                self.config.restart_backoff_base * exp
            } else {
                Duration::ZERO
            }
        };
        if !backoff.is_zero() {
            warn!(
                event_code = "subagent.sidecar.restart_backoff",
                backoff_ms = backoff.as_millis() as u64,
                "sleeping before restart"
            );
            tokio::time::sleep(backoff).await;
        }

        let mut cmd = Command::new(&self.config.node_binary);
        cmd.arg(&self.config.bundle_path);
        cmd.envs(&self.config.env);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            error!(event_code = "subagent.sidecar.spawn_failed", error = %e);
            SidecarError::Spawn(e.to_string())
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SidecarError::Spawn("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SidecarError::Spawn("no stdout".into()))?;
        // Take stderr and spawn a background line-reader that forwards each
        // line to `tracing`. Without this the piped stderr buffer fills up
        // and blocks the child process — manifesting as random turn_auto /
        // health timeouts. The TS sidecar writes to stderr on error paths
        // (rpc.ts line handler / stop callback exceptions). The task is
        // detached: it exits naturally when the pipe closes (child exits).
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                loop {
                    match reader.next_line().await {
                        Ok(Some(line)) => {
                            warn!(
                                event_code = "subagent.sidecar.stderr",
                                "sidecar stderr: {line}"
                            );
                        }
                        Ok(None) => break, // EOF — child closed stderr
                        Err(e) => {
                            warn!(
                                event_code = "subagent.sidecar.stderr_read_error",
                                error = %e,
                                "stderr reader error"
                            );
                            break;
                        }
                    }
                }
            });
        }
        let mut client = SidecarRpcClient::new(stdin, stdout);
        let init = client
            .call(
                "adapter.initialize",
                serde_json::json!({"protocol_version": PROTOCOL_VERSION}),
            )
            .await?;
        let pv = init
            .get("protocol_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if pv != PROTOCOL_VERSION as u64 {
            return Err(SidecarError::Spawn(format!(
                "protocol mismatch: expected {PROTOCOL_VERSION}, got {pv}"
            )));
        }
        let is_restart = {
            let mut state = self.state.lock().await;
            let is_restart = state.restart_attempts > 0;
            state.child = Some(child);
            state.client = Some(Arc::new(Mutex::new(client)));
            state.last_activity = tokio::time::Instant::now();
            state.restart_attempts = 0; // reset on successful spawn
            if !state.supervision_started {
                state.supervision_started = true;
                let self_clone = Arc::clone(self);
                tokio::spawn(async move {
                    self_clone.supervision_loop().await;
                });
            }
            is_restart
        };
        info!(
            event_code = "subagent.sidecar.start",
            sidecar_version = init
                .get("sidecar_version")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
            is_restart,
            "sidecar initialized"
        );
        self.write_resource_event(if is_restart {
            "sidecar_restart"
        } else {
            "sidecar_start"
        });
        Ok(())
    }

    /// Background loop: crash watcher + health pinger + idle timer.
    /// Exits when the child is taken (shutdown) or crashes (handled, then
    /// exits — next `ensure_started` respawns and re-spawns the loop).
    async fn supervision_loop(self: Arc<Self>) {
        let mut last_health = tokio::time::Instant::now();
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            let mut state = self.state.lock().await;
            if state.child.is_none() {
                return; // shut down — loop exits
            }
            // --- crash detection (non-blocking try_wait) ---
            let crash_status = match state.child.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(status)) => Some(status),
                    Ok(None) => None,
                    Err(_) => None,
                },
                None => return,
            };
            if let Some(status) = crash_status {
                state.client = None;
                state.child = None;
                state.restart_attempts += 1;
                warn!(
                    event_code = "subagent.sidecar.crash",
                    exit = ?status,
                    attempts = state.restart_attempts,
                    "sidecar crashed"
                );
                drop(state);
                // Spec §3.3 + §5.4: converge DB state before returning so the
                // next ensure_started sees a consistent store. Failure to
                // reconcile is logged but does NOT block restart — a
                // half-converged store is recoverable on the next task; a
                // blocked restart is worse.
                self.reconcile_crash();
                self.write_resource_event("sidecar_crash");
                return; // loop exits; next ensure_started respawns
            }
            // --- idle exit timer ---
            // idle_exit_seconds=0 means "exit immediately when idle" (test-
            // friendly). A large value effectively disables idle exit.
            let idle_threshold = Duration::from_secs(self.config.idle_exit_seconds);
            let idle = state.last_activity.elapsed();
            if idle > idle_threshold {
                drop(state);
                info!(
                    event_code = "subagent.sidecar.idle_exit",
                    "idle exit triggered"
                );
                let _ = self.shutdown_internal().await;
                return;
            }
            // --- health pinger (best-effort; failures logged not fatal) ---
            if last_health.elapsed() >= self.config.health_interval {
                last_health = tokio::time::Instant::now();
                let client = state.client.clone();
                drop(state);
                if let Some(client) = client {
                    let _ = client
                        .lock()
                        .await
                        .call_with_timeout(
                            "adapter.health",
                            serde_json::json!({}),
                            Duration::from_secs(2),
                        )
                        .await
                        .map_err(|e| {
                            warn!(event_code = "subagent.sidecar.health_failed", error = %e);
                        });
                }
            }
        }
    }

    /// Converge DB state after a sidecar crash (spec §3.3 + §5.4). Calls
    /// `subagent_reconcile_sidecar_crash` with the harness name from config.
    /// Synchronous (no `.await` held across the DB lock).
    fn reconcile_crash(&self) {
        if let Some(db) = &self.db {
            let db = db.lock().expect("subagent db lock poisoned");
            match db.subagent_reconcile_sidecar_crash(&self.config.harness_name) {
                Ok(counts) => {
                    warn!(
                        event_code = "subagent.sidecar.crash_reconciled",
                        tasks_failed = counts.tasks_failed,
                        bindings_released = counts.bindings_released,
                        status_rolled_back = counts.status_rolled_back,
                        "sidecar crash reconciled"
                    );
                }
                Err(e) => {
                    warn!(
                        event_code = "subagent.sidecar.crash_reconcile_failed",
                        error = %e,
                        "crash reconciliation failed; store may be half-converged"
                    );
                }
            }
        }
    }

    /// Converge DB state after a graceful sidecar shutdown (spec §3.3).
    /// Releases hot bindings (`status='closed'`, NOT `'crashed'`) and rolls
    /// back logical subagent status to `warm`/`cold` for the affected set.
    /// Synchronous (no `.await` held across the DB lock) — the DB lock is a
    /// `std::sync::Mutex`, distinct from `self.state` (`tokio::sync::Mutex`);
    /// acquire it, run the sync store function, release it, with no `.await`
    /// in between.
    fn reconcile_shutdown(&self) {
        if let Some(db) = &self.db {
            let db = db.lock().expect("subagent db lock poisoned");
            match db.subagent_release_hot_bindings_for_shutdown(&self.config.harness_name) {
                Ok(counts) => {
                    info!(
                        event_code = "subagent.sidecar.shutdown_reconciled",
                        bindings_released = counts.bindings_released,
                        status_rolled_back = counts.status_rolled_back,
                        "sidecar shutdown reconciled"
                    );
                }
                Err(e) => {
                    warn!(
                        event_code = "subagent.sidecar.shutdown_reconcile_failed",
                        error = %e,
                        "shutdown reconciliation failed; store may be half-converged"
                    );
                }
            }
        }
    }

    /// Perform one RPC call. Locks state only to clone the client Arc and bump
    /// `last_activity`; the RPC itself runs with the state lock released.
    #[instrument(skip(self, params), fields(method = %method))]
    async fn call_rpc(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, SidecarError> {
        let client = {
            let mut state = self.state.lock().await;
            state.last_activity = tokio::time::Instant::now();
            state
                .client
                .clone()
                .ok_or_else(|| SidecarError::Crashed("sidecar not running".to_string()))?
        };
        // State lock released — RPC serialized on the client's own mutex.
        let mut guard = client.lock().await;
        guard
            .call_with_timeout(method, params, self.config.task_timeout)
            .await
    }

    /// Graceful shutdown: prepare_hibernate all → adapter.shutdown → 10s grace
    /// → SIGKILL. Emits `sidecar_stop` resource event.
    #[instrument(skip(self))]
    pub async fn shutdown(&self) -> Result<(), SidecarError> {
        self.shutdown_internal().await
    }

    async fn shutdown_internal(&self) -> Result<(), SidecarError> {
        let client = { self.state.lock().await.client.take() };
        if let Some(client) = &client {
            // Best-effort: ask the sidecar to prepare all hot sessions for
            // hibernate (Plan 3 tracks per-session state; Plan 2 uses `all`).
            // Plan 3: consume memory_delta from the response.
            let _ = client
                .lock()
                .await
                .call_with_timeout(
                    "session.prepare_hibernate",
                    serde_json::json!({"all": true}),
                    Duration::from_secs(5),
                )
                .await;
            // adapter.shutdown — sidecar should exit 0 after responding.
            let _ = client
                .lock()
                .await
                .call_with_timeout(
                    "adapter.shutdown",
                    serde_json::json!({}),
                    Duration::from_secs(5),
                )
                .await;
        }
        // Kill child with 10s grace (spec §5.4). The sidecar should have
        // exited on adapter.shutdown; this is the fallback.
        let child = { self.state.lock().await.child.take() };
        if let Some(mut child) = child {
            match tokio::time::timeout(SHUTDOWN_GRACE, child.wait()).await {
                Ok(Ok(_status)) => {}
                Ok(Err(_)) | Err(_) => {
                    warn!(
                        event_code = "subagent.sidecar.shutdown_kill",
                        "grace period expired or wait failed, SIGKILL"
                    );
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
            }
        }
        // Spec §3.3 end-to-end: after the worker process is dead, release hot
        // bindings (status='closed') and roll back logical status to warm/cold
        // so the store never says "hot" with no worker running. This mirrors
        // `reconcile_crash` but uses 'closed' (graceful) instead of 'crashed'.
        // Synchronous — no `.await` held across the DB lock.
        self.reconcile_shutdown();
        info!(event_code = "subagent.sidecar.stop", "sidecar shut down");
        self.write_resource_event("sidecar_stop");
        Ok(())
    }

    /// Write a row to `subagent_resource_events` if a DB handle is attached.
    /// No-op (but still logged at debug) in unit tests where `db` is `None`.
    fn write_resource_event(&self, event_type: &str) {
        if let Some(db) = &self.db {
            if let Ok(db) = db.lock() {
                let now = busytok_domain::now_ms();
                let _ = db.subagent_insert_resource_event(&SubagentResourceEventRow {
                    id: format!("re_{}", uuid::Uuid::new_v4()),
                    event_type: event_type.to_string(),
                    target_id: None,
                    rss_mb: None,
                    cpu_percent: None,
                    detail_json: None,
                    created_at_ms: now,
                });
            }
        }
    }
}

pub struct SidecarHandle {
    supervisor: Arc<PiSidecarSupervisor>,
}

impl SidecarHandle {
    pub async fn health(&self) -> Result<serde_json::Value, SidecarError> {
        self.supervisor
            .call_rpc("adapter.health", serde_json::json!({}))
            .await
    }

    pub async fn turn_auto(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, SidecarError> {
        self.supervisor.call_rpc("session.turn_auto", params).await
    }

    /// Prepare a specific session for hibernate (spec §4.4 eviction flow).
    /// Returns `{ memory_delta, stats }`.
    pub async fn prepare_hibernate(
        &self,
        adapter_session_id: &str,
    ) -> Result<serde_json::Value, SidecarError> {
        self.supervisor
            .call_rpc(
                "session.prepare_hibernate",
                serde_json::json!({ "adapter_session_id": adapter_session_id }),
            )
            .await
    }

    /// Close a session (spec §4.4 eviction flow, final step).
    pub async fn close(&self, adapter_session_id: &str) -> Result<serde_json::Value, SidecarError> {
        self.supervisor
            .call_rpc(
                "session.close",
                serde_json::json!({ "adapter_session_id": adapter_session_id }),
            )
            .await
    }
}
