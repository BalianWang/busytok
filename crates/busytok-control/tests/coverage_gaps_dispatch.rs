#![allow(
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    clippy::await_holding_lock,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::field_reassign_with_default,
    clippy::useless_vec,
    clippy::manual_dangling_ptr,
    clippy::unused_async,
    unused_variables,
    unused_imports,
    dead_code
)]
//! Coverage gap tests for `busytok-control/src/dispatch.rs`.
//!
//! These tests target the few remaining uncovered lines identified by
//! `cargo llvm-cov`:
//! - `MethodDispatchError::from_read_error` constructor (dispatch.rs lines 23-29).
//! - `MethodDispatchError` `Display` impl (dispatch.rs lines 33-35).
//! - The trait default `RuntimeControl::on_request_meta` (dispatch.rs line 241)
//!   — exercised via a wrapper runtime that does NOT override it.
//! - The success paths for `provider.create`, `provider.update`,
//!   `provider.delete`, `profile.create`, `profile.update`, and
//!   `profile.delete` (dispatch.rs lines 535, 545, 551, 577, 583, 589).
//!   `TestRuntimeControl` returns `Err` for these methods, so a dedicated
//!   wrapper runtime that returns `Ok` is required to cover the success-path
//!   `ControlResponse::ok(serde_json::to_value(...)?)` lines.

use std::sync::Arc;

use busytok_control::dispatch::MethodDispatchError;
use busytok_control::{ControlDispatcher, RuntimeControl, TestRuntimeControl};
use busytok_domain::ProviderKind;
use busytok_events::AppEventBus;
use busytok_protocol::dto::*;
use busytok_protocol::{ControlRequest, ControlResponse};

// ---------------------------------------------------------------------------
// 1. MethodDispatchError::from_read_error (dispatch.rs lines 23-29)
// ---------------------------------------------------------------------------

#[test]
fn method_dispatch_error_from_read_error_populates_all_fields() {
    // Covers dispatch.rs lines 23-29: the public `from_read_error` constructor
    // builds a MethodDispatchError with `payload: Some(payload)` and stringifies
    // the borrowed `code`.
    let err = MethodDispatchError::from_read_error(
        "read_timeout",
        "timed out reading frame".to_string(),
        serde_json::json!({ "frame_id": 42, "kind": "read_timeout" }),
    );

    assert_eq!(err.code, "read_timeout");
    assert_eq!(err.message, "timed out reading frame");
    let payload = err.payload.expect("from_read_error should set payload");
    assert_eq!(payload["frame_id"], 42);
    assert_eq!(payload["kind"], "read_timeout");
}

// ---------------------------------------------------------------------------
// 2. MethodDispatchError Display impl (dispatch.rs lines 33-35)
// ---------------------------------------------------------------------------

#[test]
fn method_dispatch_error_display_formats_code_and_message() {
    // Covers dispatch.rs lines 33-35: the Display impl formats as
    // "{code}: {message}".
    let err = MethodDispatchError {
        code: "E123".to_string(),
        message: "boom".to_string(),
        payload: None,
    };
    let rendered = format!("{}", err);
    assert_eq!(rendered, "E123: boom");

    // Also exercise it via std::error::Error's blanket Display (which uses
    // our impl) to ensure the path through `std::fmt::Display` is taken.
    let source: Box<dyn std::error::Error> = Box::new(err);
    assert_eq!(format!("{}", source), "E123: boom");
}

#[test]
fn method_dispatch_error_display_handles_empty_message() {
    // Edge case for the Display impl: empty message still formats correctly.
    let err = MethodDispatchError {
        code: "empty".to_string(),
        message: String::new(),
        payload: Some(serde_json::Value::Null),
    };
    assert_eq!(format!("{}", err), "empty: ");
}

// ---------------------------------------------------------------------------
// 3. Helper stubs + SuccessRuntime wrapper
// ---------------------------------------------------------------------------

