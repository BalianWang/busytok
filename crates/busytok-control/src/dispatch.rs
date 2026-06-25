//! Control dispatch — routes control requests to runtime-backed handlers.
//!
//! The `RuntimeControl` trait defines the interface the dispatcher needs from
//! the runtime. `ControlDispatcher` maps method names to typed handlers.
//! `TestRuntimeControl` is a fake implementation for testing that does NOT
//! depend on `busytok-runtime`.

use std::sync::Arc;

use anyhow::Result;
use busytok_events::AppEventBus;
use busytok_protocol::dto::*;
use busytok_protocol::ControlResponse;

#[derive(Debug)]
pub struct MethodDispatchError {
    pub code: String,
    pub message: String,
    pub payload: Option<serde_json::Value>,
}

impl MethodDispatchError {
    pub fn from_read_error(code: &str, message: String, payload: serde_json::Value) -> Self {
        Self {
            code: code.to_string(),
            message,
            payload: Some(payload),
        }
    }
}

impl std::fmt::Display for MethodDispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for MethodDispatchError {}

pub fn control_response_from_error(error: anyhow::Error) -> ControlResponse {
    if let Some(dispatch_error) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<MethodDispatchError>())
    {
        if let Some(payload) = dispatch_error.payload.clone() {
            return ControlResponse::err_with_payload(
                &dispatch_error.code,
                &dispatch_error.message,
                payload,
            );
        }
        return ControlResponse::err(&dispatch_error.code, &dispatch_error.message);
    }

    ControlResponse::err("internal_error", &error.to_string())
}

/// Trait wrapping the runtime queries/mutations needed by Surge UI control methods.
///
/// Runtime-backed dispatch integration is added after the service crate exists.
/// Until then, `TestRuntimeControl` satisfies this trait for testing.
#[async_trait::async_trait]
pub trait RuntimeControl: Send + Sync {
    // Service (kept from Phase 1)
    async fn service_health(&self) -> Result<ServiceHealthDto>;
    async fn service_status(&self) -> Result<ServiceStatusDto>;

    // Shell
    async fn shell_status(&self) -> Result<ShellStatusDto>;

