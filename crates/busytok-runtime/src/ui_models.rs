//! Pure UI model mapping functions for Surge pages.
//!
//! This module centralizes label resolution, tone determination, metric-card
//! construction, and cost-status logic.  It has **no** database access and
//! **no** async — only pure transformations from store rows / domain values
//! into display-ready DTOs.
//!
//! # Design rule
//!
//! Every non-trivial mapping that a supervisor control method needs should
//! live here so it can be unit-tested in isolation and reused across pages.

use busytok_protocol::dto::*;

// ---------------------------------------------------------------------------
// Client identity labels
// ---------------------------------------------------------------------------

/// Return the human-readable display label for a client kind string.
///
/// Known clients:
///  - `"claude_code"` -> "Claude Code"
///  - `"codex_cli"`   -> "Codex"
///
/// Unknown values are returned title-cased (first letter capitalised).
pub fn client_label(kind: &str) -> String {
    match kind {
        "claude_code" => "Claude Code".to_string(),
        "codex_cli" | "codex" => "Codex".to_string(),
        other => {
            if other.is_empty() {
                "Unknown".to_string()
            } else {
                let mut chars = other.chars();
                match chars.next() {
                    None => "Unknown".to_string(),
                    Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                }
            }
        }
    }
}

/// Map a per-client rollup to the same positive/neutral signaling used by
/// the existing top-level status chips.
pub fn client_rollup_tone(active_source_count: i64) -> ToneDto {
    if active_source_count > 0 {
        ToneDto::Success
    } else {
        ToneDto::Neutral
    }
}

// ---------------------------------------------------------------------------
// Source scan-state mapping
// ---------------------------------------------------------------------------

/// Map a source scan-state string to `SourceScanStateDto`.
pub fn source_scan_state(status: &str) -> SourceScanStateDto {
    match status {
        "error" => SourceScanStateDto::Error,
        "warning" => SourceScanStateDto::Warning,
        "scanning" | "active" => SourceScanStateDto::Scanning,
        _ => SourceScanStateDto::Idle,
    }
}

// ---------------------------------------------------------------------------
// Activity status mapping
// ---------------------------------------------------------------------------

/// Determine an activity's status from its row properties.
///
/// Returns `Error` when `is_error` is true, otherwise `Ok`.
pub fn activity_status(is_error: bool) -> ActivityStatusDto {
    if is_error {
        ActivityStatusDto::Error
    } else {
        ActivityStatusDto::Ok
    }
}

// ---------------------------------------------------------------------------
// Cost status
// ---------------------------------------------------------------------------

/// Build a `CostStatusDto` from store-level indicator flags.
///
/// - `Exact` if every contributing event has a cost
/// - `Partial` if some have cost and some don't
/// - `Unavailable` if no event has cost
pub fn cost_status(has_cost: bool, has_no_cost: bool) -> CostStatusDto {
    match (has_cost, has_no_cost) {
        (true, false) => CostStatusDto::Exact,
        (true, true) => CostStatusDto::Partial,
        (false, _) => CostStatusDto::Unavailable,
    }
}

/// Return `None` when status is `Unavailable`, otherwise return `cost`.
pub fn cost_usd_for_status(cost: Option<f64>, cs: &CostStatusDto) -> Option<f64> {
    match cs {
        CostStatusDto::Unavailable => None,
        _ => cost,
    }
}

// ---------------------------------------------------------------------------
// Metric-card builders
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct UsageTotals {
    pub total_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub event_count: i64,
    pub has_cost: bool,
    pub has_no_cost: bool,
}

impl From<&busytok_store::read_models::OverviewSummaryRow> for UsageTotals {
    fn from(row: &busytok_store::read_models::OverviewSummaryRow) -> Self {
        Self {
            total_tokens: row.total_tokens,
            total_cost_usd: row.total_cost_usd,
            event_count: row.event_count,
            has_cost: row.has_cost,
            has_no_cost: row.has_no_cost,
        }
    }
}

