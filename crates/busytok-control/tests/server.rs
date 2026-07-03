#![allow(
    clippy::await_holding_lock,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::field_reassign_with_default,
    clippy::uninlined_format_args,
    clippy::inconsistent_digit_grouping,
    clippy::len_zero,
    clippy::identity_op,
    clippy::useless_vec,
    clippy::manual_dangling_ptr,
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::absurd_extreme_comparisons,
    unused_variables,
    unused_imports,
    dead_code
)]
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use busytok_control::{
    client::ControlClient, server::ControlServer, RuntimeControl, TestRuntimeControl,
};
use busytok_events::AppEvent;
use busytok_protocol::{dto::*, ControlRequest, ControlResponse};
#[cfg(unix)]
use tokio::net::UnixStream;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedLogBuffer {
    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }

    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedLogBuffer {
    type Writer = SharedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedLogWriter(Arc::clone(&self.0))
    }
}

fn test_logs() -> SharedLogBuffer {
    static LOGS: OnceLock<SharedLogBuffer> = OnceLock::new();
    LOGS.get_or_init(SharedLogBuffer::default).clone()
}

fn init_test_logging() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(test_logs())
            .with_ansi(false)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}

struct MethodDispatchErrorRuntime {
    inner: TestRuntimeControl,
}

#[async_trait::async_trait]
impl RuntimeControl for MethodDispatchErrorRuntime {
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
        _req: OverviewSummaryRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewSummaryDto>> {
        Err(anyhow::Error::new(
            busytok_control::dispatch::MethodDispatchError {
                code: "read_timeout".to_string(),
                message: "read timed out".to_string(),
                payload: Some(serde_json::json!({
                    "kind": "read_timeout",
                    "query_family": "overview_summary",
                })),
            },
        ))
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
        req: busytok_protocol::dto::SubagentDelegateRequestDto,
    ) -> anyhow::Result<busytok_protocol::dto::SubagentDelegateResponseDto> {
        self.inner.subagent_delegate(req).await
    }

    async fn subagent_list(
        &self,
        req: busytok_protocol::dto::SubagentListRequestDto,
    ) -> anyhow::Result<busytok_protocol::dto::SubagentListResponseDto> {
        self.inner.subagent_list(req).await
    }

    async fn subagent_show(
        &self,
        req: busytok_protocol::dto::SubagentResolveRequestDto,
    ) -> anyhow::Result<busytok_protocol::dto::SubagentDetailDto> {
        self.inner.subagent_show(req).await
    }

    async fn subagent_tasks(
        &self,
        req: busytok_protocol::dto::SubagentTasksRequestDto,
    ) -> anyhow::Result<busytok_protocol::dto::SubagentTasksResponseDto> {
        self.inner.subagent_tasks(req).await
    }

    async fn subagent_hibernate(
        &self,
        req: busytok_protocol::dto::SubagentResolveRequestDto,
    ) -> anyhow::Result<busytok_protocol::dto::SubagentAckDto> {
        self.inner.subagent_hibernate(req).await
    }

    async fn subagent_delete(
        &self,
        req: busytok_protocol::dto::SubagentDeleteRequestDto,
    ) -> anyhow::Result<busytok_protocol::dto::SubagentAckDto> {
        self.inner.subagent_delete(req).await
    }
    async fn subagent_runtime_status(
        &self,
        req: busytok_protocol::dto::SubagentRuntimeStatusRequestDto,
    ) -> anyhow::Result<
        busytok_protocol::dto::ReadEnvelopeDto<busytok_protocol::dto::SubagentRuntimeStatusDto>,
    > {
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

    fn event_bus(&self) -> &busytok_events::AppEventBus {
        self.inner.event_bus()
    }

    fn on_request_meta(&self, meta: &RequestMeta) {
        self.inner.on_request_meta(meta);
    }
}

#[tokio::test]
async fn normal_rpc_disconnect_does_not_emit_subscription_diagnostics() {
    let runtime = Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let mut events = runtime.event_bus().subscribe();
    let (server, socket_path): (ControlServer, _) =
        ControlServer::spawn_for_test(Arc::clone(&runtime) as Arc<dyn RuntimeControl>)
            .await
            .unwrap();
    let server = Arc::new(server);
    let server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    {
        let mut client: ControlClient = ControlClient::connect(&socket_path).await.unwrap();
        let response = client
            .call(ControlRequest::new("shell.status", serde_json::json!({})))
            .await
            .unwrap();
        assert!(matches!(response, busytok_protocol::ControlResponse::Ok(_)));
    }

    let result = tokio::time::timeout(Duration::from_millis(100), events.recv()).await;
    assert!(
        result.is_err(),
        "ordinary one-shot RPC clients must not publish subscription lifecycle events: {result:?}"
    );

    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn normal_rpc_disconnect_is_graceful_for_single_connection_handler() {
    let runtime = Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let (server, socket_path): (ControlServer, _) =
        ControlServer::spawn_for_test(runtime).await.unwrap();
    let server = Arc::new(server);
    let accept_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.accept_one().await })
    };

    {
        let mut client: ControlClient = ControlClient::connect(&socket_path).await.unwrap();
        let response = client
            .call(ControlRequest::new("shell.status", serde_json::json!({})))
            .await
            .unwrap();
        assert!(matches!(response, busytok_protocol::ControlResponse::Ok(_)));
    }

    accept_task
        .await
        .expect("accept task should not panic")
        .expect("normal RPC client close should be graceful");
}