    // Overview — modular (Task 8)
    async fn overview_summary(
        &self,
        req: OverviewSummaryRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewSummaryDto>>;
    async fn overview_trend(
        &self,
        req: OverviewTrendRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewTrendResponseDto>>;
    async fn overview_heatmap(
        &self,
        req: OverviewHeatmapRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewHeatmapResponseDto>>;
    async fn overview_rankings(
        &self,
        req: OverviewRankingsRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewRankingsResponseDto>>;

    // Activity
    async fn activity_recent(
        &self,
        req: ActivityRecentRequestDto,
    ) -> Result<ReadEnvelopeDto<ActivityRecentResponseDto>>;
    async fn activity_list(
        &self,
        req: ActivityListRequestDto,
    ) -> Result<ReadEnvelopeDto<ActivityListResponseDto>>;
    async fn activity_detail(
        &self,
        req: ActivityDetailRequestDto,
    ) -> Result<ReadEnvelopeDto<ActivityDetailDto>>;

    // Breakdown
    async fn breakdown_list(
        &self,
        req: BreakdownListRequestDto,
    ) -> Result<ReadEnvelopeDto<BreakdownListResponseDto>>;
    async fn breakdown_detail(
        &self,
        req: BreakdownDetailRequestDto,
    ) -> Result<ReadEnvelopeDto<BreakdownDetailDto>>;

    // Clients
    async fn clients_snapshot(
        &self,
        req: ClientsSnapshotRequestDto,
    ) -> Result<ReadEnvelopeDto<ClientsSnapshotDto>>;
    async fn clients_detail(
        &self,
        req: ClientSourceDetailRequestDto,
    ) -> Result<ReadEnvelopeDto<ClientSourceDetailDto>>;

    // Settings
    async fn settings_snapshot(&self) -> Result<ReadEnvelopeDto<SettingsSnapshotDto>>;
    async fn settings_update(
        &self,
        req: SettingsUpdateRequestDto,
    ) -> Result<ReadEnvelopeDto<SettingsSnapshotDto>>;
    async fn settings_diagnostics(&self) -> Result<ReadEnvelopeDto<SettingsDiagnosticsDto>>;
    async fn settings_recovery_action(
        &self,
        req: SettingsRecoveryActionRequestDto,
    ) -> Result<ReadEnvelopeDto<SettingsRecoveryActionResponseDto>>;

    // Live
    async fn live_window(
        &self,
        req: LiveWindowRequestDto,
    ) -> Result<ReadEnvelopeDto<LiveWindowDto>>;

    // Prompts
    async fn prompts_list(
        &self,
        req: PromptListQueryDto,
    ) -> Result<ReadEnvelopeDto<PromptListResponseDto>>;
    async fn prompts_get(
        &self,
        req: PromptGetRequestDto,
    ) -> Result<ReadEnvelopeDto<PromptEntryDto>>;
    async fn prompts_create(
        &self,
        req: PromptCreateRequestDto,
    ) -> Result<ReadEnvelopeDto<PromptEntryDto>>;
    async fn prompts_update(
        &self,
        req: PromptUpdateRequestDto,
    ) -> Result<ReadEnvelopeDto<PromptEntryDto>>;
    async fn prompts_delete(&self, req: PromptDeleteRequestDto) -> Result<PromptDeleteResultDto>;
    async fn prompts_use(&self, req: PromptUseRequestDto) -> Result<PromptUseResultDto>;
    async fn suggest_tags(
        &self,
        req: PromptSuggestTagsRequestDto,
    ) -> Result<PromptSuggestTagsResponseDto>;

    // Subagents
    async fn subagent_delegate(
        &self,
        req: busytok_protocol::dto::SubagentDelegateRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentDelegateResponseDto>;
    async fn subagent_list(
        &self,
        req: busytok_protocol::dto::SubagentListRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentListResponseDto>;
    async fn subagent_show(
        &self,
        req: busytok_protocol::dto::SubagentResolveRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentDetailDto>;
    async fn subagent_tasks(
        &self,
        req: busytok_protocol::dto::SubagentTasksRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentTasksResponseDto>;
    async fn subagent_hibernate(
        &self,
        req: busytok_protocol::dto::SubagentResolveRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentAckDto>;
    async fn subagent_delete(
        &self,
        req: busytok_protocol::dto::SubagentDeleteRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentAckDto>;

    // Events (kept from Phase 1)
    fn event_bus(&self) -> &AppEventBus;

    /// Latest allocated event sequence number; used for gap detection.
    fn latest_event_seq(&self) -> Option<i64> {
        None
    }

    /// Record a diagnostic event through the writer actor so it survives
    /// restarts and appears in `settings.diagnostics`.
    ///
    /// Default does nothing (sync/test contexts). Production supervisors
    /// override this to enqueue a `DiagnosticWrite` command.
    fn record_diagnostic(&self, _severity: &str, _code: &str, _message: &str) {}

    /// Hook for tests to observe the `RequestMeta` that arrived on a
    /// `ControlRequest`. Production code ignores this.
    fn on_request_meta(&self, _meta: &RequestMeta) {}
}

/// Dispatches `ControlRequest` to the appropriate runtime handler.
///
/// Uses `Arc<dyn RuntimeControl>` internally so the dispatcher can be cheaply
/// cloned for per-connection server tasks.
pub struct ControlDispatcher {
    runtime: Arc<dyn RuntimeControl>,
}

impl ControlDispatcher {
    pub fn new(runtime: impl RuntimeControl + 'static) -> Self {
        Self {
            runtime: Arc::new(runtime),
        }
    }

    /// Create a dispatcher from an already-arc'd runtime.
    pub fn from_arc(runtime: Arc<dyn RuntimeControl>) -> Self {
        Self { runtime }
    }

    /// Clone the internal arc reference for creating per-connection dispatchers.
    pub fn runtime_arc(&self) -> Arc<dyn RuntimeControl> {
        Arc::clone(&self.runtime)
    }

    /// Access the runtime's event bus for subscription handling.
    pub fn event_bus(&self) -> &AppEventBus {
        self.runtime.event_bus()
    }

    /// Latest allocated event sequence number; used for gap detection.
    pub fn latest_event_seq(&self) -> Option<i64> {
        self.runtime.latest_event_seq()
    }

    /// Record a durable diagnostic event through the writer actor so it
    /// survives restarts and appears in `settings.diagnostics`.
    pub fn record_diagnostic(&self, severity: &str, code: &str, message: &str) {
        self.runtime.record_diagnostic(severity, code, message);
    }

    /// Dispatch a control request to the appropriate handler.
    pub async fn dispatch(&self, request: ControlRequest) -> Result<ControlResponse> {
        self.runtime.on_request_meta(&request.meta);
        let response = match request.method.as_str() {
            // Service (kept from Phase 1)
            "service.health" => {
                let dto = self.runtime.service_health().await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "service.status" => {
                let dto = self.runtime.service_status().await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }

            // Shell
            "shell.status" => {
                let dto = self.runtime.shell_status().await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }

            // Overview — modular
            "overview.summary" => {
                let req: OverviewSummaryRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for overview.summary: {e}"))?;
                let dto = self.runtime.overview_summary(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "overview.trend" => {
                let req: OverviewTrendRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for overview.trend: {e}"))?;
                let dto = self.runtime.overview_trend(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "overview.heatmap" => {
                let req: OverviewHeatmapRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for overview.heatmap: {e}"))?;
                let dto = self.runtime.overview_heatmap(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "overview.rankings" => {
                let req: OverviewRankingsRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for overview.rankings: {e}"))?;
                let dto = self.runtime.overview_rankings(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }

            // Activity
            "activity.recent" => {
                let req: ActivityRecentRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for activity.recent: {e}"))?;
                let dto = self.runtime.activity_recent(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "activity.list" => {
                let req: ActivityListRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for activity.list: {e}"))?;
                let dto = self.runtime.activity_list(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "activity.detail" => {
                let req: ActivityDetailRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for activity.detail: {e}"))?;
                let dto = self.runtime.activity_detail(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }

            // Breakdown
            "breakdown.list" => {
                let req: BreakdownListRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for breakdown.list: {e}"))?;
                let dto = self.runtime.breakdown_list(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "breakdown.detail" => {
                let req: BreakdownDetailRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for breakdown.detail: {e}"))?;
                let dto = self.runtime.breakdown_detail(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }

            // Clients
            "clients.snapshot" => {
                let req: ClientsSnapshotRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for clients.snapshot: {e}"))?;
                let dto = self.runtime.clients_snapshot(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "clients.detail" => {
                let req: ClientSourceDetailRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for clients.detail: {e}"))?;
                let dto = self.runtime.clients_detail(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }

            // Settings
            "settings.snapshot" => {
                let dto = self.runtime.settings_snapshot().await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "settings.update" => {
                let req: SettingsUpdateRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for settings.update: {e}"))?;
                match self.runtime.settings_update(req).await {
                    Ok(dto) => ControlResponse::ok(serde_json::to_value(dto)?),
                    Err(e) => {
                        let msg = format!("{e}");
                        if msg.starts_with("SETTINGS_VALIDATION_FAILED:") {
                            let payload_str =
                                msg.trim_start_matches("SETTINGS_VALIDATION_FAILED: ");
                            if let Ok(payload) =
                                serde_json::from_str::<serde_json::Value>(payload_str)
                            {
                                ControlResponse::err_with_payload(
                                    "settings_validation_failed",
                                    "Settings validation failed",
                                    payload,
                                )
                            } else {
                                return Err(e);
                            }
                        } else {
                            return Err(e);
                        }
                    }
                }
            }
            "settings.diagnostics" => {
                let dto = self.runtime.settings_diagnostics().await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "settings.recovery_action" => {
                let req: SettingsRecoveryActionRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| {
                        anyhow::anyhow!("invalid params for settings.recovery_action: {e}")
                    })?;
                let dto = self.runtime.settings_recovery_action(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }

            // Live
            "live.window" => {
                let req: LiveWindowRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for live.window: {e}"))?;
                let dto = self.runtime.live_window(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }

            // Prompts
            "prompts.list" => {
                let req: PromptListQueryDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for prompts.list: {e}"))?;
                let dto = self.runtime.prompts_list(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "prompts.get" => {
                let req: PromptGetRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for prompts.get: {e}"))?;
                let dto = self.runtime.prompts_get(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "prompts.create" => {
                let req: PromptCreateRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for prompts.create: {e}"))?;
                let dto = self.runtime.prompts_create(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "prompts.update" => {
                let req: PromptUpdateRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for prompts.update: {e}"))?;
                let dto = self.runtime.prompts_update(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "prompts.delete" => {
                let req: PromptDeleteRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for prompts.delete: {e}"))?;
                let dto = self.runtime.prompts_delete(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "prompts.use" => {
                let req: PromptUseRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for prompts.use: {e}"))?;
                let dto = self.runtime.prompts_use(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "prompts.suggest_tags" => {
                let req: PromptSuggestTagsRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for prompts.suggest_tags: {e}"))?;
                let dto = self.runtime.suggest_tags(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }

            // Subagents
            "subagent.delegate" => {
                let req: SubagentDelegateRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for subagent.delegate: {e}"))?;
                let dto = self.runtime.subagent_delegate(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "subagent.list" => {
                let req: SubagentListRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for subagent.list: {e}"))?;
                let dto = self.runtime.subagent_list(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "subagent.show" => {
                let req: SubagentResolveRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for subagent.show: {e}"))?;
                let dto = self.runtime.subagent_show(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "subagent.tasks" => {
                let req: SubagentTasksRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for subagent.tasks: {e}"))?;
                let dto = self.runtime.subagent_tasks(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "subagent.hibernate" => {
                let req: SubagentResolveRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for subagent.hibernate: {e}"))?;
                let dto = self.runtime.subagent_hibernate(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }
            "subagent.delete" => {
                let req: SubagentDeleteRequestDto = serde_json::from_value(request.params)
                    .map_err(|e| anyhow::anyhow!("invalid params for subagent.delete: {e}"))?;
                let dto = self.runtime.subagent_delete(req).await?;
                ControlResponse::ok(serde_json::to_value(dto)?)
            }

            _ => {
                return Ok(ControlResponse::err(
                    "method_not_found",
                    &format!("unknown method: {}", request.method),
                ));
            }
        };
        Ok(response)
    }
}

// ---------------------------------------------------------------------------
// TestRuntimeControl — a fake implementation for testing
// ---------------------------------------------------------------------------

/// Minimal `ReadEnvelopeDto` helper for test mock responses.
fn stub_envelope<T>(data: T) -> ReadEnvelopeDto<T> {
    ReadEnvelopeDto {
        data,
        generated_at_ms: 0,
        generation_id: None,
        readiness: ReadinessStateDto::Starting,
        is_exact: false,
        is_stale: true,
        watermark_ms: None,
        progress: None,
        degraded_reason: None,
    }
}

fn test_prompt_entry() -> PromptEntryDto {
    PromptEntryDto {
        id: "prompt-1".to_string(),
        content: "Review this diff for bugs.".to_string(),
        alias: Some("review".to_string()),
        tags: vec!["review".to_string()],
        is_pinned: true,
        usage_count: 0,
        last_used_at_ms: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

/// Fake runtime control for testing. Does NOT depend on `busytok-runtime`.
///
/// Provides small but structurally complete fake responses for every Surge UI method.
pub struct TestRuntimeControl {
    event_bus: AppEventBus,
    pub last_meta: std::sync::Mutex<Option<RequestMeta>>,
}

impl TestRuntimeControl {
    pub async fn with_claude_fixture() -> Result<Self> {
        Ok(Self {
            event_bus: AppEventBus::new(64),
            last_meta: std::sync::Mutex::new(None),
        })
    }
}

#[async_trait::async_trait]
impl RuntimeControl for TestRuntimeControl {
    fn on_request_meta(&self, meta: &RequestMeta) {
        *self.last_meta.lock().unwrap() = Some(meta.clone());
    }

    async fn service_health(&self) -> Result<ServiceHealthDto> {
        Ok(ServiceHealthDto {
            ready: true,
            db_healthy: true,
            scan_state: "idle".to_string(),
        })
    }

    async fn service_status(&self) -> Result<ServiceStatusDto> {
        Ok(ServiceStatusDto {
            version: env!("CARGO_PKG_VERSION").to_string(),
            db_path: ":memory:".to_string(),
            state: "running".to_string(),
        })
    }

    async fn shell_status(&self) -> Result<ShellStatusDto> {
        Ok(ShellStatusDto {
            generated_at_ms: 0,
            status_chips: vec![],
            readiness: ReadinessStateDto::Starting,
            latest_event_seq: None,
            writer_queue_depth: None,
            aggregate_lag_ms: None,
            subscription_bridge_connectivity: None,
        })
    }

    // ── Overview — modular stubs ──────────────────────────────────────

    async fn overview_summary(
        &self,
        req: OverviewSummaryRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewSummaryDto>> {
        Ok(ReadEnvelopeDto {
            data: OverviewSummaryDto {
                timezone: "UTC".to_string(),
                selected_range: req.range,
                cost_status: CostStatusDto::Unavailable,
                metrics: vec![],
                generated_at_ms: 0,
            },
            generated_at_ms: 0,
            generation_id: None,
            readiness: ReadinessStateDto::Starting,
            is_exact: false,
            is_stale: true,
            watermark_ms: None,
            progress: None,
            degraded_reason: None,
        })
    }

    async fn overview_trend(
        &self,
        req: OverviewTrendRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewTrendResponseDto>> {
        Ok(ReadEnvelopeDto {
            data: OverviewTrendResponseDto {
                trend: OverviewTrendDto {
                    range: req.range,
                    bucket_granularity: req.granularity.unwrap_or(TrendBucketGranularityDto::Hour),
                    metric_options: vec![MetricOptionDto::Tokens, MetricOptionDto::Cost],
                    cost_status: CostStatusDto::Unavailable,
                    buckets: vec![],
                },
            },
            generated_at_ms: 0,
            generation_id: None,
            readiness: ReadinessStateDto::Starting,
            is_exact: false,
            is_stale: true,
            watermark_ms: None,
            progress: None,
            degraded_reason: None,
        })
    }

    async fn overview_heatmap(
        &self,
        _req: OverviewHeatmapRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewHeatmapResponseDto>> {
        Ok(ReadEnvelopeDto {
            data: OverviewHeatmapResponseDto {
                heatmap: OverviewHeatmapDto {
                    today: "1970-01-01".to_string(),
                    week_starts_on: WeekdayIndexDto::MONDAY,
                    days: vec![],
                },
            },
            generated_at_ms: 0,
            generation_id: None,
            readiness: ReadinessStateDto::Starting,
            is_exact: false,
            is_stale: true,
            watermark_ms: None,
            progress: None,
            degraded_reason: None,
        })
    }

    async fn overview_rankings(
        &self,
        _req: OverviewRankingsRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewRankingsResponseDto>> {
        Ok(ReadEnvelopeDto {
            data: OverviewRankingsResponseDto { rankings: vec![] },
            generated_at_ms: 0,
            generation_id: None,
            readiness: ReadinessStateDto::Starting,
            is_exact: false,
            is_stale: true,
            watermark_ms: None,
            progress: None,
            degraded_reason: None,
        })
    }

    // ── Activity — modular stubs ───────────────────────────────────────

    async fn activity_recent(
        &self,
        _req: ActivityRecentRequestDto,
    ) -> Result<ReadEnvelopeDto<ActivityRecentResponseDto>> {
        Ok(ReadEnvelopeDto {
            data: ActivityRecentResponseDto {
                recent_activity: vec![],
            },
            generated_at_ms: 0,
            generation_id: None,
            readiness: ReadinessStateDto::Starting,
            is_exact: false,
            is_stale: true,
            watermark_ms: None,
            progress: None,
            degraded_reason: None,
        })
    }

    async fn activity_list(
        &self,
        _req: ActivityListRequestDto,
    ) -> Result<ReadEnvelopeDto<ActivityListResponseDto>> {
        Ok(stub_envelope(ActivityListResponseDto {
            generated_at_ms: 0,
            items: vec![],
            next_cursor: None,
            summary: ActivityListSummaryDto {
                item_count: 0,
                total_tokens: 0,
                total_cost_usd: None,
                cost_status: CostStatusDto::Unavailable,
            },
        }))
    }

    async fn activity_detail(
        &self,
        _req: ActivityDetailRequestDto,
    ) -> Result<ReadEnvelopeDto<ActivityDetailDto>> {
        Ok(stub_envelope(ActivityDetailDto {
            id: "evt-1".to_string(),
            title: "Test Event".to_string(),
            subtitle: None,
            happened_at_ms: 0,
            client_id: "claude_code".to_string(),
            client_label: "Claude Code".to_string(),
            source_id: None,
            source_label: None,
            source_root_path: None,
            project_label: None,
            project_hash: None,
            session_id: None,
            model_id: None,
            model_label: None,
            status: ActivityStatusDto::Ok,
            tokens: 0,
            token_breakdown: None,
            cost_usd: None,
            cost_status: CostStatusDto::Unavailable,
            technical_details: ActivityTechnicalDetailsDto {
                source_id: None,
                provider: None,
                raw_model: None,
                notes: vec![],
            },
        }))
    }

    async fn breakdown_list(
        &self,
        _req: BreakdownListRequestDto,
    ) -> Result<ReadEnvelopeDto<BreakdownListResponseDto>> {
        Ok(stub_envelope(BreakdownListResponseDto {
            generated_at_ms: 0,
            kind: BreakdownKindDto::Project,
            items: vec![],
            next_cursor: None,
            summary: BreakdownListResponseSummaryDto {
                item_count: 0,
                total_tokens: 0,
                total_cost_usd: None,
                total_cost_status: CostStatusDto::Unavailable,
            },
        }))
    }

    async fn breakdown_detail(
        &self,
        _req: BreakdownDetailRequestDto,
    ) -> Result<ReadEnvelopeDto<BreakdownDetailDto>> {
        Ok(stub_envelope(BreakdownDetailDto::Project(
            ProjectBreakdownDetailDto {
                id: "proj-1".to_string(),
                label: "Test Project".to_string(),
                project_hash: "proj-1".to_string(),
                project_path: None,
                metrics: vec![],
                trend: OverviewTrendDto {
                    range: RangePresetDto::Day,
                    bucket_granularity: TrendBucketGranularityDto::Hour,
                    metric_options: vec![],
                    cost_status: CostStatusDto::Unavailable,
                    buckets: vec![],
                },
                model_mix: vec![],
                sessions: vec![],
                recent_activity: vec![],
                technical_details: vec![],
            },
        )))
    }

    async fn clients_snapshot(
        &self,
        _req: ClientsSnapshotRequestDto,
    ) -> Result<ReadEnvelopeDto<ClientsSnapshotDto>> {
        Ok(stub_envelope(ClientsSnapshotDto {
            generated_at_ms: 0,
            client_cards: vec![],
            sources: vec![],
            next_cursor: None,
            summary: ClientsSnapshotSummaryDto {
                source_count: 0,
                active_source_count: 0,
            },
        }))
    }

    async fn clients_detail(
        &self,
        _req: ClientSourceDetailRequestDto,
    ) -> Result<ReadEnvelopeDto<ClientSourceDetailDto>> {
        Ok(stub_envelope(ClientSourceDetailDto {
            source: ClientSourceRowDto {
                id: "src-1".to_string(),
                client_id: "claude_code".to_string(),
                client_label: "Claude Code".to_string(),
                root_path: "/tmp".to_string(),
                source_type: SourceTypeDto::DefaultDiscovery,
                scan_state: SourceScanStateDto::Idle,
                configured_by_user: false,
                last_scan_at_ms: None,
                file_count: 0,
                parsed_file_count: 0,
                event_count: 0,
                last_error: None,
            },
            recent_activity: vec![],
            technical_details: vec![],
        }))
    }

    async fn settings_snapshot(&self) -> Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        Ok(stub_envelope(SettingsSnapshotDto {
            timezone: busytok_domain::detect_system_iana_timezone(),
            week_starts_on: WeekdayIndexDto::MONDAY,
            discovery: SettingsDiscoveryDto {
                claude_code_default_paths: true,
                codex_default_paths: true,
                manual_roots: vec![],
            },
            privacy: SettingsPrivacyDto {
                local_only: true,
                redact_sensitive_values: true,
            },
            prompt_palette_default_action: PromptActionDto::CopyAndPaste,
            diagnostics: SettingsDiagnosticsDto {
                db_healthy: true,
                db_size_bytes: 0,
                migration_version: 0,
                usage_event_count: 0,
                last_log_checkpoint_ms: None,
                writer_queue_depth: 0,
                aggregate_lag_ms: 0,
                recent_diagnostics: vec![],
            },
            recovery_actions: vec![],
        }))
    }

    async fn settings_update(
        &self,
        req: SettingsUpdateRequestDto,
    ) -> Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        Ok(stub_envelope(SettingsSnapshotDto {
            timezone: req
                .timezone
                .unwrap_or_else(busytok_domain::detect_system_iana_timezone),
            week_starts_on: req.week_starts_on.unwrap_or(WeekdayIndexDto::MONDAY),
            discovery: req.discovery.unwrap_or(SettingsDiscoveryDto {
                claude_code_default_paths: true,
                codex_default_paths: true,
                manual_roots: vec![],
            }),
            privacy: req.privacy.unwrap_or(SettingsPrivacyDto {
                local_only: true,
                redact_sensitive_values: true,
            }),
            prompt_palette_default_action: req
                .prompt_palette_default_action
                .unwrap_or(PromptActionDto::CopyAndPaste),
            diagnostics: SettingsDiagnosticsDto {
                db_healthy: true,
                db_size_bytes: 0,
                migration_version: 0,
                usage_event_count: 0,
                last_log_checkpoint_ms: None,
                writer_queue_depth: 0,
                aggregate_lag_ms: 0,
                recent_diagnostics: vec![],
            },
            recovery_actions: vec![],
        }))
    }

    async fn settings_diagnostics(&self) -> Result<ReadEnvelopeDto<SettingsDiagnosticsDto>> {
        Ok(stub_envelope(SettingsDiagnosticsDto {
            db_healthy: true,
            db_size_bytes: 0,
            migration_version: 0,
            usage_event_count: 0,
            last_log_checkpoint_ms: None,
            writer_queue_depth: 0,
            aggregate_lag_ms: 0,
            recent_diagnostics: vec![],
        }))
    }

    async fn settings_recovery_action(
        &self,
        _req: SettingsRecoveryActionRequestDto,
    ) -> Result<ReadEnvelopeDto<SettingsRecoveryActionResponseDto>> {
        Ok(stub_envelope(SettingsRecoveryActionResponseDto {
            id: SettingsRecoveryActionIdDto::RescanAll,
            accepted: true,
            message: "Recovery action accepted".to_string(),
        }))
    }

    async fn live_window(
        &self,
        _req: LiveWindowRequestDto,
    ) -> Result<ReadEnvelopeDto<LiveWindowDto>> {
        Ok(ReadEnvelopeDto {
            data: LiveWindowDto {
                exact_samples: vec![],
                transient_samples: vec![],
                current_tokens_per_sec: 0.0,
                current_events_per_sec: 0.0,
                start_ms: 0,
                end_ms: 0,
            },
            generated_at_ms: 0,
            generation_id: None,
            readiness: ReadinessStateDto::Starting,
            is_exact: false,
            is_stale: true,
            watermark_ms: None,
            progress: None,
            degraded_reason: None,
        })
    }

    async fn prompts_list(
        &self,
        _req: PromptListQueryDto,
    ) -> Result<ReadEnvelopeDto<PromptListResponseDto>> {
        Ok(stub_envelope(PromptListResponseDto {
            entries: vec![test_prompt_entry()],
            total_count: 1,
        }))
    }

    async fn prompts_get(
        &self,
        req: PromptGetRequestDto,
    ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
        let mut entry = test_prompt_entry();
        entry.id = req.id;
        Ok(stub_envelope(entry))
    }

    async fn prompts_create(
        &self,
        req: PromptCreateRequestDto,
    ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
        Ok(stub_envelope(PromptEntryDto {
            id: "prompt-created".to_string(),
            content: req.content,
            alias: req.alias,
            tags: req.tags,
            is_pinned: false,
            usage_count: 0,
            last_used_at_ms: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }))
    }

    async fn prompts_update(
        &self,
        req: PromptUpdateRequestDto,
    ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
        Ok(stub_envelope(PromptEntryDto {
            id: req.id,
            content: req.content,
            alias: req.alias,
            tags: req.tags,
            is_pinned: req.is_pinned,
            usage_count: 0,
            last_used_at_ms: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }))
    }

    async fn prompts_delete(&self, _req: PromptDeleteRequestDto) -> Result<PromptDeleteResultDto> {
        Ok(PromptDeleteResultDto { deleted: true })
    }

    async fn prompts_use(&self, _req: PromptUseRequestDto) -> Result<PromptUseResultDto> {
        Ok(PromptUseResultDto {
            usage_count: 1,
            last_used_at_ms: Some(1),
        })
    }

    async fn suggest_tags(
        &self,
        _req: PromptSuggestTagsRequestDto,
    ) -> Result<PromptSuggestTagsResponseDto> {
        Ok(PromptSuggestTagsResponseDto { tags: vec![] })
    }

    // ── Subagents ─────────────────────────────────────────────────────

    async fn subagent_delegate(
        &self,
        _req: busytok_protocol::dto::SubagentDelegateRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentDelegateResponseDto> {
        Ok(Default::default())
    }
    async fn subagent_list(
        &self,
        _req: busytok_protocol::dto::SubagentListRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentListResponseDto> {
        Ok(Default::default())
    }
    async fn subagent_show(
        &self,
        _req: busytok_protocol::dto::SubagentResolveRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentDetailDto> {
        Ok(Default::default())
    }
    async fn subagent_tasks(
        &self,
        _req: busytok_protocol::dto::SubagentTasksRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentTasksResponseDto> {
        Ok(Default::default())
    }
    async fn subagent_hibernate(
        &self,
        _req: busytok_protocol::dto::SubagentResolveRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentAckDto> {
        Ok(Default::default())
    }
    async fn subagent_delete(
        &self,
        _req: busytok_protocol::dto::SubagentDeleteRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentAckDto> {
        Ok(Default::default())
    }

    fn event_bus(&self) -> &AppEventBus {
        &self.event_bus
    }
}

/// Blanket impl: `Arc<T>` delegates to `T` for any `RuntimeControl` type.
#[async_trait::async_trait]
impl<T: RuntimeControl> RuntimeControl for Arc<T> {
    async fn service_health(&self) -> Result<ServiceHealthDto> {
        (**self).service_health().await
    }
    async fn service_status(&self) -> Result<ServiceStatusDto> {
        (**self).service_status().await
    }
    async fn shell_status(&self) -> Result<ShellStatusDto> {
        (**self).shell_status().await
    }
    async fn overview_summary(
        &self,
        req: OverviewSummaryRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewSummaryDto>> {
        (**self).overview_summary(req).await
    }
    async fn overview_trend(
        &self,
        req: OverviewTrendRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewTrendResponseDto>> {
        (**self).overview_trend(req).await
    }
    async fn overview_heatmap(
        &self,
        req: OverviewHeatmapRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewHeatmapResponseDto>> {
        (**self).overview_heatmap(req).await
    }
    async fn overview_rankings(
        &self,
        req: OverviewRankingsRequestDto,
    ) -> Result<ReadEnvelopeDto<OverviewRankingsResponseDto>> {
        (**self).overview_rankings(req).await
    }
    async fn activity_recent(
        &self,
        req: ActivityRecentRequestDto,
    ) -> Result<ReadEnvelopeDto<ActivityRecentResponseDto>> {
        (**self).activity_recent(req).await
    }
    async fn activity_list(
        &self,
        req: ActivityListRequestDto,
    ) -> Result<ReadEnvelopeDto<ActivityListResponseDto>> {
        (**self).activity_list(req).await
    }
    async fn activity_detail(
        &self,
        req: ActivityDetailRequestDto,
    ) -> Result<ReadEnvelopeDto<ActivityDetailDto>> {
        (**self).activity_detail(req).await
    }
    async fn breakdown_list(
        &self,
        req: BreakdownListRequestDto,
    ) -> Result<ReadEnvelopeDto<BreakdownListResponseDto>> {
        (**self).breakdown_list(req).await
    }
    async fn breakdown_detail(
        &self,
        req: BreakdownDetailRequestDto,
    ) -> Result<ReadEnvelopeDto<BreakdownDetailDto>> {
        (**self).breakdown_detail(req).await
    }
    async fn clients_snapshot(
        &self,
        req: ClientsSnapshotRequestDto,
    ) -> Result<ReadEnvelopeDto<ClientsSnapshotDto>> {
        (**self).clients_snapshot(req).await
    }
    async fn clients_detail(
        &self,
        req: ClientSourceDetailRequestDto,
    ) -> Result<ReadEnvelopeDto<ClientSourceDetailDto>> {
        (**self).clients_detail(req).await
    }
    async fn settings_snapshot(&self) -> Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        (**self).settings_snapshot().await
    }
    async fn settings_update(
        &self,
        req: SettingsUpdateRequestDto,
    ) -> Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        (**self).settings_update(req).await
    }
    async fn settings_diagnostics(&self) -> Result<ReadEnvelopeDto<SettingsDiagnosticsDto>> {
        (**self).settings_diagnostics().await
    }
    async fn settings_recovery_action(
        &self,
        req: SettingsRecoveryActionRequestDto,
    ) -> Result<ReadEnvelopeDto<SettingsRecoveryActionResponseDto>> {
        (**self).settings_recovery_action(req).await
    }
    async fn live_window(
        &self,
        req: LiveWindowRequestDto,
    ) -> Result<ReadEnvelopeDto<LiveWindowDto>> {
        (**self).live_window(req).await
    }
    async fn prompts_list(
        &self,
        req: PromptListQueryDto,
    ) -> Result<ReadEnvelopeDto<PromptListResponseDto>> {
        (**self).prompts_list(req).await
    }
    async fn prompts_get(
        &self,
        req: PromptGetRequestDto,
    ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
        (**self).prompts_get(req).await
    }
    async fn prompts_create(
        &self,
        req: PromptCreateRequestDto,
    ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
        (**self).prompts_create(req).await
    }
    async fn prompts_update(
        &self,
        req: PromptUpdateRequestDto,
    ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
        (**self).prompts_update(req).await
    }
    async fn prompts_delete(&self, req: PromptDeleteRequestDto) -> Result<PromptDeleteResultDto> {
        (**self).prompts_delete(req).await
    }
    async fn prompts_use(&self, req: PromptUseRequestDto) -> Result<PromptUseResultDto> {
        (**self).prompts_use(req).await
    }
    async fn suggest_tags(
        &self,
        req: PromptSuggestTagsRequestDto,
    ) -> Result<PromptSuggestTagsResponseDto> {
        (**self).suggest_tags(req).await
    }
    async fn subagent_delegate(
        &self,
        req: busytok_protocol::dto::SubagentDelegateRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentDelegateResponseDto> {
        (**self).subagent_delegate(req).await
    }
    async fn subagent_list(
        &self,
        req: busytok_protocol::dto::SubagentListRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentListResponseDto> {
        (**self).subagent_list(req).await
    }
    async fn subagent_show(
        &self,
        req: busytok_protocol::dto::SubagentResolveRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentDetailDto> {
        (**self).subagent_show(req).await
    }
    async fn subagent_tasks(
        &self,
        req: busytok_protocol::dto::SubagentTasksRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentTasksResponseDto> {
        (**self).subagent_tasks(req).await
    }
    async fn subagent_hibernate(
        &self,
        req: busytok_protocol::dto::SubagentResolveRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentAckDto> {
        (**self).subagent_hibernate(req).await
    }
    async fn subagent_delete(
        &self,
        req: busytok_protocol::dto::SubagentDeleteRequestDto,
    ) -> Result<busytok_protocol::dto::SubagentAckDto> {
        (**self).subagent_delete(req).await
    }
    fn event_bus(&self) -> &AppEventBus {
        (**self).event_bus()
    }
    fn on_request_meta(&self, meta: &RequestMeta) {
        (**self).on_request_meta(meta);
    }
}
