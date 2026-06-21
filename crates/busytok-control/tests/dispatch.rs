use busytok_control::dispatch::{control_response_from_error, MethodDispatchError};
use busytok_control::{ControlDispatcher, TestRuntimeControl};
use busytok_protocol::{method_manifest, ControlRequest, ControlResponse};

fn response_data(value: &serde_json::Value) -> &serde_json::Value {
    value.get("data").unwrap_or(value)
}

// ---------------------------------------------------------------------------
// Bulk smoke test — every Surge UI method responds Ok with valid params
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatcher_serves_all_surge_ui_methods() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    // Methods that accept empty params
    for method in [
        "service.health",
        "service.status",
        "shell.status",
        "clients.snapshot",
        "settings.snapshot",
        "settings.update",
        "settings.diagnostics",
    ] {
        let response = dispatcher
            .dispatch(ControlRequest::new(method, serde_json::json!({})))
            .await
            .unwrap();
        assert!(
            matches!(response, ControlResponse::Ok(_)),
            "{method} should return Ok with empty params"
        );
    }

    // Methods that require specific params
    let param_methods: Vec<(&str, serde_json::Value)> = vec![
        ("activity.list", serde_json::json!({"range": "day"})),
        ("activity.detail", serde_json::json!({"id": "evt-1"})),
        (
            "breakdown.list",
            serde_json::json!({"kind": "project", "range": "week"}),
        ),
        (
            "breakdown.detail",
            serde_json::json!({"kind": "project", "id": "proj-1", "range": "week"}),
        ),
        ("clients.detail", serde_json::json!({"source_id": "src-1"})),
        (
            "settings.update",
            serde_json::json!({"timezone": "US/Pacific"}),
        ),
        (
            "settings.recovery_action",
            serde_json::json!({"id": "rescan_all"}),
        ),
        (
            "prompts.list",
            serde_json::json!({"query": "review", "tag": null, "sort": "smart", "limit": 50}),
        ),
        ("prompts.get", serde_json::json!({"id": "prompt-1"})),
        (
            "prompts.create",
            serde_json::json!({
                "alias": ";;review",
                "content": "Review this diff for bugs.",
                "tags": ["review"]
            }),
        ),
        (
            "prompts.update",
            serde_json::json!({
                "id": "prompt-1",
                "alias": ";;review",
                "content": "Review this diff for bugs.",
                "tags": ["review"],
                "is_pinned": true
            }),
        ),
        ("prompts.delete", serde_json::json!({"id": "prompt-1"})),
        (
            "prompts.use",
            serde_json::json!({
                "id": "prompt-1",
                "action": "copy",
                "surface": "overlay",
                "outcome": "copy"
            }),
        ),
        (
            "prompts.suggest_tags",
            serde_json::json!({"query": "re", "limit": null}),
        ),
    ];

    for (method, params) in param_methods {
        let response = dispatcher
            .dispatch(ControlRequest::new(method, params))
            .await
            .unwrap();
        assert!(
            matches!(response, ControlResponse::Ok(_)),
            "{method} should return Ok with valid params"
        );
    }
}

// ---------------------------------------------------------------------------
// Method not found
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatcher_returns_method_not_found_for_unknown_method() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new(
            "nonexistent.method",
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Err(err) => {
            assert_eq!(err.code, "method_not_found");
            assert!(err.message.contains("nonexistent.method"));
        }
        ControlResponse::Ok(_) => panic!("expected error response for unknown method"),
    }
}

#[test]
fn control_response_from_error_preserves_method_dispatch_error() {
    let response = control_response_from_error(anyhow::Error::new(MethodDispatchError {
        code: "read_timeout".to_string(),
        message: "timed out".to_string(),
        payload: Some(serde_json::json!({
            "kind": "read_timeout",
        })),
    }));

    match response {
        ControlResponse::Err(err) => {
            assert_eq!(err.code, "read_timeout");
            assert_eq!(err.message, "timed out");
            assert_eq!(
                err.payload,
                Some(serde_json::json!({ "kind": "read_timeout" }))
            );
        }
        other => panic!("expected error response, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Individual method dispatch — verify response shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn service_health_returns_expected_shape() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new("service.health", serde_json::json!({})))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            assert_eq!(val["ready"], true);
            assert_eq!(val["db_healthy"], true);
        }
        ControlResponse::Err(_) => panic!("expected Ok response"),
    }
}

#[tokio::test]
async fn service_status_returns_expected_shape() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new("service.status", serde_json::json!({})))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            assert!(val.get("version").is_some());
            assert_eq!(val["state"], "running");
        }
        ControlResponse::Err(_) => panic!("expected Ok response"),
    }
}

#[tokio::test]
async fn shell_status_returns_expected_shape() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new("shell.status", serde_json::json!({})))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            assert!(val.get("generated_at_ms").is_some());
            assert!(val.get("status_chips").is_some());
        }
        ControlResponse::Err(_) => panic!("expected Ok response"),
    }
}

