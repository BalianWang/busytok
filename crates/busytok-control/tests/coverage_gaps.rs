#![allow(
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    clippy::await_holding_lock,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::field_reassign_with_default,
    clippy::inconsistent_digit_grouping,
    clippy::len_zero,
    clippy::identity_op,
    clippy::useless_vec,
    clippy::manual_dangling_ptr,
    clippy::unused_async,
    clippy::absurd_extreme_comparisons,
    unused_variables,
    unused_imports,
    dead_code
)]
//! Coverage gap tests for `busytok-control`.
//!
//! These tests target uncovered code paths identified by `cargo llvm-cov`:
//! - `dispatch.rs`: Arc<T> blanket impl, dispatch match arms for untested
//!   methods, settings.update validation error handling, latest_event_seq,
//!   control_response_from_error no-payload branch.
//! - `server.rs`: endpoint() accessor, invalid handshake, invalid UTF-8 frame,
//!   partial body read, run-loop error logging, gap detection, subscription
//!   broadcast / shutdown / write-error paths, event_payload_json variants.
//! - `client.rs`: recv_event_batch, subscribe_with_meta_and_last_event_seq,
//!   invalid handshake ack detection.

use std::sync::Arc;
use std::time::Duration;

use busytok_control::dispatch::{control_response_from_error, MethodDispatchError};
use busytok_control::transport::in_memory::InMemoryTransport;
use busytok_control::transport::ControlTransport;
use busytok_control::{
    ControlClient, ControlDispatcher, ControlServer, RuntimeControl, TestRuntimeControl,
};
use busytok_domain::ProviderKind;
use busytok_events::{AppEvent, AppEventBus, PublishedEvent};
use busytok_protocol::dto::*;
use busytok_protocol::{ControlRequest, ControlResponse};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn unique_endpoint(label: &str) -> String {
    format!(
        "cov-{}-{}-{}",
        label,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// Write a 4-byte big-endian length prefix + raw payload (no validation).
async fn write_raw_frame<W: AsyncWriteExt + Unpin>(writer: &mut W, payload: &[u8]) {
    let len = payload.len() as u32;
    writer.write_all(&len.to_be_bytes()).await.unwrap();
    writer.write_all(payload).await.unwrap();
    writer.flush().await.unwrap();
}

/// Bind and start an in-memory control server. Returns the server (wrapped in
/// Arc for shared ownership), the endpoint string, and the JoinHandle of the
/// accept loop task.
async fn spawn_in_memory_server(
    runtime: Arc<dyn RuntimeControl>,
    label: &str,
) -> (
    Arc<ControlServer<InMemoryTransport>>,
    String,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let endpoint = unique_endpoint(label);
    let server = ControlServer::<InMemoryTransport>::bind(&endpoint, runtime)
        .await
        .unwrap();
    let server = Arc::new(server);
    let server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };
    (server, endpoint, server_task)
}

/// Bind an in-memory control server WITHOUT spawning the `run()` accept loop.
/// Callers that need single-shot `accept_one()` semantics use this to avoid
/// racing with the run loop for incoming connections.
async fn bind_in_memory_server(
    runtime: Arc<dyn RuntimeControl>,
    label: &str,
) -> (Arc<ControlServer<InMemoryTransport>>, String) {
    let endpoint = unique_endpoint(label);
    let server = ControlServer::<InMemoryTransport>::bind(&endpoint, runtime)
        .await
        .unwrap();
    (Arc::new(server), endpoint)
}

// ---------------------------------------------------------------------------
// 1. control_response_from_error: no-payload and non-dispatch-error branches
// ---------------------------------------------------------------------------

#[test]
fn control_response_from_error_no_payload_returns_plain_err() {
    // Covers dispatch.rs line 52: MethodDispatchError without payload.
    let response = control_response_from_error(anyhow::Error::new(MethodDispatchError {
        code: "read_timeout".to_string(),
        message: "timed out".to_string(),
        payload: None,
    }));

    match response {
        ControlResponse::Err(err) => {
            assert_eq!(err.code, "read_timeout");
            assert_eq!(err.message, "timed out");
            assert!(err.payload.is_none(), "payload should be None");
        }
        other => panic!("expected Err response, got {other:?}"),
    }
}

#[test]
fn control_response_from_error_non_dispatch_error_returns_internal_error() {
    // Covers dispatch.rs line 55: fallback for non-MethodDispatchError errors.
    let response = control_response_from_error(anyhow::anyhow!("something broke"));

    match response {
        ControlResponse::Err(err) => {
            assert_eq!(err.code, "internal_error");
            assert!(err.message.contains("something broke"));
        }
        other => panic!("expected Err response, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 2. latest_event_seq: trait default + dispatcher delegation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatcher_latest_event_seq_returns_none_by_default() {
    // Covers dispatch.rs lines 228-230 (trait default) and 275-277
    // (ControlDispatcher::latest_event_seq delegation).
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    // TestRuntimeControl doesn't override latest_event_seq, so it uses the
    // trait default which returns None.
    assert_eq!(dispatcher.latest_event_seq(), None);

    // Also exercise the default impl directly on the runtime.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    assert_eq!(runtime.latest_event_seq(), None);

    // record_diagnostic default impl is a no-op; verify it doesn't panic.
    dispatcher.record_diagnostic("info", "test_code", "test message");
}

// ---------------------------------------------------------------------------
// 3. Arc<T> blanket impl: covers dispatch.rs lines 1283-1508
// ---------------------------------------------------------------------------

#[tokio::test]
async fn arc_blanket_impl_delegates_all_runtime_control_methods() {
    // The blanket `impl<T: RuntimeControl> RuntimeControl for Arc<T>`
    // forwards every method to `(**self)`. Calling methods directly on a
    // concrete `Arc<TestRuntimeControl>` (not via `Arc<dyn RuntimeControl>`)
    // exercises the blanket impl bodies.
    let runtime: Arc<TestRuntimeControl> =
        Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());

    // Service
    assert!(runtime.service_health().await.unwrap().ready);
    assert!(runtime.service_status().await.unwrap().state == "running");
    let _ = runtime.shell_status().await.unwrap();

    // Overview
    let _ = runtime
        .overview_summary(OverviewSummaryRequestDto {
            range: RangePresetDto::Day,
        })
        .await
        .unwrap();
    let _ = runtime
        .overview_trend(OverviewTrendRequestDto {
            range: RangePresetDto::Day,
            granularity: None,
        })
        .await
        .unwrap();
    let _ = runtime
        .overview_heatmap(OverviewHeatmapRequestDto {
            range: RangePresetDto::Day,
        })
        .await
        .unwrap();
    let _ = runtime
        .overview_rankings(OverviewRankingsRequestDto {
            range: RangePresetDto::Day,
        })
        .await
        .unwrap();

    // Receipt
    let _ = runtime
        .receipt_daily(ReceiptDailyRequestDto { date: None })
        .await
        .unwrap();

    // Activity
    let _ = runtime
        .activity_recent(ActivityRecentRequestDto {
            range: RangePresetDto::Day,
            limit: None,
        })
        .await
        .unwrap();
    let _ = runtime
        .activity_list(ActivityListRequestDto {
            range: RangePresetDto::Day,
            cursor: None,
            limit: None,
            client_id: None,
            source_id: None,
            project_hash: None,
            model_id: None,
        })
        .await
        .unwrap();
    let _ = runtime
        .activity_detail(ActivityDetailRequestDto {
            id: "evt-1".to_string(),
        })
        .await
        .unwrap();

    // Breakdown
    let _ = runtime
        .breakdown_list(BreakdownListRequestDto {
            kind: BreakdownKindDto::Project,
            range: RangePresetDto::Week,
            cursor: None,
            limit: None,
        })
        .await
        .unwrap();
    let _ = runtime
        .breakdown_detail(BreakdownDetailRequestDto {
            kind: BreakdownKindDto::Project,
            id: "proj-1".to_string(),
            range: RangePresetDto::Week,
        })
        .await
        .unwrap();

    // Clients
    let _ = runtime
        .clients_snapshot(ClientsSnapshotRequestDto {
            cursor: None,
            limit: None,
            client_id: None,
            scan_state: None,
        })
        .await
        .unwrap();
    let _ = runtime
        .clients_detail(ClientSourceDetailRequestDto {
            source_id: "src-1".to_string(),
        })
        .await
        .unwrap();

    // Settings
    let _ = runtime.settings_snapshot().await.unwrap();
    let _ = runtime
        .settings_update(SettingsUpdateRequestDto {
            timezone: None,
            week_starts_on: None,
            discovery: None,
            privacy: None,
            prompt_palette_default_action: None,
        })
        .await
        .unwrap();
    let _ = runtime.settings_diagnostics().await.unwrap();
    let _ = runtime
        .settings_recovery_action(SettingsRecoveryActionRequestDto {
            id: SettingsRecoveryActionIdDto::RescanAll,
        })
        .await
        .unwrap();

    // Live
    let _ = runtime
        .live_window(LiveWindowRequestDto {
            window_seconds: None,
        })
        .await
        .unwrap();

    // Prompts
    let _ = runtime
        .prompts_list(PromptListQueryDto {
            query: None,
            tag: None,
            sort: None,
            limit: None,
        })
        .await
        .unwrap();
    let _ = runtime
        .prompts_get(PromptGetRequestDto {
            id: "prompt-1".to_string(),
        })
        .await
        .unwrap();
    let _ = runtime
        .prompts_create(PromptCreateRequestDto {
            content: "test".to_string(),
            alias: None,
            tags: vec![],
        })
        .await
        .unwrap();
    let _ = runtime
        .prompts_update(PromptUpdateRequestDto {
            id: "prompt-1".to_string(),
            content: "test".to_string(),
            alias: None,
            tags: vec![],
            is_pinned: false,
        })
        .await
        .unwrap();
    let _ = runtime
        .prompts_delete(PromptDeleteRequestDto {
            id: "prompt-1".to_string(),
        })
        .await
        .unwrap();
    let _ = runtime
        .prompts_use(PromptUseRequestDto {
            id: "prompt-1".to_string(),
            action: PromptActionDto::CopyAndPaste,
            surface: PromptUseSurfaceDto::Overlay,
            outcome: PromptUseOutcomeDto::Copy,
            failure_reason: None,
        })
        .await
        .unwrap();
    let _ = runtime
        .suggest_tags(PromptSuggestTagsRequestDto {
            query: None,
            limit: None,
        })
        .await
        .unwrap();

    // Subagents
    let _ = runtime
        .subagent_delegate(SubagentDelegateRequestDto {
            subagent_name: "test".to_string(),
            subagent_id: None,
            cwd: "/tmp".to_string(),
            profile: "default".to_string(),
            intent: None,
            prompt: "test".to_string(),
            prompt_artifact_ref: None,
            timeout_seconds: None,
            model_override: None,
            source_harness: None,
            source_session_id: None,
        })
        .await
        .unwrap();
    let _ = runtime
        .subagent_list(SubagentListRequestDto {
            status: None,
            project: None,
            include_deleted: None,
        })
        .await
        .unwrap();
    let _ = runtime
        .subagent_show(SubagentResolveRequestDto {
            name: None,
            id: Some("sa-1".to_string()),
            cwd: None,
        })
        .await
        .unwrap();
    let _ = runtime
        .subagent_tasks(SubagentTasksRequestDto {
            name: None,
            id: Some("sa-1".to_string()),
            cwd: None,
            limit: None,
        })
        .await
        .unwrap();
    let _ = runtime
        .subagent_hibernate(SubagentResolveRequestDto {
            name: None,
            id: Some("sa-1".to_string()),
            cwd: None,
        })
        .await
        .unwrap();
    let _ = runtime
        .subagent_delete(SubagentDeleteRequestDto {
            name: None,
            id: Some("sa-1".to_string()),
            cwd: None,
            hard: None,
        })
        .await
        .unwrap();
    let _ = runtime
        .subagent_runtime_status(SubagentRuntimeStatusRequestDto { project: None })
        .await
        .unwrap();

    // Providers — provider_create/update/delete return errors in
    // TestRuntimeControl, but the blanket impl body still executes.
    let _ = runtime
        .provider_create(ProviderCreateRequestDto {
            name: "P".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "http://x".to_string(),
            api_key: None,
        })
        .await;
    let list = runtime.provider_list().await.unwrap();
    assert!(list.providers.is_empty());
    let _ = runtime
        .provider_update(ProviderUpdateRequestDto {
            id: "p1".to_string(),
            name: None,
            base_url: None,
            enabled: None,
            api_key: None,
        })
        .await;
    let _ = runtime
        .provider_delete(ProviderDeleteRequestDto {
            id: "p1".to_string(),
        })
        .await;
    let test_result = runtime
        .provider_test_connection(ProviderTestConnectionRequestDto {
            id: "p1".to_string(),
        })
        .await
        .unwrap();
    assert!(!test_result.ok);

    // Pi sidecar locator
    let locator = runtime
        .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
            runtime_dir: "/tmp/pi".to_string(),
            enabled: true,
        })
        .await
        .unwrap();
    assert!(locator.in_memory_updated);

    // Profiles — return errors in TestRuntimeControl; blanket impl still runs.
    let _ = runtime
        .profile_create(ProfileCreateRequestDto {
            id: "prof1".to_string(),
            model: "m".to_string(),
            provider_id: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await;
    let _ = runtime
        .profile_update(ProfileUpdateRequestDto {
            id: "prof1".to_string(),
            provider_id: None,
            model: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await;
    let _ = runtime
        .profile_delete(ProfileDeleteRequestDto {
            id: "prof1".to_string(),
        })
        .await;

    // Non-async methods
    let _bus = runtime.event_bus();
    let _ = runtime.latest_event_seq();
    runtime.on_request_meta(&RequestMeta::default());
    runtime.record_diagnostic("info", "test", "message");
}

// ---------------------------------------------------------------------------
// 4. Dispatcher match arms for previously-untested methods
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatcher_routes_overview_trend() {
    // Covers dispatch.rs lines 313-316.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"range": "day"});
    let response = dispatcher
        .dispatch(ControlRequest::new("overview.trend", params))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert!(val.get("data").is_some(), "should be enveloped");
            assert!(val["data"].get("trend").is_some());
        }
        _ => panic!("expected Ok"),
    }
}

#[tokio::test]
async fn dispatcher_routes_overview_heatmap() {
    // Covers dispatch.rs lines 319-322.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"range": "week"});
    let response = dispatcher
        .dispatch(ControlRequest::new("overview.heatmap", params))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert!(val.get("data").is_some());
            assert!(val["data"].get("heatmap").is_some());
        }
        _ => panic!("expected Ok"),
    }
}

