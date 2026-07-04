use busytok_domain::ProviderKind;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Deserialize helper for three-state `Option<Option<T>>` fields.
///
/// With plain `#[serde(default)]`, JSON `null` collapses to `None` (indistinguishable
/// from an absent field), which breaks three-state patch semantics. Wrapping the
/// inner deserialize in `Some(...)` preserves the distinction:
///   - field absent  → `None`            (default; means "unchanged")
///   - `null`        → `Some(None)`      (means "clear")
///   - `"value"`     → `Some(Some(v))`   (means "update to v")
fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

// ---------------------------------------------------------------------------
// Request / Response envelope
// ---------------------------------------------------------------------------

/// Observability metadata attached to every control-plane request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct RequestMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ControlRequest {
    pub method: String,
    pub params: serde_json::Value,
    #[serde(default)]
    pub meta: RequestMeta,
}

impl ControlRequest {
    pub fn new(method: &str, params: serde_json::Value) -> Self {
        Self {
            method: method.to_string(),
            params,
            meta: RequestMeta::default(),
        }
    }

    pub fn with_meta(method: &str, params: serde_json::Value, meta: RequestMeta) -> Self {
        Self {
            method: method.to_string(),
            params,
            meta,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub enum ControlResponse {
    Ok(serde_json::Value),
    Err(ControlError),
}

impl ControlResponse {
    pub fn ok(value: serde_json::Value) -> Self {
        ControlResponse::Ok(value)
    }

    pub fn err(code: &str, message: &str) -> Self {
        ControlResponse::Err(ControlError {
            code: code.to_string(),
            message: message.to_string(),
            payload: None,
        })
    }

    pub fn err_with_payload(code: &str, message: &str, payload: serde_json::Value) -> Self {
        ControlResponse::Err(ControlError {
            code: code.to_string(),
            message: message.to_string(),
            payload: Some(payload),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ControlError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

impl ControlError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.to_string(),
            message: message.to_string(),
            payload: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ServiceHealthDto {
    pub ready: bool,
    pub db_healthy: bool,
    pub scan_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ServiceStatusDto {
    pub version: String,
    pub db_path: String,
    pub state: String,
}

// ---------------------------------------------------------------------------
// Shared DTOs
// ---------------------------------------------------------------------------

pub type ClientIdDto = String;
pub type ModelIdDto = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RangePresetDto {
    Day,
    Week,
    Month,
    Year,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ToneDto {
    Neutral,
    Success,
    Warning,
    Danger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatusDto {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CostStatusDto {
    Exact,
    Partial,
    Unavailable,
}

/// Numeric weekday index: 0=Sunday, 1=Monday, ... 6=Saturday.
/// Serializes as a number (0-6), not a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, TS)]
#[ts(type = "0 | 1 | 2 | 3 | 4 | 5 | 6")]
pub struct WeekdayIndexDto(u8);

impl WeekdayIndexDto {
    pub const SUNDAY: Self = WeekdayIndexDto(0);
    pub const MONDAY: Self = WeekdayIndexDto(1);
    pub const TUESDAY: Self = WeekdayIndexDto(2);
    pub const WEDNESDAY: Self = WeekdayIndexDto(3);
    pub const THURSDAY: Self = WeekdayIndexDto(4);
    pub const FRIDAY: Self = WeekdayIndexDto(5);
    pub const SATURDAY: Self = WeekdayIndexDto(6);

    pub fn value(self) -> u8 {
        self.0
    }

    /// Create from a raw u8 value (0=Sunday..6=Saturday).
    /// Returns `None` for values > 6.
    pub fn from_u8(v: u8) -> Option<Self> {
        if v > 6 {
            None
        } else {
            Some(WeekdayIndexDto(v))
        }
    }
}

impl Serialize for WeekdayIndexDto {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.0)
    }
}

impl<'de> Deserialize<'de> for WeekdayIndexDto {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let val = u8::deserialize(deserializer)?;
        if val > 6 {
            return Err(serde::de::Error::custom(
                "WeekdayIndexDto must be in range 0..=6",
            ));
        }
        Ok(WeekdayIndexDto(val))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SourceTypeDto {
    DefaultDiscovery,
    ManualRoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum StatusActionDto {
    OpenActivity,
    OpenSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SeverityDto {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum MetricOptionDto {
    Tokens,
    Cost,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct MethodErrorDto<TPayload = serde_json::Value> {
    pub code: String,
    pub message: String,
    pub payload: Option<TPayload>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TrendBucketGranularityDto {
    Hour,
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum BreakdownKindDto {
    Project,
    Model,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SourceScanStateDto {
    Idle,
    Scanning,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SettingsValidationErrorCodeDto {
    InvalidTimezone,
    InvalidWeekStartsOn,
    InvalidClientId,
    InvalidSourceType,
    InvalidRootPath,
    DuplicateManualRoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SettingsRecoveryActionIdDto {
    RescanAll,
    RebuildRollups,
    ResetFailedCheckpoints,
}

// ---------------------------------------------------------------------------
// Shell and Overview DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct StatusChipDto {
    pub id: String,
    pub label: String,
    pub tone: ToneDto,
    pub detail: Option<String>,
    pub action: Option<StatusActionDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ShellStatusDto {
    pub generated_at_ms: i64,
    pub status_chips: Vec<StatusChipDto>,
    pub readiness: ReadinessStateDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_event_seq: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer_queue_depth: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate_lag_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_bridge_connectivity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewMetricDto {
    pub id: String,
    pub label: String,
    pub value: String,
    pub helper: Option<String>,
    pub tone: ToneDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewTrendBucketDto {
    pub key: String,
    pub label: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
    pub cost_status: CostStatusDto,
    pub event_count: i64,
    pub is_current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewTrendDto {
    pub range: RangePresetDto,
    pub bucket_granularity: TrendBucketGranularityDto,
    pub metric_options: Vec<MetricOptionDto>,
    pub cost_status: CostStatusDto,
    pub buckets: Vec<OverviewTrendBucketDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewRankingItemDto {
    pub id: String,
    pub label: String,
    pub value: String,
    pub helper: Option<String>,
    pub bar_value: f64,
    pub action: Option<StatusActionDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewRankingSectionDto {
    pub id: String,
    pub title: String,
    pub items: Vec<OverviewRankingItemDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewHeatmapDayDto {
    pub date: String,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
    pub cost_status: CostStatusDto,
    pub event_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewHeatmapDto {
    pub today: String,
    pub week_starts_on: WeekdayIndexDto,
    pub days: Vec<OverviewHeatmapDayDto>,
}

// ---------------------------------------------------------------------------
// Activity DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ActivityListRequestDto {
    pub range: RangePresetDto,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub client_id: Option<String>,
    pub source_id: Option<String>,
    pub project_hash: Option<String>,
    pub model_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ActivityListSummaryDto {
    pub item_count: i64,
    pub total_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub cost_status: CostStatusDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ActivityListItemDto {
    pub id: String,
    pub happened_at_ms: i64,
    pub client_id: String,
    pub client_label: String,
    pub source_id: Option<String>,
    pub source_label: Option<String>,
    pub source_root_path: Option<String>,
    pub project_label: Option<String>,
    pub project_hash: Option<String>,
    pub model_id: Option<String>,
    pub model_label: Option<String>,
    pub tokens: i64,
    pub cache_hit_rate: Option<f64>,
    pub cost_usd: Option<f64>,
    pub cost_status: CostStatusDto,
    pub status: ActivityStatusDto,
    pub detail_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ActivityListResponseDto {
    pub generated_at_ms: i64,
    pub items: Vec<ActivityListItemDto>,
    pub next_cursor: Option<String>,
    pub summary: ActivityListSummaryDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ActivityDetailRequestDto {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TokenBreakdownDto {
    // Unified product metrics (shown by default in the UI):
    pub prompt_input_total_tokens: Option<i64>,
    pub prompt_input_non_cached_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cache_hit_rate: Option<f64>,
    // Raw audit fields (kept for technical-details/debug visibility):
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ActivityTechnicalDetailsDto {
    pub source_id: Option<String>,
    pub provider: Option<String>,
    pub raw_model: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ActivityDetailDto {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub happened_at_ms: i64,
    pub client_id: String,
    pub client_label: String,
    pub source_id: Option<String>,
    pub source_label: Option<String>,
    pub source_root_path: Option<String>,
    pub project_label: Option<String>,
    pub project_hash: Option<String>,
    pub session_id: Option<String>,
    pub model_id: Option<String>,
    pub model_label: Option<String>,
    pub status: ActivityStatusDto,
    pub tokens: i64,
    pub token_breakdown: Option<TokenBreakdownDto>,
    pub cost_usd: Option<f64>,
    pub cost_status: CostStatusDto,
    pub technical_details: ActivityTechnicalDetailsDto,
}

// ---------------------------------------------------------------------------
// Breakdown DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct BreakdownListRequestDto {
    pub kind: BreakdownKindDto,
    pub range: RangePresetDto,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProjectBreakdownListItemDto {
    pub id: String,
    pub project_hash: String,
    pub label: String,
    pub subtitle: Option<String>,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
    pub cost_status: CostStatusDto,
    pub event_count: i64,
    pub last_active_at_ms: Option<i64>,
    pub top_model_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ModelBreakdownListItemDto {
    pub id: String,
    pub label: String,
    pub subtitle: Option<String>,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
    pub cost_status: CostStatusDto,
    pub event_count: i64,
    pub last_active_at_ms: Option<i64>,
    pub client_labels: Vec<String>,
    pub top_project_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SessionBreakdownListItemDto {
    pub id: String,
    pub label: String,
    pub subtitle: Option<String>,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
    pub cost_status: CostStatusDto,
    pub event_count: i64,
    pub last_active_at_ms: Option<i64>,
    pub client_label: String,
    pub project_label: Option<String>,
    pub project_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
pub enum BreakdownListItemDto {
    #[serde(rename = "project")]
    Project(ProjectBreakdownListItemDto),
    #[serde(rename = "model")]
    Model(ModelBreakdownListItemDto),
    #[serde(rename = "session")]
    Session(SessionBreakdownListItemDto),
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct BreakdownListResponseSummaryDto {
    pub item_count: i64,
    pub total_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub total_cost_status: CostStatusDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct BreakdownListResponseDto {
    pub generated_at_ms: i64,
    pub kind: BreakdownKindDto,
    pub items: Vec<BreakdownListItemDto>,
    pub next_cursor: Option<String>,
    pub summary: BreakdownListResponseSummaryDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct BreakdownDetailRequestDto {
    pub kind: BreakdownKindDto,
    pub id: String,
    pub range: RangePresetDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct BreakdownMiniItemDto {
    pub id: String,
    pub label: String,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
    pub cost_status: CostStatusDto,
    pub event_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TechnicalDetailDto {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProjectBreakdownDetailDto {
    pub id: String,
    pub label: String,
    pub project_hash: String,
    pub project_path: Option<String>,
    pub metrics: Vec<OverviewMetricDto>,
    pub trend: OverviewTrendDto,
    pub model_mix: Vec<BreakdownMiniItemDto>,
    pub sessions: Vec<SessionBreakdownListItemDto>,
    pub recent_activity: Vec<ActivityListItemDto>,
    pub technical_details: Vec<TechnicalDetailDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ModelBreakdownDetailDto {
    pub id: String,
    pub label: String,
    pub metrics: Vec<OverviewMetricDto>,
    pub trend: OverviewTrendDto,
    pub token_breakdown: TokenBreakdownDto,
    pub client_mix: Vec<BreakdownMiniItemDto>,
    pub project_mix: Vec<ProjectBreakdownListItemDto>,
    pub recent_activity: Vec<ActivityListItemDto>,
    pub technical_details: Vec<TechnicalDetailDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SessionTimelineItemDto {
    pub id: String,
    pub happened_at_ms: i64,
    pub label: String,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
    pub cost_status: CostStatusDto,
    pub status: ActivityStatusDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SourceContextItemDto {
    pub source_id: String,
    pub client_label: String,
    pub root_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SessionBreakdownDetailDto {
    pub id: String,
    pub label: String,
    pub client_id: String,
    pub client_label: String,
    pub project_label: Option<String>,
    pub project_hash: Option<String>,
    pub last_active_at_ms: Option<i64>,
    pub metrics: Vec<OverviewMetricDto>,
    pub token_breakdown: TokenBreakdownDto,
    pub timeline: Vec<SessionTimelineItemDto>,
    pub models_used: Vec<BreakdownMiniItemDto>,
    pub source_context: Vec<SourceContextItemDto>,
    pub technical_details: Vec<TechnicalDetailDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
pub enum BreakdownDetailDto {
    #[serde(rename = "project")]
    Project(ProjectBreakdownDetailDto),
    #[serde(rename = "model")]
    Model(ModelBreakdownDetailDto),
    #[serde(rename = "session")]
    Session(SessionBreakdownDetailDto),
}

// ---------------------------------------------------------------------------
// Clients DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClientsSnapshotRequestDto {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub client_id: Option<String>,
    pub scan_state: Option<SourceScanStateDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClientStatusCardDto {
    pub id: String,
    pub label: String,
    pub tone: ToneDto,
    pub active_source_count: i64,
    pub event_count: i64,
    pub last_scan_at_ms: Option<i64>,
    pub helper: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClientSourceRowDto {
    pub id: String,
    pub client_id: String,
    pub client_label: String,
    pub root_path: String,
    pub source_type: SourceTypeDto,
    pub scan_state: SourceScanStateDto,
    pub configured_by_user: bool,
    pub last_scan_at_ms: Option<i64>,
    pub file_count: i64,
    pub parsed_file_count: i64,
    pub event_count: i64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClientsSnapshotSummaryDto {
    pub source_count: i64,
    pub active_source_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClientsSnapshotDto {
    pub generated_at_ms: i64,
    pub client_cards: Vec<ClientStatusCardDto>,
    pub sources: Vec<ClientSourceRowDto>,
    pub next_cursor: Option<String>,
    pub summary: ClientsSnapshotSummaryDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClientSourceDetailRequestDto {
    pub source_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClientSourceDetailDto {
    pub source: ClientSourceRowDto,
    pub recent_activity: Vec<ActivityListItemDto>,
    pub technical_details: Vec<TechnicalDetailDto>,
}

// ---------------------------------------------------------------------------
// Settings DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ManualRootDto {
    pub id: String,
    pub client_id: String,
    pub root_path: String,
    pub source_type: SourceTypeDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SettingsDiscoveryDto {
    pub claude_code_default_paths: bool,
    pub codex_default_paths: bool,
    pub manual_roots: Vec<ManualRootDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SettingsPrivacyDto {
    pub local_only: bool,
    pub redact_sensitive_values: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SettingsDiagnosticsDto {
    pub db_healthy: bool,
    pub db_size_bytes: i64,
    pub migration_version: i64,
    pub usage_event_count: i64,
    pub last_log_checkpoint_ms: Option<i64>,
    /// Current writer channel queue depth (0 = idle).
    pub writer_queue_depth: i64,
    /// Current aggregate lag in milliseconds (0 = caught up).
    pub aggregate_lag_ms: i64,
    /// Recent runtime diagnostic events (e.g. subscription lifecycle,
    /// writer thresholds, drift events).
    pub recent_diagnostics: Vec<SettingsDiagnosticEventDto>,
    /// Subagent doctor checks (spec §7.1). Always populated when the runtime
    /// constructs this DTO; per-check status reflects current configuration
    /// (e.g. `sidecar_launchable` is "ok" when `pi_sidecar.enabled=false`).
    /// The `Option` is for wire-level backwards-compatibility only — older
    /// clients may omit the field on deserialize. Reuses the existing
    /// `settings.diagnostics` RPC path — no separate `subagent.doctor` RPC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<SubagentDoctorResultDto>,
}

/// Result of running subagent doctor checks (spec §7.1). Returned as the
/// optional `subagent` field of `SettingsDiagnosticsDto` — no separate RPC
/// method, reuses the existing `settings.diagnostics` path.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SubagentDoctorResultDto {
    pub checks: Vec<DoctorCheckDto>,
    /// True iff no check has `status == "error"`. Warnings don't fail.
    pub overall_ok: bool,
}

/// One doctor check result. `status` is one of: `"ok"`, `"warning"`, `"error"`.
/// - `"ok"`: check passed.
/// - `"warning"`: check surfaced a non-blocking issue (e.g. stale subagents,
///   or a stubbed check not yet implemented — stubs return "warning" so
///   `overall_ok` doesn't claim a green check on unverified ground).
/// - `"error"`: check failed and `overall_ok` will be false.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DoctorCheckDto {
    pub name: String,
    pub status: String,
    pub detail: Option<String>,
}

/// A lightweight diagnostic event suitable for display in Settings/Diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SettingsDiagnosticEventDto {
    pub code: String,
    pub severity: String,
    pub message: String,
    pub happened_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SettingsRecoveryActionDto {
    pub id: SettingsRecoveryActionIdDto,
    pub label: String,
    pub description: String,
    pub dangerous: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SettingsRecoveryActionRequestDto {
    pub id: SettingsRecoveryActionIdDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SettingsRecoveryActionResponseDto {
    pub id: SettingsRecoveryActionIdDto,
    pub accepted: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SettingsValidationErrorDto {
    pub code: SettingsValidationErrorCodeDto,
    pub field_path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct SettingsSubagentDto {
    pub enabled: bool,
    pub profiles: Vec<ProfileDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SettingsSnapshotDto {
    pub timezone: String,
    pub week_starts_on: WeekdayIndexDto,
    pub discovery: SettingsDiscoveryDto,
    pub privacy: SettingsPrivacyDto,
    pub diagnostics: SettingsDiagnosticsDto,
    pub recovery_actions: Vec<SettingsRecoveryActionDto>,
    pub prompt_palette_default_action: PromptActionDto,
    pub subagent: SettingsSubagentDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SettingsUpdateRequestDto {
    pub timezone: Option<String>,
    pub week_starts_on: Option<WeekdayIndexDto>,
    pub discovery: Option<SettingsDiscoveryDto>,
    pub privacy: Option<SettingsPrivacyDto>,
    pub prompt_palette_default_action: Option<PromptActionDto>,
}

// ---------------------------------------------------------------------------
// Prompt Palette DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum PromptActionDto {
    #[serde(rename = "OnlyCopy", alias = "copy")]
    OnlyCopy,
    #[serde(rename = "OnlyPaste")]
    OnlyPaste,
    #[serde(rename = "CopyAndPaste", alias = "paste")]
    CopyAndPaste,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PromptSortDto {
    Smart,
    RecentlyUsed,
    MostUsed,
    RecentlyUpdated,
    Alphabetical,
    PinnedFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PromptUseSurfaceDto {
    Overlay,
    Page,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PromptUseOutcomeDto {
    Copy,
    PasteAttempted,
    PasteFellBackToCopy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PromptUseFailureReasonDto {
    PermissionMissing,
    FocusLost,
    InjectionFailed,
    UnsupportedPlatform,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PromptEntryDto {
    pub id: String,
    pub content: String,
    pub alias: Option<String>,
    pub tags: Vec<String>,
    pub is_pinned: bool,
    pub usage_count: i64,
    pub last_used_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PromptListQueryDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<PromptSortDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PromptListResponseDto {
    pub entries: Vec<PromptEntryDto>,
    pub total_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PromptGetRequestDto {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PromptCreateRequestDto {
    pub content: String,
    pub alias: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PromptUpdateRequestDto {
    pub id: String,
    pub content: String,
    pub alias: Option<String>,
    pub tags: Vec<String>,
    pub is_pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PromptDeleteRequestDto {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PromptDeleteResultDto {
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PromptUseRequestDto {
    pub id: String,
    pub action: PromptActionDto,
    pub surface: PromptUseSurfaceDto,
    pub outcome: PromptUseOutcomeDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<PromptUseFailureReasonDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PromptUseResultDto {
    pub usage_count: i64,
    pub last_used_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PromptSuggestTagsRequestDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PromptSuggestTagsResponseDto {
    pub tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// Readiness / scan progress (used by ReadEnvelope)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStateDto {
    Starting,
    Rebuilding,
    ReadyDegraded,
    ReadyExact,
}

/// Scan progress snapshot for `ReadEnvelopeDto`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ScanProgressDto {
    pub scanned_files: i64,
    pub total_files: Option<i64>,
    pub current_path: Option<String>,
    pub elapsed_ms: i64,
}

// ---------------------------------------------------------------------------
// ReadEnvelope — wraps all read-plane responses
// ---------------------------------------------------------------------------

/// Generic envelope for every read-plane method response.
///
/// `T` defaults to `serde_json::Value` so the envelope can be used in
/// TypeScript generation without a concrete payload type.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReadEnvelopeDto<T = serde_json::Value> {
    pub data: T,
    pub generated_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_id: Option<String>,
    pub readiness: ReadinessStateDto,
    pub is_exact: bool,
    pub is_stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watermark_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<ScanProgressDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Overview modular DTOs (replaces single overview.snapshot)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewSummaryRequestDto {
    pub range: RangePresetDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewSummaryDto {
    pub timezone: String,
    pub selected_range: RangePresetDto,
    pub cost_status: CostStatusDto,
    pub metrics: Vec<OverviewMetricDto>,
    pub generated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewTrendRequestDto {
    pub range: RangePresetDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granularity: Option<TrendBucketGranularityDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewTrendResponseDto {
    pub trend: OverviewTrendDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewHeatmapRequestDto {
    pub range: RangePresetDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewHeatmapResponseDto {
    pub heatmap: OverviewHeatmapDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewRankingsRequestDto {
    pub range: RangePresetDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OverviewRankingsResponseDto {
    pub rankings: Vec<OverviewRankingSectionDto>,
}

// ---------------------------------------------------------------------------
// Activity modular DTOs (activity.recent is new)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ActivityRecentRequestDto {
    pub range: RangePresetDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ActivityRecentResponseDto {
    pub recent_activity: Vec<ActivityListItemDto>,
}

// ---------------------------------------------------------------------------
// Live window DTO (replaces live.backfill)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct LiveWindowRequestDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct LiveWindowDto {
    /// Exact samples from the active promoted generation (usage_buckets_2s).
    pub exact_samples: Vec<LiveSampleDto>,
    /// Transient samples from the in-memory ring buffer (available during
    /// rebuild or first run before a generation is promoted).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transient_samples: Vec<LiveSampleDto>,
    pub current_tokens_per_sec: f64,
    pub current_events_per_sec: f64,
    pub start_ms: i64,
    pub end_ms: i64,
}

// ---------------------------------------------------------------------------
// Event sequence state DTO
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct EventSequenceStateDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_id: Option<i64>,
    pub sequence_gap: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sequence_ms: Option<i64>,
}

// ---------------------------------------------------------------------------
// Invalidation protocol types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum InvalidationDatasetDto {
    // Legacy page-level scopes (keep for backward compat)
    Overview,
    Activity,
    Clients,
    Breakdown,
    Settings,
    // New modular scopes for fine-grained invalidation
    OverviewSummary,
    OverviewTrend,
    OverviewHeatmap,
    OverviewRankings,
    ActivityRecent,
    ActivityList,
    ClientsSnapshot,
    SettingsDiagnostics,
    Diagnostics,
    LiveRealtime,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct InvalidationScopeDto {
    pub dataset: InvalidationDatasetDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakdown_kind: Option<BreakdownKindDto>,
}

/// Canonical invalidation scope set emitted after durable audit data changes.
///
/// Keep this list aligned with frontend query-key invalidation semantics.
pub fn canonical_invalidation_scopes() -> Vec<InvalidationScopeDto> {
    use InvalidationDatasetDto::*;
    vec![
        InvalidationScopeDto {
            dataset: OverviewSummary,
            breakdown_kind: None,
        },
        InvalidationScopeDto {
            dataset: OverviewTrend,
            breakdown_kind: None,
        },
        InvalidationScopeDto {
            dataset: OverviewHeatmap,
            breakdown_kind: None,
        },
        InvalidationScopeDto {
            dataset: OverviewRankings,
            breakdown_kind: None,
        },
        InvalidationScopeDto {
            dataset: ActivityRecent,
            breakdown_kind: None,
        },
        InvalidationScopeDto {
            dataset: ActivityList,
            breakdown_kind: None,
        },
        InvalidationScopeDto {
            dataset: Breakdown,
            breakdown_kind: None,
        },
        InvalidationScopeDto {
            dataset: Clients,
            breakdown_kind: None,
        },
        InvalidationScopeDto {
            dataset: LiveRealtime,
            breakdown_kind: None,
        },
    ]
}

// ---------------------------------------------------------------------------
// Live sample types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct LiveSampleDto {
    pub bucket_start_ms: i64,
    pub tokens_per_sec: f64,
    pub cost_per_sec: Option<f64>,
    pub events_per_sec: f64,
}

// ---------------------------------------------------------------------------
// IPC Event DTOs (used by Unix domain socket server/client)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RuntimeEventDto {
    pub event_type: String,
    pub payload: serde_json::Value,
    /// Global sequence number; None for ephemeral events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_seq: Option<i64>,
    /// Whether this event is ephemeral (not checkpointed, not durable).
    pub ephemeral: bool,
    /// Invalidation scopes carried by the envelope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<InvalidationScopeDto>,
    /// Generation ID at commit time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_id: Option<String>,
    /// Watermark timestamp of the generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watermark_ms: Option<i64>,
    /// Whether this event carries exact (committed / sampler) data.
    pub is_exact: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct EventSubscriptionBatchDto {
    pub events: Vec<RuntimeEventDto>,
}

// ---------------------------------------------------------------------------
// Subagent control DTOs (subagent.* methods)
// ---------------------------------------------------------------------------

// --- requests -------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct SubagentDelegateRequestDto {
    pub subagent_name: String,
    pub subagent_id: Option<String>,
    pub cwd: String,
    pub profile: String,
    pub intent: Option<String>,
    pub prompt: String,
    /// Spec §4.3: when set, references a stored artifact (relative path within
    /// the artifact store root) instead of the inline `prompt`. Mutually
    /// exclusive with `prompt` — exactly one must be non-empty/Some.
    pub prompt_artifact_ref: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub model_override: Option<String>,
    pub source_harness: Option<String>,
    pub source_session_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct SubagentListRequestDto {
    /// "hot" | "warm" | "cold"
    pub status: Option<String>,
    pub project: Option<String>,
    pub include_deleted: Option<bool>,
}

/// Resolution params for single-subagent operations (show/tasks/hibernate/delete).
/// Exactly one of `id` (UUID) or `name` (+ `cwd`) should be set.
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct SubagentResolveRequestDto {
    pub name: Option<String>,
    pub id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct SubagentTasksRequestDto {
    pub name: Option<String>,
    pub id: Option<String>,
    pub cwd: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct SubagentDeleteRequestDto {
    pub name: Option<String>,
    pub id: Option<String>,
    pub cwd: Option<String>,
    pub hard: Option<bool>,
}

// --- responses ------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentUsageDto {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentDelegateResponseDto {
    pub task_id: String,
    pub subagent_id: String,
    pub subagent_name: String,
    pub adapter: String,
    pub adapter_session_id: Option<String>,
    pub session_reused: bool,
    pub status: String,
    pub profile: String,
    pub model: Option<String>,
    pub summary: Option<String>,
    pub usage: SubagentUsageDto,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentDetailDto {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub repo_path: String,
    pub repo_hash: String,
    pub branch: Option<String>,
    pub intent: Option<String>,
    pub default_profile: String,
    pub default_model: Option<String>,
    pub status: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_active_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentListResponseDto {
    pub subagents: Vec<SubagentDetailDto>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentTaskSummaryDto {
    pub id: String,
    pub subagent_id: String,
    pub profile: String,
    pub status: String,
    pub prompt: Option<String>,
    pub result_summary: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentTasksResponseDto {
    pub tasks: Vec<SubagentTaskSummaryDto>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentAckDto {
    pub id: String,
    pub status: String,
}

// ---------------------------------------------------------------------------
// Subagent runtime status DTOs (spec §4 Phase 2)
// Wrapped by ReadEnvelopeDto<SubagentRuntimeStatusDto> at the handler layer.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize, Serialize, TS)]
pub struct SubagentRuntimeStatusRequestDto {
    /// Reserved for future filtering; Phase 2 ignores this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentPressureGateDto {
    pub level: String,
    pub memory_used_pct: u32,
    pub hot_sessions_total: u32,
    pub hot_sessions_limit: u32,
    /// Absolute ms when the worker ResourceSample was taken (via
    /// `busytok_domain::now_ms()`). `None` if no sample has been taken yet.
    /// Enables frontend freshness display. This is NOT the same as
    /// `ReadEnvelopeDto.generated_at_ms` (response construction time).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_sampled_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentRuntimeSubagentDto {
    pub name: String,
    pub status: String,
    pub task_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_task_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_task_status: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentRuntimeTaskDto {
    pub task_id: String,
    pub subagent_name: String,
    pub status: String,
    pub created_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentWorkerDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<u64>,
    pub hot_sessions: u32,
}

/// Inner data of `ReadEnvelopeDto` for `subagent.runtime_status`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SubagentRuntimeStatusDto {
    pub pressure_gate: SubagentPressureGateDto,
    pub subagents: Vec<SubagentRuntimeSubagentDto>,
    pub tasks_recent: Vec<SubagentRuntimeTaskDto>,
    pub workers: Vec<SubagentWorkerDto>,
}

// ─── Receipt DTOs (from main merge) ───
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct ReceiptDailyRequestDto {
    /// `YYYY-MM-DD` in the current reporting timezone. `None` = today
    /// (server-resolved). See `receipt.daily` spec.
    #[serde(default)]
    pub date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReceiptDailyDto {
    pub date: String,
    /// Server-produced label, e.g. "FRI · JUN 26, 2026". Format semantics
    /// intentionally match the GUI's `src/lib/formatters.ts`; produced
    /// server-side so the future Rust render path can share the ViewModel.
    pub date_label: String,
    pub timezone: String,
    pub metrics: ReceiptMetricsDto,
    pub top_models: Vec<ReceiptModelSliceDto>,
    pub brand: ReceiptBrandDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReceiptMetricsDto {
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    /// `cache_read_tokens / (input_tokens + cache_read_tokens)`, else `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_hit_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub cost_status: CostStatusDto,
    pub event_count: i64,
    pub session_count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_hour: Option<ReceiptPeakHourDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReceiptPeakHourDto {
    /// Reporting-TZ wall-clock hour, e.g. "14:00".
    pub label: String,
    pub tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReceiptModelSliceDto {
    pub name: String,
    pub tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub cost_status: CostStatusDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReceiptBrandDto {
    pub name: String,
    pub tagline: String,
    pub github: String,
    pub generated_at_ms: i64,
}

// ─── Provider DTOs (Phase 1: Credential Foundation) ───────────────────────

/// Provider as seen by the GUI. `has_api_key` indicates whether a key is
/// stored in the provider catalog, without exposing the key itself.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderDto {
    pub id: String,
    pub name: String,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub enabled: bool,
    pub has_api_key: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderCreateRequestDto {
    pub name: String,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderUpdateRequestDto {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    // None=不改, Some(None)=清除, Some(Some(k))=更新
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_some")]
    #[ts(type = "string | null | undefined")]
    pub api_key: Option<Option<String>>,
}

// ─── Model Catalog DTOs (Phase 3: Provider/Model Catalog Refactor) ───────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelCatalogEntryDto {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_kind: ProviderKind,
    pub provider_enabled: bool,
    pub model_db_id: String,
    pub model_id: String,
    pub model_enabled: bool,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelCreateRequestDto {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelUpdateRequestDto {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelDeleteRequestDto {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelListRequestDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub include_disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelListResponseDto {
    pub models: Vec<ModelCatalogEntryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ModelTagUpdateDto {
    pub model_id: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderListResponseDto {
    pub providers: Vec<ProviderDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderDeleteRequestDto {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderTestConnectionRequestDto {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProviderTestConnectionResponseDto {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_detected: Option<Vec<String>>,
}

// ─── Pi Sidecar Locator DTOs (Phase 5: GUI Startup Runtime-Dir Persistence) ─

/// Request to update the pi_sidecar locator fields (runtime_dir + enabled).
/// Spec §371: GUI injects the packaged sidecar path on startup; the service
/// owns the in-memory + on-disk mutation (mirrors provider_update pattern).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct PiSidecarLocatorUpdateRequestDto {
    /// Absolute path to the sidecar resource directory
    /// (e.g. `/Applications/Busytok.app/Contents/Resources/pi-sidecar`).
    pub runtime_dir: String,
    /// Whether the pi_sidecar subsystem should be enabled. In packaged
    /// mode, this is `true` so the protocol_version doctor check moves
    /// from "warning" to "ok" (spec §406).
    pub enabled: bool,
}

/// Response confirming the persisted locator state.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct PiSidecarLocatorUpdateResponseDto {
    pub runtime_dir: String,
    pub enabled: bool,
    /// `true` if the in-memory settings were also updated (service was
    /// reachable); `false` if only the file was written (fallback path).
    pub in_memory_updated: bool,
}

// ── Profiles (Phase 4: Profile/Model Configuration UI) ──────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProfileDto {
    pub id: String,
    /// True if this is one of the 3 built-in profiles (pi/search-cheap, etc.).
    /// Derived by the service from `is_builtin_profile()` — not stored in config.
    pub is_builtin: bool,
    /// Provider this profile runs on. None = unbound (delegate will reject).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub model: String,
    pub tools: Vec<String>,
    pub context_budget_tokens: u32,
    pub timeout_seconds: u64,
    pub write_access: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProfileCreateRequestDto {
    pub id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_budget_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_access: Option<bool>,
}

/// Patch semantics: None = leave unchanged. For provider_id, Some("") = unbind.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProfileUpdateRequestDto {
    pub id: String,
    /// Some("openai") = bind to openai; Some("") = unbind; None = unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_budget_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_access: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub struct ProfileDeleteRequestDto {
    pub id: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn provider_dto_round_trips() {
        let dto = ProviderDto {
            id: "deepseek-prod".to_string(),
            name: "DeepSeek".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.deepseek.com/v1".to_string(),
            enabled: true,
            has_api_key: true,
            created_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_000,
        };
        let json_str = serde_json::to_string(&dto).unwrap();
        let parsed: ProviderDto = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.id, "deepseek-prod");
        assert!(parsed.has_api_key);
        assert_eq!(parsed.provider_kind, ProviderKind::OpenAiCompatible);
        // No bare `api_key` field on the wire — only `has_api_key`.
        assert!(!json_str.contains("\"api_key\""));
        let json: serde_json::Value = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["provider_kind"], "openai_compatible");
    }

    #[test]
    fn provider_dto_does_not_expose_api_key_field() {
        // 严禁输出 api_key: ProviderDto has no api_key field at all.
        let dto = ProviderDto {
            id: "p".to_string(),
            name: "P".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://example.com".to_string(),
            enabled: true,
            has_api_key: false,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let json = serde_json::to_value(&dto).unwrap();
        // The only api_key-ish field is `has_api_key` (a bool flag).
        assert!(json.get("api_key").is_none());
        assert_eq!(json["has_api_key"], false);
    }

    #[test]
    fn provider_create_request_dto_omits_id_and_defaults_api_key() {
        // No `id` field: system generates UUID. api_key defaults to None.
        let minimal: ProviderCreateRequestDto = serde_json::from_str(
            r#"{"name":"DeepSeek","provider_kind":"openai_compatible","base_url":"https://api.deepseek.com/v1"}"#,
        )
        .unwrap();
        assert_eq!(minimal.name, "DeepSeek");
        assert_eq!(minimal.provider_kind, ProviderKind::OpenAiCompatible);
        assert!(minimal.api_key.is_none());

        // `id` is not a known field — JSON with `id` must still deserialize
        // (serde ignores unknown fields by default for these DTOs, but more
        // importantly the struct itself has no `id` field).
        let with_id: ProviderCreateRequestDto = serde_json::from_str(
            r#"{"id":"should-be-ignored","name":"X","provider_kind":"openai_compatible","base_url":"u","api_key":"k"}"#,
        )
        .unwrap();
        assert_eq!(with_id.name, "X");
        assert_eq!(with_id.api_key.as_deref(), Some("k"));
    }

    #[test]
    fn provider_update_request_dto_api_key_three_state() {
        // None = unchanged: field omitted on wire.
        let unchanged = ProviderUpdateRequestDto {
            id: "p".to_string(),
            name: None,
            base_url: None,
            enabled: None,
            api_key: None,
        };
        let json = serde_json::to_value(&unchanged).unwrap();
        assert!(json.get("api_key").is_none());
        assert!(json.get("name").is_none());

        // Some(None) = clear: serializes as JSON null.
        let clear = ProviderUpdateRequestDto {
            id: "p".to_string(),
            name: None,
            base_url: None,
            enabled: None,
            api_key: Some(None),
        };
        let json = serde_json::to_value(&clear).unwrap();
        assert_eq!(json["api_key"], serde_json::Value::Null);

        // Some(Some(k)) = update: serializes as the new key string.
        let update = ProviderUpdateRequestDto {
            id: "p".to_string(),
            name: None,
            base_url: None,
            enabled: None,
            api_key: Some(Some("new-key".to_string())),
        };
        let json = serde_json::to_value(&update).unwrap();
        assert_eq!(json["api_key"], "new-key");

        // Round-trip each state.
        let parsed_clear: ProviderUpdateRequestDto =
            serde_json::from_str(r#"{"id":"p","api_key":null}"#).unwrap();
        assert_eq!(parsed_clear.api_key, Some(None));

        let parsed_update: ProviderUpdateRequestDto =
            serde_json::from_str(r#"{"id":"p","api_key":"k"}"#).unwrap();
        assert_eq!(parsed_update.api_key, Some(Some("k".to_string())));

        // Absent on the wire deserializes to None (unchanged).
        let parsed_absent: ProviderUpdateRequestDto =
            serde_json::from_str(r#"{"id":"p"}"#).unwrap();
        assert_eq!(parsed_absent.api_key, None);
    }

    #[test]
    fn provider_kind_serializes_as_openai_compatible() {
        let json = serde_json::to_value(ProviderKind::OpenAiCompatible).unwrap();
        assert_eq!(json, "openai_compatible");
        let parsed: ProviderKind = serde_json::from_str("\"openai_compatible\"").unwrap();
        assert_eq!(parsed, ProviderKind::OpenAiCompatible);
    }

    #[test]
    fn model_catalog_entry_dto_round_trips() {
        let dto = ModelCatalogEntryDto {
            provider_id: "prov-1".to_string(),
            provider_name: "OpenAI".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            provider_enabled: true,
            model_db_id: "model-db-1".to_string(),
            model_id: "gpt-4o".to_string(),
            model_enabled: true,
            tags: vec!["fast".to_string(), "cheap".to_string()],
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["provider_id"], "prov-1");
        assert_eq!(json["provider_kind"], "openai_compatible");
        assert_eq!(json["model_db_id"], "model-db-1");
        assert_eq!(json["model_id"], "gpt-4o");
        assert_eq!(json["tags"][0], "fast");
        let parsed: ModelCatalogEntryDto = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.model_id, "gpt-4o");
        assert_eq!(parsed.tags.len(), 2);
    }

    #[test]
    fn model_create_request_dto_defaults() {
        // enabled + tags default when omitted.
        let minimal: ModelCreateRequestDto =
            serde_json::from_str(r#"{"provider_id":"p","model_id":"m"}"#).unwrap();
        assert_eq!(minimal.provider_id, "p");
        assert_eq!(minimal.model_id, "m");
        assert!(minimal.enabled.is_none());
        assert!(minimal.tags.is_empty());

        let full: ModelCreateRequestDto = serde_json::from_str(
            r#"{"provider_id":"p","model_id":"m","enabled":true,"tags":["t1"]}"#,
        )
        .unwrap();
        assert_eq!(full.enabled, Some(true));
        assert_eq!(full.tags, vec!["t1".to_string()]);
    }

    #[test]
    fn model_update_request_dto_patch_semantics() {
        let noop: ModelUpdateRequestDto = serde_json::from_str(r#"{"id":"m"}"#).unwrap();
        assert_eq!(noop.id, "m");
        assert!(noop.enabled.is_none());

        let patch = ModelUpdateRequestDto {
            id: "m".to_string(),
            enabled: Some(false),
        };
        let json = serde_json::to_value(&patch).unwrap();
        assert_eq!(json["enabled"], false);
        let parsed: ModelUpdateRequestDto = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.enabled, Some(false));
    }

    #[test]
    fn model_delete_request_dto_round_trips() {
        let dto = ModelDeleteRequestDto {
            id: "m".to_string(),
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json, serde_json::json!({"id": "m"}));
        let back: ModelDeleteRequestDto = serde_json::from_value(json).unwrap();
        assert_eq!(back.id, "m");
    }

    #[test]
    fn model_list_request_dto_defaults() {
        let minimal: ModelListRequestDto = serde_json::from_str(r#"{}"#).unwrap();
        assert!(minimal.provider_id.is_none());
        assert!(minimal.tags.is_empty());
        assert!(!minimal.include_disabled);

        let full: ModelListRequestDto = serde_json::from_str(
            r#"{"provider_id":"p","tags":["fast"],"include_disabled":true}"#,
        )
        .unwrap();
        assert_eq!(full.provider_id.as_deref(), Some("p"));
        assert_eq!(full.tags, vec!["fast".to_string()]);
        assert!(full.include_disabled);

        // provider_id=None is skipped on serialize.
        let json = serde_json::to_value(&minimal).unwrap();
        assert!(json.get("provider_id").is_none());
    }

    #[test]
    fn model_list_response_dto_round_trips() {
        let dto = ModelListResponseDto {
            models: vec![ModelCatalogEntryDto {
                provider_id: "p".to_string(),
                provider_name: "P".to_string(),
                provider_kind: ProviderKind::OpenAiCompatible,
                provider_enabled: true,
                model_db_id: "mdb".to_string(),
                model_id: "m".to_string(),
                model_enabled: true,
                tags: vec![],
            }],
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["models"][0]["model_id"], "m");
        let parsed: ModelListResponseDto = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.models.len(), 1);
    }

    #[test]
    fn model_tag_update_dto_round_trips() {
        let dto = ModelTagUpdateDto {
            model_id: "m".to_string(),
            tags: vec!["a".to_string(), "b".to_string()],
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["model_id"], "m");
        assert_eq!(json["tags"][0], "a");
        let back: ModelTagUpdateDto = serde_json::from_value(json).unwrap();
        assert_eq!(back.tags, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn provider_list_response_dto_round_trips() {
        let dto = ProviderListResponseDto {
            providers: vec![ProviderDto {
                id: "p1".to_string(),
                name: "OpenAI".to_string(),
                provider_kind: ProviderKind::OpenAiCompatible,
                base_url: "https://api.openai.com/v1".to_string(),
                enabled: true,
                has_api_key: true,
                created_at_ms: 1000,
                updated_at_ms: 2000,
            }],
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["providers"][0]["id"], "p1");
        assert_eq!(json["providers"][0]["has_api_key"], true);
        let back: ProviderListResponseDto = serde_json::from_value(json).unwrap();
        assert_eq!(back.providers.len(), 1);
        assert_eq!(back.providers[0].name, "OpenAI");
        assert!(back.providers[0].has_api_key);
    }

    #[test]
    fn overview_heatmap_zero_days_preserve_unavailable_cost() {
        let dto = OverviewHeatmapDto {
            today: "2026-05-20".to_string(),
            week_starts_on: WeekdayIndexDto::MONDAY,
            days: vec![OverviewHeatmapDayDto {
                date: "2026-05-20".to_string(),
                tokens: 0,
                cost_usd: None,
                cost_status: CostStatusDto::Unavailable,
                event_count: 0,
            }],
        };

        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["days"][0]["cost_status"], "unavailable");
        assert!(json["days"][0]["cost_usd"].is_null());
    }

    #[test]
    fn cost_status_dto_serde() {
        let exact = serde_json::to_value(CostStatusDto::Exact).unwrap();
        assert_eq!(exact, "exact");
        let partial: CostStatusDto = serde_json::from_str("\"partial\"").unwrap();
        assert_eq!(partial, CostStatusDto::Partial);
        let unavailable: CostStatusDto = serde_json::from_str("\"unavailable\"").unwrap();
        assert_eq!(unavailable, CostStatusDto::Unavailable);
    }

    #[test]
    fn range_preset_dto_serde() {
        let day = serde_json::to_value(RangePresetDto::Day).unwrap();
        assert_eq!(day, "day");
        let week: RangePresetDto = serde_json::from_str("\"week\"").unwrap();
        assert_eq!(week, RangePresetDto::Week);
    }

    #[test]
    fn weekday_index_dto_serde() {
        let sun = serde_json::to_value(WeekdayIndexDto::SUNDAY).unwrap();
        assert_eq!(sun, 0);
        let mon = serde_json::to_value(WeekdayIndexDto::MONDAY).unwrap();
        assert_eq!(mon, 1);
        let sat = serde_json::to_value(WeekdayIndexDto::SATURDAY).unwrap();
        assert_eq!(sat, 6);

        let parsed: WeekdayIndexDto = serde_json::from_str("3").unwrap();
        assert_eq!(parsed, WeekdayIndexDto::WEDNESDAY);

        let err: Result<WeekdayIndexDto, _> = serde_json::from_str("7");
        assert!(err.is_err());
    }

    #[test]
    fn activity_list_response_dto_serde() {
        let dto = ActivityListResponseDto {
            generated_at_ms: 1000,
            items: vec![ActivityListItemDto {
                id: "evt-1".to_string(),
                happened_at_ms: 1000,
                client_id: "claude_code".to_string(),
                client_label: "Claude Code".to_string(),
                source_id: None,
                source_label: None,
                source_root_path: None,
                project_label: Some("my-project".to_string()),
                project_hash: Some("abc".to_string()),
                model_id: Some("claude-sonnet-4".to_string()),
                model_label: Some("claude-sonnet-4".to_string()),
                tokens: 1000,
                cache_hit_rate: Some(0.3),
                cost_usd: Some(0.05),
                cost_status: CostStatusDto::Exact,
                status: ActivityStatusDto::Ok,
                detail_available: true,
            }],
            next_cursor: None,
            summary: ActivityListSummaryDto {
                item_count: 1,
                total_tokens: 1000,
                total_cost_usd: Some(0.05),
                cost_status: CostStatusDto::Exact,
            },
        };

        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["items"][0]["cost_status"], "exact");
        assert_eq!(json["items"][0]["status"], "ok");
        assert_eq!(json["summary"]["item_count"], 1);
    }

    #[test]
    fn breakdown_list_response_dto_serde() {
        let dto = BreakdownListResponseDto {
            generated_at_ms: 2000,
            kind: BreakdownKindDto::Project,
            items: vec![BreakdownListItemDto::Project(ProjectBreakdownListItemDto {
                id: "proj-1".to_string(),
                project_hash: "proj-1".to_string(),
                label: "my-project".to_string(),
                subtitle: None,
                tokens: 5000,
                cost_usd: None,
                cost_status: CostStatusDto::Unavailable,
                event_count: 10,
                last_active_at_ms: Some(2000),
                top_model_label: Some("claude-sonnet-4".to_string()),
            })],
            next_cursor: Some("cursor-1".to_string()),
            summary: BreakdownListResponseSummaryDto {
                item_count: 1,
                total_tokens: 5000,
                total_cost_usd: None,
                total_cost_status: CostStatusDto::Unavailable,
            },
        };

        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["kind"], "project");
        assert_eq!(json["items"][0]["kind"], "project");
        assert!(json["items"][0]["cost_usd"].is_null());
        assert_eq!(json["items"][0]["cost_status"], "unavailable");
    }

    #[test]
    fn settings_validation_error_dto_serde() {
        let dto = SettingsValidationErrorDto {
            code: SettingsValidationErrorCodeDto::InvalidTimezone,
            field_path: "timezone".to_string(),
            message: "Invalid timezone".to_string(),
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["code"], "invalid_timezone");
        assert_eq!(json["field_path"], "timezone");
    }

    #[test]
    fn method_error_dto_serde() {
        let err: MethodErrorDto<Vec<SettingsValidationErrorDto>> = MethodErrorDto {
            code: "settings_validation_failed".to_string(),
            message: "Validation failed".to_string(),
            payload: Some(vec![SettingsValidationErrorDto {
                code: SettingsValidationErrorCodeDto::InvalidRootPath,
                field_path: "discovery.manual_roots[0].root_path".to_string(),
                message: "Path does not exist".to_string(),
            }]),
        };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["code"], "settings_validation_failed");
        assert_eq!(json["payload"][0]["code"], "invalid_root_path");
    }

    #[test]
    fn control_request_with_meta_roundtrip() {
        let meta = RequestMeta {
            session_id: Some("sess-abc".into()),
            correlation_id: Some("corr-xyz".into()),
        };
        let req = ControlRequest::with_meta("shell.status", serde_json::json!({}), meta.clone());
        let json = serde_json::to_string(&req).unwrap();
        let roundtripped: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtripped.meta.session_id.as_deref(), Some("sess-abc"));
        assert_eq!(
            roundtripped.meta.correlation_id.as_deref(),
            Some("corr-xyz")
        );
    }

    #[test]
    fn control_request_default_meta_is_none() {
        let req = ControlRequest::new("shell.status", serde_json::json!({}));
        assert!(req.meta.session_id.is_none());
        assert!(req.meta.correlation_id.is_none());
    }

    #[test]
    fn control_request_omitted_meta_deserializes_as_default() {
        let json = r#"{"method":"shell.status","params":{}}"#;
        let req: ControlRequest = serde_json::from_str(json).unwrap();
        assert!(req.meta.session_id.is_none());
        assert!(req.meta.correlation_id.is_none());
    }

    #[test]
    fn token_breakdown_dto_keeps_raw_and_adds_unified() {
        let tb = TokenBreakdownDto {
            prompt_input_total_tokens: Some(1000),
            prompt_input_non_cached_tokens: Some(10),
            cache_read_tokens: Some(990),
            cache_write_tokens: None,
            cache_hit_rate: Some(0.99),
            input_tokens: Some(10),
            output_tokens: Some(50),
            cached_input_tokens: Some(990),
            reasoning_tokens: None,
            total_tokens: 1050,
        };
        let json = serde_json::to_string(&tb).unwrap();
        // Unified additions:
        assert!(json.contains("prompt_input_total_tokens"));
        assert!(json.contains("cache_hit_rate"));
        // Raw audit field still present (not collapsed away):
        assert!(json.contains("cached_input_tokens"));
    }

    #[test]
    fn subagent_doctor_result_dto_serializes_round_trip() {
        let dto = SubagentDoctorResultDto {
            checks: vec![DoctorCheckDto {
                name: "resource_policy_valid".to_string(),
                status: "ok".to_string(),
                detail: None,
            }],
            overall_ok: true,
        };
        let json = serde_json::to_string(&dto).unwrap();
        let back: SubagentDoctorResultDto = serde_json::from_str(&json).unwrap();
        assert_eq!(back.checks.len(), 1);
        assert_eq!(back.checks[0].name, "resource_policy_valid");
        assert!(back.overall_ok);
    }

    #[test]
    fn settings_diagnostics_dto_serializes_with_optional_subagent_none() {
        // Backwards-compat: existing clients don't send `subagent` field.
        // Deserialization must still work.
        let json = r#"{
            "db_healthy": true,
            "db_size_bytes": 4096,
            "migration_version": 3,
            "usage_event_count": 0,
            "last_log_checkpoint_ms": null,
            "writer_queue_depth": 0,
            "aggregate_lag_ms": 0,
            "recent_diagnostics": []
        }"#;
        let dto: SettingsDiagnosticsDto = serde_json::from_str(json).unwrap();
        assert!(
            dto.subagent.is_none(),
            "missing field => None (backwards-compat)"
        );
    }

    #[test]
    fn settings_diagnostics_dto_serializes_with_subagent_present() {
        let dto = SettingsDiagnosticsDto {
            db_healthy: true,
            db_size_bytes: 4096,
            migration_version: 3,
            usage_event_count: 0,
            last_log_checkpoint_ms: None,
            writer_queue_depth: 0,
            aggregate_lag_ms: 0,
            recent_diagnostics: vec![],
            subagent: Some(SubagentDoctorResultDto {
                checks: vec![DoctorCheckDto {
                    name: "service_running".to_string(),
                    status: "ok".to_string(),
                    detail: None,
                }],
                overall_ok: true,
            }),
        };
        let json = serde_json::to_string(&dto).unwrap();
        let back: SettingsDiagnosticsDto = serde_json::from_str(&json).unwrap();
        assert!(back.subagent.is_some());
        assert_eq!(back.subagent.unwrap().checks.len(), 1);
    }

    #[test]
    fn profile_dto_round_trips_with_snake_case_wire() {
        let dto = ProfileDto {
            id: "pi/search-cheap".to_string(),
            is_builtin: true,
            provider_id: Some("deepseek-prod".to_string()),
            model: "deepseek-chat".to_string(),
            tools: vec!["read".to_string(), "write".to_string()],
            context_budget_tokens: 8000,
            timeout_seconds: 30,
            write_access: false,
        };
        let json = serde_json::to_value(&dto).unwrap();
        // snake_case wire names.
        assert_eq!(json["id"], "pi/search-cheap");
        assert_eq!(json["is_builtin"], true);
        assert_eq!(json["provider_id"], "deepseek-prod");
        assert_eq!(json["context_budget_tokens"], 8000);
        assert_eq!(json["timeout_seconds"], 30);
        assert_eq!(json["write_access"], false);

        let parsed: ProfileDto = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.id, "pi/search-cheap");
        assert!(parsed.is_builtin);
        assert_eq!(parsed.provider_id.as_deref(), Some("deepseek-prod"));
        assert_eq!(parsed.tools, vec!["read".to_string(), "write".to_string()]);
    }

    #[test]
    fn profile_dto_unbound_provider_omits_field() {
        // Unbound profile: provider_id = None must be skipped on serialize.
        let dto = ProfileDto {
            id: "draft".to_string(),
            is_builtin: false,
            provider_id: None,
            model: "gpt-4o".to_string(),
            tools: vec![],
            context_budget_tokens: 4096,
            timeout_seconds: 60,
            write_access: true,
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert!(json.get("provider_id").is_none());
        // Round-trips with provider_id absent in JSON.
        let parsed: ProfileDto = serde_json::from_value(json).unwrap();
        assert!(parsed.provider_id.is_none());
    }

    #[test]
    fn profile_create_request_dto_defaults_omitted_fields() {
        // Only `id` and `model` are required; everything else is optional.
        let minimal: ProfileCreateRequestDto =
            serde_json::from_str(r#"{"id":"p","model":"m"}"#).unwrap();
        assert_eq!(minimal.id, "p");
        assert_eq!(minimal.model, "m");
        assert!(minimal.provider_id.is_none());
        assert!(minimal.tools.is_none());
        assert!(minimal.context_budget_tokens.is_none());
        assert!(minimal.timeout_seconds.is_none());
        assert!(minimal.write_access.is_none());

        let full = ProfileCreateRequestDto {
            id: "p2".to_string(),
            model: "gpt-4o".to_string(),
            provider_id: Some("openai".to_string()),
            tools: Some(vec!["read".to_string()]),
            context_budget_tokens: Some(16000),
            timeout_seconds: Some(120),
            write_access: Some(true),
        };
        let json = serde_json::to_value(&full).unwrap();
        // Optional fields present when set.
        assert_eq!(json["provider_id"], "openai");
        assert_eq!(json["context_budget_tokens"], 16000);
        assert_eq!(json["write_access"], true);
    }

    #[test]
    fn profile_update_request_dto_patch_semantics() {
        // None = leave unchanged. Empty string on provider_id = unbind.
        let patch = ProfileUpdateRequestDto {
            id: "p".to_string(),
            provider_id: Some(String::new()), // unbind
            model: None,
            tools: None,
            context_budget_tokens: None,
            timeout_seconds: None,
            write_access: None,
        };
        let json = serde_json::to_value(&patch).unwrap();
        assert_eq!(json["provider_id"], "");
        // None fields skipped.
        assert!(json.get("model").is_none());
        assert!(json.get("tools").is_none());
        assert!(json.get("write_access").is_none());

        let parsed: ProfileUpdateRequestDto = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.provider_id.as_deref(), Some(""));
        assert!(parsed.model.is_none());

        // Minimal patch with just an id is valid (no-op update).
        let noop: ProfileUpdateRequestDto = serde_json::from_str(r#"{"id":"p"}"#).unwrap();
        assert_eq!(noop.id, "p");
        assert!(noop.provider_id.is_none());
    }

    #[test]
    fn profile_delete_request_dto_round_trips() {
        let dto = ProfileDeleteRequestDto {
            id: "draft".to_string(),
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json, serde_json::json!({"id": "draft"}));
        let back: ProfileDeleteRequestDto = serde_json::from_value(json).unwrap();
        assert_eq!(back.id, "draft");
    }

    #[test]
    fn settings_snapshot_dto_requires_subagent_field() {
        // Spec §4 Phase 4: SettingsSnapshotDto.subagent must be present.
        // Constructing without subagent must fail to deserialize (field is required).
        let payload = serde_json::json!({
            "timezone": "UTC",
            "week_starts_on": 1,
            "discovery": {
                "claude_code_default_paths": true,
                "codex_default_paths": true,
                "manual_roots": []
            },
            "privacy": {
                "local_only": false,
                "redact_sensitive_values": true
            },
            "diagnostics": {
                "db_healthy": true,
                "db_size_bytes": 0,
                "migration_version": 1,
                "usage_event_count": 0,
                "last_log_checkpoint_ms": null,
                "writer_queue_depth": 0,
                "aggregate_lag_ms": 0,
                "recent_diagnostics": []
            },
            "recovery_actions": [],
            "prompt_palette_default_action": "OnlyCopy"
        });
        let err = serde_json::from_value::<SettingsSnapshotDto>(payload);
        assert!(
            err.is_err(),
            "missing subagent field must fail to deserialize"
        );
    }

    #[test]
    fn settings_subagent_dto_round_trips_with_profiles() {
        let dto = SettingsSubagentDto {
            enabled: true,
            profiles: vec![ProfileDto {
                id: "pi/search-cheap".to_string(),
                is_builtin: true,
                provider_id: Some("deepseek-prod".to_string()),
                model: "deepseek-chat".to_string(),
                tools: vec!["read".to_string()],
                context_budget_tokens: 8000,
                timeout_seconds: 30,
                write_access: false,
            }],
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["enabled"], true);
        assert_eq!(json["profiles"][0]["id"], "pi/search-cheap");
        assert_eq!(json["profiles"][0]["is_builtin"], true);

        let back: SettingsSubagentDto = serde_json::from_value(json).unwrap();
        assert!(back.enabled);
        assert_eq!(back.profiles.len(), 1);
        assert!(back.profiles[0].is_builtin);
    }
}
