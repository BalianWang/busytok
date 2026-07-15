//! Concurrent JSON-RPC 2.0 client over a newline-delimited stdio stream.
//!
//! A sidecar owns multiple logical subagent sessions and may process their
//! turns concurrently. The client therefore separates request writes from
//! response reads: one reader task dispatches responses by JSON-RPC id while
//! callers only share a short-lived stdin lock for each write.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{oneshot, Mutex as AsyncMutex, Notify};
use tokio::time::timeout;
use tracing::debug;

use crate::sidecar::protocol::{SidecarRequest, SidecarResponse};
use crate::sidecar::SidecarError;

const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(30);

type PendingSender = oneshot::Sender<Result<serde_json::Value, SidecarError>>;
type Pending = Arc<Mutex<HashMap<u64, PendingSender>>>;

/// JSON-RPC client that supports concurrent in-flight requests on one
/// sidecar process. JSON-RPC responses may arrive out of order, so every
/// caller waits on a request-specific oneshot channel.
pub struct SidecarRpcClient {
    stdin: AsyncMutex<ChildStdin>,
    next_id: AtomicU64,
    pending: Pending,
    active_calls: Arc<AtomicUsize>,
    idle_notify: Arc<Notify>,
    closing: Arc<std::sync::atomic::AtomicBool>,
    closed: Arc<std::sync::atomic::AtomicBool>,
}

struct CallGuard {
    active_calls: Arc<AtomicUsize>,
    idle_notify: Arc<Notify>,
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        if self.active_calls.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.idle_notify.notify_waiters();
        }
    }
}

struct PendingGuard {
    pending: Pending,
    id: u64,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.pending
            .lock()
            .expect("sidecar pending lock poisoned")
            .remove(&self.id);
    }
}

impl SidecarRpcClient {
    /// Construct a client and start its response-dispatch reader task.
    pub fn new(stdin: ChildStdin, stdout: ChildStdout) -> Self {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let reader_pending = Arc::clone(&pending);
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader_closed = Arc::clone(&closed);
        tokio::spawn(async move {
            read_responses(stdout, reader_pending, reader_closed).await;
        });
        Self {
            stdin: AsyncMutex::new(stdin),
            next_id: AtomicU64::new(1),
            pending,
            active_calls: Arc::new(AtomicUsize::new(0)),
            idle_notify: Arc::new(Notify::new()),
            closing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            closed,
        }
    }

    fn begin_call(&self, allow_closing: bool) -> Result<CallGuard, SidecarError> {
        if (!allow_closing && self.closing.load(Ordering::Acquire))
            || self.closed.load(Ordering::Acquire)
        {
            return Err(SidecarError::Crashed(
                "sidecar RPC client is closed".to_string(),
            ));
        }
        self.active_calls.fetch_add(1, Ordering::AcqRel);
        // Close admission atomically with respect to the active-call count:
        // shutdown may race a caller that already cloned the client Arc.
        if (!allow_closing && self.closing.load(Ordering::Acquire))
            || self.closed.load(Ordering::Acquire)
        {
            if self.active_calls.fetch_sub(1, Ordering::AcqRel) == 1 {
                self.idle_notify.notify_waiters();
            }
            return Err(SidecarError::Crashed(
                "sidecar RPC client is closed".to_string(),
            ));
        }
        Ok(CallGuard {
            active_calls: Arc::clone(&self.active_calls),
            idle_notify: Arc::clone(&self.idle_notify),
        })
    }

