use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use crate::panel_bridge::{
    build_dispatch_script, PaletteEvent, PaletteRequest, PaletteResponse, PanelBridge,
};

// ---------------------------------------------------------------------------
// PaletteRequest deserialization
// ---------------------------------------------------------------------------

#[test]
fn request_deserializes_invoke() {
    let req: PaletteRequest = serde_json::from_str(
        r#"{"id":"42","type":"invoke","method":"prompts.list","payload":{"limit":10}}"#,
    )
    .unwrap();
    assert_eq!(req.id, "42");
    assert_eq!(req.req_type, "invoke");
    assert_eq!(req.method.as_deref(), Some("prompts.list"));
    assert_eq!(req.payload.as_ref().unwrap()["limit"], 10);
}

#[test]
fn request_deserializes_close() {
    let req: PaletteRequest =
        serde_json::from_str(r#"{"id":"7","type":"invoke","method":"palette:close"}"#).unwrap();
    assert_eq!(req.id, "7");
    assert_eq!(req.method.as_deref(), Some("palette:close"));
    assert!(req.payload.is_none());
}

#[test]
fn request_deserializes_subscribe() {
    let req: PaletteRequest = serde_json::from_str(r#"{"id":"99","type":"subscribe"}"#).unwrap();
    assert_eq!(req.id, "99");
    assert_eq!(req.req_type, "subscribe");
    assert!(req.method.is_none());
}

// ---------------------------------------------------------------------------
// PaletteResponse serialization
// ---------------------------------------------------------------------------

#[test]
fn response_serializes_ok() {
    let resp = PaletteResponse {
        id: "42".to_string(),
        ok: true,
        data: Some(serde_json::json!({"items": []})),
        error: None,
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"ok\":true"));
    assert!(json.contains("\"data\""));
    assert!(!json.contains("\"error\"")); // skip_serializing_if = None
}

#[test]
fn response_serializes_error() {
    let resp = PaletteResponse {
        id: "42".to_string(),
        ok: false,
        data: None,
        error: Some("something broke".to_string()),
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"ok\":false"));
    assert!(json.contains("\"error\""));
    assert!(!json.contains("\"data\"")); // skip_serializing_if = None
}

// ---------------------------------------------------------------------------
// PaletteEvent serialization
// ---------------------------------------------------------------------------

#[test]
fn event_serializes_correctly() {
    let event = PaletteEvent {
        request_id: Some("abc".to_string()),
        event_type: "response".to_string(),
        payload: serde_json::json!({"ok": true}),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"response\""));
    assert!(json.contains("\"request_id\":\"abc\""));
    assert!(json.contains("\"payload\""));

    // Without request_id — should be omitted
    let event_no_req = PaletteEvent {
        request_id: None,
        event_type: "prompts:invalidate".to_string(),
        payload: serde_json::json!({}),
    };
    let json2 = serde_json::to_string(&event_no_req).unwrap();
    assert!(!json2.contains("request_id"));
}

// ---------------------------------------------------------------------------
// Dispatch script
// ---------------------------------------------------------------------------

#[test]
fn dispatch_script_wraps_in_window_function() {
    let event = PaletteEvent {
        request_id: Some("1".to_string()),
        event_type: "response".to_string(),
        payload: serde_json::json!({"id":"1","ok":true}),
    };
    let script = build_dispatch_script(&event).unwrap();
    assert!(script.starts_with("window.__busytokPanelBridgeDispatch("));
    assert!(script.ends_with(")"));
    assert!(script.contains("\"type\":\"response\""));
}

// ---------------------------------------------------------------------------
// PanelBridge::new / Default
// ---------------------------------------------------------------------------

#[test]
fn bridge_creates_handler_callback() {
    let bridge = PanelBridge::new();
    // Default should work too
    let _bridge2 = PanelBridge::default();

    // We can create a callback — it just won't do much without a webview.
    // End-to-end bridge dispatch (JS → ObjC → callback → Rust) is verified
    // manually by running the panel and triggering prompt operations.
    let services = crate::host_application_services::HostServices::new("/tmp/test.sock".into());
    let _callback = bridge.create_message_callback(services);
}

#[test]
fn invoke_callback_does_not_require_current_tokio_runtime() {
    let bridge = PanelBridge::new();
    bridge.set_webview(1usize as *mut std::ffi::c_void);
    bridge.set_eval_fn(Box::new(|_, _| {}));
    let services = crate::host_application_services::HostServices::new("/tmp/test.sock".into());
    let callback = bridge.create_message_callback(services);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        callback(r#"{"id":"1","type":"invoke","method":"prompts.list","payload":{}}"#);
    }));

    assert!(
        result.is_ok(),
        "panel invoke callback must not panic when called outside a Tokio runtime"
    );
}