#[tokio::test]
async fn dispatcher_routes_overview_rankings() {
    // Covers dispatch.rs lines 325-328.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"range": "month"});
    let response = dispatcher
        .dispatch(ControlRequest::new("overview.rankings", params))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert!(val.get("data").is_some());
            assert!(val["data"].get("rankings").is_some());
        }
        _ => panic!("expected Ok"),
    }
}

#[tokio::test]
async fn dispatcher_routes_receipt_daily() {
    // Covers dispatch.rs lines 333-336.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"date": "2026-06-26"});
    let response = dispatcher
        .dispatch(ControlRequest::new("receipt.daily", params))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert!(val.get("data").is_some(), "should be enveloped");
            assert_eq!(val["data"]["date"], "2026-06-26");
            assert!(val["data"].get("metrics").is_some());
        }
        _ => panic!("expected Ok"),
    }
}

#[tokio::test]
async fn dispatcher_routes_activity_recent() {
    // Covers dispatch.rs lines 341-344.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"range": "week", "limit": 5});
    let response = dispatcher
        .dispatch(ControlRequest::new("activity.recent", params))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert!(val.get("data").is_some());
            assert!(val["data"].get("recent_activity").is_some());
        }
        _ => panic!("expected Ok"),
    }
}

#[tokio::test]
async fn dispatcher_routes_live_window() {
    // Covers dispatch.rs lines 434-437.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"window_seconds": 60});
    let response = dispatcher
        .dispatch(ControlRequest::new("live.window", params))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert!(val.get("data").is_some());
            assert!(val["data"].get("exact_samples").is_some());
        }
        _ => panic!("expected Ok"),
    }
}