/// Build the three overview metric cards from range totals and range type.
pub fn overview_metrics(
    range: RangePresetDto,
    totals: &UsageTotals,
) -> Vec<OverviewMetricDto> {
    let (tokens_label, cost_label, events_label) = metric_labels(range);

    let cs = cost_status(totals.has_cost, totals.has_no_cost);

    vec![
        OverviewMetricDto {
            id: "tokens".to_string(),
            label: tokens_label,
            value: format_tokens(totals.total_tokens),
            helper: None,
            tone: ToneDto::Neutral,
        },
        OverviewMetricDto {
            id: "cost".to_string(),
            label: cost_label,
            value: format_cost(totals.total_cost_usd, &cs),
            helper: None,
            tone: ToneDto::Neutral,
        },
        OverviewMetricDto {
            id: "events".to_string(),
            label: events_label,
            value: format_thousands(totals.event_count),
            helper: None,
            tone: ToneDto::Neutral,
        },
    ]
}

/// Build metric cards for a breakdown detail.
pub fn breakdown_metrics(range: RangePresetDto, totals: &UsageTotals) -> Vec<OverviewMetricDto> {
    let (tokens_label, cost_label, events_label) = metric_labels(range);
    let cs = cost_status(totals.has_cost, totals.has_no_cost);

    vec![
        OverviewMetricDto {
            id: "tokens".to_string(),
            label: tokens_label,
            value: format_tokens(totals.total_tokens),
            helper: None,
            tone: ToneDto::Neutral,
        },
        OverviewMetricDto {
            id: "cost".to_string(),
            label: cost_label,
            value: format_cost(totals.total_cost_usd, &cs),
            helper: None,
            tone: ToneDto::Neutral,
        },
        OverviewMetricDto {
            id: "events".to_string(),
            label: events_label,
            value: format_thousands(totals.event_count),
            helper: None,
            tone: ToneDto::Neutral,
        },
    ]
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Format a token count for display with compact suffixes.
///
/// - `< 1,000` → plain number (`"500"`)
/// - `1,000 ..< 1_000_000` → `"1.5K"`, `"24K"`
/// - `1_000_000 ..< 1_000_000_000` → `"3.4M"`, `"24M"`
/// - `>= 1_000_000_000` → `"2.5B"`, `"10B"`
///
/// Rounding follows the frontend `formatCompactNumber`: fractions carry one
/// decimal place when the integral part is a single digit.
pub fn format_tokens(tokens: i64) -> String {
    if tokens < 0 {
        return format!("-{}", format_tokens(-tokens));
    }

    let n = tokens as u64;

    if n < 1_000 {
        return n.to_string();
    }

    if n < 1_000_000 {
        return format_compact(n, 1_000, "K");
    }

    if n < 1_000_000_000 {
        return format_compact(n, 1_000_000, "M");
    }

    format_compact(n, 1_000_000_000, "B")
}

fn format_compact(n: u64, divisor: u64, suffix: &str) -> String {
    let scaled = n as f64 / divisor as f64;

    // Round to one decimal place. When this pushes to the next scale
    // (e.g. 999_950 → 1000.0K → 1M), escalate.  Matches the frontend
    // formatCompactNumber's toFixed(1) + ≥1000 overflow check.
    let rounded = (scaled * 10.0).round() / 10.0;
    if rounded >= 1_000.0 {
        return match suffix {
            "K" => format_compact(n, 1_000_000, "M"),
            "M" => format_compact(n, 1_000_000_000, "B"),
            "B" => format_compact(n, 1_000_000_000_000, "T"),
            _ => format!("{:.1}{suffix}", n as f64 / (divisor as f64 * 1_000.0)),
        };
    }

    // Always use one decimal place, stripping ".0" for round numbers.
    // Matching the frontend's _stripZero(toFixed(1)) behaviour.
    let s = format!("{rounded:.1}");
    let s = s.strip_suffix(".0").unwrap_or(&s);
    format!("{s}{suffix}")
}

/// Format thousands with commas (e.g. `"1,234"`).
pub fn format_thousands(n: i64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    let len = s.len();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result
}

/// Format a cost value for display (e.g. `"$1.23"`).
/// Uses adaptive decimals for small amounts:
///   >= $0.10 → 2dp, >= $0.01 → 3dp, >= $0.001 → 4dp, >= $0.0001 → 5dp, > $0 → 6dp.
/// Returns `"N/A"` when cost is unavailable.
/// Both `Exact` and `Partial` produce the same formatted string; the distinction
/// is preserved in the data layer for diagnostics and sorting.
pub fn format_cost(cost: Option<f64>, cs: &CostStatusDto) -> String {
    match cs {
        CostStatusDto::Unavailable => "N/A".to_string(),
        _ => {
            let c = cost.unwrap_or(0.0);
            let abs = c.abs();
            let dp = if abs >= 0.1 {
                2
            } else if abs >= 0.01 {
                3
            } else if abs >= 0.001 {
                4
            } else if abs >= 0.0001 {
                5
            } else if c == 0.0 {
                2
            } else {
                6
            };
            format!("${:.dp$}", c, dp = dp)
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Return the three metric labels keyed by the selected range.
fn metric_labels(range: RangePresetDto) -> (String, String, String) {
    match range {
        RangePresetDto::Day => (
            "Today Tokens".to_string(),
            "Today Cost".to_string(),
            "Today Events".to_string(),
        ),
        RangePresetDto::Week => (
            "Week Tokens".to_string(),
            "Week Cost".to_string(),
            "Week Events".to_string(),
        ),
        RangePresetDto::Month => (
            "Month Tokens".to_string(),
            "Month Cost".to_string(),
            "Month Events".to_string(),
        ),
        RangePresetDto::Year => (
            "Year Tokens".to_string(),
            "Year Cost".to_string(),
            "Year Events".to_string(),
        ),
    }
}

// ---------------------------------------------------------------------------
// Trend helpers
// ---------------------------------------------------------------------------

/// Map a `RangePresetDto` to the granularity used for trend buckets.
pub fn trend_granularity(range: RangePresetDto) -> TrendBucketGranularityDto {
    match range {
        RangePresetDto::Day => TrendBucketGranularityDto::Hour,
        RangePresetDto::Week => TrendBucketGranularityDto::Day,
        RangePresetDto::Month => TrendBucketGranularityDto::Day,
        RangePresetDto::Year => TrendBucketGranularityDto::Month,
    }
}

/// Format a trend bucket key into a human-readable label based on granularity.
///
/// Hour -> "9:00 AM", Day -> "Mon 19", Month -> "January"
pub fn format_trend_label(granularity: &TrendBucketGranularityDto, key: &str) -> String {
    match granularity {
        // Week granularity is not currently emitted by trend_granularity(),
        // but is required for exhaustive matching.
        TrendBucketGranularityDto::Week => {
            use time::Date;
            // Fall back to day formatting since weekly data is bucketed by day
            if let Ok(date) = Date::parse(
                key,
                &time::format_description::parse("[year]-[month]-[day]").unwrap(),
            ) {
                let weekday = match date.weekday() {
                    time::Weekday::Monday => "Mon",
                    time::Weekday::Tuesday => "Tue",
                    time::Weekday::Wednesday => "Wed",
                    time::Weekday::Thursday => "Thu",
                    time::Weekday::Friday => "Fri",
                    time::Weekday::Saturday => "Sat",
                    time::Weekday::Sunday => "Sun",
                };
                return format!("{} {}", weekday, date.day());
            }
            key.to_string()
        }
        TrendBucketGranularityDto::Hour => {
            // key format: "2026-05-20T09:00:00"
            if let Some(time_part) = key.split('T').nth(1) {
                if let Some(h) = time_part
                    .split(':')
                    .next()
                    .and_then(|h| h.parse::<u8>().ok())
                {
                    return format!("{h}:00");
                }
            }
            key.to_string()
        }
        TrendBucketGranularityDto::Day => {
            // key format: "2026-05-19"
            use time::Date;
            if let Ok(date) = Date::parse(
                key,
                &time::format_description::parse("[year]-[month]-[day]").unwrap(),
            ) {
                let month_abbr = match date.month() {
                    time::Month::January => "Jan",
                    time::Month::February => "Feb",
                    time::Month::March => "Mar",
                    time::Month::April => "Apr",
                    time::Month::May => "May",
                    time::Month::June => "Jun",
                    time::Month::July => "Jul",
                    time::Month::August => "Aug",
                    time::Month::September => "Sep",
                    time::Month::October => "Oct",
                    time::Month::November => "Nov",
                    time::Month::December => "Dec",
                };
                return format!("{} {}", month_abbr, date.day());
            }
            key.to_string()
        }
        TrendBucketGranularityDto::Month => {
            // key format: "2026-01"
            use time::Month;
            let parts: Vec<&str> = key.split('-').collect();
            if parts.len() == 2 {
                if let Ok(month_num) = parts[1].parse::<u8>() {
                    if let Ok(m) = Month::try_from(month_num) {
                        let abbr = match m {
                            Month::January => "Jan",
                            Month::February => "Feb",
                            Month::March => "Mar",
                            Month::April => "Apr",
                            Month::May => "May",
                            Month::June => "Jun",
                            Month::July => "Jul",
                            Month::August => "Aug",
                            Month::September => "Sep",
                            Month::October => "Oct",
                            Month::November => "Nov",
                            Month::December => "Dec",
                        };
                        return abbr.to_string();
                    }
                }
            }
            key.to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// Model label helpers
// ---------------------------------------------------------------------------

/// Format a model ID into a display label.
/// Strips provider prefixes like `anthropic::` and falls back to "Unknown" for empty values.
pub fn model_label(model: &str) -> String {
    if model.is_empty() {
        return "Unknown".to_string();
    }
    model
        .strip_prefix("anthropic::")
        .or_else(|| model.strip_prefix("openai::"))
        .or_else(|| model.strip_prefix("google::"))
        .unwrap_or(model)
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── client_label ───────────────────────────────────────────────────

    #[test]
    fn client_label_claude_code() {
        assert_eq!(client_label("claude_code"), "Claude Code");
    }

    #[test]
    fn client_label_codex_cli() {
        assert_eq!(client_label("codex_cli"), "Codex");
        assert_eq!(client_label("codex"), "Codex");
    }

    #[test]
    fn client_label_unknown() {
        assert_eq!(client_label(""), "Unknown");
        assert_eq!(client_label("unknown_agent"), "Unknown_agent");
    }

    #[test]
    fn client_rollup_tone_success_for_active_clients() {
        assert_eq!(client_rollup_tone(1), ToneDto::Success);
        assert_eq!(client_rollup_tone(2), ToneDto::Success);
    }

    #[test]
    fn client_rollup_tone_neutral_without_active_sources() {
        assert_eq!(client_rollup_tone(0), ToneDto::Neutral);
        assert_eq!(client_rollup_tone(-1), ToneDto::Neutral);
    }

    // ── cost_status ────────────────────────────────────────────────────

    #[test]
    fn cost_status_exact() {
        assert_eq!(cost_status(true, false), CostStatusDto::Exact);
    }

    #[test]
    fn cost_status_partial() {
        assert_eq!(cost_status(true, true), CostStatusDto::Partial);
    }

    #[test]
    fn cost_status_unavailable() {
        assert_eq!(cost_status(false, true), CostStatusDto::Unavailable);
        assert_eq!(cost_status(false, false), CostStatusDto::Unavailable);
    }

    #[test]
    fn cost_usd_none_when_unavailable() {
        assert_eq!(
            cost_usd_for_status(Some(1.0), &CostStatusDto::Unavailable),
            None
        );
        assert_eq!(cost_usd_for_status(None, &CostStatusDto::Unavailable), None);
        assert_eq!(
            cost_usd_for_status(Some(1.0), &CostStatusDto::Exact),
            Some(1.0)
        );
    }

    // ── format helpers ─────────────────────────────────────────────────

    #[test]
    fn format_tokens_small() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(42), "42");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn format_tokens_k() {
        assert_eq!(format_tokens(1_000), "1K");
        assert_eq!(format_tokens(1_500), "1.5K");
        assert_eq!(format_tokens(24_000), "24K");
        assert_eq!(format_tokens(24_500), "24.5K");
        assert_eq!(format_tokens(999_500), "999.5K");
    }

    #[test]
    fn format_tokens_k_overflow_to_m() {
        assert_eq!(format_tokens(999_950), "1M");
        assert_eq!(format_tokens(999_999), "1M");
    }

    #[test]
    fn format_tokens_m() {
        assert_eq!(format_tokens(1_000_000), "1M");
        assert_eq!(format_tokens(3_400_000), "3.4M");
        assert_eq!(format_tokens(24_000_000), "24M");
        assert_eq!(format_tokens(25_500_000), "25.5M");
    }

    #[test]
    fn format_tokens_m_overflow_to_b() {
        assert_eq!(format_tokens(999_999_999), "1B");
    }

    #[test]
    fn format_tokens_b() {
        assert_eq!(format_tokens(1_000_000_000), "1B");
        assert_eq!(format_tokens(2_500_000_000), "2.5B");
        assert_eq!(format_tokens(10_000_000_000), "10B");
    }

    #[test]
    fn format_tokens_negative() {
        assert_eq!(format_tokens(-500), "-500");
        assert_eq!(format_tokens(-1_500), "-1.5K");
        assert_eq!(format_tokens(-3_400_000), "-3.4M");
    }

    #[test]
    fn format_cost_available() {
        assert_eq!(format_cost(Some(1.23), &CostStatusDto::Exact), "$1.23");
        assert_eq!(format_cost(Some(0.0), &CostStatusDto::Exact), "$0.00");
    }

    #[test]
    fn format_cost_adaptive_decimals() {
        assert_eq!(format_cost(Some(0.15), &CostStatusDto::Exact), "$0.15");
        assert_eq!(format_cost(Some(0.012), &CostStatusDto::Exact), "$0.012");
        assert_eq!(format_cost(Some(0.0012), &CostStatusDto::Exact), "$0.0012");
        assert_eq!(
            format_cost(Some(0.00012), &CostStatusDto::Exact),
            "$0.00012"
        );
        assert_eq!(
            format_cost(Some(0.0000005438), &CostStatusDto::Exact),
            "$0.000001"
        );
    }

    #[test]
    fn format_cost_partial_same_as_exact() {
        assert_eq!(format_cost(Some(1.23), &CostStatusDto::Partial), "$1.23");
        assert_eq!(format_cost(Some(0.012), &CostStatusDto::Partial), "$0.012");
        assert_eq!(format_cost(None, &CostStatusDto::Partial), "$0.00");
    }

    #[test]
    fn format_cost_na() {
        assert_eq!(format_cost(None, &CostStatusDto::Unavailable), "N/A");
        assert_eq!(format_cost(Some(1.23), &CostStatusDto::Unavailable), "N/A");
    }

    // ── source_scan_state ──────────────────────────────────────────────

    #[test]
    fn source_scan_state_error() {
        assert_eq!(source_scan_state("error"), SourceScanStateDto::Error);
    }

    #[test]
    fn source_scan_state_warning() {
        assert_eq!(source_scan_state("warning"), SourceScanStateDto::Warning);
    }

    #[test]
    fn source_scan_state_active() {
        assert_eq!(source_scan_state("active"), SourceScanStateDto::Scanning);
    }

    #[test]
    fn source_scan_state_idle() {
        assert_eq!(source_scan_state("idle"), SourceScanStateDto::Idle);
    }

    // ── activity_status ────────────────────────────────────────────────

    #[test]
    fn activity_status_error_when_is_error() {
        assert_eq!(activity_status(true), ActivityStatusDto::Error);
    }

    #[test]
    fn activity_status_ok_when_no_error() {
        assert_eq!(activity_status(false), ActivityStatusDto::Ok);
    }

    // ── overview_metrics ───────────────────────────────────────────────

    #[test]
    fn overview_metrics_returns_three_cards() {
        let totals = UsageTotals {
            total_tokens: 5000,
            total_cost_usd: Some(1.23),
            event_count: 42,
            has_cost: true,
            has_no_cost: false,
        };
        let metrics = overview_metrics(RangePresetDto::Day, &totals);
        assert_eq!(metrics.len(), 3);
        assert_eq!(metrics[0].id, "tokens");
        assert_eq!(metrics[0].label, "Today Tokens");
        assert_eq!(metrics[0].value, "5K");
        assert_eq!(metrics[1].id, "cost");
        assert_eq!(metrics[1].label, "Today Cost");
        assert_eq!(metrics[1].value, "$1.23");
        assert_eq!(metrics[2].id, "events");
    }

    #[test]
    fn usage_totals_from_overview_summary_row_preserves_all_fields() {
        let row = busytok_store::read_models::OverviewSummaryRow {
            total_tokens: 4321,
            total_cost_usd: Some(2.5),
            event_count: 9,
            has_cost: true,
            has_no_cost: true,
        };

        assert_eq!(
            UsageTotals::from(&row),
            UsageTotals {
                total_tokens: 4321,
                total_cost_usd: Some(2.5),
                event_count: 9,
                has_cost: true,
                has_no_cost: true,
            }
        );
    }

    #[test]
    fn overview_metrics_accept_read_model_totals() {
        let totals = UsageTotals {
            total_tokens: 1234,
            total_cost_usd: Some(1.25),
            event_count: 7,
            has_cost: true,
            has_no_cost: false,
        };

        let cards = overview_metrics(RangePresetDto::Day, &totals);
        assert_eq!(cards[0].value, "1.2K");
        assert_eq!(cards[1].value, "$1.25");
        assert_eq!(cards[2].value, "7");
    }

    #[test]
    fn overview_metrics_week_labels() {
        let totals = UsageTotals {
            total_tokens: 0,
            total_cost_usd: None,
            event_count: 0,
            has_cost: false,
            has_no_cost: false,
        };
        let metrics = overview_metrics(RangePresetDto::Week, &totals);
        assert_eq!(metrics[0].label, "Week Tokens");
        assert_eq!(metrics[1].label, "Week Cost");
        assert_eq!(metrics[2].label, "Week Events");
    }

    // ── trend_granularity ──────────────────────────────────────────────

    #[test]
    fn trend_granularity_day_is_hour() {
        assert_eq!(
            trend_granularity(RangePresetDto::Day),
            TrendBucketGranularityDto::Hour
        );
    }

    #[test]
    fn trend_granularity_week_is_day() {
        assert_eq!(
            trend_granularity(RangePresetDto::Week),
            TrendBucketGranularityDto::Day
        );
    }

    #[test]
    fn trend_granularity_year_is_month() {
        assert_eq!(
            trend_granularity(RangePresetDto::Year),
            TrendBucketGranularityDto::Month
        );
    }

    // ── model_label ────────────────────────────────────────────────────

    #[test]
    fn model_label_strips_provider_prefix() {
        assert_eq!(
            model_label("anthropic::claude-sonnet-4-20250514"),
            "claude-sonnet-4-20250514"
        );
    }

    #[test]
    fn model_label_empty_is_unknown() {
        assert_eq!(model_label(""), "Unknown");
    }

    #[test]
    fn model_label_passthrough() {
        assert_eq!(model_label("gpt-4o"), "gpt-4o");
    }

    #[test]
    fn model_label_strips_openai_prefix() {
        assert_eq!(model_label("openai::gpt-5.1-codex"), "gpt-5.1-codex");
    }

    #[test]
    fn model_label_strips_google_prefix() {
        assert_eq!(model_label("google::gemini-2.5-pro"), "gemini-2.5-pro");
    }
}
