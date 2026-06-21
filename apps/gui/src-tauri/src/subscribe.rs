//! Subscription bridge -- sole owner of the event subscription Unix socket
//! connection. Bridges events to the frontend via Tauri's event system.
//!
//! On reconnect, the bridge fetches `shell.status` to compare the current
//! `latest_event_seq` with the stored `last_seen` event_seq. If events were
//! missed, it invalidates affected queries and reloads `live.window` before
//! resuming the event stream.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::desktop_service_status::{ServiceBootstrapState, ServiceStatusEvent};
use crate::lifecycle_coordinator::{LifecycleCause, LifecycleCoordinator};
use crate::service_recovery::connect_with_service_recovery;
use busytok_control::client::ControlClient;
use busytok_control::transport::PlatformTransport;
use busytok_protocol::dto::{
    canonical_invalidation_scopes, ControlRequest, ControlResponse, InvalidationScopeDto,
    RequestMeta,
};
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::watch;
use tracing::{info, warn};

/// Fetch the coordinator from Tauri state if available. Callers build
/// the bootstrap callback inline so the future type matches what
/// `connect_with_service_recovery` expects.
fn coordinator_from_state(app_handle: &AppHandle) -> Option<std::sync::Arc<LifecycleCoordinator>> {
    app_handle
        .try_state::<std::sync::Arc<LifecycleCoordinator>>()
        .map(|s| s.inner().clone())
}

/// Invoke a control method, routing socket-recovery bootstrap through the
/// coordinator when available.
async fn invoke_via_coordinator(
    app_handle: &AppHandle,
    method: &str,
    params: serde_json::Value,
    socket_path: &str,
) -> Result<serde_json::Value, String> {
    use crate::host_application_services::invoke_busytok_via_socket_with_bootstrap;
    if let Some(coordinator) = coordinator_from_state(app_handle) {
        invoke_busytok_via_socket_with_bootstrap(
            method,
            params,
            socket_path,
            RequestMeta::default(),
            || async move {
                coordinator
                    .ensure_running(LifecycleCause::CliInvocation)
                    .await
                    .map_err(|e| format!("service bootstrap failed: {e}"))?;
                Ok(())
            },
        )
        .await
    } else {
        // No coordinator: direct connect attempt only.
        let mut client = ControlClient::<PlatformTransport>::connect(socket_path)
            .await
            .map_err(|e| format!("service unavailable: {e}"))?;
        let req = ControlRequest::with_meta(method, params, RequestMeta::default());
        let resp = client
            .call(req)
            .await
            .map_err(|e| format!("dispatch error: {e}"))?;
        match resp {
            ControlResponse::Ok(v) => Ok(v),
            ControlResponse::Err(err) => Err(format!("[{}] {}", err.code, err.message)),
        }
    }
}

/// Emit an event to the webview, logging any errors instead of silently
/// discarding them.
fn emit_event<T: Serialize + Clone>(app_handle: &AppHandle, name: &str, payload: &T) {
    if let Err(e) = app_handle.emit(name, payload) {
        warn!("failed to emit {name}: {e}");
    }
}

