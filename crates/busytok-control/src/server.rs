//! Local IPC server with length-prefixed JSON frames.
//!
//! Framing: 4-byte big-endian length prefix + JSON payload.
//! The server listens on a platform transport endpoint. Clients connect, send
//! a hello handshake, then make RPC calls.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::{Context, Result};
use busytok_events::AppEvent;
use busytok_protocol::dto::*;
use tokio::sync::watch;
use tracing::{debug, error, info, Instrument};

/// Default per-request dispatch timeout (30 s).
const REQUEST_DISPATCH_TIMEOUT: Duration = Duration::from_secs(30);

use crate::dispatch::{control_response_from_error, ControlDispatcher, RuntimeControl};
use crate::protocol::{read_frame, write_frame, HELLO, HELLO_ACK};
use crate::transport::{ControlTransport, PlatformTransport};

/// Local IPC control server.
pub struct ControlServer<T: ControlTransport = PlatformTransport> {
    listener: Arc<T::Listener>,
    endpoint: String,
    runtime: Arc<dyn RuntimeControl>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    connections: tokio::sync::Mutex<tokio::task::JoinSet<()>>,
}

impl<T: ControlTransport> ControlServer<T> {
    /// Bind to the given transport endpoint.
    pub async fn bind(endpoint: impl AsRef<str>, runtime: Arc<dyn RuntimeControl>) -> Result<Self> {
        let endpoint = endpoint.as_ref().to_string();
        let listener = Arc::new(T::bind(&endpoint).await?);
        info!(event_code = "control.server.bound", endpoint = %endpoint);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Ok(Self {
            listener,
            endpoint,
            runtime,
            shutdown_tx,
            shutdown_rx,
            connections: tokio::sync::Mutex::new(tokio::task::JoinSet::new()),
        })
    }

    /// Spawn a test server on a unique endpoint.
    pub async fn spawn_for_test(
        runtime: Arc<dyn RuntimeControl>,
    ) -> Result<(Self, String)> {
        let endpoint = unique_test_endpoint();
        let server = Self::bind(&endpoint, runtime).await?;
        Ok((server, endpoint))
    }

    /// Run the accept loop. Returns when [`shutdown`] is called.
    pub async fn run(&self) -> Result<()> {
        let mut rx = self.shutdown_rx.clone();
        // Monotonic counter for subscription connection IDs.
        static SUBSCRIPTION_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

        loop {
            tokio::select! {
                result = T::accept(&self.listener) => {
                    let stream = result.context("accepting connection")?;
                    let dispatcher = ControlDispatcher::from_arc(Arc::clone(&self.runtime));
                    let conn_shutdown = self.shutdown_rx.clone();
                    let client_id = format!(
                        "sub-{}",
                        SUBSCRIPTION_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
                    );

                    self.connections.lock().await.spawn(async move {
                        let result = handle_connection(
                            stream, dispatcher, conn_shutdown, &client_id,
                        ).await;
                        match result {
                            Ok(()) => {
                                info!(%client_id, "client disconnected");
                            }
                            Err(e) => {
                                error!(%client_id, error = %e, "Connection error");
                            }
                        }
                    });
                }
                _ = rx.changed() => {
                    info!(event_code = "control.server.shutdown");
                    break;
                }
            }
        }
        Ok(())
    }

    /// Signal the server to shut down gracefully.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Wait for all in-flight connection handlers to finish.
    pub async fn await_drain(&self) {
        while self.connections.lock().await.join_next().await.is_some() {}
    }

    /// Accept a single connection (useful for testing).
    ///
    /// Single-shot debug helper; safe for test use. The hardcoded client_id
    /// (`test-sub-0`) is acceptable because this entry point is intended for
    /// one-off diagnostics, not for the production accept loop which generates
    /// monotonic subscription IDs in [`ControlServer::run`].
    pub async fn accept_one(&self) -> Result<()> {
        let stream = T::accept(&self.listener)
            .await
            .context("accepting connection")?;
        let dispatcher = ControlDispatcher::from_arc(Arc::clone(&self.runtime));
        let conn_shutdown = self.shutdown_rx.clone();
        let client_id = "test-sub-0".to_string();
        handle_connection(stream, dispatcher, conn_shutdown, &client_id).await
    }

    /// Return the transport endpoint.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl<T: ControlTransport> Drop for ControlServer<T> {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.endpoint);
        }
        #[cfg(windows)]
        {
            // Named pipes have no filesystem path.
        }
    }
}

fn unique_test_endpoint() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.keep().join(format!("busytok-test-{pid}-{n}.sock"));
        path.display().to_string()
    }
    #[cfg(windows)]
    {
        format!(r"\\.\pipe\busytok-test-{pid}-{n}")
    }
    #[cfg(not(any(unix, windows)))]
    {
        format!("in-memory-test-{pid}-{n}")
    }
}