#[cfg(unix)]
#[tokio::test]
async fn socket_probe_without_handshake_is_graceful() {
    let runtime = Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let (server, socket_path): (ControlServer, _) =
        ControlServer::spawn_for_test(runtime).await.unwrap();
    let server = Arc::new(server);
    let accept_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.accept_one().await })
    };

    {
        let _probe = UnixStream::connect(&socket_path).await.unwrap();
    }

    accept_task
        .await
        .expect("accept task should not panic")
        .expect("socket readiness probes should be graceful");
}

#[tokio::test]
async fn event_subscription_emits_subscription_connected() {
    let runtime = Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let mut events = runtime.event_bus().subscribe();
    let (server, socket_path): (ControlServer, _) =
        ControlServer::spawn_for_test(Arc::clone(&runtime) as Arc<dyn RuntimeControl>)
            .await
            .unwrap();
    let server = Arc::new(server);
    let server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    let mut client: ControlClient = ControlClient::connect(&socket_path).await.unwrap();
    let ack = client
        .subscribe(vec!["live:sample".to_string()])
        .await
        .unwrap();
    assert!(matches!(ack, busytok_protocol::ControlResponse::Ok(_)));

    let published = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("subscription lifecycle event should be emitted")
        .expect("event bus should stay open");
    assert!(matches!(
        published.event,
        AppEvent::SubscriptionConnected { .. }
    ));

    drop(client);
    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn method_dispatch_error_preserves_code_and_payload() {
    let runtime = Arc::new(MethodDispatchErrorRuntime {
        inner: TestRuntimeControl::with_claude_fixture().await.unwrap(),
    });
    let (server, socket_path): (ControlServer, _) =
        ControlServer::spawn_for_test(Arc::clone(&runtime) as Arc<dyn RuntimeControl>)
            .await
            .unwrap();
    let server = Arc::new(server);
    let server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    let mut client: ControlClient = ControlClient::connect(&socket_path).await.unwrap();
    let response = client
        .call(ControlRequest::new(
            "overview.summary",
            serde_json::json!({"range": "day"}),
        ))
        .await
        .unwrap();

    match response {
        ControlResponse::Err(err) => {
            assert_eq!(err.code, "read_timeout");
            assert_eq!(err.message, "read timed out");
            assert_eq!(
                err.payload,
                Some(serde_json::json!({
                    "kind": "read_timeout",
                    "query_family": "overview_summary",
                }))
            );
        }
        other => panic!("expected error response, got {other:?}"),
    }

    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn control_dispatch_logs_completion_fields() {
    init_test_logging();
    let logs = test_logs();
    logs.clear();

    let runtime = Arc::new(TestRuntimeControl::with_claude_fixture().await.unwrap());
    let (server, socket_path): (ControlServer, _) =
        ControlServer::spawn_for_test(Arc::clone(&runtime) as Arc<dyn RuntimeControl>)
            .await
            .unwrap();
    let server = Arc::new(server);
    let server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    let mut client: ControlClient = ControlClient::connect(&socket_path).await.unwrap();
    let ok_response = client
        .call(ControlRequest::new("shell.status", serde_json::json!({})))
        .await
        .unwrap();
    assert!(matches!(ok_response, ControlResponse::Ok(_)));

    let err_response = client
        .call(ControlRequest::new(
            "nonexistent.method",
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert!(matches!(err_response, ControlResponse::Err(_)));

    tokio::time::sleep(Duration::from_millis(50)).await;
    let rendered = logs.text();
    assert!(rendered.contains("control.dispatch.completed"));
    assert!(rendered.contains("shell.status"));
    assert!(rendered.contains("nonexistent.method"));
    assert!(rendered.contains("method_not_found"));
    assert!(rendered.contains("payload_bytes="));

    drop(client);
    server.shutdown();
    server.await_drain().await;
    server_task.await.unwrap().unwrap();
}
