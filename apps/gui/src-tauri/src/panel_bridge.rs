//! JS <-> Native bridge for the Prompt Palette WKWebView panel.
//!
//! `PanelBridge` provides the Rust callback that the ObjC message handler
//! invokes, deserializes JSON into `PaletteRequest`, routes invoke calls
//! through `HostServices`, and sends responses/events back to JS via
//! `evaluateJavaScript`.
//!
//! This module MUST NOT import `tauri`. It depends only on
//! `host_application_services` and `palette_native`.

use std::ffi::c_void;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::host_application_services::HostServices;
use crate::logging::{self, FrontendLogEntryDto};
use crate::palette_native::MessageCallback;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Incoming request from JavaScript.
#[derive(Debug, Deserialize)]
pub struct PaletteRequest {
    pub id: String,
    #[serde(rename = "type")]
    pub req_type: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub payload: Option<JsonValue>,
}

/// Outgoing response payload (wrapped inside a `PaletteEvent`).
#[derive(Debug, Serialize)]
pub struct PaletteResponse {
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Event pushed from native to JavaScript (responses + push events).
#[derive(Debug, Serialize)]
pub struct PaletteEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: JsonValue,
}

// ---------------------------------------------------------------------------
// Close handler type
// ---------------------------------------------------------------------------

/// Callback invoked when JS requests `palette:close`.
pub type CloseHandler = Box<dyn Fn() + Send>;

/// Thread-safe callback that evaluates a JavaScript string in a webview.
///
/// Set by `PaletteController` with a closure that marshals onto the main
/// thread before calling `eval_js`. This keeps `tauri` out of
/// `panel_bridge.rs` while ensuring `evaluateJavaScript` is always called
/// from the main thread.
type EvalFn = Box<dyn Fn(*mut c_void, &str) + Send + Sync>;

type TaskFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
type TaskSpawner = Arc<dyn Fn(TaskFuture) + Send + Sync>;

// ---------------------------------------------------------------------------
// SendableWebview — raw pointer wrapper that is Send
// ---------------------------------------------------------------------------

/// Wrapper around a raw webview pointer that implements `Send`.
///
/// The webview is an ObjC object that must only be used from the main thread.
/// We declare it `Send` so the pointer can be moved into a host-spawned async
/// task; the actual `eval_js` call will marshal back onto the main thread
/// internally.
struct SendableWebview(*mut c_void);

unsafe impl Send for SendableWebview {}

impl SendableWebview {
    fn ptr(&self) -> *mut c_void {
        self.0
    }
}

// ---------------------------------------------------------------------------
// PanelBridge
// ---------------------------------------------------------------------------

/// Bridge managing the JS <-> Rust communication channel for the palette.
///
/// Stores the webview pointer (for pushing events) and a close handler
/// (called when JS sends `palette:close`).
pub struct PanelBridge {
    close_handler: Arc<Mutex<Option<CloseHandler>>>,
    webview: Arc<Mutex<Option<SendableWebview>>>,
    eval_fn: Arc<Mutex<Option<EvalFn>>>,
    task_spawner: Arc<Mutex<Option<TaskSpawner>>>,
}

// Safety: the webview pointer is only used on the main thread (via eval_js)
// and is protected by a Mutex.
unsafe impl Send for PanelBridge {}
unsafe impl Sync for PanelBridge {}

impl Default for PanelBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl PanelBridge {
    /// Create an empty bridge (no webview, no close handler, no eval fn).
    pub fn new() -> Self {
        Self {
            close_handler: Arc::new(Mutex::new(None)),
            webview: Arc::new(Mutex::new(None)),
            eval_fn: Arc::new(Mutex::new(None)),
            task_spawner: Arc::new(Mutex::new(None)),
        }
    }

    /// Store the webview pointer for `push_event`.
    ///
    /// Passing a null pointer clears the stored webview, preventing further
    /// `eval_fn` calls (e.g. after panel destruction).
    pub fn set_webview(&self, ptr: *mut c_void) {
        let mut guard = self.webview.lock().unwrap();
        if ptr.is_null() {
            *guard = None;
        } else {
            *guard = Some(SendableWebview(ptr));
        }
    }