    /// Close admission and wait until all already-admitted calls settle. This
    /// lets the supervisor keep turns concurrent while ensuring graceful
    /// shutdown never races `adapter.shutdown` with a late request.
    pub async fn close_for_shutdown(&self) {
        self.closing.store(true, Ordering::Release);
        loop {
            let notified = self.idle_notify.notified();
            if self.active_calls.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    /// Send a JSON-RPC request and wait for its matching response.
    pub async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, SidecarError> {
        self.call_with_timeout(method, params, DEFAULT_CALL_TIMEOUT)
            .await
    }

    pub async fn call_with_timeout(
        &self,
        method: &str,
        params: serde_json::Value,
        dur: Duration,
    ) -> Result<serde_json::Value, SidecarError> {
        self.call_with_timeout_inner(method, params, dur, false)
            .await
    }

    /// Send a shutdown control RPC after normal call admission has been
    /// closed. Only the supervisor uses this once all in-flight turns have
    /// drained; it is deliberately separate so ordinary callers cannot race
    /// a graceful shutdown.
    pub async fn call_for_shutdown(
        &self,
        method: &str,
        params: serde_json::Value,
        dur: Duration,
    ) -> Result<serde_json::Value, SidecarError> {
        self.call_with_timeout_inner(method, params, dur, true)
            .await
    }

    async fn call_with_timeout_inner(
        &self,
        method: &str,
        params: serde_json::Value,
        dur: Duration,
        allow_closing: bool,
    ) -> Result<serde_json::Value, SidecarError> {
        let _call_guard = self.begin_call(allow_closing)?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = SidecarRequest::new(method, params, id);
        let mut line = serde_json::to_string(&req)
            .map_err(|e| SidecarError::Rpc(format!("serialize: {e}")))?;
        line.push('\n');

        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .expect("sidecar pending lock poisoned")
            .insert(id, sender);
        let _pending_guard = PendingGuard {
            pending: Arc::clone(&self.pending),
            id,
        };

        // The reader may have observed EOF after `begin_call()` but before
        // this request was inserted. Re-check after insertion so a late
        // caller fails immediately instead of writing to a dead sidecar and
        // waiting for the full request timeout.
        if self.closed.load(Ordering::Acquire) {
            self.remove_pending(id);
            return Err(SidecarError::Crashed(
                "sidecar RPC client is closed".to_string(),
            ));
        }

        // Serialize only writes. The response reader remains independent, so
        // a slow request never prevents another request from being sent.
        let write_result = {
            let mut stdin = self.stdin.lock().await;
            match stdin.write_all(line.as_bytes()).await {
                Ok(()) => stdin.flush().await,
                Err(e) => Err(e),
            }
        };
        if let Err(e) = write_result {
            self.remove_pending(id);
            return Err(SidecarError::Io(e.to_string()));
        }

        match timeout(dur, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(SidecarError::Crashed(
                "sidecar response dispatcher stopped".to_string(),
            )),
            Err(_) => {
                self.remove_pending(id);
                Err(SidecarError::Timeout(method.to_string()))
            }
        }
    }

    fn remove_pending(&self, id: u64) {
        self.pending
            .lock()
            .expect("sidecar pending lock poisoned")
            .remove(&id);
    }
}

async fn read_responses(
    stdout: ChildStdout,
    pending: Pending,
    closed: Arc<std::sync::atomic::AtomicBool>,
) {
    let mut reader = BufReader::new(stdout).lines();
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => dispatch_line(&line, &pending),
            Ok(None) => {
                closed.store(true, Ordering::Release);
                fail_pending(&pending, "sidecar stdout closed".to_string(), |message| {
                    SidecarError::Crashed(message)
                });
                return;
            }
            Err(e) => {
                closed.store(true, Ordering::Release);
                let message = e.to_string();
                fail_pending(&pending, message.clone(), |message| {
                    SidecarError::Io(message)
                });
                debug!(
                    event_code = "subagent.sidecar.client.reader_failed",
                    error = %message,
                    "sidecar response reader stopped"
                );
                return;
            }
        }
    }
}

fn dispatch_line(line: &str, pending: &Pending) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    let val: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(e) => {
            debug!(
                event_code = "subagent.sidecar.client.parse_skipped",
                error = %e,
                "skipping unparseable line"
            );
            return;
        }
    };

    // Notifications have no id and are intentionally not consumed by a
    // caller. They may be added to a future notification sink.
    if val.get("method").is_some() && val.get("id").is_none() {
        debug!(
            event_code = "subagent.sidecar.client.notification_skipped",
            method = %val["method"],
            "sidecar notification skipped (not consumed in MVP)"
        );
        return;
    }

    let Some(id) = val.get("id").and_then(|v| v.as_u64()) else {
        debug!(
            event_code = "subagent.sidecar.client.response_without_id",
            "skipping sidecar response without an id"
        );
        return;
    };
    let Some(sender) = pending
        .lock()
        .expect("sidecar pending lock poisoned")
        .remove(&id)
    else {
        debug!(
            event_code = "subagent.sidecar.client.unknown_response_id",
            id, "skipping response for an expired or unknown request"
        );
        return;
    };

    let response: Result<serde_json::Value, SidecarError> =
        serde_json::from_value::<SidecarResponse>(val)
            .map_err(|e| SidecarError::Rpc(format!("deserialize: {e}")))
            .and_then(|resp| {
                if let Some(err) = resp.error {
                    Err(SidecarError::Application(err.code, err.message, err.data))
                } else {
                    Ok(resp.result.unwrap_or(serde_json::Value::Null))
                }
            });
    let _ = sender.send(response);
}

fn fail_pending<F>(pending: &Pending, message: String, make_error: F)
where
    F: Fn(String) -> SidecarError,
{
    let senders = pending
        .lock()
        .expect("sidecar pending lock poisoned")
        .drain()
        .map(|(_, sender)| sender)
        .collect::<Vec<_>>();
    for sender in senders {
        let _ = sender.send(Err(make_error(message.clone())));
    }
}