#[test]
fn invoke_callback_uses_configured_task_spawner() {
    let bridge = PanelBridge::new();
    bridge.set_webview(1usize as *mut std::ffi::c_void);
    bridge.set_eval_fn(Box::new(|_, _| {}));

    let spawned = Arc::new(AtomicUsize::new(0));
    let spawned_for_spawner = Arc::clone(&spawned);
    bridge.set_task_spawner(move |_task| {
        spawned_for_spawner.fetch_add(1, Ordering::SeqCst);
    });

    let services = crate::host_application_services::HostServices::new("/tmp/test.sock".into());
    let callback = bridge.create_message_callback(services);
    callback(r#"{"id":"1","type":"invoke","method":"prompts.list","payload":{}}"#);

    assert_eq!(spawned.load(Ordering::SeqCst), 1);
}

#[test]
fn local_panel_diagnostic_does_not_spawn_service_task() {
    let bridge = PanelBridge::new();
    bridge.set_webview(1usize as *mut std::ffi::c_void);

    let eval_count = Arc::new(AtomicUsize::new(0));
    let eval_count_for_fn = Arc::clone(&eval_count);
    bridge.set_eval_fn(Box::new(move |_, _| {
        eval_count_for_fn.fetch_add(1, Ordering::SeqCst);
    }));

    let spawned = Arc::new(AtomicUsize::new(0));
    let spawned_for_spawner = Arc::clone(&spawned);
    bridge.set_task_spawner(move |_task| {
        spawned_for_spawner.fetch_add(1, Ordering::SeqCst);
    });

    let services = crate::host_application_services::HostServices::new("/tmp/test.sock".into());
    let callback = bridge.create_message_callback(services);
    callback(
        r#"{"id":"diag-1","type":"invoke","method":"panel:diagnostic","payload":{"name":"probe","details":{"rootChildCount":0}}}"#,
    );

    assert_eq!(spawned.load(Ordering::SeqCst), 0);
    assert_eq!(eval_count.load(Ordering::SeqCst), 1);
}

#[test]
fn local_panel_frontend_log_does_not_spawn_service_task() {
    let bridge = PanelBridge::new();
    bridge.set_webview(1usize as *mut std::ffi::c_void);

    let eval_count = Arc::new(AtomicUsize::new(0));
    let eval_count_for_fn = Arc::clone(&eval_count);
    bridge.set_eval_fn(Box::new(move |_, _| {
        eval_count_for_fn.fetch_add(1, Ordering::SeqCst);
    }));

    let spawned = Arc::new(AtomicUsize::new(0));
    let spawned_for_spawner = Arc::clone(&spawned);
    bridge.set_task_spawner(move |_task| {
        spawned_for_spawner.fetch_add(1, Ordering::SeqCst);
    });

    let services = crate::host_application_services::HostServices::new("/tmp/test.sock".into());
    let callback = bridge.create_message_callback(services);
    callback(
        r#"{"id":"log-1","type":"invoke","method":"log_frontend_event","payload":{"entry":{"ts":"2026-06-04T00:00:00Z","level":"INFO","session_id":"test-session","event_code":"gui.prompt_palette.panel_probe","message":"panel probe","details":{"rootChildCount":0}}}}"#,
    );

    assert_eq!(spawned.load(Ordering::SeqCst), 0);
    assert_eq!(eval_count.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// is_prompt_mutation
// ---------------------------------------------------------------------------

#[test]
fn is_prompt_mutation_detects_mutations() {
    assert!(crate::panel_bridge::is_prompt_mutation("prompts.create"));
    assert!(crate::panel_bridge::is_prompt_mutation("prompts.update"));
    assert!(crate::panel_bridge::is_prompt_mutation("prompts.delete"));
    assert!(crate::panel_bridge::is_prompt_mutation("prompts.use"));
}

#[test]
fn is_prompt_mutation_rejects_reads() {
    assert!(!crate::panel_bridge::is_prompt_mutation("prompts.list"));
    assert!(!crate::panel_bridge::is_prompt_mutation("prompts.get"));
    assert!(!crate::panel_bridge::is_prompt_mutation("shell.status"));
}

// ---------------------------------------------------------------------------
// Error / edge-case paths
// ---------------------------------------------------------------------------

#[test]
fn parse_error_does_not_panic() {
    let bridge = PanelBridge::new();
    let services = crate::host_application_services::HostServices::new("/tmp/test.sock".into());
    let callback = bridge.create_message_callback(services);

    // Malformed JSON should not panic — just log and return.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        callback("not valid json");
    }));
    assert!(result.is_ok(), "malformed JSON should not panic");
}