#[tokio::test]
async fn dispatcher_routes_subagent_runtime_status() {
    // Covers dispatch.rs lines 522-527.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({});
    let response = dispatcher
        .dispatch(ControlRequest::new("subagent.runtime_status", params))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert!(val.get("data").is_some());
            assert!(val["data"].get("pressure_gate").is_some());
        }
        _ => panic!("expected Ok"),
    }
}

#[tokio::test]
async fn dispatcher_routes_provider_list() {
    // Covers dispatch.rs lines 538-539.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let response = dispatcher
        .dispatch(ControlRequest::new("provider.list", serde_json::json!({})))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert!(val.get("providers").is_some());
            assert_eq!(val["providers"].as_array().unwrap().len(), 0);
        }
        _ => panic!("expected Ok"),
    }
}

#[tokio::test]
async fn dispatcher_routes_provider_test_connection() {
    // Covers dispatch.rs lines 554-559.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"id": "p1"});
    let response = dispatcher
        .dispatch(ControlRequest::new("provider.test_connection", params))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert_eq!(val["ok"], false);
        }
        _ => panic!("expected Ok"),
    }
}

#[tokio::test]
async fn dispatcher_routes_pi_sidecar_locator_update() {
    // Covers dispatch.rs lines 564-569.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"runtime_dir": "/tmp/pi", "enabled": true});
    let response = dispatcher
        .dispatch(ControlRequest::new("pi_sidecar_locator_update", params))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert_eq!(val["runtime_dir"], "/tmp/pi");
            assert_eq!(val["enabled"], true);
            assert_eq!(val["in_memory_updated"], true);
        }
        _ => panic!("expected Ok"),
    }
}

#[tokio::test]
async fn dispatcher_routes_provider_create_returns_error() {
    // Covers dispatch.rs lines 532-535 — TestRuntimeControl bails on
    // provider_create. The dispatcher should propagate the error.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({
        "name": "P",
        "provider_kind": "openai_compatible",
        "base_url": "http://x"
    });
    let result = dispatcher
        .dispatch(ControlRequest::new("provider.create", params))
        .await;
    assert!(
        result.is_err(),
        "provider.create should propagate the error"
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("not yet implemented"));
}

#[tokio::test]
async fn dispatcher_routes_provider_update_returns_error() {
    // Covers dispatch.rs lines 542-545.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"id": "p1"});
    let result = dispatcher
        .dispatch(ControlRequest::new("provider.update", params))
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("not yet implemented"));
}

#[tokio::test]
async fn dispatcher_routes_provider_delete_returns_error() {
    // Covers dispatch.rs lines 548-551.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"id": "p1"});
    let result = dispatcher
        .dispatch(ControlRequest::new("provider.delete", params))
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("not yet implemented"));
}

#[tokio::test]
async fn dispatcher_routes_profile_create_returns_error() {
    // Covers dispatch.rs lines 574-577.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"id": "prof1", "model": "m"});
    let result = dispatcher
        .dispatch(ControlRequest::new("profile.create", params))
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("not yet implemented"));
}

#[tokio::test]
async fn dispatcher_routes_profile_update_returns_error() {
    // Covers dispatch.rs lines 580-583.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"id": "prof1"});
    let result = dispatcher
        .dispatch(ControlRequest::new("profile.update", params))
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("not yet implemented"));
}

#[tokio::test]
async fn dispatcher_routes_profile_delete_returns_error() {
    // Covers dispatch.rs lines 586-589.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"id": "prof1"});
    let result = dispatcher
        .dispatch(ControlRequest::new("profile.delete", params))
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("not yet implemented"));
}

// ---------------------------------------------------------------------------
// 5. Invalid params for every method that deserializes params
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatcher_invalid_params_returns_error_for_every_typed_method() {
    // Covers all `map_err(|e| anyhow::anyhow!("invalid params for ...: {e}"))`
    // branches in the dispatch match arms. Sending a non-object JSON value
    // (42) fails serde deserialization for every struct DTO.
    let runtime = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let dispatcher = ControlDispatcher::new(runtime);

    let methods: Vec<&str> = vec![
        "overview.summary",
        "overview.trend",
        "overview.heatmap",
        "overview.rankings",
        "receipt.daily",
        "activity.recent",
        "activity.list",
        "activity.detail",
        "breakdown.list",
        "breakdown.detail",
        "clients.snapshot",
        "clients.detail",
        "settings.update",
        "settings.recovery_action",
        "live.window",
        "prompts.list",
        "prompts.get",
        "prompts.create",
        "prompts.update",
        "prompts.delete",
        "prompts.use",
        "prompts.suggest_tags",
        "subagent.delegate",
        "subagent.list",
        "subagent.show",
        "subagent.tasks",
        "subagent.hibernate",
        "subagent.delete",
        "subagent.runtime_status",
        "provider.create",
        "provider.update",
        "provider.delete",
        "provider.test_connection",
        "pi_sidecar_locator_update",
        "profile.create",
        "profile.update",
        "profile.delete",
    ];

    for method in methods {
        let result = dispatcher
            .dispatch(ControlRequest::new(method, serde_json::json!(42)))
            .await;
        assert!(
            result.is_err(),
            "{method} should return Err when params cannot be deserialized"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains(&format!("invalid params for {method}")),
            "{method} error message should mention invalid params, got: {msg}"
        );
    }
}

// ---------------------------------------------------------------------------
// 6. settings.update validation failure paths
// ---------------------------------------------------------------------------

/// Runtime wrapper that overrides `settings_update` to return a configurable
/// error, exercising the SETTINGS_VALIDATION_FAILED handling in dispatch.rs.
struct SettingsValidationRuntime {
    inner: TestRuntimeControl,
    error_message: String,
}