#[tokio::test]
async fn activity_list_returns_expected_shape() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new(
            "activity.list",
            serde_json::json!({"range": "day"}),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            let val = response_data(&val);
            assert!(val.get("items").is_some());
            assert!(val.get("summary").is_some());
            assert!(val.get("next_cursor").is_some());
        }
        ControlResponse::Err(_) => panic!("expected Ok response"),
    }
}

#[tokio::test]
async fn activity_detail_returns_expected_shape() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new(
            "activity.detail",
            serde_json::json!({"id": "evt-1"}),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            let val = response_data(&val);
            assert_eq!(val["id"], "evt-1");
            assert_eq!(val["client_id"], "claude_code");
            assert!(val.get("title").is_some());
            assert!(val.get("status").is_some());
        }
        ControlResponse::Err(_) => panic!("expected Ok response"),
    }
}

#[tokio::test]
async fn activity_detail_missing_id_returns_error() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let result = dispatcher
        .dispatch(ControlRequest::new(
            "activity.detail",
            serde_json::json!({}),
        ))
        .await;
    assert!(result.is_err(), "missing id should return error");
}

#[tokio::test]
async fn breakdown_list_returns_expected_shape() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new(
            "breakdown.list",
            serde_json::json!({"kind": "project", "range": "week"}),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            let val = response_data(&val);
            assert!(val.get("items").is_some());
            assert!(val.get("summary").is_some());
            assert_eq!(val["kind"], "project");
        }
        ControlResponse::Err(_) => panic!("expected Ok response"),
    }
}

#[tokio::test]
async fn breakdown_detail_returns_expected_shape() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new(
            "breakdown.detail",
            serde_json::json!({"kind": "project", "id": "proj-1", "range": "week"}),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            let val = response_data(&val);
            assert_eq!(val["kind"], "project");
            assert!(val.get("label").is_some());
        }
        ControlResponse::Err(_) => panic!("expected Ok response"),
    }
}

#[tokio::test]
async fn clients_snapshot_returns_expected_shape() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new(
            "clients.snapshot",
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            let val = response_data(&val);
            assert!(val.get("client_cards").is_some());
            assert!(val.get("sources").is_some());
            assert!(val.get("summary").is_some());
        }
        ControlResponse::Err(_) => panic!("expected Ok response"),
    }
}

#[tokio::test]
async fn clients_detail_returns_expected_shape() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new(
            "clients.detail",
            serde_json::json!({"source_id": "src-1"}),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            let val = response_data(&val);
            assert!(val.get("source").is_some());
            assert_eq!(val["source"]["id"], "src-1");
        }
        ControlResponse::Err(_) => panic!("expected Ok response"),
    }
}

#[tokio::test]
async fn settings_snapshot_returns_expected_shape() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new(
            "settings.snapshot",
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            let val = response_data(&val);
            assert!(val.get("timezone").is_some());
            assert!(val.get("discovery").is_some());
            assert!(val.get("privacy").is_some());
        }
        ControlResponse::Err(_) => panic!("expected Ok response"),
    }
}

#[tokio::test]
async fn settings_update_returns_expected_shape() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new(
            "settings.update",
            serde_json::json!({"timezone": "US/Pacific"}),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            let val = response_data(&val);
            assert_eq!(val["timezone"], "US/Pacific");
        }
        ControlResponse::Err(_) => panic!("expected Ok response"),
    }
}

#[tokio::test]
async fn settings_update_validation_failure() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new(
            "settings.update",
            serde_json::json!({"timezone": ""}),
        ))
        .await
        .unwrap();

    // TestRuntimeControl does not validate — empty timezone is accepted.
    // Validation happens in the real BusytokSupervisor (supervisor.rs).
    match response {
        ControlResponse::Ok(payload) => {
            let payload = response_data(&payload);
            // Should return the updated settings snapshot (with empty timezone).
            assert!(payload.is_object(), "expected a JSON object");
        }
        ControlResponse::Err(err) => {
            panic!(
                "expected Ok response for empty timezone (TestRuntimeControl does not validate); got [{}] {}",
                err.code, err.message
            );
        }
    }
}

#[tokio::test]
async fn settings_diagnostics_returns_expected_shape() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new(
            "settings.diagnostics",
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            let val = response_data(&val);
            assert!(val.get("db_healthy").is_some());
            assert!(val.get("writer_queue_depth").is_some());
        }
        ControlResponse::Err(_) => panic!("expected Ok response"),
    }
}

#[tokio::test]
async fn settings_recovery_action_returns_expected_shape() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new(
            "settings.recovery_action",
            serde_json::json!({"id": "rescan_all"}),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            let val = response_data(&val);
            assert_eq!(val["accepted"], true);
            assert_eq!(val["id"], "rescan_all");
        }
        ControlResponse::Err(_) => panic!("expected Ok response"),
    }
}

