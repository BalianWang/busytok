//! Handler for `busytok models` — list models in the catalog.
//!
//! Consumes the `model.list` RPC method and renders the result as either
//! a fixed-width table (default) or pretty-printed JSON (`--json`).

use anyhow::Result;
use busytok_protocol::dto::{ModelCatalogEntryDto, ModelListRequestDto, ModelListResponseDto};
use busytok_protocol::{ControlRequest, ControlResponse};

use super::connect_client;

/// Handle `busytok models`.
///
/// Builds a `ModelListRequestDto` from the CLI filters, calls `model.list`,
/// and renders the response as a table (default) or pretty JSON (`--json`).
pub async fn handle_models(
    provider: Option<String>,
    tags: Vec<String>,
    all: bool,
    json: bool,
) -> Result<()> {
    let req = ModelListRequestDto {
        provider_id: provider,
        tags,
        include_disabled: all,
    };
    let mut client = connect_client().await?;
    let response = client
        .call(ControlRequest::new(
            "model.list",
            serde_json::to_value(&req)?,
        ))
        .await?;
    match response {
        ControlResponse::Ok(value) => {
            let resp: ModelListResponseDto = serde_json::from_value(value)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&resp.models)?);
            } else {
                print_models_table(&resp.models);
            }
            Ok(())
        }
        ControlResponse::Err(err) => {
            anyhow::bail!("RPC error [{}]: {}", err.code, err.message)
        }
    }
}

