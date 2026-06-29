//! Read-model structs for the store query surface.
//!
//! These are simple row / result DTOs returned by `read_queries` functions.
//! They are deliberately separate from the protocol DTOs in
//! `busytok-protocol`.

/// A time range with inclusive start and exclusive end.
#[derive(Debug, Clone)]
pub struct RangeWindow {
    pub start_ms: i64,
    pub end_ms: i64,
}

impl RangeWindow {
    pub fn new(start_ms: i64, end_ms: i64) -> Self {
        Self { start_ms, end_ms }
    }
}

// ── Service state ────────────────────────────────────────────────────────────

/// Maps to `service_state` joined with `event_sequence_state`.
#[derive(Debug, Clone)]
pub struct ServiceStateRow {
    pub writer_queue_depth: i64,
    pub aggregate_lag_ms: i64,
    pub readiness: Option<String>,
    pub active_generation_id: Option<String>,
    pub last_exact_rebuild_at_ms: Option<i64>,
    pub updated_at_ms: i64,
    /// Latest sequence number from `event_sequence_state`, if any.
    pub latest_event_seq: Option<i64>,
}

/// Fresh-install defaults — the single authoritative source for what the
/// service looks like before the first `service_state` row is persisted.
/// `busytok_runtime::status::ServiceStateRow` has no `Default` and maps from this row.
impl Default for ServiceStateRow {
    fn default() -> Self {
        Self {
            writer_queue_depth: 0,
            aggregate_lag_ms: 0,
            readiness: Some("starting".to_string()),
            active_generation_id: None,
            last_exact_rebuild_at_ms: None,
            updated_at_ms: 0,
            latest_event_seq: None,
        }
    }
}

// ── Overview ─────────────────────────────────────────────────────────────────

/// Aggregate totals for the overview summary panel.
#[derive(Debug, Clone)]
pub struct OverviewSummaryRow {
    pub total_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub event_count: i64,
    pub has_cost: bool,
    pub has_no_cost: bool,
}

/// A single trend bucket aggregation.
#[derive(Debug, Clone)]
pub struct OverviewTrendBucketRow {
    pub key: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
    pub event_count: i64,
    pub has_cost: bool,
    pub has_no_cost: bool,
}

/// An explicit time window used for exact-range overview aggregation.
#[derive(Debug, Clone)]
pub struct OverviewExactWindow {
    pub key: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// A single day in the overview heatmap.
#[derive(Debug, Clone)]
pub struct OverviewHeatmapDayRow {
    pub date: String,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
    pub event_count: i64,
    pub has_cost: bool,
    pub has_no_cost: bool,
}

// ── Rankings ─────────────────────────────────────────────────────────────────

/// A single ranking group with aggregated token/cost counts.
#[derive(Debug, Clone)]
pub struct RankingRow {
    pub group_key: String,
    pub label: Option<String>,
    pub total_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub event_count: i64,
    pub has_cost: bool,
    pub has_no_cost: bool,
}

// ── Shared pagination / breakdown ────────────────────────────────────────────

/// Supported materialized dimensions for breakdown pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakdownDimension {
    Project,
    Model,
    Session,
}

/// Supported raw fact filters used by breakdown detail reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakdownFilterField {
    Project,
    Model,
    Session,
}

/// Cursor-paginated page returned by read-model list queries.
#[derive(Debug, Clone)]
pub struct CursorPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

// ── Activity ─────────────────────────────────────────────────────────────────

/// A single activity list item row.
#[derive(Debug, Clone)]
pub struct ActivityListRow {
    pub id: String,
    pub happened_at_ms: i64,
    pub client_kind: String,
    pub session_id: String,
    pub source_file_id: String,
    pub source_path: String,
    pub project_hash: Option<String>,
    pub project_path: Option<String>,
    pub model: Option<String>,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    /// Unified: total prompt input including the cacheable portion.
    pub prompt_input_total_tokens: i64,
    /// Unified: prompt input not served from cache.
    pub prompt_input_non_cached_tokens: i64,
    /// Unified: prompt input served from cache (cache hit).
    pub cache_read_tokens: i64,
    /// Unified: prompt input written to cache (cache fill / alias for cache_write).
    pub cache_creation_tokens: i64,
    pub cost_usd: Option<f64>,
    pub is_error: bool,
}

/// Source metadata associated with a usage event's source file.
#[derive(Debug, Clone)]
pub struct ActivitySourceInfoRow {
    pub source_id: String,
    pub agent: String,
    pub root_path: String,
}

// ── Clients ──────────────────────────────────────────────────────────────────

/// A single client source snapshot row.
#[derive(Debug, Clone)]
pub struct ClientSnapshotRow {
    pub id: String,
    pub client_kind: String,
    pub root_path: String,
    pub source_type: String,
    pub scan_state: String,
    pub configured_by_user: bool,
    pub last_scan_at_ms: Option<i64>,
    pub file_count: i64,
    pub parsed_file_count: i64,
    pub event_count: i64,
    pub last_error: Option<String>,
    pub severity: Option<String>,
}

/// A single materialized client source health row.
#[derive(Debug, Clone)]
pub struct SourceHealthSummaryRow {
    pub source_id: String,
    pub agent: String,
    pub root_path: String,
    pub source_type: String,
    pub status: String,
    pub configured_by_user: bool,
    pub last_scan_at_ms: Option<i64>,
    pub file_count: i64,
    pub parsed_file_count: i64,
    pub event_count: i64,
    pub last_error: Option<String>,
    pub latest_activity_at_ms: Option<i64>,
}