#[derive(Debug, Clone, Serialize)]
struct SubscriptionStatusEvent {
    status: String, // "connected" | "disconnected" | "reconnecting"
    since_ms: u64,
    /// Latest event sequence on the server; used for gap detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_event_seq: Option<i64>,
    /// The client's last seen sequence before reconnect.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen_seq: Option<i64>,
    /// Whether a sequence gap was detected.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    gap_detected: bool,
    /// Scopes replayed during recovery — same shape as `InvalidationScopeDto`
    /// so the frontend can iterate them with its existing type definitions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    replayed_scopes: Vec<InvalidationScopeDto>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Start the subscription bridge. Spawned once at app startup in setup().
/// Returns a shutdown sender.
pub fn start_subscription_bridge(
    app_handle: AppHandle,
    socket_path: String,
) -> watch::Sender<bool> {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    // Track the last seen event_seq for reconnect recovery.
    let last_seen_seq: Arc<AtomicI64> = Arc::new(AtomicI64::new(0));

    tauri::async_runtime::spawn(async move {
        let mut backoff = 1u64;

        loop {
            emit_event(
                &app_handle,
                "busytok:subscription-status",
                &SubscriptionStatusEvent {
                    status: "reconnecting".into(),
                    since_ms: now_ms(),
                    latest_event_seq: None,
                    last_seen_seq: None,
                    gap_detected: false,
                    replayed_scopes: vec![],
                },
            );

            match connect_and_subscribe(&app_handle, &socket_path, &last_seen_seq).await {
                Ok(()) => {
                    backoff = 1;
                    emit_event(
                        &app_handle,
                        "busytok:subscription-status",
                        &SubscriptionStatusEvent {
                            status: "disconnected".into(),
                            since_ms: now_ms(),
                            latest_event_seq: None,
                            last_seen_seq: None,
                            gap_detected: false,
                            replayed_scopes: vec![],
                        },
                    );
                }
                Err(e) => {
                    warn!("Subscription connection error: {e}");
                    emit_event(
                        &app_handle,
                        "busytok:subscription-status",
                        &SubscriptionStatusEvent {
                            status: "disconnected".into(),
                            since_ms: now_ms(),
                            latest_event_seq: None,
                            last_seen_seq: None,
                            gap_detected: false,
                            replayed_scopes: vec![],
                        },
                    );
                }
            }

            if backoff >= 30 {
                emit_event(
                    &app_handle,
                    "busytok:service-status",
                    &ServiceStatusEvent {
                        status: ServiceBootstrapState::Unavailable,
                        since_ms: now_ms(),
                        reason: Some("service unreachable after multiple retries".into()),
                    },
                );
            }

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(backoff)) => {
                    backoff = (backoff * 2).min(30);
                }
                _ = shutdown_rx.changed() => {
                    info!("Subscription bridge received shutdown");
                    break;
                }
            }
        }
    });

    shutdown_tx
}

/// Perform reconnect recovery checks before resuming the event stream.
///
/// Fetches `shell.status`, compares `latest_event_seq` with our stored
/// `last_seen`, and if events were missed, emits `DataInvalidated` for the
/// affected datasets and reloads `live.window`.
///
/// Returns `(gap_detected, latest_event_seq, prev_seq, client_last_seq)` for
/// use in the subscription and connection-status event.
async fn recover_after_reconnect(
    app_handle: &AppHandle,
    socket_path: &str,
    last_seen_seq: &Arc<AtomicI64>,
) -> (bool, Option<i64>, Option<i64>, Option<i64>) {
    let stored_last = last_seen_seq.load(Ordering::SeqCst);

    // Fetch shell.status to get current latest_event_seq.
    match invoke_via_coordinator(app_handle, "shell.status", json!({}), socket_path).await {
        Ok(payload) => {
            let current_seq = payload
                .get("latest_event_seq")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

            // Detect both forward gaps (events missed during disconnect) and
            // backward gaps (service restarted, sequence counter reset).
            let gap = classify_gap(stored_last, current_seq);

            if gap.detected {
                let direction = if gap.service_restarted {
                    "restart"
                } else {
                    "missed"
                };
                info!(
                    stored_last,
                    current_seq,
                    gap = (current_seq as i128 - stored_last as i128).abs(),
                    direction,
                    "reconnect: sequence gap, invalidating affected queries"
                );

                // Reload live.window to refresh real-time data.
                if let Ok(window_payload) = invoke_via_coordinator(
                    app_handle,
                    "live.window",
                    json!({"window_seconds": 900}),
                    socket_path,
                )
                .await
                {
                    emit_event(
                        app_handle,
                        "busytok:event",
                        &serde_json::json!({
                            "events": [{
                                "event_type": "live:window_reloaded",
                                "payload": window_payload,
                            }]
                        }),
                    );
                }
            }

            // Update our stored seq to the current value.
            // Stored before the subscription is established — if the
            // subscription subsequently fails, the next reconnect will
            // see no gap (current_seq was already updated to the
            // server's real value), which is the correct behavior.
            last_seen_seq.store(current_seq, Ordering::SeqCst);

            // On service restart, pass prev_seq = None so the subscription
            // starts fresh (no stale sequence comparison on either side).
            // On forward gaps, pass the old prev_seq so the server can
            // emit its own gap_detected event.
            let prev_seq = if gap.service_restarted {
                None
            } else if stored_last > 0 {
                Some(stored_last)
            } else {
                None
            };
            // Always expose client_last_seq for the status event (frontend
            // requires last_seen_seq != null to enter the recovery branch).
            let client_last_seq = if stored_last > 0 {
                Some(stored_last)
            } else {
                None
            };
            (gap.detected, Some(current_seq), prev_seq, client_last_seq)
        }
        Err(e) => {
            warn!("reconnect recovery: failed to fetch shell.status: {e}");
            (false, None, None, None)
        }
    }
}