    /// Store a callback invoked when JS requests `palette:close`.
    pub fn register_close_handler(&self, handler: CloseHandler) {
        let mut guard = self.close_handler.lock().unwrap();
        *guard = Some(handler);
    }

    /// Store a thread-safe eval function for dispatching JavaScript to the
    /// webview. The closure is responsible for ensuring `evaluateJavaScript`
    /// is called on the main thread (e.g. via `app.run_on_main_thread`).
    pub fn set_eval_fn(&self, f: EvalFn) {
        *self.eval_fn.lock().unwrap() = Some(f);
    }

    /// Store a host-provided async task spawner.
    ///
    /// The ObjC WKScriptMessageHandler callback runs on the AppKit main run
    /// loop, which is not entered into a Tokio runtime. The host layer owns the
    /// runtime choice and injects it here; this keeps PanelBridge independent
    /// from Tauri while avoiding `tokio::spawn` panics outside runtime context.
    pub(crate) fn set_task_spawner(&self, f: impl Fn(TaskFuture) + Send + Sync + 'static) {
        *self.task_spawner.lock().unwrap() = Some(Arc::new(f));
    }

    /// Push a `PaletteEvent` to the stored webview using the stored eval_fn.
    ///
    /// Convenience wrapper for callers (e.g. `PaletteController`) that don't
    /// need to manage the eval_fn Arc directly.
    pub fn push_event_to_webview(&self, event: &PaletteEvent) {
        let wv = self.webview.lock().unwrap();
        if let Some(ref swv) = *wv {
            Self::push_event_with(&self.eval_fn, swv.ptr(), event);
        }
    }

    /// Build the `MessageCallback` suitable for `palette_native::create_message_handler`.
    ///
    /// The callback runs on the ObjC main thread and therefore must not block.
    /// Invoke calls are dispatched through the host-provided task spawner.
    pub fn create_message_callback(&self, services: HostServices) -> MessageCallback {
        let close_handler = Arc::clone(&self.close_handler);
        let webview = Arc::clone(&self.webview);
        let eval_fn = Arc::clone(&self.eval_fn);
        let task_spawner = Arc::clone(&self.task_spawner);

        Box::new(move |body: &str| {
            // Helpers to avoid repeating the lock-respond pattern for every
            // synchronous local method.  Defined inside the move closure so
            // they borrow the moved Arcs.
            let respond_ok = |req_id: &str, data: Option<JsonValue>| {
                let wv = webview.lock().unwrap();
                if let Some(ref swv) = *wv {
                    Self::respond_with(&eval_fn, swv.ptr(), req_id, true, data, None);
                }
            };
            let respond_err = |req_id: &str, err: &str| {
                let wv = webview.lock().unwrap();
                if let Some(ref swv) = *wv {
                    Self::respond_with(
                        &eval_fn,
                        swv.ptr(),
                        req_id,
                        false,
                        None,
                        Some(err.to_string()),
                    );
                }
            };

            let request: PaletteRequest = match serde_json::from_str(body) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        event_code = "panel_bridge.parse_error",
                        error = %e,
                        "failed to parse palette request"
                    );
                    respond_err("parse-error", &format!("parse error: {e}"));
                    return;
                }
            };

            tracing::debug!(
                event_code = "panel_bridge.request_received",
                request_type = %request.req_type,
                method = request.method.as_deref().unwrap_or(""),
                request_id = %request.id,
                "received prompt palette panel bridge request"
            );