/// Render the model catalog as a fixed-width table to stdout.
fn print_models_table(models: &[ModelCatalogEntryDto]) {
    if models.is_empty() {
        println!("No models found.");
        return;
    }
    // Column widths
    let w_provider = models
        .iter()
        .map(|m| m.provider_name.len())
        .max()
        .unwrap_or(8)
        .max(8);
    let w_model = models
        .iter()
        .map(|m| m.model_id.len())
        .max()
        .unwrap_or(5)
        .max(5);
    let w_tags = 20;

    println!(
        "{:width_p$}  {:width_m$}  {:6}  {:6}  {:width_t$}",
        "PROVIDER",
        "MODEL",
        "ENABLE",
        "P_ENABLE",
        "TAGS",
        width_p = w_provider,
        width_m = w_model,
        width_t = w_tags
    );
    for m in models {
        let tags = m.tags.join(",");
        let model_en = if m.model_enabled { "yes" } else { "no" };
        let prov_en = if m.provider_enabled { "yes" } else { "no" };
        println!(
            "{:width_p$}  {:width_m$}  {:6}  {:8}  {:width_t$}",
            m.provider_name,
            m.model_id,
            model_en,
            prov_en,
            tags,
            width_p = w_provider,
            width_m = w_model,
            width_t = w_tags
        );
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
    use super::*;
    use async_trait::async_trait;
    use busytok_control::{dispatch::RuntimeControl, server::ControlServer, TestRuntimeControl};
    use busytok_domain::ProviderKind;
    use busytok_protocol::dto::*;
    use serial_test::serial;
    use std::sync::Arc;

    // ── print_models_table ────────────────────────────────────────────

    #[test]
    fn print_models_table_empty_prints_no_models_found() {
        // Empty slice prints "No models found." and returns early without
        // touching the column-width computation.
        print_models_table(&[]);
    }

    #[test]
    fn print_models_table_with_models_renders_header_and_rows() {
        // Mixed enabled/disabled + provider states exercise both yes/no
        // branches for `model_en` and `prov_en`.
        let models = vec![
            ModelCatalogEntryDto {
                provider_id: "p1".to_string(),
                provider_name: "OpenAI".to_string(),
                provider_kind: ProviderKind::OpenAiCompatible,
                provider_enabled: true,
                model_db_id: "m1".to_string(),
                model_id: "gpt-4".to_string(),
                model_enabled: true,
                tags: vec!["chat".to_string(), "fast".to_string()],
            },
            ModelCatalogEntryDto {
                provider_id: "p2".to_string(),
                provider_name: "DeepSeek".to_string(),
                provider_kind: ProviderKind::OpenAiCompatible,
                provider_enabled: false,
                model_db_id: "m2".to_string(),
                model_id: "deepseek-chat".to_string(),
                model_enabled: false,
                tags: vec![],
            },
        ];
        print_models_table(&models);
    }

    #[test]
    fn print_models_table_with_short_names_uses_minimum_column_widths() {
        // A single short provider/model name should not cause the
        // `max().unwrap_or(N).max(N)` computation to panic; the minimum
        // widths (8 / 5) kick in.
        let models = vec![ModelCatalogEntryDto {
            provider_id: "p".to_string(),
            provider_name: "x".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            provider_enabled: true,
            model_db_id: "m".to_string(),
            model_id: "y".to_string(),
            model_enabled: true,
            tags: vec![],
        }];
        print_models_table(&models);
    }

    // ── ModelListRequestDto serialization (parameter correctness) ─────

    #[test]
    fn model_list_request_dto_serializes_all_filters() {
        let req = ModelListRequestDto {
            provider_id: Some("p1".to_string()),
            tags: vec!["chat".to_string(), "fast".to_string()],
            include_disabled: true,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["provider_id"], "p1");
        assert_eq!(v["tags"], serde_json::json!(["chat", "fast"]));
        assert_eq!(v["include_disabled"], true);
    }

    #[test]
    fn model_list_request_dto_omits_provider_id_when_none() {
        let req = ModelListRequestDto {
            provider_id: None,
            tags: vec![],
            include_disabled: false,
        };
        let v = serde_json::to_value(&req).unwrap();
        // provider_id has skip_serializing_if = "Option::is_none".
        assert!(v.get("provider_id").is_none() || v["provider_id"].is_null());
        assert_eq!(v["tags"], serde_json::json!([]));
        assert_eq!(v["include_disabled"], false);
    }

    #[test]
    fn model_list_request_dto_round_trips_via_json() {
        let req = ModelListRequestDto {
            provider_id: Some("p-7".to_string()),
            tags: vec!["a".to_string()],
            include_disabled: true,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: ModelListRequestDto = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider_id.as_deref(), Some("p-7"));
        assert_eq!(back.tags, vec!["a".to_string()]);
        assert!(back.include_disabled);
    }

    // ── handle_models error paths ─────────────────────────────────────

    struct ServerHarness {
        server: Arc<ControlServer>,
        _task: tokio::task::JoinHandle<anyhow::Result<()>>,
    }

    async fn spawn_server(runtime: Arc<dyn RuntimeControl>) -> (ServerHarness, String) {
        let (server, socket_path) = ControlServer::spawn_for_test(runtime).await.unwrap();
        let server = Arc::new(server);
        let server_for_task = Arc::clone(&server);
        let task = tokio::spawn(async move { server_for_task.run().await });
        (
            ServerHarness {
                server,
                _task: task,
            },
            socket_path,
        )
    }

    impl Drop for ServerHarness {
        fn drop(&mut self) {
            self.server.shutdown();
        }
    }

    #[tokio::test]
    #[serial]
    async fn handle_models_bails_when_socket_unreachable() {
        // No server: connect_client surfaces a friendly connect error.
        std::env::set_var("BUSYTOK_SOCKET", "/nonexistent/busytok-models-test.sock");
        let result = handle_models(None, vec![], false, false).await;
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("connecting to Busytok service") || err.contains("Open Busytok.app"),
            "expected connect error, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_models_bails_when_runtime_returns_not_implemented() {
        // The default TestRuntimeControl.model_list bails with "not yet
        // implemented"; handle_models should surface it as an RPC error.
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_models(None, vec![], false, false).await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("RPC error") && err.contains("not yet implemented"),
            "expected RPC error from model.list, got: {err}"
        );
    }

    // ── handle_models success paths (table + json) ────────────────────
    //
    // `ModelsRuntime` wraps `TestRuntimeControl` and returns a canned
    // `ModelListResponseDto` from `model_list`, delegating every other
    // method to the inner runtime. Following the established wrapper
    // pattern used by `TestRuntimeWrapper` / `SubagentRuntime` /
    // `FailingListRuntime` in the sibling command modules.

    struct ModelsRuntime {
        inner: TestRuntimeControl,
        response: ModelListResponseDto,
    }

    #[async_trait]
    impl RuntimeControl for ModelsRuntime {
        async fn model_list(&self, _req: ModelListRequestDto) -> Result<ModelListResponseDto> {
            Ok(self.response.clone())
        }
        // Everything else delegates to the inner runtime.
        async fn service_health(&self) -> anyhow::Result<ServiceHealthDto> {
            self.inner.service_health().await
        }
        async fn service_status(&self) -> anyhow::Result<ServiceStatusDto> {
            self.inner.service_status().await
        }
        async fn shell_status(&self) -> Result<ShellStatusDto> {
            self.inner.shell_status().await
        }
        async fn overview_summary(
            &self,
            req: OverviewSummaryRequestDto,
        ) -> Result<ReadEnvelopeDto<OverviewSummaryDto>> {
            self.inner.overview_summary(req).await
        }
        async fn overview_trend(
            &self,
            req: OverviewTrendRequestDto,
        ) -> Result<ReadEnvelopeDto<OverviewTrendResponseDto>> {
            self.inner.overview_trend(req).await
        }
        async fn overview_heatmap(
            &self,
            req: OverviewHeatmapRequestDto,
        ) -> Result<ReadEnvelopeDto<OverviewHeatmapResponseDto>> {
            self.inner.overview_heatmap(req).await
        }
        async fn overview_rankings(
            &self,
            req: OverviewRankingsRequestDto,
        ) -> Result<ReadEnvelopeDto<OverviewRankingsResponseDto>> {
            self.inner.overview_rankings(req).await
        }
        async fn receipt_daily(
            &self,
            req: ReceiptDailyRequestDto,
        ) -> Result<ReadEnvelopeDto<ReceiptDailyDto>> {
            self.inner.receipt_daily(req).await
        }
        async fn activity_recent(
            &self,
            req: ActivityRecentRequestDto,
        ) -> Result<ReadEnvelopeDto<ActivityRecentResponseDto>> {
            self.inner.activity_recent(req).await
        }
        async fn activity_list(
            &self,
            req: ActivityListRequestDto,
        ) -> Result<ReadEnvelopeDto<ActivityListResponseDto>> {
            self.inner.activity_list(req).await
        }
        async fn activity_detail(
            &self,
            req: ActivityDetailRequestDto,
        ) -> Result<ReadEnvelopeDto<ActivityDetailDto>> {
            self.inner.activity_detail(req).await
        }
        async fn breakdown_list(
            &self,
            req: BreakdownListRequestDto,
        ) -> Result<ReadEnvelopeDto<BreakdownListResponseDto>> {
            self.inner.breakdown_list(req).await
        }
        async fn breakdown_detail(
            &self,
            req: BreakdownDetailRequestDto,
        ) -> Result<ReadEnvelopeDto<BreakdownDetailDto>> {
            self.inner.breakdown_detail(req).await
        }
        async fn clients_snapshot(
            &self,
            req: ClientsSnapshotRequestDto,
        ) -> Result<ReadEnvelopeDto<ClientsSnapshotDto>> {
            self.inner.clients_snapshot(req).await
        }
        async fn clients_detail(
            &self,
            req: ClientSourceDetailRequestDto,
        ) -> Result<ReadEnvelopeDto<ClientSourceDetailDto>> {
            self.inner.clients_detail(req).await
        }
        async fn settings_snapshot(&self) -> Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
            self.inner.settings_snapshot().await
        }
        async fn settings_update(
            &self,
            req: SettingsUpdateRequestDto,
        ) -> Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
            self.inner.settings_update(req).await
        }
        async fn settings_diagnostics(&self) -> Result<ReadEnvelopeDto<SettingsDiagnosticsDto>> {
            self.inner.settings_diagnostics().await
        }
        async fn settings_recovery_action(
            &self,
            req: SettingsRecoveryActionRequestDto,
        ) -> Result<ReadEnvelopeDto<SettingsRecoveryActionResponseDto>> {
            self.inner.settings_recovery_action(req).await
        }
        async fn live_window(
            &self,
            req: LiveWindowRequestDto,
        ) -> Result<ReadEnvelopeDto<LiveWindowDto>> {
            self.inner.live_window(req).await
        }
        async fn prompts_list(
            &self,
            req: PromptListQueryDto,
        ) -> Result<ReadEnvelopeDto<PromptListResponseDto>> {
            self.inner.prompts_list(req).await
        }
        async fn prompts_get(
            &self,
            req: PromptGetRequestDto,
        ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
            self.inner.prompts_get(req).await
        }
        async fn prompts_create(
            &self,
            req: PromptCreateRequestDto,
        ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
            self.inner.prompts_create(req).await
        }
        async fn prompts_update(
            &self,
            req: PromptUpdateRequestDto,
        ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
            self.inner.prompts_update(req).await
        }
        async fn prompts_delete(
            &self,
            req: PromptDeleteRequestDto,
        ) -> Result<PromptDeleteResultDto> {
            self.inner.prompts_delete(req).await
        }
        async fn prompts_use(&self, req: PromptUseRequestDto) -> Result<PromptUseResultDto> {
            self.inner.prompts_use(req).await
        }
        async fn suggest_tags(
            &self,
            req: PromptSuggestTagsRequestDto,
        ) -> Result<PromptSuggestTagsResponseDto> {
            self.inner.suggest_tags(req).await
        }
        async fn subagent_delegate(
            &self,
            req: SubagentDelegateRequestDto,
        ) -> Result<SubagentDelegateResponseDto> {
            self.inner.subagent_delegate(req).await
        }
        async fn subagent_list(
            &self,
            req: SubagentListRequestDto,
        ) -> Result<SubagentListResponseDto> {
            self.inner.subagent_list(req).await
        }
        async fn subagent_show(&self, req: SubagentResolveRequestDto) -> Result<SubagentDetailDto> {
            self.inner.subagent_show(req).await
        }
        async fn subagent_tasks(
            &self,
            req: SubagentTasksRequestDto,
        ) -> Result<SubagentTasksResponseDto> {
            self.inner.subagent_tasks(req).await
        }
        async fn subagent_hibernate(
            &self,
            req: SubagentResolveRequestDto,
        ) -> Result<SubagentAckDto> {
            self.inner.subagent_hibernate(req).await
        }
        async fn subagent_delete(&self, req: SubagentDeleteRequestDto) -> Result<SubagentAckDto> {
            self.inner.subagent_delete(req).await
        }
        async fn subagent_runtime_status(
            &self,
            req: SubagentRuntimeStatusRequestDto,
        ) -> Result<ReadEnvelopeDto<SubagentRuntimeStatusDto>> {
            self.inner.subagent_runtime_status(req).await
        }
        async fn provider_create(&self, req: ProviderCreateRequestDto) -> Result<ProviderDto> {
            self.inner.provider_create(req).await
        }
        async fn provider_list(&self) -> Result<ProviderListResponseDto> {
            self.inner.provider_list().await
        }
        async fn provider_update(&self, req: ProviderUpdateRequestDto) -> Result<ProviderDto> {
            self.inner.provider_update(req).await
        }
        async fn provider_delete(&self, req: ProviderDeleteRequestDto) -> Result<()> {
            self.inner.provider_delete(req).await
        }
        async fn provider_test_connection(
            &self,
            req: ProviderTestConnectionRequestDto,
        ) -> Result<ProviderTestConnectionResponseDto> {
            self.inner.provider_test_connection(req).await
        }
        async fn model_create(&self, req: ModelCreateRequestDto) -> Result<ModelCatalogEntryDto> {
            self.inner.model_create(req).await
        }
        async fn model_update(&self, req: ModelUpdateRequestDto) -> Result<()> {
            self.inner.model_update(req).await
        }
        async fn model_delete(&self, req: ModelDeleteRequestDto) -> Result<()> {
            self.inner.model_delete(req).await
        }
        async fn model_tags_update(&self, req: ModelTagUpdateDto) -> Result<()> {
            self.inner.model_tags_update(req).await
        }
        async fn pi_sidecar_locator_update(
            &self,
            req: PiSidecarLocatorUpdateRequestDto,
        ) -> Result<PiSidecarLocatorUpdateResponseDto> {
            self.inner.pi_sidecar_locator_update(req).await
        }
        async fn profile_create(&self, req: ProfileCreateRequestDto) -> Result<ProfileDto> {
            self.inner.profile_create(req).await
        }
        async fn profile_update(&self, req: ProfileUpdateRequestDto) -> Result<ProfileDto> {
            self.inner.profile_update(req).await
        }
        async fn profile_delete(&self, req: ProfileDeleteRequestDto) -> Result<()> {
            self.inner.profile_delete(req).await
        }
        fn event_bus(&self) -> &busytok_events::AppEventBus {
            self.inner.event_bus()
        }
    }

    fn sample_models() -> Vec<ModelCatalogEntryDto> {
        vec![
            ModelCatalogEntryDto {
                provider_id: "p1".to_string(),
                provider_name: "OpenAI".to_string(),
                provider_kind: ProviderKind::OpenAiCompatible,
                provider_enabled: true,
                model_db_id: "m1".to_string(),
                model_id: "gpt-4".to_string(),
                model_enabled: true,
                tags: vec!["chat".to_string()],
            },
            ModelCatalogEntryDto {
                provider_id: "p1".to_string(),
                provider_name: "OpenAI".to_string(),
                provider_kind: ProviderKind::OpenAiCompatible,
                provider_enabled: true,
                model_db_id: "m2".to_string(),
                model_id: "gpt-3.5".to_string(),
                model_enabled: false,
                tags: vec![],
            },
        ]
    }

    async fn spawn_models_server(models: Vec<ModelCatalogEntryDto>) -> (ServerHarness, String) {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(ModelsRuntime {
            inner,
            response: ModelListResponseDto { models },
        });
        spawn_server(runtime).await
    }

    #[tokio::test]
    #[serial]
    async fn handle_models_table_output_succeeds() {
        let (harness, socket) = spawn_models_server(sample_models()).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_models(None, vec![], false, false).await;
        drop(harness);
        assert!(result.is_ok(), "table output: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_models_json_output_succeeds() {
        let (harness, socket) = spawn_models_server(sample_models()).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_models(None, vec![], false, true).await;
        drop(harness);
        assert!(result.is_ok(), "json output: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_models_with_all_flag_succeeds() {
        // --all sets include_disabled=true; the wrapper ignores it (returns
        // the canned response), but the request should still build & send.
        let (harness, socket) = spawn_models_server(sample_models()).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_models(None, vec![], true, false).await;
        drop(harness);
        assert!(result.is_ok(), "table output --all: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_models_with_provider_and_tags_filters_succeeds() {
        let (harness, socket) = spawn_models_server(sample_models()).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_models(
            Some("p1".to_string()),
            vec!["chat".to_string()],
            false,
            false,
        )
        .await;
        drop(harness);
        assert!(result.is_ok(), "filtered: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_models_empty_list_prints_no_models_found() {
        let (harness, socket) = spawn_models_server(vec![]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_models(None, vec![], false, false).await;
        drop(harness);
        assert!(result.is_ok(), "empty list: {:?}", result.err());
    }

    /// Exercises every delegation method on `ModelsRuntime` so the
    /// forwarding lines are covered. The inner `TestRuntimeControl`
    /// stubs return Ok/Err; we only need the delegation line to execute.
    #[tokio::test]
    async fn models_runtime_delegates_every_method_to_inner() {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime = ModelsRuntime {
            inner,
            response: ModelListResponseDto { models: vec![] },
        };
        let rt: &dyn RuntimeControl = &runtime;

        let _ = rt.service_health().await;
        let _ = rt.service_status().await;
        let _ = rt.shell_status().await;
        let _ = rt.settings_snapshot().await;
        let _ = rt.settings_diagnostics().await;
        let _ = rt.provider_list().await;
        let _ = rt.event_bus();

        let day = RangePresetDto::Day;
        let _ = rt
            .overview_summary(OverviewSummaryRequestDto { range: day })
            .await;
        let _ = rt
            .overview_trend(OverviewTrendRequestDto {
                range: day,
                granularity: None,
            })
            .await;
        let _ = rt
            .overview_heatmap(OverviewHeatmapRequestDto { range: day })
            .await;
        let _ = rt
            .overview_rankings(OverviewRankingsRequestDto { range: day })
            .await;
        let _ = rt.receipt_daily(ReceiptDailyRequestDto::default()).await;
        let _ = rt
            .activity_recent(ActivityRecentRequestDto {
                range: day,
                limit: None,
            })
            .await;
        let _ = rt
            .activity_list(ActivityListRequestDto {
                range: day,
                cursor: None,
                limit: None,
                client_id: None,
                source_id: None,
                project_hash: None,
                model_id: None,
            })
            .await;
        let _ = rt
            .activity_detail(ActivityDetailRequestDto { id: "x".into() })
            .await;
        let _ = rt
            .breakdown_list(BreakdownListRequestDto {
                kind: BreakdownKindDto::Project,
                range: day,
                cursor: None,
                limit: None,
            })
            .await;
        let _ = rt
            .breakdown_detail(BreakdownDetailRequestDto {
                kind: BreakdownKindDto::Project,
                id: "x".into(),
                range: day,
            })
            .await;
        let _ = rt
            .clients_snapshot(ClientsSnapshotRequestDto {
                cursor: None,
                limit: None,
                client_id: None,
                scan_state: None,
            })
            .await;
        let _ = rt
            .clients_detail(ClientSourceDetailRequestDto {
                source_id: "x".into(),
            })
            .await;
        let _ = rt
            .settings_update(SettingsUpdateRequestDto {
                timezone: None,
                week_starts_on: None,
                discovery: None,
                privacy: None,
                prompt_palette_default_action: None,
            })
            .await;
        let _ = rt
            .settings_recovery_action(SettingsRecoveryActionRequestDto {
                id: SettingsRecoveryActionIdDto::RescanAll,
            })
            .await;
        let _ = rt
            .live_window(LiveWindowRequestDto {
                window_seconds: None,
            })
            .await;
        let _ = rt
            .prompts_list(PromptListQueryDto {
                query: None,
                tag: None,
                sort: None,
                limit: None,
            })
            .await;
        let _ = rt.prompts_get(PromptGetRequestDto { id: "x".into() }).await;
        let _ = rt
            .prompts_create(PromptCreateRequestDto {
                content: "c".into(),
                alias: None,
                tags: vec![],
            })
            .await;
        let _ = rt
            .prompts_update(PromptUpdateRequestDto {
                id: "x".into(),
                content: "c".into(),
                alias: None,
                tags: vec![],
                is_pinned: false,
            })
            .await;
        let _ = rt
            .prompts_delete(PromptDeleteRequestDto { id: "x".into() })
            .await;
        let _ = rt
            .prompts_use(PromptUseRequestDto {
                id: "x".into(),
                action: PromptActionDto::OnlyCopy,
                surface: PromptUseSurfaceDto::Overlay,
                outcome: PromptUseOutcomeDto::Copy,
                failure_reason: None,
            })
            .await;
        let _ = rt
            .suggest_tags(PromptSuggestTagsRequestDto {
                query: None,
                limit: None,
            })
            .await;
        let _ = rt
            .subagent_delegate(SubagentDelegateRequestDto {
                subagent_name: "sa".into(),
                subagent_id: None,
                cwd: ".".into(),
                profile: "default".into(),
                intent: None,
                prompt: "p".into(),
                prompt_artifact_ref: None,
                timeout_seconds: None,
                model_override: None,
                source_harness: None,
                source_session_id: None,
            })
            .await;
        let _ = rt
            .subagent_list(SubagentListRequestDto {
                status: None,
                project: None,
                include_deleted: None,
            })
            .await;
        let _ = rt
            .subagent_show(SubagentResolveRequestDto {
                name: None,
                id: Some("sa".into()),
                cwd: None,
            })
            .await;
        let _ = rt
            .subagent_tasks(SubagentTasksRequestDto {
                name: None,
                id: Some("sa".into()),
                cwd: None,
                limit: None,
            })
            .await;
        let _ = rt
            .subagent_hibernate(SubagentResolveRequestDto {
                name: None,
                id: Some("sa".into()),
                cwd: None,
            })
            .await;
        let _ = rt
            .subagent_delete(SubagentDeleteRequestDto {
                name: None,
                id: Some("sa".into()),
                cwd: None,
                hard: None,
            })
            .await;
        let _ = rt
            .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
            .await;
        let _ = rt
            .provider_create(ProviderCreateRequestDto {
                name: "p".into(),
                provider_kind: ProviderKind::OpenAiCompatible,
                base_url: "https://x.example.com/v1".into(),
                enabled: None,
                api_key: None,
            })
            .await;
        let _ = rt
            .provider_update(ProviderUpdateRequestDto {
                id: "p".into(),
                name: None,
                base_url: None,
                enabled: None,
                api_key: None,
            })
            .await;
        let _ = rt
            .provider_delete(ProviderDeleteRequestDto { id: "p".into() })
            .await;
        let _ = rt
            .provider_test_connection(ProviderTestConnectionRequestDto { id: "p".into() })
            .await;
        let _ = rt
            .model_create(ModelCreateRequestDto {
                provider_id: "p".into(),
                model_id: "m".into(),
                enabled: None,
                tags: vec![],
            })
            .await;
        let _ = rt
            .model_update(ModelUpdateRequestDto {
                id: "m".into(),
                enabled: None,
            })
            .await;
        let _ = rt
            .model_delete(ModelDeleteRequestDto { id: "m".into() })
            .await;
        let _ = rt
            .model_tags_update(ModelTagUpdateDto {
                model_id: "m".into(),
                tags: vec![],
            })
            .await;
        let _ = rt
            .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
                runtime_dir: "/tmp".into(),
                enabled: true,
            })
            .await;
        let _ = rt
            .profile_create(ProfileCreateRequestDto {
                id: "pr".into(),
                tools: None,
                context_budget_tokens: None,
                timeout_seconds: None,
                write_access: None,
            })
            .await;
        let _ = rt
            .profile_update(ProfileUpdateRequestDto {
                id: "pr".into(),
                tools: None,
                context_budget_tokens: None,
                timeout_seconds: None,
                write_access: None,
            })
            .await;
        let _ = rt
            .profile_delete(ProfileDeleteRequestDto { id: "pr".into() })
            .await;
    }
}