fn stub_provider_dto() -> ProviderDto {
    ProviderDto {
        id: "p-stub".to_string(),
        name: "Stub Provider".to_string(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "http://stub.example".to_string(),
        enabled: true,
        has_api_key: false,
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

fn stub_model_catalog_entry_dto() -> ModelCatalogEntryDto {
    ModelCatalogEntryDto {
        provider_id: "p-stub".to_string(),
        provider_name: "Stub Provider".to_string(),
        provider_kind: ProviderKind::OpenAiCompatible,
        provider_enabled: true,
        model_db_id: "m-stub".to_string(),
        model_id: "stub-model".to_string(),
        model_enabled: true,
        tags: vec![],
        display_name: None,
        reasoning: false,
        context_window: None,
        max_tokens: None,
    }
}

fn stub_profile_dto() -> ProfileDto {
    ProfileDto {
        id: "prof-stub".to_string(),
        is_builtin: false,
        tools: vec![],
        context_budget_tokens: 8000,
        timeout_seconds: 60,
        write_access: false,
    }
}

/// Wrapper around `Arc<TestRuntimeControl>` that:
/// - Overrides `provider_create`, `provider_update`, `provider_delete`,
///   `profile_create`, `profile_update`, and `profile_delete` to return `Ok`,
///   exercising the dispatcher's success-path `ControlResponse::ok(...)`
///   lines (dispatch.rs lines 535, 545, 551, 577, 583, 589).
/// - Deliberately does NOT override `on_request_meta`, so the dispatcher's
///   `self.runtime.on_request_meta(&request.meta)` call hits the trait
///   default body (dispatch.rs line 241).
struct SuccessRuntime {
    inner: Arc<TestRuntimeControl>,
}

#[async_trait::async_trait]
impl RuntimeControl for SuccessRuntime {
    // NOTE: `on_request_meta` is intentionally NOT overridden — the trait
    // default body (dispatch.rs line 241) is exercised when the dispatcher
    // calls `self.runtime.on_request_meta(...)` on this wrapper.

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

    // ── Provider overrides (return Ok to cover dispatch success paths) ──
    async fn provider_create(&self, _req: ProviderCreateRequestDto) -> anyhow::Result<ProviderDto> {
        // Covers dispatch.rs line 535: ControlResponse::ok(serde_json::to_value(dto)?)
        Ok(stub_provider_dto())
    }
    async fn provider_list(&self) -> anyhow::Result<ProviderListResponseDto> {
        self.inner.provider_list().await
    }
    async fn provider_update(&self, _req: ProviderUpdateRequestDto) -> anyhow::Result<ProviderDto> {
        // Covers dispatch.rs line 545: ControlResponse::ok(serde_json::to_value(dto)?)
        Ok(stub_provider_dto())
    }
    async fn provider_delete(&self, _req: ProviderDeleteRequestDto) -> anyhow::Result<()> {
        // Covers dispatch.rs line 551: ControlResponse::ok(serde_json::to_value(())?)
        Ok(())
    }
    async fn provider_test_connection(
        &self,
        req: ProviderTestConnectionRequestDto,
    ) -> anyhow::Result<ProviderTestConnectionResponseDto> {
        self.inner.provider_test_connection(req).await
    }

    // ── Model overrides (return Ok to cover dispatch success paths) ──
    async fn model_create(
        &self,
        _req: ModelCreateRequestDto,
    ) -> anyhow::Result<ModelCatalogEntryDto> {
        // Covers dispatch.rs line 573: ControlResponse::ok(serde_json::to_value(dto)?)
        Ok(stub_model_catalog_entry_dto())
    }
    async fn model_list(&self, _req: ModelListRequestDto) -> anyhow::Result<ModelListResponseDto> {
        // Covers dispatch.rs line 579: ControlResponse::ok(serde_json::to_value(dto)?)
        Ok(ModelListResponseDto {
            models: vec![stub_model_catalog_entry_dto()],
        })
    }
    async fn model_update(&self, _req: ModelUpdateRequestDto) -> anyhow::Result<()> {
        // Covers dispatch.rs line 585: ControlResponse::ok(serde_json::to_value(())?)
        Ok(())
    }
    async fn model_delete(&self, _req: ModelDeleteRequestDto) -> anyhow::Result<()> {
        // Covers dispatch.rs line 591: ControlResponse::ok(serde_json::to_value(())?)
        Ok(())
    }
    async fn model_tags_update(&self, _req: ModelTagUpdateDto) -> anyhow::Result<()> {
        // Covers dispatch.rs line 597: ControlResponse::ok(serde_json::to_value(())?)
        Ok(())
    }

    async fn pi_sidecar_locator_update(
        &self,
        req: PiSidecarLocatorUpdateRequestDto,
    ) -> anyhow::Result<PiSidecarLocatorUpdateResponseDto> {
        self.inner.pi_sidecar_locator_update(req).await
    }

    // ── Profile overrides (return Ok to cover dispatch success paths) ──
    async fn profile_create(&self, _req: ProfileCreateRequestDto) -> anyhow::Result<ProfileDto> {
        // Covers dispatch.rs line 577: ControlResponse::ok(serde_json::to_value(dto)?)
        Ok(stub_profile_dto())
    }
    async fn profile_update(&self, _req: ProfileUpdateRequestDto) -> anyhow::Result<ProfileDto> {
        // Covers dispatch.rs line 583: ControlResponse::ok(serde_json::to_value(dto)?)
        Ok(stub_profile_dto())
    }
    async fn profile_delete(&self, _req: ProfileDeleteRequestDto) -> anyhow::Result<()> {
        // Covers dispatch.rs line 589: ControlResponse::ok(serde_json::to_value(())?)
        Ok(())
    }

    fn event_bus(&self) -> &AppEventBus {
        self.inner.event_bus()
    }
}

async fn success_runtime() -> SuccessRuntime {
    let inner = Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    SuccessRuntime { inner }
}

// ---------------------------------------------------------------------------
// 4. Provider success-path dispatch tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatcher_routes_provider_create_returns_ok() {
    // Covers dispatch.rs line 535: serde_json::to_value(dto) succeeds and the
    // dispatcher wraps it in ControlResponse::ok(...).
    let runtime = success_runtime().await;
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({
        "name": "P",
        "provider_kind": "openai_compatible",
        "base_url": "http://x"
    });
    let response = dispatcher
        .dispatch(ControlRequest::new("provider.create", params))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            assert_eq!(val["id"], "p-stub");
            assert_eq!(val["name"], "Stub Provider");
            assert_eq!(val["enabled"], true);
            assert_eq!(val["has_api_key"], false);
        }
        other => panic!("expected Ok response, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatcher_routes_provider_update_returns_ok() {
    // Covers dispatch.rs line 545.
    let runtime = success_runtime().await;
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"id": "p1"});
    let response = dispatcher
        .dispatch(ControlRequest::new("provider.update", params))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            assert_eq!(val["id"], "p-stub");
        }
        other => panic!("expected Ok response, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatcher_routes_provider_delete_returns_ok_with_null() {
    // Covers dispatch.rs line 551: serde_json::to_value(())? serializes to `null`,
    // which becomes ControlResponse::ok(Value::Null).
    let runtime = success_runtime().await;
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"id": "p1"});
    let response = dispatcher
        .dispatch(ControlRequest::new("provider.delete", params))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            assert!(val.is_null(), "unit return serializes to null, got {val}");
        }
        other => panic!("expected Ok response, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 5. Profile success-path dispatch tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatcher_routes_profile_create_returns_ok() {
    // Covers dispatch.rs line 577.
    let runtime = success_runtime().await;
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"id": "prof1"});
    let response = dispatcher
        .dispatch(ControlRequest::new("profile.create", params))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            assert_eq!(val["id"], "prof-stub");
            assert_eq!(val["is_builtin"], false);
            assert_eq!(val["write_access"], false);
        }
        other => panic!("expected Ok response, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatcher_routes_profile_update_returns_ok() {
    // Covers dispatch.rs line 583.
    let runtime = success_runtime().await;
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"id": "prof1"});
    let response = dispatcher
        .dispatch(ControlRequest::new("profile.update", params))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            assert_eq!(val["id"], "prof-stub");
        }
        other => panic!("expected Ok response, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatcher_routes_profile_delete_returns_ok_with_null() {
    // Covers dispatch.rs line 589: serde_json::to_value(())? serializes to `null`.
    let runtime = success_runtime().await;
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"id": "prof1"});
    let response = dispatcher
        .dispatch(ControlRequest::new("profile.delete", params))
        .await
        .unwrap();

    match response {
        ControlResponse::Ok(val) => {
            assert!(val.is_null(), "unit return serializes to null, got {val}");
        }
        other => panic!("expected Ok response, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 6. Trait default on_request_meta (dispatch.rs line 241)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trait_default_on_request_meta_is_invoked_by_dispatcher() {
    // Covers dispatch.rs line 241: the trait default `fn on_request_meta(&self,
    // _meta: &RequestMeta) {}`. SuccessRuntime does NOT override on_request_meta,
    // so the dispatcher's `self.runtime.on_request_meta(&request.meta)` call
    // hits the default body. The default is a no-op; we verify behavior by
    // dispatching a request and confirming the response still succeeds.
    let runtime = success_runtime().await;
    let dispatcher = ControlDispatcher::new(runtime);

    let response = dispatcher
        .dispatch(ControlRequest::new("service.health", serde_json::json!({})))
        .await
        .unwrap();
    assert!(matches!(response, ControlResponse::Ok(_)));
}

// ---------------------------------------------------------------------------
// 7. Sanity: SuccessRuntime delegates methods it does not override
// ---------------------------------------------------------------------------

#[tokio::test]
async fn success_runtime_delegates_non_overridden_methods_to_inner() {
    // Verify the wrapper still delegates methods like provider_list,
    // provider_test_connection, and pi_sidecar_locator_update that we did NOT
    // override. This proves the wrapper is wired correctly and isn't
    // accidentally swallowing calls.
    let runtime = success_runtime().await;
    let dispatcher = ControlDispatcher::new(runtime);

    // provider.list — delegated to TestRuntimeControl, returns empty list.
    let response = dispatcher
        .dispatch(ControlRequest::new("provider.list", serde_json::json!({})))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert_eq!(val["providers"].as_array().unwrap().len(), 0);
        }
        other => panic!("provider.list should delegate to inner, got {other:?}"),
    }

    // provider.test_connection — delegated to TestRuntimeControl, returns ok=false.
    let response = dispatcher
        .dispatch(ControlRequest::new(
            "provider.test_connection",
            serde_json::json!({"id": "p1"}),
        ))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert_eq!(val["ok"], false);
        }
        other => panic!("provider.test_connection should delegate to inner, got {other:?}"),
    }

    // pi_sidecar_locator_update — delegated to TestRuntimeControl.
    let response = dispatcher
        .dispatch(ControlRequest::new(
            "pi_sidecar_locator_update",
            serde_json::json!({"runtime_dir": "/tmp/pi", "enabled": true}),
        ))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert_eq!(val["in_memory_updated"], true);
        }
        other => panic!("pi_sidecar_locator_update should delegate to inner, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 8. AllErrorRuntime — returns Err for every method, to cover the `?` Err
//    paths (await `?` error propagation) in dispatch match arms.
// ---------------------------------------------------------------------------

/// Wrapper that returns `Err` for every `Result`-returning method. This
/// exercises the `?` operator's Err path in each dispatch match arm
/// (e.g. `let dto = self.runtime.METHOD(...).await?;`), covering the
/// missed segments at the `?` position.
struct AllErrorRuntime {
    inner: Arc<TestRuntimeControl>,
}

#[async_trait::async_trait]
impl RuntimeControl for AllErrorRuntime {
    fn event_bus(&self) -> &AppEventBus {
        self.inner.event_bus()
    }

    async fn service_health(&self) -> anyhow::Result<ServiceHealthDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn service_status(&self) -> anyhow::Result<ServiceStatusDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn shell_status(&self) -> anyhow::Result<ShellStatusDto> {
        Err(anyhow::anyhow!("runtime error"))
    }

    async fn overview_summary(
        &self,
        _req: OverviewSummaryRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewSummaryDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn overview_trend(
        &self,
        _req: OverviewTrendRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewTrendResponseDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn overview_heatmap(
        &self,
        _req: OverviewHeatmapRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewHeatmapResponseDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn overview_rankings(
        &self,
        _req: OverviewRankingsRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewRankingsResponseDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }

    async fn receipt_daily(
        &self,
        _req: ReceiptDailyRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ReceiptDailyDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }

    async fn activity_recent(
        &self,
        _req: ActivityRecentRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ActivityRecentResponseDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn activity_list(
        &self,
        _req: ActivityListRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ActivityListResponseDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn activity_detail(
        &self,
        _req: ActivityDetailRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ActivityDetailDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }

    async fn breakdown_list(
        &self,
        _req: BreakdownListRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<BreakdownListResponseDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn breakdown_detail(
        &self,
        _req: BreakdownDetailRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<BreakdownDetailDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }

    async fn clients_snapshot(
        &self,
        _req: ClientsSnapshotRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ClientsSnapshotDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn clients_detail(
        &self,
        _req: ClientSourceDetailRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ClientSourceDetailDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }

    async fn settings_snapshot(&self) -> anyhow::Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn settings_update(
        &self,
        _req: SettingsUpdateRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn settings_diagnostics(
        &self,
    ) -> anyhow::Result<ReadEnvelopeDto<SettingsDiagnosticsDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn settings_recovery_action(
        &self,
        _req: SettingsRecoveryActionRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<SettingsRecoveryActionResponseDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }

    async fn live_window(
        &self,
        _req: LiveWindowRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<LiveWindowDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }

    async fn prompts_list(
        &self,
        _req: PromptListQueryDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptListResponseDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn prompts_get(
        &self,
        _req: PromptGetRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn prompts_create(
        &self,
        _req: PromptCreateRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn prompts_update(
        &self,
        _req: PromptUpdateRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn prompts_delete(
        &self,
        _req: PromptDeleteRequestDto,
    ) -> anyhow::Result<PromptDeleteResultDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn prompts_use(&self, _req: PromptUseRequestDto) -> anyhow::Result<PromptUseResultDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn suggest_tags(
        &self,
        _req: PromptSuggestTagsRequestDto,
    ) -> anyhow::Result<PromptSuggestTagsResponseDto> {
        Err(anyhow::anyhow!("runtime error"))
    }

    async fn subagent_delegate(
        &self,
        _req: SubagentDelegateRequestDto,
    ) -> anyhow::Result<SubagentDelegateResponseDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn subagent_list(
        &self,
        _req: SubagentListRequestDto,
    ) -> anyhow::Result<SubagentListResponseDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn subagent_show(
        &self,
        _req: SubagentResolveRequestDto,
    ) -> anyhow::Result<SubagentDetailDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn subagent_tasks(
        &self,
        _req: SubagentTasksRequestDto,
    ) -> anyhow::Result<SubagentTasksResponseDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn subagent_hibernate(
        &self,
        _req: SubagentResolveRequestDto,
    ) -> anyhow::Result<SubagentAckDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn subagent_delete(
        &self,
        _req: SubagentDeleteRequestDto,
    ) -> anyhow::Result<SubagentAckDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn subagent_runtime_status(
        &self,
        _req: SubagentRuntimeStatusRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<SubagentRuntimeStatusDto>> {
        Err(anyhow::anyhow!("runtime error"))
    }

    async fn provider_create(&self, _req: ProviderCreateRequestDto) -> anyhow::Result<ProviderDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn provider_list(&self) -> anyhow::Result<ProviderListResponseDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn provider_update(&self, _req: ProviderUpdateRequestDto) -> anyhow::Result<ProviderDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn provider_delete(&self, _req: ProviderDeleteRequestDto) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn provider_test_connection(
        &self,
        _req: ProviderTestConnectionRequestDto,
    ) -> anyhow::Result<ProviderTestConnectionResponseDto> {
        Err(anyhow::anyhow!("runtime error"))
    }

    async fn model_create(
        &self,
        _req: ModelCreateRequestDto,
    ) -> anyhow::Result<ModelCatalogEntryDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn model_list(&self, _req: ModelListRequestDto) -> anyhow::Result<ModelListResponseDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn model_update(&self, _req: ModelUpdateRequestDto) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn model_delete(&self, _req: ModelDeleteRequestDto) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn model_tags_update(&self, _req: ModelTagUpdateDto) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("runtime error"))
    }

    async fn pi_sidecar_locator_update(
        &self,
        _req: PiSidecarLocatorUpdateRequestDto,
    ) -> anyhow::Result<PiSidecarLocatorUpdateResponseDto> {
        Err(anyhow::anyhow!("runtime error"))
    }

    async fn profile_create(&self, _req: ProfileCreateRequestDto) -> anyhow::Result<ProfileDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn profile_update(&self, _req: ProfileUpdateRequestDto) -> anyhow::Result<ProfileDto> {
        Err(anyhow::anyhow!("runtime error"))
    }
    async fn profile_delete(&self, _req: ProfileDeleteRequestDto) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("runtime error"))
    }
}

async fn error_runtime() -> AllErrorRuntime {
    let inner = Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    AllErrorRuntime { inner }
}

/// Dispatch every method through `AllErrorRuntime` (which returns Err for all
/// methods) and verify each dispatch returns `Err`. This exercises the `?`
/// operator's Err path in each dispatch match arm -- the missed segments at
/// the `?` position of `let dto = self.runtime.METHOD(...).await?;` lines.
#[tokio::test]
async fn dispatch_all_methods_through_error_runtime_returns_err() {
    let runtime = error_runtime().await;
    let dispatcher = ControlDispatcher::new(runtime);

    // Methods that accept empty params -- the `?` Err path on the await line
    // is the ONLY `?` (no from_value `?` before it), so these are the most
    // important to cover.
    let empty_param_methods = [
        "service.health",
        "service.status",
        "shell.status",
        "settings.snapshot",
        "settings.diagnostics",
        "provider.list",
    ];
    for method in empty_param_methods {
        let result = dispatcher
            .dispatch(ControlRequest::new(method, serde_json::json!({})))
            .await;
        assert!(
            result.is_err(),
            "{method} should propagate Err from runtime"
        );
    }

    // Methods that require specific params -- pass valid params so from_value
    // succeeds and we reach the `await?` line, which then triggers the `?`
    // Err path because AllErrorRuntime returns Err.
    let param_methods: Vec<(&str, serde_json::Value)> = vec![
        ("overview.summary", serde_json::json!({"range": "day"})),
        ("overview.trend", serde_json::json!({"range": "day"})),
        ("overview.heatmap", serde_json::json!({"range": "day"})),
        ("overview.rankings", serde_json::json!({"range": "day"})),
        ("receipt.daily", serde_json::json!({})),
        ("activity.recent", serde_json::json!({"range": "day"})),
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
        ("clients.snapshot", serde_json::json!({})),
        ("clients.detail", serde_json::json!({"source_id": "src-1"})),
        (
            "settings.update",
            serde_json::json!({"timezone": "US/Pacific"}),
        ),
        (
            "settings.recovery_action",
            serde_json::json!({"id": "rescan_all"}),
        ),
        ("live.window", serde_json::json!({})),
        ("prompts.list", serde_json::json!({})),
        ("prompts.get", serde_json::json!({"id": "prompt-1"})),
        (
            "prompts.create",
            serde_json::json!({"content": "test", "alias": null, "tags": []}),
        ),
        (
            "prompts.update",
            serde_json::json!({
                "id": "prompt-1", "content": "test", "alias": null, "tags": [], "is_pinned": false
            }),
        ),
        ("prompts.delete", serde_json::json!({"id": "prompt-1"})),
        (
            "prompts.use",
            serde_json::json!({
                "id": "prompt-1", "action": "copy_and_paste",
                "surface": "overlay", "outcome": "copy"
            }),
        ),
        ("prompts.suggest_tags", serde_json::json!({})),
        (
            "subagent.delegate",
            serde_json::json!({
                "subagent_name": "reviewer", "cwd": "/tmp/repo",
                "profile": "pi/search-cheap", "prompt": "x"
            }),
        ),
        ("subagent.list", serde_json::json!({})),
        ("subagent.show", serde_json::json!({"id": "sa-1"})),
        ("subagent.tasks", serde_json::json!({"id": "sa-1"})),
        ("subagent.hibernate", serde_json::json!({"id": "sa-1"})),
        ("subagent.delete", serde_json::json!({"id": "sa-1"})),
        ("subagent.runtime_status", serde_json::json!({})),
        (
            "provider.create",
            serde_json::json!({
                "name": "P", "provider_kind": "openai_compatible", "base_url": "http://x"
            }),
        ),
        ("provider.update", serde_json::json!({"id": "p1"})),
        ("provider.delete", serde_json::json!({"id": "p1"})),
        ("provider.test_connection", serde_json::json!({"id": "p1"})),
        (
            "model.create",
            serde_json::json!({"provider_id": "p1", "model_id": "m1"}),
        ),
        ("model.list", serde_json::json!({})),
        (
            "model.update",
            serde_json::json!({"id": "m1", "enabled": false}),
        ),
        ("model.delete", serde_json::json!({"id": "m1"})),
        (
            "model.tags.update",
            serde_json::json!({"model_id": "m1", "tags": ["fast"]}),
        ),
        (
            "pi_sidecar_locator_update",
            serde_json::json!({"runtime_dir": "/tmp/pi", "enabled": true}),
        ),
        (
            "profile.create",
            serde_json::json!({"id": "prof1", "model": "m"}),
        ),
        ("profile.update", serde_json::json!({"id": "prof1"})),
        ("profile.delete", serde_json::json!({"id": "prof1"})),
    ];

    for (method, params) in param_methods {
        let result = dispatcher
            .dispatch(ControlRequest::new(method, params))
            .await;
        assert!(
            result.is_err(),
            "{method} should propagate Err from runtime"
        );
    }
}

// ---------------------------------------------------------------------------
// 7. Model success-path dispatch tests (covers dispatch.rs L571-598)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatcher_routes_model_create_returns_ok() {
    // Covers dispatch.rs line 573: ControlResponse::ok(serde_json::to_value(dto)?)
    let runtime = success_runtime().await;
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({
        "provider_id": "p-stub",
        "model_id": "stub-model",
        "context_window": 8192,
        "max_tokens": 4096
    });
    let response = dispatcher
        .dispatch(ControlRequest::new("model.create", params))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert_eq!(val["model_db_id"], "m-stub");
            assert_eq!(val["model_id"], "stub-model");
        }
        other => panic!("expected Ok response, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatcher_routes_model_list_returns_ok() {
    // Covers dispatch.rs line 579: ControlResponse::ok(serde_json::to_value(dto)?)
    let runtime = success_runtime().await;
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({});
    let response = dispatcher
        .dispatch(ControlRequest::new("model.list", params))
        .await
        .unwrap();
    match response {
        ControlResponse::Ok(val) => {
            assert!(val["models"].is_array());
            assert_eq!(val["models"][0]["model_id"], "stub-model");
        }
        other => panic!("expected Ok response, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatcher_routes_model_update_returns_ok_with_null() {
    // Covers dispatch.rs line 585: ControlResponse::ok(serde_json::to_value(())?)
    let runtime = success_runtime().await;
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"id": "m-stub", "enabled": false});
    let response = dispatcher
        .dispatch(ControlRequest::new("model.update", params))
        .await
        .unwrap();
    assert!(matches!(response, ControlResponse::Ok(_)));
}

#[tokio::test]
async fn dispatcher_routes_model_delete_returns_ok_with_null() {
    // Covers dispatch.rs line 591: ControlResponse::ok(serde_json::to_value(())?)
    let runtime = success_runtime().await;
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"id": "m-stub"});
    let response = dispatcher
        .dispatch(ControlRequest::new("model.delete", params))
        .await
        .unwrap();
    assert!(matches!(response, ControlResponse::Ok(_)));
}

#[tokio::test]
async fn dispatcher_routes_model_tags_update_returns_ok_with_null() {
    // Covers dispatch.rs line 597: ControlResponse::ok(serde_json::to_value(())?)
    let runtime = success_runtime().await;
    let dispatcher = ControlDispatcher::new(runtime);
    let params = serde_json::json!({"model_id": "m-stub", "tags": ["fast"]});
    let response = dispatcher
        .dispatch(ControlRequest::new("model.tags.update", params))
        .await
        .unwrap();
    assert!(matches!(response, ControlResponse::Ok(_)));
}

// ---------------------------------------------------------------------------
// 5. Arc<T> blanket impl delegation (dispatch.rs lines 1545-1559)
// ---------------------------------------------------------------------------
//
// `impl<T: RuntimeControl> RuntimeControl for Arc<T>` forwards every method
// via `(**self).method(req).await`. The model_*, pi_sidecar_locator_update,
// and profile_* forwarding lines (dispatch.rs L1545-1559) are uncovered
// because no existing test calls these methods directly on an `Arc<T>`.
// Calling each method through `Arc<TestRuntimeControl>` exercises the
// blanket-impl forwarding bodies. The inner `TestRuntimeControl` returns
// `Err` for model_*/profile_* (which is fine — the forwarding line is
// covered regardless of the inner result) and `Ok` for pi_sidecar.

#[tokio::test]
async fn arc_blanket_impl_delegates_model_profile_and_sidecar_methods() {
    use std::sync::Arc;
    // Covers dispatch.rs L1545-1559: `impl RuntimeControl for Arc<T>` forwarding
    // bodies for model_create/model_list/model_update/model_delete/
    // model_tags_update/pi_sidecar_locator_update/profile_create/
    // profile_update/profile_delete.
    let rt: Arc<TestRuntimeControl> =
        Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());

    // model_* — TestRuntimeControl bails "not yet implemented"; the forwarding
    // line is covered either way.
    let _ = rt
        .model_create(ModelCreateRequestDto {
            provider_id: "p1".to_string(),
            model_id: "m1".to_string(),
            enabled: None,
            tags: vec![],
            context_window: 8192,
            max_tokens: 4096,
            display_name: None,
            reasoning: None,
        })
        .await;
    let _ = rt
        .model_list(ModelListRequestDto {
            provider_id: None,
            tags: vec![],
            include_disabled: false,
        })
        .await;
    let _ = rt
        .model_update(ModelUpdateRequestDto {
            id: "m1".to_string(),
            enabled: None,
            display_name: None,
            reasoning: None,
            context_window: None,
            max_tokens: None,
        })
        .await;
    let _ = rt
        .model_delete(ModelDeleteRequestDto {
            id: "m1".to_string(),
        })
        .await;
    let _ = rt
        .model_tags_update(ModelTagUpdateDto {
            model_id: "m1".to_string(),
            tags: vec![],
        })
        .await;

    // pi_sidecar_locator_update — TestRuntimeControl returns Ok.
    let sidecar_resp = rt
        .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
            runtime_dir: "/tmp/sidecar".to_string(),
            enabled: true,
        })
        .await
        .expect("pi_sidecar_locator_update should succeed on TestRuntimeControl");
    assert_eq!(sidecar_resp.runtime_dir, "/tmp/sidecar");
    assert!(sidecar_resp.enabled);
    assert!(sidecar_resp.in_memory_updated);

    // profile_* — TestRuntimeControl bails "not yet implemented".
    let _ = rt
        .profile_create(ProfileCreateRequestDto {
            id: "prof1".to_string(),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await;
    let _ = rt
        .profile_update(ProfileUpdateRequestDto {
            id: "prof1".to_string(),
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        })
        .await;
    let _ = rt
        .profile_delete(ProfileDeleteRequestDto {
            id: "prof1".to_string(),
        })
        .await;
}