            match request.req_type.as_str() {
                "invoke" => {
                    let method = match request.method {
                        Some(ref m) => m.clone(),
                        None => {
                            respond_err(&request.id, "missing method field");
                            return;
                        }
                    };

                    // Handle palette:close synchronously
                    if method == "palette:close" {
                        tracing::info!(
                            event_code = "panel_bridge.palette_close_requested",
                            request_id = %request.id,
                            "panel requested palette close"
                        );
                        let handler = close_handler.lock().unwrap();
                        if let Some(ref cb) = *handler {
                            cb();
                        } else {
                            tracing::warn!(
                                event_code = "panel_bridge.palette_close_no_handler",
                                request_id = %request.id,
                                "panel requested close but no close handler is registered"
                            );
                        }
                        respond_ok(&request.id, None);
                        return;
                    }

                    if method == "panel:diagnostic" {
                        tracing::info!(
                            event_code = "panel_bridge.diagnostic",
                            request_id = %request.id,
                            payload = ?request.payload,
                            "prompt palette panel diagnostic event"
                        );
                        respond_ok(&request.id, None);
                        return;
                    }

                    if method == "log_frontend_event" {
                        match write_panel_frontend_log(request.payload.as_ref()) {
                            Ok(()) => respond_ok(&request.id, None),
                            Err(error) => respond_err(&request.id, &error),
                        }
                        return;
                    }

                    if method == "flush_frontend_logs" {
                        match flush_panel_frontend_logs(request.payload.as_ref()) {
                            Ok(data) => respond_ok(&request.id, Some(data)),
                            Err(error) => respond_err(&request.id, &error),
                        }
                        return;
                    }

                    // Synchronous: accessibility check (no async work needed).
                    if method == "prompt_palette_accessibility_status" {
                        let data = crate::prompt_palette_native::accessibility_status();
                        respond_ok(&request.id, Some(data));
                        return;
                    }

                    // Async: paste Cmd+V into the previously-active app.
                    if method == "prompt_palette_paste_active_app" {
                        let req_id = request.id.clone();
                        let eval_fn_task = Arc::clone(&eval_fn);
                        let webview_task = Arc::clone(&webview);
                        let spawn = task_spawner.lock().unwrap().clone();
                        if let Some(spawn) = spawn {
                            let task = async move {
                                let result_data =
                                    crate::prompt_palette_native::paste_active_app().await;
                                let data = match result_data {
                                    Ok(v) => v,
                                    Err(e) => {
                                        serde_json::json!({"ok": false, "failure_reason": e.to_string()})
                                    }
                                };
                                let wv_ptr = {
                                    let guard = webview_task.lock().unwrap();
                                    guard.as_ref().map(|swv| swv.ptr())
                                };
                                if let Some(ptr) = wv_ptr {
                                    Self::respond_with(
                                        &eval_fn_task,
                                        ptr,
                                        &req_id,
                                        true,
                                        Some(data),
                                        None,
                                    );
                                }
                            };
                            spawn(Box::pin(task));
                        } else {
                            respond_err(&request.id, "panel task spawner is not configured");
                        }
                        return;
                    }

                    // Async invoke for all other methods.
                    // The async task reads the webview pointer under the lock
                    // at response time rather than capturing a raw pointer
                    // upfront, so a concurrent destroy() won't leave a dangling
                    // reference.
                    let params = request.payload.clone().unwrap_or(JsonValue::Null);
                    let req_id = request.id.clone();
                    let method_for_invalidation = method.clone();
                    let services = services.clone();
                    let eval_fn_task = Arc::clone(&eval_fn);
                    let webview_task = Arc::clone(&webview);

                    let spawn = task_spawner.lock().unwrap().clone();
                    if let Some(spawn) = spawn {
                        let task = async move {
                            let result = services.invoke(&method, params).await;

                            // Read webview pointer under lock at response time
                            let wv_ptr = {
                                let guard = webview_task.lock().unwrap();
                                guard.as_ref().map(|swv| swv.ptr())
                            };
                            if let Some(ptr) = wv_ptr {
                                match result {
                                    Ok(data) => {
                                        Self::respond_with(
                                            &eval_fn_task,
                                            ptr,
                                            &req_id,
                                            true,
                                            Some(data),
                                            None,
                                        );
                                    }
                                    Err(e) => {
                                        Self::respond_with(
                                            &eval_fn_task,
                                            ptr,
                                            &req_id,
                                            false,
                                            None,
                                            Some(e),
                                        );
                                    }
                                }

                                // Auto-push prompts:invalidate after prompt mutations
                                if is_prompt_mutation(&method_for_invalidation) {
                                    let invalidate_event = PaletteEvent {
                                        request_id: None,
                                        event_type: "prompts:invalidate".to_string(),
                                        payload: serde_json::json!({}),
                                    };
                                    Self::push_event_with(&eval_fn_task, ptr, &invalidate_event);
                                }
                            } else {
                                tracing::warn!(
                                    event_code = "panel_bridge.no_webview_at_response",
                                    method = %method_for_invalidation,
                                    "cannot deliver response: webview was cleared"
                                );
                            }
                        };
                        spawn(Box::pin(task));
                    } else {
                        tracing::warn!(
                            event_code = "panel_bridge.no_task_spawner",
                            method = %method_for_invalidation,
                            "cannot dispatch invoke: task spawner not set"
                        );
                        respond_err(&req_id, "panel task spawner is not configured");
                    }
                }
                "subscribe" => {
                    respond_ok(&request.id, None);
                }
                other => {
                    respond_err(&request.id, &format!("unknown request type: {other}"));
                }
            }
        })
    }

    /// Push a `PaletteEvent` to the webview by evaluating JS.
    ///
    /// Uses the stored `eval_fn` which handles thread marshaling to the main
    /// thread.
    pub fn push_event_with(
        eval_fn: &Arc<Mutex<Option<EvalFn>>>,
        webview: *mut c_void,
        event: &PaletteEvent,
    ) {
        match build_dispatch_script(event) {
            Ok(script) => {
                let guard = eval_fn.lock().unwrap();
                if let Some(ref f) = *guard {
                    f(webview, &script);
                } else {
                    tracing::warn!(
                        event_code = "panel_bridge.no_eval_fn",
                        "cannot push event: eval_fn not set"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    event_code = "panel_bridge.serialize_error",
                    error = %e,
                    "failed to serialize palette event"
                );
            }
        }
    }

    /// Send a response event to the webview.
    ///
    /// Wraps the result in a `PaletteResponse` and then a `PaletteEvent` of
    /// type `"response"`.
    fn respond_with(
        eval_fn: &Arc<Mutex<Option<EvalFn>>>,
        webview: *mut c_void,
        request_id: &str,
        ok: bool,
        data: Option<JsonValue>,
        error: Option<String>,
    ) {
        let response = PaletteResponse {
            id: request_id.to_string(),
            ok,
            data,
            error,
        };
        let event = PaletteEvent {
            request_id: Some(request_id.to_string()),
            event_type: "response".to_string(),
            payload: serde_json::to_value(&response).unwrap_or(JsonValue::Null),
        };
        Self::push_event_with(eval_fn, webview, &event);
    }
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Build the JavaScript dispatch script for a given event.
///
/// Produces `window.__busytokPanelBridgeDispatch({...json...})`.
pub fn build_dispatch_script(event: &PaletteEvent) -> Result<String, serde_json::Error> {
    let json = serde_json::to_string(event)?;
    Ok(format!("window.__busytokPanelBridgeDispatch({json})"))
}

/// Returns `true` if the method is a prompt mutation that should trigger
/// a `prompts:invalidate` push event.
pub(crate) fn is_prompt_mutation(method: &str) -> bool {
    matches!(
        method,
        "prompts.create" | "prompts.update" | "prompts.delete" | "prompts.use"
    )
}

fn write_panel_frontend_log(payload: Option<&JsonValue>) -> Result<(), String> {
    let Some(payload) = payload else {
        return Err("missing frontend log payload".to_string());
    };
    let entry_value = payload
        .get("entry")
        .cloned()
        .ok_or_else(|| "missing frontend log entry".to_string())?;
    let entry: FrontendLogEntryDto = serde_json::from_value(entry_value)
        .map_err(|error| format!("invalid frontend log entry: {error}"))?;
    logging::write_frontend_log_entry(&entry);
    Ok(())
}

fn flush_panel_frontend_logs(payload: Option<&JsonValue>) -> Result<JsonValue, String> {
    let Some(payload) = payload else {
        return Err("missing frontend log flush payload".to_string());
    };
    let entries_value = payload
        .get("entries")
        .cloned()
        .ok_or_else(|| "missing frontend log entries".to_string())?;
    let entries: Vec<FrontendLogEntryDto> = serde_json::from_value(entries_value)
        .map_err(|error| format!("invalid frontend log entries: {error}"))?;
    let result = logging::flush_frontend_logs_inner(&entries);
    serde_json::to_value(result).map_err(|error| format!("serialize flush result: {error}"))
}