#[async_trait::async_trait]
impl RuntimeControl for SettingsValidationRuntime {
    async fn service_health(&self) -> anyhow::Result<ServiceHealthDto> {
        self.inner.service_health().await
    }
    async fn service_status(&self) -> anyhow::Result<ServiceStatusDto> {
        self.inner.service_status().await
    }
    async fn shell_status(&self) -> anyhow::Result<ShellStatusDto> {
        self.inner.shell_status().await
    }
    async fn overview_summary(
        &self,
        req: OverviewSummaryRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewSummaryDto>> {
        self.inner.overview_summary(req).await
    }
    async fn overview_trend(
        &self,
        req: OverviewTrendRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewTrendResponseDto>> {
        self.inner.overview_trend(req).await
    }
    async fn overview_heatmap(
        &self,
        req: OverviewHeatmapRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewHeatmapResponseDto>> {
        self.inner.overview_heatmap(req).await
    }
    async fn overview_rankings(
        &self,
        req: OverviewRankingsRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewRankingsResponseDto>> {
        self.inner.overview_rankings(req).await
    }
    async fn receipt_daily(
        &self,
        req: ReceiptDailyRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ReceiptDailyDto>> {
        self.inner.receipt_daily(req).await
    }
    async fn activity_recent(
        &self,
        req: ActivityRecentRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ActivityRecentResponseDto>> {
        self.inner.activity_recent(req).await
    }
    async fn activity_list(
        &self,
        req: ActivityListRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ActivityListResponseDto>> {
        self.inner.activity_list(req).await
    }
    async fn activity_detail(
        &self,
        req: ActivityDetailRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ActivityDetailDto>> {
        self.inner.activity_detail(req).await
    }
    async fn breakdown_list(
        &self,
        req: BreakdownListRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<BreakdownListResponseDto>> {
        self.inner.breakdown_list(req).await
    }
    async fn breakdown_detail(
        &self,
        req: BreakdownDetailRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<BreakdownDetailDto>> {
        self.inner.breakdown_detail(req).await
    }
    async fn clients_snapshot(
        &self,
        req: ClientsSnapshotRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ClientsSnapshotDto>> {
        self.inner.clients_snapshot(req).await
    }
    async fn clients_detail(
        &self,
        req: ClientSourceDetailRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ClientSourceDetailDto>> {
        self.inner.clients_detail(req).await
    }
    async fn settings_snapshot(&self) -> anyhow::Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        self.inner.settings_snapshot().await
    }
    async fn settings_update(
        &self,
        req: SettingsUpdateRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        // Override: return the configured error message.
        Err(anyhow::anyhow!("{}", self.error_message))
    }
    async fn settings_diagnostics(
        &self,
    ) -> anyhow::Result<ReadEnvelopeDto<SettingsDiagnosticsDto>> {
        self.inner.settings_diagnostics().await
    }
    async fn settings_recovery_action(
        &self,
        req: SettingsRecoveryActionRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<SettingsRecoveryActionResponseDto>> {
        self.inner.settings_recovery_action(req).await
    }
    async fn live_window(
        &self,
        req: LiveWindowRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<LiveWindowDto>> {
        self.inner.live_window(req).await
    }
    async fn prompts_list(
        &self,
        req: PromptListQueryDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptListResponseDto>> {
        self.inner.prompts_list(req).await
    }
    async fn prompts_get(
        &self,
        req: PromptGetRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
        self.inner.prompts_get(req).await
    }
    async fn prompts_create(
        &self,
        req: PromptCreateRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
        self.inner.prompts_create(req).await
    }
    async fn prompts_update(
        &self,
        req: PromptUpdateRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
        self.inner.prompts_update(req).await
    }
    async fn prompts_delete(
        &self,
        req: PromptDeleteRequestDto,
    ) -> anyhow::Result<PromptDeleteResultDto> {
        self.inner.prompts_delete(req).await
    }
    async fn prompts_use(&self, req: PromptUseRequestDto) -> anyhow::Result<PromptUseResultDto> {
        self.inner.prompts_use(req).await
    }
    async fn suggest_tags(
        &self,
        req: PromptSuggestTagsRequestDto,
    ) -> anyhow::Result<PromptSuggestTagsResponseDto> {
        self.inner.suggest_tags(req).await
    }
    async fn subagent_delegate(
        &self,
        req: SubagentDelegateRequestDto,
    ) -> anyhow::Result<SubagentDelegateResponseDto> {
        self.inner.subagent_delegate(req).await
    }
    async fn subagent_list(
        &self,
        req: SubagentListRequestDto,
    ) -> anyhow::Result<SubagentListResponseDto> {
        self.inner.subagent_list(req).await
    }
    async fn subagent_show(
        &self,
        req: SubagentResolveRequestDto,
    ) -> anyhow::Result<SubagentDetailDto> {
        self.inner.subagent_show(req).await
    }
    async fn subagent_tasks(
        &self,
        req: SubagentTasksRequestDto,
    ) -> anyhow::Result<SubagentTasksResponseDto> {
        self.inner.subagent_tasks(req).await
    }
    async fn subagent_hibernate(
        &self,
        req: SubagentResolveRequestDto,
    ) -> anyhow::Result<SubagentAckDto> {
        self.inner.subagent_hibernate(req).await
    }
    async fn subagent_delete(
        &self,
        req: SubagentDeleteRequestDto,
    ) -> anyhow::Result<SubagentAckDto> {
        self.inner.subagent_delete(req).await
    }
    async fn subagent_runtime_status(
        &self,
        req: SubagentRuntimeStatusRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<SubagentRuntimeStatusDto>> {
        self.inner.subagent_runtime_status(req).await
    }
    async fn provider_create(&self, req: ProviderCreateRequestDto) -> anyhow::Result<ProviderDto> {
        self.inner.provider_create(req).await
    }
    async fn provider_list(&self) -> anyhow::Result<ProviderListResponseDto> {
        self.inner.provider_list().await
    }
    async fn provider_update(&self, req: ProviderUpdateRequestDto) -> anyhow::Result<ProviderDto> {
        self.inner.provider_update(req).await
    }
    async fn provider_delete(&self, req: ProviderDeleteRequestDto) -> anyhow::Result<()> {
        self.inner.provider_delete(req).await
    }
    async fn provider_test_connection(
        &self,
        req: ProviderTestConnectionRequestDto,
    ) -> anyhow::Result<ProviderTestConnectionResponseDto> {
        self.inner.provider_test_connection(req).await
    }
    async fn model_create(&self, req: ModelCreateRequestDto) -> anyhow::Result<ModelCatalogEntryDto> {
        self.inner.model_create(req).await
    }
    async fn model_list(&self, req: ModelListRequestDto) -> anyhow::Result<ModelListResponseDto> {
        self.inner.model_list(req).await
    }
    async fn model_update(&self, req: ModelUpdateRequestDto) -> anyhow::Result<()> {
        self.inner.model_update(req).await
    }
    async fn model_delete(&self, req: ModelDeleteRequestDto) -> anyhow::Result<()> {
        self.inner.model_delete(req).await
    }
    async fn model_tags_update(&self, req: ModelTagUpdateDto) -> anyhow::Result<()> {
        self.inner.model_tags_update(req).await
    }
    async fn pi_sidecar_locator_update(
        &self,
        req: PiSidecarLocatorUpdateRequestDto,
    ) -> anyhow::Result<PiSidecarLocatorUpdateResponseDto> {
        self.inner.pi_sidecar_locator_update(req).await
    }
    async fn profile_create(&self, req: ProfileCreateRequestDto) -> anyhow::Result<ProfileDto> {
        self.inner.profile_create(req).await
    }
    async fn profile_update(&self, req: ProfileUpdateRequestDto) -> anyhow::Result<ProfileDto> {
        self.inner.profile_update(req).await
    }
    async fn profile_delete(&self, req: ProfileDeleteRequestDto) -> anyhow::Result<()> {
        self.inner.profile_delete(req).await
    }
    fn event_bus(&self) -> &AppEventBus {
        self.inner.event_bus()
    }
    fn on_request_meta(&self, meta: &RequestMeta) {
        self.inner.on_request_meta(meta);
    }
}

#[tokio::test]
async fn settings_update_validation_failed_with_valid_json_payload() {
    // Covers dispatch.rs lines 397-409: when settings_update returns an error
    // whose message starts with "SETTINGS_VALIDATION_FAILED: " and the
    // remainder is valid JSON, the dispatcher should return a
    // `settings_validation_failed` error response carrying the payload.
    let runtime = SettingsValidationRuntime {
        inner: TestRuntimeControl::with_claude_fixture().await.unwrap(),
        error_message: r#"SETTINGS_VALIDATION_FAILED: {"errors":[{"code":"invalid_timezone","field":"timezone"}]}"#.to_string(),
    };
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new(
            "settings.update",
            serde_json::json!({"timezone": "bad"}),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Err(err) => {
            assert_eq!(err.code, "settings_validation_failed");
            assert_eq!(err.message, "Settings validation failed");
            let payload = err.payload.expect("payload should be present");
            assert_eq!(payload["errors"][0]["code"], "invalid_timezone");
        }
        other => panic!("expected Err response, got {other:?}"),
    }
}

#[tokio::test]
async fn settings_update_validation_failed_with_invalid_json_payload_propagates_error() {
    // Covers dispatch.rs line 411: when the payload after
    // "SETTINGS_VALIDATION_FAILED: " is NOT valid JSON, the dispatcher
    // propagates the original error rather than synthesizing a response.
    let runtime = SettingsValidationRuntime {
        inner: TestRuntimeControl::with_claude_fixture().await.unwrap(),
        error_message: "SETTINGS_VALIDATION_FAILED: not valid json {{{".to_string(),
    };
    let dispatcher = ControlDispatcher::new(runtime);

    let result = dispatcher
        .dispatch(ControlRequest::new(
            "settings.update",
            serde_json::json!({"timezone": "bad"}),
        ))
        .await;
    assert!(
        result.is_err(),
        "invalid JSON payload should propagate as Err"
    );
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("SETTINGS_VALIDATION_FAILED"));
}

#[tokio::test]
async fn settings_update_other_error_propagates() {
    // Covers dispatch.rs line 414: when settings_update returns an error
    // WITHOUT the SETTINGS_VALIDATION_FAILED prefix, the dispatcher
    // propagates it directly.
    let runtime = SettingsValidationRuntime {
        inner: TestRuntimeControl::with_claude_fixture().await.unwrap(),
        error_message: "db is locked".to_string(),
    };
    let dispatcher = ControlDispatcher::new(runtime);

    let result = dispatcher
        .dispatch(ControlRequest::new(
            "settings.update",
            serde_json::json!({}),
        ))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("db is locked"));
}

// ---------------------------------------------------------------------------
// 7. Server: endpoint() accessor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn server_endpoint_accessor_returns_bound_endpoint() {
    // Covers server.rs lines 126-128.
    let runtime: Arc<dyn RuntimeControl> =
        Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let endpoint = unique_endpoint("endpoint-test");
    let server = ControlServer::<InMemoryTransport>::bind(&endpoint, Arc::clone(&runtime))
        .await
        .unwrap();
    assert_eq!(server.endpoint(), endpoint);
}

// ---------------------------------------------------------------------------
// 8. Server: invalid handshake (server.rs line 212)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invalid_handshake_returns_error_via_accept_one() {
    // Covers server.rs line 212: a non-HELLO first frame causes
    // handle_connection to bail with "invalid handshake".
    let runtime: Arc<dyn RuntimeControl> =
        Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    // Use bind (not spawn) so accept_one doesn't race with run() for the
    // incoming connection.
    let (server, endpoint) = bind_in_memory_server(runtime, "bad-handshake").await;

    // Connect a raw client and send a non-HELLO frame.
    let mut stream = InMemoryTransport::connect(&endpoint).await.unwrap();
    write_raw_frame(&mut stream, b"not-the-hello").await;

    // accept_one should return an Err mentioning the invalid handshake.
    let result = server.accept_one().await;
    assert!(result.is_err(), "expected Err for invalid handshake");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("invalid handshake"),
        "error should mention invalid handshake, got: {msg}"
    );

    server.shutdown();
    server.await_drain().await;
}

#[tokio::test]
async fn run_loop_continues_after_connection_error() {
    // Covers server.rs lines 84-85: when handle_connection returns Err inside
    // the run() accept loop, the error is logged and the server keeps running.
    let runtime: Arc<dyn RuntimeControl> =
        Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let (server, endpoint, server_task) = spawn_in_memory_server(runtime, "run-err").await;

    // First client: send invalid handshake to trigger handle_connection Err.
    {
        let mut bad = InMemoryTransport::connect(&endpoint).await.unwrap();
        write_raw_frame(&mut bad, b"wrong-handshake").await;
        // Give the server time to process the bad handshake.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Second client: a normal RPC should still succeed, proving the run loop
    // survived the previous connection error.
    let mut client = <ControlClient<InMemoryTransport>>::connect(&endpoint)
        .await
        .unwrap();
    let response = client
        .call(ControlRequest::new("shell.status", serde_json::json!({})))
        .await
        .unwrap();
    assert!(matches!(response, ControlResponse::Ok(_)));

    drop(client);
    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 9. Server: invalid UTF-8 frame body (server.rs lines 239-240)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invalid_utf8_frame_body_returns_read_error() {
    // Covers server.rs lines 239-240: a frame body that is not valid UTF-8
    // is not a "connection closed" error, so handle_connection returns Err
    // with "reading frame" context.
    //
    // tokio::join! runs the client side and accept_one concurrently in the
    // same task, avoiding the deadlock that occurs when spawning accept_one
    // as a separate task (the client's read_exact for the ack and the
    // server's accept need to be polled cooperatively).
    let runtime: Arc<dyn RuntimeControl> =
        Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let (server, endpoint) = bind_in_memory_server(runtime, "bad-utf8").await;

    let endpoint_clone = endpoint.clone();
    let client_fut = async {
        let mut stream = InMemoryTransport::connect(&endpoint_clone).await.unwrap();
        write_raw_frame(&mut stream, b"busytok-hello").await;
        let mut ack_buf = vec![0u8; 4 + 10]; // 4-byte len + "busytok-ok" (10 bytes)
        stream.read_exact(&mut ack_buf).await.unwrap();
        write_raw_frame(&mut stream, &[0xFF, 0xFE, 0xFD]).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let ((), result) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(client_fut, server.accept_one())
    })
    .await
    .expect("test timed out");

    assert!(result.is_err(), "expected Err for invalid UTF-8 frame");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("reading frame"),
        "error should be wrapped with 'reading frame' context, got: {msg}"
    );

    server.shutdown();
    server.await_drain().await;
}

// ---------------------------------------------------------------------------
// 10. Server: partial body read triggers is_connection_closed io-error path
//     (server.rs lines 174-186)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn partial_body_read_is_treated_as_connection_closed() {
    // Covers server.rs lines 174-186: when read_frame fails during the body
    // read (not the length read), the error context is "reading frame body"
    // which does NOT match the text-based checks. The function then falls
    // through to the io::Error downcast path, where UnexpectedEof matches
    // and is_connection_closed returns true. handle_connection returns Ok
    // (graceful disconnect).
    //
    // Uses tokio::join! (see invalid_utf8_frame_body_returns_read_error for
    // rationale).
    let runtime: Arc<dyn RuntimeControl> =
        Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let (server, endpoint) = bind_in_memory_server(runtime, "partial-body").await;

    let endpoint_clone = endpoint.clone();
    let client_fut = async {
        let mut stream = InMemoryTransport::connect(&endpoint_clone).await.unwrap();
        write_raw_frame(&mut stream, b"busytok-hello").await;
        let mut ack_buf = vec![0u8; 4 + 10]; // 4-byte len + "busytok-ok" (10 bytes)
        stream.read_exact(&mut ack_buf).await.unwrap();
        // Send a 4-byte length prefix claiming 100 bytes of body, but only
        // send 5 bytes then drop the stream. The server's read_exact for the
        // body will hit UnexpectedEof.
        stream.write_all(&100u32.to_be_bytes()).await.unwrap();
        stream.write_all(b"hello").await.unwrap();
        stream.flush().await.unwrap();
        // Drop the client half — server's read_exact for the remaining 95
        // bytes returns UnexpectedEof.
        drop(stream);
    };

    let ((), result) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(client_fut, server.accept_one())
    })
    .await
    .expect("test timed out");

    // is_connection_closed returns true for body-read EOF, so
    // handle_connection returns Ok(()) (graceful disconnect).
    assert!(
        result.is_ok(),
        "partial body read should be treated as a graceful disconnect, got: {result:?}"
    );

    server.shutdown();
    server.await_drain().await;
}

// ---------------------------------------------------------------------------
// 11. Runtime wrapper with latest_event_seq override (for gap detection)
// ---------------------------------------------------------------------------

struct RuntimeWithLatestSeq {
    inner: Arc<TestRuntimeControl>,
    seq: i64,
}

#[async_trait::async_trait]
impl RuntimeControl for RuntimeWithLatestSeq {
    fn latest_event_seq(&self) -> Option<i64> {
        Some(self.seq)
    }
    fn event_bus(&self) -> &AppEventBus {
        self.inner.event_bus()
    }
    fn on_request_meta(&self, meta: &RequestMeta) {
        self.inner.on_request_meta(meta);
    }
    fn record_diagnostic(&self, severity: &str, code: &str, message: &str) {
        self.inner.record_diagnostic(severity, code, message);
    }

    // Delegate the rest to inner (via Arc<TestRuntimeControl>'s blanket impl).
    async fn service_health(&self) -> anyhow::Result<ServiceHealthDto> {
        self.inner.service_health().await
    }
    async fn service_status(&self) -> anyhow::Result<ServiceStatusDto> {
        self.inner.service_status().await
    }
    async fn shell_status(&self) -> anyhow::Result<ShellStatusDto> {
        self.inner.shell_status().await
    }
    async fn overview_summary(
        &self,
        req: OverviewSummaryRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewSummaryDto>> {
        self.inner.overview_summary(req).await
    }
    async fn overview_trend(
        &self,
        req: OverviewTrendRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewTrendResponseDto>> {
        self.inner.overview_trend(req).await
    }
    async fn overview_heatmap(
        &self,
        req: OverviewHeatmapRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewHeatmapResponseDto>> {
        self.inner.overview_heatmap(req).await
    }
    async fn overview_rankings(
        &self,
        req: OverviewRankingsRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewRankingsResponseDto>> {
        self.inner.overview_rankings(req).await
    }
    async fn receipt_daily(
        &self,
        req: ReceiptDailyRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ReceiptDailyDto>> {
        self.inner.receipt_daily(req).await
    }
    async fn activity_recent(
        &self,
        req: ActivityRecentRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ActivityRecentResponseDto>> {
        self.inner.activity_recent(req).await
    }
    async fn activity_list(
        &self,
        req: ActivityListRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ActivityListResponseDto>> {
        self.inner.activity_list(req).await
    }
    async fn activity_detail(
        &self,
        req: ActivityDetailRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ActivityDetailDto>> {
        self.inner.activity_detail(req).await
    }
    async fn breakdown_list(
        &self,
        req: BreakdownListRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<BreakdownListResponseDto>> {
        self.inner.breakdown_list(req).await
    }
    async fn breakdown_detail(
        &self,
        req: BreakdownDetailRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<BreakdownDetailDto>> {
        self.inner.breakdown_detail(req).await
    }
    async fn clients_snapshot(
        &self,
        req: ClientsSnapshotRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ClientsSnapshotDto>> {
        self.inner.clients_snapshot(req).await
    }
    async fn clients_detail(
        &self,
        req: ClientSourceDetailRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ClientSourceDetailDto>> {
        self.inner.clients_detail(req).await
    }
    async fn settings_snapshot(&self) -> anyhow::Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        self.inner.settings_snapshot().await
    }
    async fn settings_update(
        &self,
        req: SettingsUpdateRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        self.inner.settings_update(req).await
    }
    async fn settings_diagnostics(
        &self,
    ) -> anyhow::Result<ReadEnvelopeDto<SettingsDiagnosticsDto>> {
        self.inner.settings_diagnostics().await
    }
    async fn settings_recovery_action(
        &self,
        req: SettingsRecoveryActionRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<SettingsRecoveryActionResponseDto>> {
        self.inner.settings_recovery_action(req).await
    }
    async fn live_window(
        &self,
        req: LiveWindowRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<LiveWindowDto>> {
        self.inner.live_window(req).await
    }
    async fn prompts_list(
        &self,
        req: PromptListQueryDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptListResponseDto>> {
        self.inner.prompts_list(req).await
    }
    async fn prompts_get(
        &self,
        req: PromptGetRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
        self.inner.prompts_get(req).await
    }
    async fn prompts_create(
        &self,
        req: PromptCreateRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
        self.inner.prompts_create(req).await
    }
    async fn prompts_update(
        &self,
        req: PromptUpdateRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
        self.inner.prompts_update(req).await
    }
    async fn prompts_delete(
        &self,
        req: PromptDeleteRequestDto,
    ) -> anyhow::Result<PromptDeleteResultDto> {
        self.inner.prompts_delete(req).await
    }
    async fn prompts_use(&self, req: PromptUseRequestDto) -> anyhow::Result<PromptUseResultDto> {
        self.inner.prompts_use(req).await
    }
    async fn suggest_tags(
        &self,
        req: PromptSuggestTagsRequestDto,
    ) -> anyhow::Result<PromptSuggestTagsResponseDto> {
        self.inner.suggest_tags(req).await
    }
    async fn subagent_delegate(
        &self,
        req: SubagentDelegateRequestDto,
    ) -> anyhow::Result<SubagentDelegateResponseDto> {
        self.inner.subagent_delegate(req).await
    }
    async fn subagent_list(
        &self,
        req: SubagentListRequestDto,
    ) -> anyhow::Result<SubagentListResponseDto> {
        self.inner.subagent_list(req).await
    }
    async fn subagent_show(
        &self,
        req: SubagentResolveRequestDto,
    ) -> anyhow::Result<SubagentDetailDto> {
        self.inner.subagent_show(req).await
    }
    async fn subagent_tasks(
        &self,
        req: SubagentTasksRequestDto,
    ) -> anyhow::Result<SubagentTasksResponseDto> {
        self.inner.subagent_tasks(req).await
    }
    async fn subagent_hibernate(
        &self,
        req: SubagentResolveRequestDto,
    ) -> anyhow::Result<SubagentAckDto> {
        self.inner.subagent_hibernate(req).await
    }
    async fn subagent_delete(
        &self,
        req: SubagentDeleteRequestDto,
    ) -> anyhow::Result<SubagentAckDto> {
        self.inner.subagent_delete(req).await
    }
    async fn subagent_runtime_status(
        &self,
        req: SubagentRuntimeStatusRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<SubagentRuntimeStatusDto>> {
        self.inner.subagent_runtime_status(req).await
    }
    async fn provider_create(&self, req: ProviderCreateRequestDto) -> anyhow::Result<ProviderDto> {
        self.inner.provider_create(req).await
    }
    async fn provider_list(&self) -> anyhow::Result<ProviderListResponseDto> {
        self.inner.provider_list().await
    }
    async fn provider_update(&self, req: ProviderUpdateRequestDto) -> anyhow::Result<ProviderDto> {
        self.inner.provider_update(req).await
    }
    async fn provider_delete(&self, req: ProviderDeleteRequestDto) -> anyhow::Result<()> {
        self.inner.provider_delete(req).await
    }
    async fn provider_test_connection(
        &self,
        req: ProviderTestConnectionRequestDto,
    ) -> anyhow::Result<ProviderTestConnectionResponseDto> {
        self.inner.provider_test_connection(req).await
    }
    async fn model_create(&self, req: ModelCreateRequestDto) -> anyhow::Result<ModelCatalogEntryDto> {
        self.inner.model_create(req).await
    }
    async fn model_list(&self, req: ModelListRequestDto) -> anyhow::Result<ModelListResponseDto> {
        self.inner.model_list(req).await
    }
    async fn model_update(&self, req: ModelUpdateRequestDto) -> anyhow::Result<()> {
        self.inner.model_update(req).await
    }
    async fn model_delete(&self, req: ModelDeleteRequestDto) -> anyhow::Result<()> {
        self.inner.model_delete(req).await
    }
    async fn model_tags_update(&self, req: ModelTagUpdateDto) -> anyhow::Result<()> {
        self.inner.model_tags_update(req).await
    }
    async fn pi_sidecar_locator_update(
        &self,
        req: PiSidecarLocatorUpdateRequestDto,
    ) -> anyhow::Result<PiSidecarLocatorUpdateResponseDto> {
        self.inner.pi_sidecar_locator_update(req).await
    }
    async fn profile_create(&self, req: ProfileCreateRequestDto) -> anyhow::Result<ProfileDto> {
        self.inner.profile_create(req).await
    }
    async fn profile_update(&self, req: ProfileUpdateRequestDto) -> anyhow::Result<ProfileDto> {
        self.inner.profile_update(req).await
    }
    async fn profile_delete(&self, req: ProfileDeleteRequestDto) -> anyhow::Result<()> {
        self.inner.profile_delete(req).await
    }
}

// ---------------------------------------------------------------------------
// 12. Gap detection: server.rs lines 288-310
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_gap_detection_emits_gap_event_batch() {
    // Covers server.rs lines 288-310: when a client subscribes with a
    // last_event_seq that is behind the server's latest_event_seq, the
    // server emits a data:gap_detected event batch before streaming live
    // events.
    let inner = Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let runtime: Arc<dyn RuntimeControl> = Arc::new(RuntimeWithLatestSeq {
        inner: Arc::clone(&inner),
        seq: 100,
    });
    let (server, endpoint, server_task) = spawn_in_memory_server(runtime, "gap-detect").await;

    let mut client = <ControlClient<InMemoryTransport>>::connect(&endpoint)
        .await
        .unwrap();

    // Subscribe with a stale last_event_seq (5 < 100). The server should
    // detect the gap and emit a gap_detected batch.
    let ack = client
        .subscribe_with_meta_and_last_event_seq(vec![], RequestMeta::default(), Some(5))
        .await
        .unwrap();
    assert!(matches!(ack, ControlResponse::Ok(_)));

    let batch = client.recv_event_batch().await.unwrap();
    assert_eq!(batch.events.len(), 1);
    let event = &batch.events[0];
    assert_eq!(event.event_type, "data:gap_detected");
    assert_eq!(event.event_seq, Some(100));
    assert_eq!(event.payload["last_seen_seq"], 5);
    assert_eq!(event.payload["last_event_id"], 100);
    assert_eq!(event.payload["sequence_gap"], true);
    assert_eq!(event.payload["gap_size"], 95);

    drop(client);
    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 13. Subscription broadcast: server.rs lines 324-348 + event_payload_json
//     variants (lines 446-510)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_receives_and_serializes_every_event_variant() {
    // Covers server.rs lines 324-348 (the Ok(published) broadcast branch and
    // successful write_frame) AND lines 446-510 (event_payload_json match arms
    // for DataInvalidated, LiveSample, SubscriptionConnected,
    // SubscriptionDisconnected, SubscriptionReconnectFailed,
    // WriterQueueThreshold, WriterLagThreshold, and the `other =>` fallback).
    let runtime: Arc<dyn RuntimeControl> =
        Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let event_bus_ref = runtime.event_bus();
    let (server, endpoint, server_task) =
        spawn_in_memory_server(Arc::clone(&runtime), "sub-broadcast").await;

    let mut client = <ControlClient<InMemoryTransport>>::connect(&endpoint)
        .await
        .unwrap();
    let ack = client.subscribe(vec![]).await.unwrap();
    assert!(matches!(ack, ControlResponse::Ok(_)));

    // Publish events of each variant and verify the client receives them.
    // 1. DataInvalidated (covers server.rs lines 448-450).
    event_bus_ref
        .publish_ephemeral(AppEvent::DataInvalidated { datasets: vec![] })
        .unwrap();
    let batch = client.recv_event_batch().await.unwrap();
    assert_eq!(batch.events.len(), 1);
    assert_eq!(batch.events[0].event_type, "data:invalidated");
    assert!(batch.events[0].payload.get("datasets").is_some());

    // 2. LiveSample (covers server.rs lines 451-465).
    event_bus_ref
        .publish_ephemeral(AppEvent::LiveSample {
            bucket_start_ms: 1_000,
            tokens_per_sec: 150.5,
            cost_per_sec: Some(0.003),
            events_per_sec: 3.0,
            transient: false,
        })
        .unwrap();
    let batch = client.recv_event_batch().await.unwrap();
    assert_eq!(batch.events[0].event_type, "live:sample");
    assert_eq!(batch.events[0].payload["tokens_per_sec"], 150.5);
    assert_eq!(batch.events[0].payload["transient"], false);
    // LiveSample is ephemeral, so is_exact should be false.
    assert!(batch.events[0].ephemeral);
    assert!(!batch.events[0].is_exact);

    // 3. SubscriptionConnected (covers server.rs lines 466-469).
    event_bus_ref
        .publish_ephemeral(AppEvent::SubscriptionConnected {
            client_id: Some("c1".to_string()),
        })
        .unwrap();
    let batch = client.recv_event_batch().await.unwrap();
    assert_eq!(batch.events[0].event_type, "subscription:connected");
    assert_eq!(batch.events[0].payload["client_id"], "c1");

    // 4. SubscriptionDisconnected (covers server.rs lines 471-476).
    event_bus_ref
        .publish_ephemeral(AppEvent::SubscriptionDisconnected {
            client_id: Some("c1".to_string()),
            reason: Some("eof".to_string()),
        })
        .unwrap();
    let batch = client.recv_event_batch().await.unwrap();
    assert_eq!(batch.events[0].event_type, "subscription:disconnected");
    assert_eq!(batch.events[0].payload["reason"], "eof");

    // 5. SubscriptionReconnectFailed (covers server.rs lines 477-485).
    event_bus_ref
        .publish_ephemeral(AppEvent::SubscriptionReconnectFailed {
            attempts: 3,
            last_error: "timeout".to_string(),
        })
        .unwrap();
    let batch = client.recv_event_batch().await.unwrap();
    assert_eq!(batch.events[0].event_type, "subscription:reconnect_failed");
    assert_eq!(batch.events[0].payload["attempts"], 3);
    assert_eq!(batch.events[0].payload["last_error"], "timeout");

    // 6. WriterQueueThreshold (covers server.rs lines 486-496).
    event_bus_ref
        .publish_ephemeral(AppEvent::WriterQueueThreshold {
            queue_depth: 100,
            threshold: 80,
            severity: "warning".to_string(),
        })
        .unwrap();
    let batch = client.recv_event_batch().await.unwrap();
    assert_eq!(
        batch.events[0].event_type,
        "diagnostic:writer_queue_threshold"
    );
    assert_eq!(batch.events[0].payload["queue_depth"], 100);
    assert_eq!(batch.events[0].payload["threshold"], 80);

    // 7. WriterLagThreshold (covers server.rs lines 497-507).
    event_bus_ref
        .publish_ephemeral(AppEvent::WriterLagThreshold {
            lag_ms: 5000,
            threshold: 3000,
            severity: "warning".to_string(),
        })
        .unwrap();
    let batch = client.recv_event_batch().await.unwrap();
    assert_eq!(
        batch.events[0].event_type,
        "diagnostic:writer_lag_threshold"
    );
    assert_eq!(batch.events[0].payload["lag_ms"], 5000);

    // 8. `other =>` fallback arm (covers server.rs line 508): publish a
    // non-ephemeral UsageEventInserted with a generation_id, which exercises
    // the is_exact = true path AND the serde::to_value fallback.
    event_bus_ref
        .publish(PublishedEvent::durable(
            AppEvent::UsageEventInserted {
                event_id: "evt-1".to_string(),
                agent: "claude_code".to_string(),
            },
            42,
            "gen-1".to_string(),
            1_700_000_000_000,
            vec![],
        ))
        .unwrap();
    let batch = client.recv_event_batch().await.unwrap();
    assert_eq!(batch.events[0].event_type, "usage:event_inserted");
    assert_eq!(batch.events[0].event_seq, Some(42));
    assert_eq!(batch.events[0].generation_id.as_deref(), Some("gen-1"));
    // Non-ephemeral with generation_id => is_exact = true.
    assert!(!batch.events[0].ephemeral);
    assert!(batch.events[0].is_exact);

    drop(client);
    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 14. Subscription filter: events not matching filter_types are skipped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_filter_skips_non_matching_event_types() {
    // Covers server.rs line 327-329: when filter_types is non-empty and the
    // event type doesn't match, the loop continues without writing.
    let runtime: Arc<dyn RuntimeControl> =
        Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let event_bus_ref = runtime.event_bus();
    let (server, endpoint, server_task) =
        spawn_in_memory_server(Arc::clone(&runtime), "sub-filter").await;

    let mut client = <ControlClient<InMemoryTransport>>::connect(&endpoint)
        .await
        .unwrap();
    // Subscribe only to live:sample events.
    let ack = client
        .subscribe(vec!["live:sample".to_string()])
        .await
        .unwrap();
    assert!(matches!(ack, ControlResponse::Ok(_)));

    // Publish a non-matching event — client should NOT receive it.
    event_bus_ref
        .publish_ephemeral(AppEvent::DataInvalidated { datasets: vec![] })
        .unwrap();
    // Publish a matching event — client should receive it.
    event_bus_ref
        .publish_ephemeral(AppEvent::LiveSample {
            bucket_start_ms: 0,
            tokens_per_sec: 1.0,
            cost_per_sec: None,
            events_per_sec: 1.0,
            transient: true,
        })
        .unwrap();

    let batch = tokio::time::timeout(Duration::from_secs(2), client.recv_event_batch())
        .await
        .expect("client should receive the matching event")
        .unwrap();
    assert_eq!(batch.events.len(), 1);
    assert_eq!(batch.events[0].event_type, "live:sample");

    drop(client);
    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 15. Subscription write error: server.rs lines 349-364
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_write_error_emits_subscription_disconnected() {
    // Covers server.rs lines 349-364: when write_frame to a subscribed client
    // fails (because the client dropped), the server publishes
    // SubscriptionDisconnected and records a diagnostic.
    let runtime: Arc<dyn RuntimeControl> =
        Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let event_bus_ref = runtime.event_bus();
    let (server, endpoint, server_task) =
        spawn_in_memory_server(Arc::clone(&runtime), "sub-write-err").await;

    // A separate listener that will catch the SubscriptionDisconnected event
    // published by the server when the write fails.
    let mut listener = event_bus_ref.subscribe();

    {
        let mut client = <ControlClient<InMemoryTransport>>::connect(&endpoint)
            .await
            .unwrap();
        let ack = client.subscribe(vec![]).await.unwrap();
        assert!(matches!(ack, ControlResponse::Ok(_)));
        // Drop the client without reading any events. The server's next
        // write will fail.
        drop(client);
    }

    // Publish an event — the server's write to the dropped client will fail.
    event_bus_ref
        .publish_ephemeral(AppEvent::LiveSample {
            bucket_start_ms: 0,
            tokens_per_sec: 1.0,
            cost_per_sec: None,
            events_per_sec: 1.0,
            transient: true,
        })
        .unwrap();

    // The server should publish SubscriptionDisconnected with reason
    // "write_error".
    let mut got_disconnected = false;
    for _ in 0..20 {
        let result = tokio::time::timeout(Duration::from_millis(200), listener.recv()).await;
        if let Ok(Ok(published)) = result {
            if let AppEvent::SubscriptionDisconnected { reason, .. } = &published.event {
                assert_eq!(
                    reason.as_deref(),
                    Some("write_error"),
                    "disconnect reason should be write_error"
                );
                got_disconnected = true;
                break;
            }
        }
    }
    assert!(
        got_disconnected,
        "server should publish SubscriptionDisconnected when write fails"
    );

    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 16. Subscription shutdown: server.rs lines 375-378
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_loop_exits_on_server_shutdown() {
    // Covers server.rs lines 375-378: the subscription loop's
    // shutdown_rx.changed() branch breaks the loop when the server shuts down.
    let runtime: Arc<dyn RuntimeControl> =
        Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let (server, endpoint, server_task) = spawn_in_memory_server(runtime, "sub-shutdown").await;

    let mut client = <ControlClient<InMemoryTransport>>::connect(&endpoint)
        .await
        .unwrap();
    let ack = client.subscribe(vec![]).await.unwrap();
    assert!(matches!(ack, ControlResponse::Ok(_)));

    // Shut down the server while the client is subscribed. The connection
    // handler should exit cleanly via the shutdown_rx branch.
    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();

    // The client's read should eventually fail (server closed the stream).
    // We don't assert on the error kind — just that the stream is closed.
    let _ = tokio::time::timeout(Duration::from_secs(1), client.recv_event_batch()).await;
}

// ---------------------------------------------------------------------------
// 17. Client: recv_event_batch (client.rs lines 108-115)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn client_recv_event_batch_reads_published_event() {
    // Covers client.rs lines 108-115: recv_event_batch parses a
    // EventSubscriptionBatchDto frame.
    let runtime: Arc<dyn RuntimeControl> =
        Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let event_bus_ref = runtime.event_bus();
    let (server, endpoint, server_task) =
        spawn_in_memory_server(Arc::clone(&runtime), "client-recv").await;

    let mut client = <ControlClient<InMemoryTransport>>::connect(&endpoint)
        .await
        .unwrap();
    let ack = client.subscribe(vec![]).await.unwrap();
    assert!(matches!(ack, ControlResponse::Ok(_)));

    event_bus_ref
        .publish_ephemeral(AppEvent::LiveSample {
            bucket_start_ms: 0,
            tokens_per_sec: 42.0,
            cost_per_sec: None,
            events_per_sec: 1.0,
            transient: true,
        })
        .unwrap();

    let batch = client.recv_event_batch().await.unwrap();
    assert_eq!(batch.events.len(), 1);
    assert_eq!(batch.events[0].event_type, "live:sample");
    assert_eq!(batch.events[0].payload["tokens_per_sec"], 42.0);

    drop(client);
    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 18. Client: subscribe_with_meta_and_last_event_seq (client.rs line 90)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn client_subscribe_with_last_event_seq_sends_cursor() {
    // Covers client.rs line 90: the Some(seq) branch of the match builds a
    // JSON params object that includes last_event_seq.
    let runtime: Arc<dyn RuntimeControl> =
        Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let (server, endpoint, server_task) = spawn_in_memory_server(runtime, "client-cursor").await;

    let mut client = <ControlClient<InMemoryTransport>>::connect(&endpoint)
        .await
        .unwrap();
    // Subscribe with a last_event_seq cursor. The TestRuntimeControl's
    // latest_event_seq returns None (default), so no gap event is emitted —
    // but the client's subscribe_with_meta_and_last_event_seq still exercises
    // the Some(seq) params-construction branch.
    let ack = client
        .subscribe_with_meta_and_last_event_seq(
            vec!["live:sample".to_string()],
            RequestMeta::default(),
            Some(42),
        )
        .await
        .unwrap();
    assert!(matches!(ack, ControlResponse::Ok(_)));

    drop(client);
    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 19. Client: invalid handshake ack (client.rs line 44)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn client_invalid_handshake_ack_returns_error() {
    // Covers client.rs line 44: when the server sends a non-HELLO_ACK first
    // frame, the client bails with "invalid handshake ack".
    let endpoint = unique_endpoint("client-bad-ack");
    let listener = InMemoryTransport::bind(&endpoint).await.unwrap();
    let accept_task = tokio::spawn(async move {
        // Accept one connection and send a wrong ack instead of HELLO_ACK.
        let mut server_stream = InMemoryTransport::accept(&listener).await.unwrap();
        // Read the hello frame (4-byte len + "busytok-hello").
        let mut len_buf = [0u8; 4];
        server_stream.read_exact(&mut len_buf).await.unwrap();
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut hello_buf = vec![0u8; len];
        server_stream.read_exact(&mut hello_buf).await.unwrap();
        // Send a wrong ack.
        let wrong_ack = b"busytok-not-ok";
        let ack_len = wrong_ack.len() as u32;
        server_stream
            .write_all(&ack_len.to_be_bytes())
            .await
            .unwrap();
        server_stream.write_all(wrong_ack).await.unwrap();
        server_stream.flush().await.unwrap();
        // Keep the stream alive briefly so the client reads the ack.
        tokio::time::sleep(Duration::from_millis(100)).await;
    });

    let result = <ControlClient<InMemoryTransport>>::connect(&endpoint).await;
    assert!(result.is_err(), "client should error on wrong ack");
    let msg = result.err().unwrap().to_string();
    assert!(
        msg.contains("invalid handshake ack"),
        "error should mention invalid handshake ack, got: {msg}"
    );
    let _ = accept_task.await;
}

// ---------------------------------------------------------------------------
// 20. Client: connection failure (client.rs line 25 context)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn client_connect_to_unknown_endpoint_returns_error() {
    // Covers the with_context error path on client.rs line 25 when
    // T::connect fails.
    let endpoint = unique_endpoint("nonexistent");
    let result = <ControlClient<InMemoryTransport>>::connect(&endpoint).await;
    assert!(result.is_err());
    let msg = result.err().unwrap().to_string();
    assert!(
        msg.contains("connecting to"),
        "error should mention connecting to endpoint, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 21. Client: malformed JSON response (client.rs lines 60-61)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn client_call_with_malformed_response_returns_error() {
    // Covers the serde_json::from_str error path on client.rs line 61.
    let endpoint = unique_endpoint("client-bad-json");
    let listener = InMemoryTransport::bind(&endpoint).await.unwrap();
    let accept_task = tokio::spawn(async move {
        let mut server_stream = InMemoryTransport::accept(&listener).await.unwrap();
        // Read hello.
        let mut len_buf = [0u8; 4];
        server_stream.read_exact(&mut len_buf).await.unwrap();
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut hello_buf = vec![0u8; len];
        server_stream.read_exact(&mut hello_buf).await.unwrap();
        // Send HELLO_ACK.
        let ack = b"busytok-ok";
        let ack_len = ack.len() as u32;
        server_stream
            .write_all(&ack_len.to_be_bytes())
            .await
            .unwrap();
        server_stream.write_all(ack).await.unwrap();
        server_stream.flush().await.unwrap();
        // Read the request frame.
        server_stream.read_exact(&mut len_buf).await.unwrap();
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut req_buf = vec![0u8; len];
        server_stream.read_exact(&mut req_buf).await.unwrap();
        // Send a malformed (non-JSON) response.
        let bad = b"not json at all";
        let bad_len = bad.len() as u32;
        server_stream
            .write_all(&bad_len.to_be_bytes())
            .await
            .unwrap();
        server_stream.write_all(bad).await.unwrap();
        server_stream.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    });

    let mut client = <ControlClient<InMemoryTransport>>::connect(&endpoint)
        .await
        .unwrap();
    let result = client
        .call(ControlRequest::new("shell.status", serde_json::json!({})))
        .await;
    assert!(result.is_err(), "malformed response should error");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("parsing response"),
        "error should mention parsing response, got: {msg}"
    );
    let _ = accept_task.await;
}