async fn connect_and_subscribe(
    app_handle: &AppHandle,
    socket_path: &str,
    last_seen_seq: &Arc<AtomicI64>,
) -> Result<(), String> {
    let coordinator = coordinator_from_state(app_handle);
    let client_result: Result<ControlClient<PlatformTransport>, String> = if let Some(c) = coordinator {
        tokio::time::timeout(
            Duration::from_secs(10),
            connect_with_service_recovery(socket_path, || async move {
                c.ensure_running(LifecycleCause::CliInvocation)
                    .await
                    .map_err(|e| format!("service bootstrap failed: {e}"))?;
                Ok(())
            }),
        )
        .await
        .map_err(|_| "connect/bootstrap phase timed out".to_string())?
    } else {
        tokio::time::timeout(
            Duration::from_secs(10),
            ControlClient::<PlatformTransport>::connect(socket_path),
        )
        .await
        .map_err(|_| "connect phase timed out".to_string())?
        .map_err(|e| format!("service unavailable: {e}"))
    };
    let mut client = client_result?;

    // Perform reconnect recovery checks before subscribing.
    let (gap_detected, latest_event_seq, prev_seq, client_last_seq) =
        recover_after_reconnect(app_handle, socket_path, last_seen_seq).await;

    let filter_types = vec![
        "usage:event_inserted".to_string(),
        "data:invalidated".to_string(),
        "live:sample".to_string(),
    ];

    let meta = busytok_protocol::dto::RequestMeta {
        session_id: Some(crate::logging::tauri_session_id().to_string()),
        correlation_id: Some(format!(
            "subscription-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        )),
    };

    let ack = client
        .subscribe_with_meta_and_last_event_seq(filter_types, meta, prev_seq)
        .await
        .map_err(|e| format!("subscribe failed: {e}"))?;

    if let busytok_protocol::dto::ControlResponse::Ok(_) = ack {
        emit_event(
            app_handle,
            "busytok:subscription-status",
            &SubscriptionStatusEvent {
                status: "connected".into(),
                since_ms: now_ms(),
                latest_event_seq,
                last_seen_seq: client_last_seq,
                gap_detected,
                replayed_scopes: if gap_detected {
                    canonical_invalidation_scopes()
                } else {
                    vec![]
                },
            },
        );
        emit_event(
            app_handle,
            "busytok:service-status",
            &ServiceStatusEvent {
                status: ServiceBootstrapState::Ready,
                since_ms: now_ms(),
                reason: None,
            },
        );
    } else {
        return Err(format!("unexpected subscribe ack: {:?}", ack));
    }

    info!("Subscription established, streaming events...");

    loop {
        match client.recv_event_batch().await {
            Ok(batch) => {
                // Update last_seen_seq from durable events in the batch.
                // Only durable events carry a global event_seq (ephemeral
                // events like live:sample have event_seq = None).
                if !batch.events.is_empty() {
                    let max_seq = batch.events.iter().filter_map(|e| e.event_seq).max();
                    if let Some(seq) = max_seq {
                        let current = last_seen_seq.load(Ordering::SeqCst);
                        if seq > current {
                            last_seen_seq.store(seq, Ordering::SeqCst);
                        }
                    }
                }
                emit_event(app_handle, "busytok:event", &batch);

                // Forward data:invalidated events to the panel webview so
                // the palette can refresh its prompts. Only forward events
                // whose scopes include data relevant to the palette (or
                // events with no scopes for backward compatibility).
                for event in &batch.events {
                    if event.event_type != "data:invalidated" {
                        continue;
                    }

                    // Check scopes on the envelope first (new-style), then
                    // fall back to legacy payload.datasets.
                    let should_forward = if !event.scopes.is_empty() {
                        // Forward all scoped events because InvalidationDatasetDto
                        // has no prompts-specific variant yet; any data change may
                        // affect the prompt palette. When a Prompts variant is added,
                        // filter to: scopes.iter().any(|s| matches!(s.dataset, Prompts))
                        true
                    } else {
                        // Legacy fallback: check payload.datasets array.
                        // If absent or empty, forward conservatively.
                        let datasets = event.payload.get("datasets");
                        datasets.map_or(true, |ds| !ds.as_array().map_or(true, |a| a.is_empty()))
                    };

                    if should_forward {
                        if let Some(mutex) = app_handle.try_state::<std::sync::Mutex<crate::palette_controller::PaletteController>>() {
                            if let Ok(ctrl) = mutex.lock() {
                                if ctrl.is_panel_visible() {
                                    ctrl.push_prompts_invalidate();
                                }
                            }
                        }
                        // Only forward once per batch even if multiple invalidated
                        // events match; the palette invalidates all prompts anyway.
                        break;
                    }
                }
            }
            Err(e) => {
                return Err(format!("subscription stream error: {e}"));
            }
        }
    }
}

/// Result of comparing the stored last-seen sequence with the server's current
/// sequence to determine what kind of gap (if any) occurred.
struct GapClassification {
    detected: bool,
    /// True when the server's sequence is LOWER than the client's, indicating
    /// the service restarted and its counter reset.
    service_restarted: bool,
}

/// Compare stored and current sequence numbers to classify the gap.
///
/// - No gap: stored == 0 (first connection) or stored == current
/// - Forward gap: current > stored (events missed during disconnect)
/// - Backward gap: current < stored (service restarted, counter reset)
fn classify_gap(stored_last: i64, current_seq: i64) -> GapClassification {
    GapClassification {
        detected: stored_last > 0 && current_seq != stored_last,
        service_restarted: stored_last > 0 && current_seq < stored_last,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_gap_on_first_connection() {
        let gap = classify_gap(0, 0);
        assert!(!gap.detected);
        assert!(!gap.service_restarted);

        let gap = classify_gap(0, 500);
        assert!(!gap.detected);
        assert!(!gap.service_restarted);
    }

    #[test]
    fn no_gap_when_sequences_match() {
        let gap = classify_gap(42, 42);
        assert!(!gap.detected);
        assert!(!gap.service_restarted);
    }

    #[test]
    fn forward_gap_when_server_ahead() {
        let gap = classify_gap(100, 150);
        assert!(gap.detected);
        assert!(!gap.service_restarted);
    }

    #[test]
    fn backward_gap_on_service_restart() {
        // This is the bug scenario: service restarted, seq counter reset to 0
        let gap = classify_gap(15000, 0);
        assert!(gap.detected);
        assert!(gap.service_restarted);
    }

    #[test]
    fn backward_gap_on_service_restart_with_few_events() {
        // Service restarted and generated a few events before we reconnected
        let gap = classify_gap(15000, 5);
        assert!(gap.detected);
        assert!(gap.service_restarted);
    }
}