/// A materialized rollup for the client summary cards.
#[derive(Debug, Clone)]
pub struct ClientRollupRow {
    pub client_kind: String,
    pub active_source_count: i64,
    pub event_count: i64,
    pub last_scan_at_ms: Option<i64>,
}

/// Filtered source-summary totals for the clients snapshot header.
#[derive(Debug, Clone)]
pub struct SourceHealthSummaryTotalsRow {
    pub source_count: i64,
    pub active_source_count: i64,
}

/// Full source row for clients.detail.
#[derive(Debug, Clone)]
pub struct ClientSourceDetailRow {
    pub source_id: String,
    pub agent: String,
    pub root_path: String,
    pub source_type: String,
    pub status: String,
    pub configured_by_user: bool,
    pub last_scan_at_ms: Option<i64>,
    pub file_count: i64,
    pub parsed_file_count: i64,
    pub event_count: i64,
    pub last_error: Option<String>,
}

// ── Settings ─────────────────────────────────────────────────────────────────

/// A single settings key-value snapshot row.
#[derive(Debug, Clone)]
pub struct SettingsSnapshotRow {
    pub key: String,
    pub value_json: String,
}

/// A single recent diagnostic event row for the settings diagnostics page.
#[derive(Debug, Clone)]
pub struct SettingsDiagnosticEventRow {
    pub id: String,
    pub severity: String,
    pub code: String,
    pub message: String,
    pub happened_at_ms: i64,
}

/// Diagnostics aggregate row including queue depth, lag, and recent events.
#[derive(Debug, Clone)]
pub struct SettingsDiagnosticsRow {
    pub writer_queue_depth: i64,
    pub aggregate_lag_ms: i64,
    pub readiness: Option<String>,
    pub active_generation_id: Option<String>,
    pub recent_events: Vec<SettingsDiagnosticEventRow>,
}

/// Full detail for a single usage event (activity.detail).
#[derive(Debug, Clone)]
pub struct ActivityDetailRow {
    pub id: String,
    pub agent: String,
    pub source_file_id: String,
    pub source_path: Option<String>,
    pub source_line: Option<i64>,
    pub source_offset_start: Option<i64>,
    pub source_offset_end: Option<i64>,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub source_request_id: Option<String>,
    pub message_id: Option<String>,
    pub timestamp_ms: i64,
    pub project_path: Option<String>,
    pub project_hash: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub agent_version: Option<String>,
    pub client_kind: Option<String>,
    pub speed: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub reasoning_tokens: i64,
    pub thoughts_tokens: i64,
    pub tool_tokens: i64,
    pub cost_usd: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
    pub cost_currency: Option<String>,
    pub cost_source: Option<String>,
    pub price_catalog_version: Option<String>,
    pub is_error: bool,
    pub error_type: Option<String>,
    pub raw_event_hash: Option<String>,
    pub usage_limit_reset_time_ms: Option<i64>,
    pub generation_id: Option<String>,
    /// Unified: total prompt input including the cacheable portion.
    pub prompt_input_total_tokens: i64,
    /// Unified: prompt input not served from cache.
    pub prompt_input_non_cached_tokens: i64,
    /// Discriminator recording how the raw provider payload reported tokens.
    pub provider_payload_shape: String,
}

/// A single breakdown group row.
#[derive(Debug, Clone)]
pub struct BreakdownGroupRow {
    pub group_key: String,
    pub label: Option<String>,
    pub subtitle: Option<String>,
    pub total_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub event_count: i64,
    pub last_active_at_ms: Option<i64>,
    pub has_cost: bool,
    pub has_no_cost: bool,
    pub extra_values: Vec<Option<String>>,
}

/// Aggregated totals for a breakdown list response.
#[derive(Debug, Clone)]
pub struct BreakdownTotalsRow {
    pub grouped_count: i64,
    pub total_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub has_cost: bool,
    pub has_no_cost: bool,
}

/// Token component totals for a single model detail view.
///
/// The unified sums (`prompt_input_*`, `cache_read_tokens`,
/// `cache_creation_tokens`) are the cross-provider product metrics;
/// `cached_input_tokens` is retained as the raw provider audit value.
#[derive(Debug, Clone)]
pub struct ModelTokenBreakdownRow {
    pub prompt_input_total_tokens: i64,
    pub prompt_input_non_cached_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// Raw provider audit value (kept for auditing, not for product metrics).
    pub cached_input_tokens: i64,
    pub reasoning_tokens: i64,
}

/// A daily_usage row aggregated by date for trend/heatmap display.
#[derive(Debug, Clone)]
pub struct DailyUsageTrendRow {
    pub date: String,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
    pub event_count: i64,
    pub has_cost: bool,
    pub has_no_cost: bool,
}

/// Hero token/cost totals for one receipt day (aggregated from `daily_usage`,
/// single date + timezone + generation). `has_cost`/`has_no_cost` drive the
/// derived `cost_status` (the column does not exist on `daily_usage`).
#[derive(Debug, Clone)]
pub struct ReceiptDailyTotalsRow {
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cost_usd: Option<f64>,
    pub has_cost: bool,
    pub has_no_cost: bool,
    pub event_count: i64,
}

/// One model's day slice for the receipt items section.
#[derive(Debug, Clone)]
pub struct ReceiptModelSliceRow {
    pub name: String,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
    pub has_cost: bool,
    pub has_no_cost: bool,
}

/// The highest-token UTC hour bucket within the receipt day window.
/// `bucket_start_ms` is UTC-aligned; the caller converts it to a reporting-TZ
/// hour label.
#[derive(Debug, Clone)]
pub struct PeakHourRow {
    pub bucket_start_ms: i64,
    pub tokens: i64,
}