#[test]
fn invoke_without_webview_does_not_panic() {
    let bridge = PanelBridge::new();
    // No webview set, no eval_fn — invoke should gracefully skip.

    let spawned = Arc::new(AtomicUsize::new(0));
    let spawned_for_spawner = Arc::clone(&spawned);
    bridge.set_task_spawner(move |_task| {
        spawned_for_spawner.fetch_add(1, Ordering::SeqCst);
    });

    let services = crate::host_application_services::HostServices::new("/tmp/test.sock".into());
    let callback = bridge.create_message_callback(services);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        callback(r#"{"id":"1","type":"invoke","method":"prompts.list","payload":{}}"#);
    }));
    assert!(result.is_ok(), "invoke without webview should not panic");
    // Task was spawned but will find no webview at response time — no crash
    assert_eq!(spawned.load(Ordering::SeqCst), 1);
}

#[test]
fn invoke_without_task_spawner_does_not_panic() {
    let bridge = PanelBridge::new();
    bridge.set_webview(1usize as *mut std::ffi::c_void);

    let eval_count = Arc::new(AtomicUsize::new(0));
    let eval_count_for_fn = Arc::clone(&eval_count);
    bridge.set_eval_fn(Box::new(move |_, _| {
        eval_count_for_fn.fetch_add(1, Ordering::SeqCst);
    }));
    // No task spawner set — should respond with error via eval_fn

    let services = crate::host_application_services::HostServices::new("/tmp/test.sock".into());
    let callback = bridge.create_message_callback(services);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        callback(r#"{"id":"1","type":"invoke","method":"prompts.list","payload":{}}"#);
    }));
    assert!(result.is_ok(), "invoke without spawner should not panic");
    assert_eq!(
        eval_count.load(Ordering::SeqCst),
        1,
        "should send error response via eval_fn"
    );
}

