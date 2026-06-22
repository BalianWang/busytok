//! Transport-agnostic service-client layer — socket primitives shared by
//! Tauri commands and panel bridges.
//!
//! This module MUST NOT import `tauri`. All items here are pure Rust and can
//! be used from any host (Tauri, WKWebView panel, CLI, tests).
//!
//! All socket-recovery bootstrap paths route through an explicit caller-
//! supplied callback (`FnOnce() -> Future<...>`). There is no implicit
//! global lifecycle: callers must thread the [`crate::lifecycle_coordinator::LifecycleCoordinator`]
//! from Tauri state into the bootstrap closure so the session-suppression
//! and ensure-coalescing contracts are honored.

use std::future::Future;
use std::time::Duration;

use busytok_protocol::dto::{ControlRequest, RequestMeta};
use serde_json::Value as JsonValue;

/// Timeout for connect + bootstrap phase (10 s).
const CONNECT_BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for the call dispatch phase (30 s).
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// State managed by the host application: holds the control endpoint (Unix socket
/// path or Windows named pipe) served by busytok-service.
pub struct BusytokState {
    pub control_endpoint: String,
}

/// Metadata attached to an invoke call by the frontend.
#[derive(serde::Deserialize)]
pub struct InvokeMeta {
    #[serde(default)]
    correlation_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
}

impl InvokeMeta {
    pub fn correlation_id(&self) -> Option<&str> {
        self.correlation_id.as_deref()
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
}

pub(crate) async fn invoke_busytok_via_socket_with_bootstrap<F, Fut>(
    method: &str,
    params: JsonValue,
    socket_path: &str,
    meta: RequestMeta,
    bootstrap_service: F,
) -> Result<JsonValue, String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    // Phase 1: connect + bootstrap (bounded by timeout).
    let mut client = tokio::time::timeout(
        CONNECT_BOOTSTRAP_TIMEOUT,
        crate::service_recovery::connect_with_service_recovery(socket_path, bootstrap_service),
    )
    .await
    .map_err(|_| "connect/bootstrap phase timed out".to_string())??;

    let request = ControlRequest::with_meta(method, params, meta);

    // Phase 2: call dispatch (bounded by timeout).
    let response = tokio::time::timeout(CALL_TIMEOUT, client.call(request))
        .await
        .map_err(|_| format!("call to '{method}' timed out"))?;

    match response {
        Ok(busytok_protocol::dto::ControlResponse::Ok(payload)) => Ok(payload),
        Ok(busytok_protocol::dto::ControlResponse::Err(err)) => {
            if let Some(payload) = err.payload {
                Err(format!(
                    "[{}] {} | payload: {}",
                    err.code, err.message, payload
                ))
            } else {
                Err(format!("[{}] {}", err.code, err.message))
            }
        }
        Err(e) => Err(format!("dispatch error: {e}")),
    }
}

/// Transport-agnostic service handle wrapping a control endpoint (Unix
/// socket path on macOS/Linux, named pipe on Windows) plus the bootstrap
/// callback to invoke when the socket is unreachable.
///
/// Used by panel bridge (panel_bridge.rs) for WKWebView-hosted palette.
/// The bootstrap callback captures an `Arc<LifecycleCoordinator>` so the
/// session-suppression and ensure-coalescing contracts are honored.
#[derive(Clone)]
pub struct HostServices {
    control_endpoint: String,
}

impl HostServices {
    pub fn new(control_endpoint: String) -> Self {
        Self { control_endpoint }
    }

    pub fn endpoint(&self) -> &str {
        &self.control_endpoint
    }

    pub async fn invoke(&self, method: &str, params: JsonValue) -> Result<JsonValue, String> {
        tracing::info!(
            event_code = "host_services.invoke",
            method = %method,
            "forwarding panel invoke to busytok-service"
        );
        // Panel bridge path: direct connect only. Bootstrap / recovery is
        // the responsibility of higher-level retry logic — returning an
        // error here surfaces unavailability immediately rather than
        // silently constructing a one-shot lifecycle.
        let connect_fut = busytok_control::client::ControlClient::<
            busytok_control::transport::PlatformTransport,
        >::connect(&self.control_endpoint);
        let mut client = tokio::time::timeout(CONNECT_BOOTSTRAP_TIMEOUT, connect_fut)
            .await
            .map_err(|_| "connect phase timed out".to_string())?
            .map_err(|e| format!("service unavailable: {e}"))?;

        let request = ControlRequest::with_meta(method, params, RequestMeta::default());
        let response = tokio::time::timeout(CALL_TIMEOUT, client.call(request))
            .await
            .map_err(|_| format!("call to '{method}' timed out"))?;
        match response {
            Ok(busytok_protocol::dto::ControlResponse::Ok(payload)) => Ok(payload),
            Ok(busytok_protocol::dto::ControlResponse::Err(err)) => {
                if let Some(payload) = err.payload {
                    Err(format!(
                        "[{}] {} | payload: {}",
                        err.code, err.message, payload
                    ))
                } else {
                    Err(format!("[{}] {}", err.code, err.message))
                }
            }
            Err(e) => Err(format!("dispatch error: {e}")),
        }
    }
}