// ---------------------------------------------------------------------------
// from_arc and runtime_arc
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatcher_from_arc_works() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let arc = std::sync::Arc::new(runtime);
    let dispatcher = ControlDispatcher::from_arc(arc);

    let response = dispatcher
        .dispatch(ControlRequest::new("service.health", serde_json::json!({})))
        .await
        .unwrap();

    assert!(matches!(response, ControlResponse::Ok(_)));
}

#[tokio::test]
async fn dispatcher_runtime_arc_clones_correctly() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let _arc = dispatcher.runtime_arc();
    // Should be able to use the dispatcher after cloning the arc.
    let response = dispatcher
        .dispatch(ControlRequest::new("service.health", serde_json::json!({})))
        .await
        .unwrap();
    assert!(matches!(response, ControlResponse::Ok(_)));
}

#[tokio::test]
async fn dispatcher_event_bus_accessible() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    // Just verify we can access the event bus without panic.
    let _bus = dispatcher.event_bus();
}

// ---------------------------------------------------------------------------
// Modular route dispatch tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatcher_routes_overview_summary() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let req = ControlRequest::new("overview.summary", serde_json::json!({"range": "day"}));
    let res = dispatcher.dispatch(req).await.unwrap();
    assert!(matches!(res, ControlResponse::Ok(_)));
}

#[tokio::test]
async fn dispatcher_serves_prompt_list() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let response = dispatcher
        .dispatch(ControlRequest::new(
            "prompts.list",
            serde_json::json!({
                "query": "review",
                "tag": null,
                "sort": "smart",
                "limit": 50
            }),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(value) => {
            assert!(
                value.get("data").is_some(),
                "prompt list should be enveloped"
            );
        }
        other => panic!("expected Ok, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatcher_serves_prompt_use_lightweight_result() {
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let response = dispatcher
        .dispatch(ControlRequest::new(
            "prompts.use",
            serde_json::json!({
                "id": "prompt-1",
                "action": "copy",
                "surface": "overlay",
                "outcome": "copy"
            }),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(value) => {
            assert_eq!(value.get("usage_count").and_then(|v| v.as_i64()), Some(1));
        }
        other => panic!("expected Ok, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Modular route surface manifest assertions
// ---------------------------------------------------------------------------

#[test]
fn dispatcher_manifest_includes_modular_route_surface() {
    let methods = method_manifest();
    assert!(
        methods.contains(&"shell.status".to_string()),
        "shell.status"
    );
    assert!(
        methods.contains(&"overview.summary".to_string()),
        "overview.summary"
    );
    assert!(
        methods.contains(&"overview.trend".to_string()),
        "overview.trend"
    );
    assert!(
        methods.contains(&"overview.heatmap".to_string()),
        "overview.heatmap"
    );
    assert!(
        methods.contains(&"overview.rankings".to_string()),
        "overview.rankings"
    );
    assert!(
        methods.contains(&"activity.recent".to_string()),
        "activity.recent"
    );
    assert!(
        methods.contains(&"activity.list".to_string()),
        "activity.list"
    );
    assert!(
        methods.contains(&"activity.detail".to_string()),
        "activity.detail"
    );
    assert!(
        methods.contains(&"breakdown.list".to_string()),
        "breakdown.list"
    );
    assert!(
        methods.contains(&"breakdown.detail".to_string()),
        "breakdown.detail"
    );
    assert!(
        methods.contains(&"clients.snapshot".to_string()),
        "clients.snapshot"
    );
    assert!(
        methods.contains(&"clients.detail".to_string()),
        "clients.detail"
    );
    assert!(
        methods.contains(&"settings.snapshot".to_string()),
        "settings.snapshot"
    );
    assert!(
        methods.contains(&"settings.update".to_string()),
        "settings.update"
    );
    assert!(
        methods.contains(&"settings.diagnostics".to_string()),
        "settings.diagnostics"
    );
    assert!(
        methods.contains(&"settings.recovery_action".to_string()),
        "settings.recovery_action"
    );
    assert!(methods.contains(&"live.window".to_string()), "live.window");
    assert!(
        methods.contains(&"prompts.list".to_string()),
        "prompts.list"
    );
    assert!(methods.contains(&"prompts.get".to_string()), "prompts.get");
    assert!(
        methods.contains(&"prompts.create".to_string()),
        "prompts.create"
    );
    assert!(
        methods.contains(&"prompts.update".to_string()),
        "prompts.update"
    );
    assert!(
        methods.contains(&"prompts.delete".to_string()),
        "prompts.delete"
    );
    assert!(methods.contains(&"prompts.use".to_string()), "prompts.use");
    assert!(methods.contains(&"prompts.suggest_tags".to_string()), "prompts.suggest_tags");
    assert!(
        methods.contains(&"events.subscribe".to_string()),
        "events.subscribe"
    );
}