fn is_connection_closed(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let text = cause.to_string().to_ascii_lowercase();
        if text.contains("reading frame length")
            || text.contains("early eof")
            || text.contains("unexpected eof")
            || text.contains("0 bytes")
        {
            return true;
        }
        cause
            .downcast_ref::<std::io::Error>()
            .map(|io| {
                matches!(
                    io.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::NotConnected
                )
            })
            .unwrap_or(false)
    })
}

/// Handle a single client connection.
async fn handle_connection<S>(
    stream: S,
    dispatcher: ControlDispatcher,
    mut shutdown_rx: watch::Receiver<bool>,
    client_id: &str,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut buf = Vec::new();

    // Read hello handshake.
    let hello_frame = match read_frame(&mut reader, &mut buf).await {
        Ok(frame) => frame,
        Err(e) => {
            debug!(error = %e, "Client disconnected before handshake");
            return Ok(());
        }
    };
    if hello_frame != HELLO {
        anyhow::bail!("invalid handshake: expected '{HELLO}', got '{hello_frame}'");
    }
    // Send hello acknowledgment.
    write_frame(&mut writer, HELLO_ACK)
        .await
        .context("writing hello ack")?;

    let client_id_owned = client_id.to_string();

    loop {
        // Race the next inbound frame against the server-wide shutdown signal
        // so that connection handlers terminate promptly when `ControlServer::shutdown`
        // fires, even if the client never sends another frame or closes the socket.
        // Without this, `await_drain` would block forever waiting for handlers
        // whose clients are idle but still connected.
        let payload = tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                debug!("Connection handler shutting down");
                break;
            }
            frame_result = read_frame(&mut reader, &mut buf) => match frame_result {
                Ok(p) => p,
                Err(e) => {
                    if is_connection_closed(&e) {
                        debug!("Client disconnected");
                        break;
                    }
                    return Err(e.context("reading frame"));
                }
            },
        };

        let request: ControlRequest = serde_json::from_str(&payload).context("parsing request")?;

        if request.method == "events.subscribe" {
            tracing::info!(
                client_id = %client_id_owned,
                method = %request.method,
                session_id = request.meta.session_id.as_deref().unwrap_or(""),
                correlation_id = request.meta.correlation_id.as_deref().unwrap_or(""),
                "accepted event subscription request"
            );
            let _ = dispatcher
                .event_bus()
                .publish_ephemeral(AppEvent::SubscriptionConnected {
                    client_id: Some(client_id_owned.clone()),
                });
            dispatcher.record_diagnostic(
                "info",
                "subscription_connected",
                &format!("client {} connected", client_id_owned),
            );
            // For subscribe, set up a subscription stream.
            let filter_types: Vec<String> = request
                .params
                .get("types")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            // Extract client's last_event_seq for gap detection.
            let client_last_seq: Option<i64> = request
                .params
                .get("last_event_seq")
                .and_then(|v| v.as_i64());

            let mut rx = dispatcher.event_bus().subscribe();
            // Acknowledge subscription.
            let ack = ControlResponse::ok(serde_json::json!({"subscribed": true}));
            let ack_json = serde_json::to_string(&ack)?;
            write_frame(&mut writer, &ack_json).await?;

            // If the client provided a last_event_seq and the server has
            // advanced past it, emit a gap notification with the current
            // sequence state so the client can invalidate affected scopes.
            if let Some(client_seq) = client_last_seq {
                if let Some(current_seq) = dispatcher.latest_event_seq() {
                    if current_seq > client_seq {
                        let gap_dto = EventSubscriptionBatchDto {
                            events: vec![RuntimeEventDto {
                                event_type: "data:gap_detected".to_string(),
                                payload: serde_json::json!({
                                    "last_event_id": current_seq,
                                    "sequence_gap": true,
                                    "last_seen_seq": client_seq,
                                    "gap_size": current_seq - client_seq,
                                }),
                                event_seq: Some(current_seq),
                                ephemeral: true,
                                scopes: canonical_invalidation_scopes(),
                                generation_id: None,
                                watermark_ms: Some(busytok_domain::now_ms()),
                                is_exact: false,
                            }],
                        };
                        let gap_json = serde_json::to_string(&gap_dto)?;
                        let _ = write_frame(&mut writer, &gap_json).await;
                    }
                }
            }

            let sub_span = tracing::info_span!(
                "event_subscription",
                session_id = request.meta.session_id.as_deref().unwrap_or(""),
                correlation_id = request.meta.correlation_id.as_deref().unwrap_or(""),
            );

            // Stream events to the client.
            async {
                loop {
                tokio::select! {
                    result = rx.recv() => {
                        match result {
                            Ok(published) => {
                                let event_type = published.event.event_type().to_string();
                                if !filter_types.is_empty() && !filter_types.contains(&event_type) {
                                    continue;
                                }
                                // Extract flat payload from the enum variant so the
                                // frontend receives the inner data directly.
                                let payload = event_payload_json(&published.event);
                                let is_ephemeral = published.event.is_ephemeral();
                                let is_exact = !is_ephemeral
                                    && published.generation_id.is_some();
                                let dto = RuntimeEventDto {
                                    event_type: event_type.clone(),
                                    payload,
                                    event_seq: published.event_seq,
                                    ephemeral: is_ephemeral,
                                    scopes: published.scopes.clone(),
                                    generation_id: published.generation_id.clone(),
                                    watermark_ms: published.watermark_ms,
                                    is_exact,
                                };
                                let batch = EventSubscriptionBatchDto {
                                    events: vec![dto],
                                };
                                let batch_json = serde_json::to_string(&batch)?;
                                if write_frame(&mut writer, &batch_json).await.is_err() {
                                    // Client disconnected during write.
                                    let _ = dispatcher.event_bus().publish_ephemeral(
                                        AppEvent::SubscriptionDisconnected {
                                            client_id: Some(client_id_owned.clone()),
                                            reason: Some("write_error".to_string()),
                                        },
                                    );
                                    dispatcher.record_diagnostic(
                                        "info",
                                        "subscription_disconnected",
                                        &format!("client {} write error", client_id_owned),
                                    );
                                    break;
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                debug!("Subscription lagged by {n} events");
                                continue;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                break;
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        debug!("Subscription loop shutting down");
                        break;
                    }
                }
            }
                #[allow(unreachable_code)]
                Ok::<(), anyhow::Error>(())
            }.instrument(sub_span).await?;

            break;
        }

        let request_method = request.method.clone();
        let session_id = request.meta.session_id.clone().unwrap_or_default();
        let correlation_id = request.meta.correlation_id.clone().unwrap_or_default();
        let dispatch_span = tracing::info_span!(
            "control_dispatch",
            method = %request_method,
            session_id = %session_id,
            correlation_id = %correlation_id,
        );
        tracing::info!(
            client_id = %client_id_owned,
            method = %request_method,
            session_id = %session_id,
            correlation_id = %correlation_id,
            "dispatching control request"
        );

        let started = Instant::now();
        let response = tokio::time::timeout(
            REQUEST_DISPATCH_TIMEOUT,
            dispatcher.dispatch(request).instrument(dispatch_span),
        )
        .await
        .unwrap_or_else(|_elapsed| {
            Ok(ControlResponse::err(
                "dispatch_timeout",
                "request exceeded dispatch timeout",
            ))
        })
        .unwrap_or_else(control_response_from_error);
        let response_json = serde_json::to_string(&response)?;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let (status, error_code) = match &response {
            ControlResponse::Ok(_) => ("ok", ""),
            ControlResponse::Err(err) => ("error", err.code.as_str()),
        };
        tracing::info!(
            method = %request_method,
            session_id = %session_id,
            correlation_id = %correlation_id,
            elapsed_ms,
            status,
            error_code,
            payload_bytes = response_json.len(),
            "control.dispatch.completed"
        );
        write_frame(&mut writer, &response_json)
            .await
            .context("writing response")?;
    }

    Ok(())
}

/// Extract the payload JSON from an `AppEvent` variant.
///
/// Returns the flat inner data for each variant so the frontend does not
/// need to handle the externally-tagged enum wrapper.
fn event_payload_json(event: &AppEvent) -> serde_json::Value {
    match event {
        AppEvent::DataInvalidated { datasets } => {
            serde_json::json!({ "datasets": datasets })
        }
        AppEvent::LiveSample {
            bucket_start_ms,
            tokens_per_sec,
            cost_per_sec,
            events_per_sec,
            transient,
        } => {
            serde_json::json!({
                "bucket_start_ms": bucket_start_ms,
                "tokens_per_sec": tokens_per_sec,
                "cost_per_sec": cost_per_sec,
                "events_per_sec": events_per_sec,
                "transient": transient,
            })
        }
        AppEvent::SubscriptionConnected { client_id } => {
            serde_json::json!({
                "client_id": client_id,
            })
        }
        AppEvent::SubscriptionDisconnected { client_id, reason } => {
            serde_json::json!({
                "client_id": client_id,
                "reason": reason,
            })
        }
        AppEvent::SubscriptionReconnectFailed {
            attempts,
            last_error,
        } => {
            serde_json::json!({
                "attempts": attempts,
                "last_error": last_error,
            })
        }
        AppEvent::WriterQueueThreshold {
            queue_depth,
            threshold,
            severity,
        } => {
            serde_json::json!({
                "queue_depth": queue_depth,
                "threshold": threshold,
                "severity": severity,
            })
        }
        AppEvent::WriterLagThreshold {
            lag_ms,
            threshold,
            severity,
        } => {
            serde_json::json!({
                "lag_ms": lag_ms,
                "threshold": threshold,
                "severity": severity,
            })
        }
        other => serde_json::to_value(other).unwrap_or_default(),
    }
}