#[test]
fn unknown_request_type_does_not_panic() {
    let bridge = PanelBridge::new();
    bridge.set_webview(1usize as *mut std::ffi::c_void);

    let eval_count = Arc::new(AtomicUsize::new(0));
    let eval_count_for_fn = Arc::clone(&eval_count);
    bridge.set_eval_fn(Box::new(move |_, _| {
        eval_count_for_fn.fetch_add(1, Ordering::SeqCst);
    }));

    let services = crate::host_application_services::HostServices::new("/tmp/test.sock".into());
    let callback = bridge.create_message_callback(services);
    callback(r#"{"id":"1","type":"unknown_type"}"#);

    assert_eq!(
        eval_count.load(Ordering::SeqCst),
        1,
        "unknown type should get an error response"
    );
}

#[test]
fn missing_method_field_responds_with_error() {
    let bridge = PanelBridge::new();
    bridge.set_webview(1usize as *mut std::ffi::c_void);

    let eval_count = Arc::new(AtomicUsize::new(0));
    let eval_count_for_fn = Arc::clone(&eval_count);
    bridge.set_eval_fn(Box::new(move |_, _| {
        eval_count_for_fn.fetch_add(1, Ordering::SeqCst);
    }));

    let services = crate::host_application_services::HostServices::new("/tmp/test.sock".into());
    let callback = bridge.create_message_callback(services);
    callback(r#"{"id":"1","type":"invoke","payload":{}}"#);

    assert_eq!(
        eval_count.load(Ordering::SeqCst),
        1,
        "missing method should get an error response"
    );
}

#[test]
fn flush_frontend_logs_local_route() {
    let bridge = PanelBridge::new();
    bridge.set_webview(1usize as *mut std::ffi::c_void);

    let eval_count = Arc::new(AtomicUsize::new(0));
    let eval_count_for_fn = Arc::clone(&eval_count);
    bridge.set_eval_fn(Box::new(move |_, _| {
        eval_count_for_fn.fetch_add(1, Ordering::SeqCst);
    }));

    let spawned = Arc::new(AtomicUsize::new(0));
    let spawned_for_spawner = Arc::clone(&spawned);
    bridge.set_task_spawner(move |_task| {
        spawned_for_spawner.fetch_add(1, Ordering::SeqCst);
    });

    let services = crate::host_application_services::HostServices::new("/tmp/test.sock".into());
    let callback = bridge.create_message_callback(services);
    callback(
        r#"{"id":"flush-1","type":"invoke","method":"flush_frontend_logs","payload":{"entries":[]}}"#,
    );

    assert_eq!(
        spawned.load(Ordering::SeqCst),
        0,
        "flush should be handled locally"
    );
    assert_eq!(eval_count.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// prompt_palette_accessibility_status — local synchronous route
// ---------------------------------------------------------------------------

#[test]
fn accessibility_status_local_route_does_not_spawn_service_task() {
    let bridge = PanelBridge::new();
    bridge.set_webview(1usize as *mut std::ffi::c_void);

    let captured_script: Arc<std::sync::Mutex<String>> =
        Arc::new(std::sync::Mutex::new(String::new()));
    let captured_for_fn = Arc::clone(&captured_script);
    bridge.set_eval_fn(Box::new(move |_, script: &str| {
        *captured_for_fn.lock().unwrap() = script.to_string();
    }));

    let spawned = Arc::new(AtomicUsize::new(0));
    let spawned_for_spawner = Arc::clone(&spawned);
    bridge.set_task_spawner(move |_task| {
        spawned_for_spawner.fetch_add(1, Ordering::SeqCst);
    });

    let services = crate::host_application_services::HostServices::new("/tmp/test.sock".into());
    let callback = bridge.create_message_callback(services);
    callback(
        r#"{"id":"ax-1","type":"invoke","method":"prompt_palette_accessibility_status","payload":null}"#,
    );

    assert_eq!(
        spawned.load(Ordering::SeqCst),
        0,
        "accessibility_status should be handled locally without task spawn"
    );

    let script = captured_script.lock().unwrap();
    assert!(!script.is_empty(), "should send a response via eval_fn");
    // The response must contain the actual PasteAttemptResult — not an RPC error.
    assert!(
        script.contains("\"ok\":"),
        "response should contain PasteAttemptResult data, got: {script}"
    );
    assert!(
        !script.contains("unknown request type"),
        "must not fall through to generic RPC path"
    );
}

// ---------------------------------------------------------------------------
// prompt_palette_paste_active_app — local async route via task spawner
// ---------------------------------------------------------------------------

#[test]
fn paste_active_app_local_route_uses_task_spawner() {
    let bridge = PanelBridge::new();
    bridge.set_webview(1usize as *mut std::ffi::c_void);

    let eval_count = Arc::new(AtomicUsize::new(0));
    let eval_count_for_fn = Arc::clone(&eval_count);
    bridge.set_eval_fn(Box::new(move |_, _| {
        eval_count_for_fn.fetch_add(1, Ordering::SeqCst);
    }));

    let spawned = Arc::new(AtomicUsize::new(0));
    let spawned_for_check = Arc::clone(&spawned);
    bridge.set_task_spawner(move |_task| {
        spawned_for_check.fetch_add(1, Ordering::SeqCst);
        // Deliberately do NOT run the task — paste_active_app calls
        // AXIsProcessTrusted + CGEvent which require a real macOS session.
        // The routing (spawn vs generic RPC) is what matters here.
    });

    let services = crate::host_application_services::HostServices::new("/tmp/test.sock".into());
    let callback = bridge.create_message_callback(services);
    callback(
        r#"{"id":"paste-1","type":"invoke","method":"prompt_palette_paste_active_app","payload":null}"#,
    );

    // paste_active_app must go through the task spawner, not the generic RPC path.
    assert_eq!(
        spawned.load(Ordering::SeqCst),
        1,
        "paste should use task spawner (not generic RPC)"
    );

    // eval_fn is not called because we didn't actually run the task — that's fine.
    // The accessibility_status test covers synchronous eval_fn responses.
}

// ---------------------------------------------------------------------------
// set_webview(null) clears pointer — regression test for I3
// ---------------------------------------------------------------------------

#[test]
fn clearing_webview_prevents_push_event_from_calling_eval_fn() {
    let bridge = PanelBridge::new();

    let eval_count = Arc::new(AtomicUsize::new(0));
    let eval_count_for_fn = Arc::clone(&eval_count);
    bridge.set_eval_fn(Box::new(move |_, _| {
        eval_count_for_fn.fetch_add(1, Ordering::SeqCst);
    }));

    // Set a real pointer, then clear it.
    bridge.set_webview(1usize as *mut std::ffi::c_void);
    bridge.push_event_to_webview(&PaletteEvent {
        request_id: None,
        event_type: "test".to_string(),
        payload: serde_json::json!({}),
    });
    assert_eq!(
        eval_count.load(Ordering::SeqCst),
        1,
        "should eval with live webview"
    );

    // Clear with null — push_event_to_webview must NOT call eval_fn.
    bridge.set_webview(std::ptr::null_mut());
    bridge.push_event_to_webview(&PaletteEvent {
        request_id: None,
        event_type: "test".to_string(),
        payload: serde_json::json!({}),
    });
    assert_eq!(
        eval_count.load(Ordering::SeqCst),
        1,
        "eval_fn must not be called after webview is cleared with null"
    );
}
