//! Daily rollup rebuild utilities.
//!
//! Timezone resolution:
//! - IANA timezone names (e.g. "America/New_York", "Asia/Shanghai") are
//!   fully supported via `ReportingTimezone`.
//! - Fixed offsets like "+08:00" or "-05:00" are always supported.
//! - The special string "local" resolves to the system's detected IANA timezone.
//! - "UTC" resolves to UTC.

use anyhow::{Context, Result};
use busytok_domain::{NormalizedUsageEvent, ReportingTimezone};
use time::{Date, Weekday};

use crate::mutations::RollupOptions;

/// Derive a local date string (YYYY-MM-DD) from a UTC timestamp in
/// milliseconds, using the given reporting timezone.
pub fn date_from_timestamp_ms(timestamp_ms: i64, rtz: &ReportingTimezone) -> Result<String> {
    rtz.local_date_for_timestamp_ms(timestamp_ms)
}

/// Merge two optional costs by summation.
fn merge_cost(existing: Option<f64>, incoming: Option<f64>) -> Option<f64> {
    match (existing, incoming) {
        (Some(a), Some(b)) => Some(a + b),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Get the start date of the week containing the given date string.
///
/// `date_str` must be in YYYY-MM-DD format.
/// `start_day` is the weekday that starts the week.
/// Returns a YYYY-MM-DD string for the week start date.
pub fn get_date_week(date_str: &str, start_day: Weekday) -> Result<String, anyhow::Error> {
    let date = Date::parse(
        date_str,
        &time::format_description::well_known::Iso8601::DATE,
    )
    .with_context(|| format!("invalid date format: {date_str}"))?;
    let weekday = date.weekday();
    // Compute days back to the start of the week.
    // number_days_from_sunday() returns 0-6 for Sun-Sat.
    let date_num = weekday.number_days_from_sunday() as i32;
    let start_num = start_day.number_days_from_sunday() as i32;
    let days_back = (date_num - start_num + 7) % 7;
    let week_start_jd = date.to_julian_day() - days_back;
    let start_date = Date::from_julian_day(week_start_jd)
        .with_context(|| format!("invalid julian day: {week_start_jd}"))?;
    Ok(format!(
        "{:04}-{:02}-{:02}",
        start_date.year(),
        u8::from(start_date.month()),
        start_date.day()
    ))
}

/// Rebuild weekly usage aggregates from the full event corpus.
///
/// Groups daily usage rows by week start date (using Sunday start),
/// producing a JSON array suitable for inclusion in the realtime summary.
pub fn build_weekly_usage_value(
    events: &[NormalizedUsageEvent],
    options: RollupOptions,
) -> Result<serde_json::Value> {
    let rtz = &options.timezone;

    // Group by (week, agent, project_hash, model).
    let mut weekly_map: std::collections::HashMap<
        (String, String, String, String),
        DailyAccumulator,
    > = std::collections::HashMap::new();

    for event in events {
        let date = rtz.local_date_for_timestamp_ms(event.timestamp_ms)?;
        let week = get_date_week(&date, time::Weekday::Sunday)?;
        let agent = event.agent.as_str().to_string();
        let project_hash = event.project_hash.clone().unwrap_or_default();
        let model = event.model.clone().unwrap_or_default();
        let key = (week, agent, project_hash, model);

        let acc = weekly_map.entry(key).or_default();
        acc.input_tokens += event.input_tokens;
        acc.output_tokens += event.output_tokens;
        acc.total_tokens += event.total_tokens;
        acc.cached_input_tokens += event.cached_input_tokens;
        acc.cache_creation_tokens += event.cache_creation_tokens;
        acc.cache_read_tokens += event.cache_read_tokens;
        acc.reasoning_tokens += event.reasoning_tokens;
        acc.thoughts_tokens += event.thoughts_tokens;
        acc.tool_tokens += event.tool_tokens;
        acc.cost_usd = merge_cost(acc.cost_usd, event.cost_usd);
        acc.estimated_cost_usd = merge_cost(acc.estimated_cost_usd, event.estimated_cost_usd);
        acc.event_count += 1;
    }

    // Build JSON array for realtime summary.
    let tz_str = rtz.canonical_name().to_string();
    let weekly_rows: Vec<serde_json::Value> = weekly_map
        .into_iter()
        .map(|((week, agent, project_hash, model), acc)| {
            serde_json::json!({
                "week": week,
                "timezone": tz_str,
                "agent": agent,
                "project_hash": project_hash,
                "model": model,
                "input_tokens": acc.input_tokens,
                "output_tokens": acc.output_tokens,
                "total_tokens": acc.total_tokens,
                "cached_input_tokens": acc.cached_input_tokens,
                "cache_creation_tokens": acc.cache_creation_tokens,
                "cache_read_tokens": acc.cache_read_tokens,
                "reasoning_tokens": acc.reasoning_tokens,
                "thoughts_tokens": acc.thoughts_tokens,
                "tool_tokens": acc.tool_tokens,
                "cost_usd": acc.cost_usd,
                "estimated_cost_usd": acc.estimated_cost_usd,
                "event_count": acc.event_count,
            })
        })
        .collect();

    Ok(serde_json::Value::Array(weekly_rows))
}

/// Accumulator for daily usage aggregation.
#[derive(Default)]
struct DailyAccumulator {
    input_tokens: i64,
    output_tokens: i64,
    total_tokens: i64,
    cached_input_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
    reasoning_tokens: i64,
    thoughts_tokens: i64,
    tool_tokens: i64,
    cost_usd: Option<f64>,
    estimated_cost_usd: Option<f64>,
    event_count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_from_timestamp_ms_utc() {
        // 2025-01-15 08:30:00 UTC = 1736929800 seconds since epoch.
        let ts_ms: i64 = 1736929800 * 1000;
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let date = date_from_timestamp_ms(ts_ms, &rtz).unwrap();
        assert_eq!(date, "2025-01-15");
    }

    #[test]
    fn date_from_timestamp_ms_shanghai() {
        // 2025-01-15 08:30:00 UTC = 2025-01-15 16:30:00 CST
        let ts_ms: i64 = 1736929800 * 1000;
        let rtz = ReportingTimezone::parse("Asia/Shanghai").unwrap();
        let date = date_from_timestamp_ms(ts_ms, &rtz).unwrap();
        assert_eq!(date, "2025-01-15");
    }

    #[test]
    fn date_from_timestamp_ms_new_york_dst() {
        // Verify IANA timezone handles DST correctly
        let rtz = ReportingTimezone::parse("America/New_York").unwrap();
        let date = date_from_timestamp_ms(1736929800 * 1000, &rtz).unwrap();
        // 2025-01-15 03:30:00 EST (UTC-5) -> still Jan 15
        assert_eq!(date, "2025-01-15");
    }

    // -- get_date_week tests --

    #[test]
    fn get_date_week_sunday_start_midweek() {
        // 2025-01-15 is a Wednesday. With Sunday start, the week starts on 2025-01-12 (Sunday).
        let result = get_date_week("2025-01-15", Weekday::Sunday).unwrap();
        assert_eq!(result, "2025-01-12");
    }

    #[test]
    fn get_date_week_sunday_start_sunday() {
        // 2025-01-12 is a Sunday. Week start should be the same day.
        let result = get_date_week("2025-01-12", Weekday::Sunday).unwrap();
        assert_eq!(result, "2025-01-12");
    }

    #[test]
    fn get_date_week_sunday_start_saturday() {
        // 2025-01-18 is a Saturday. With Sunday start, the week started on 2025-01-12.
        let result = get_date_week("2025-01-18", Weekday::Sunday).unwrap();
        assert_eq!(result, "2025-01-12");
    }

    #[test]
    fn get_date_week_monday_start() {
        // 2025-01-15 is a Wednesday. With Monday start, week starts on 2025-01-13 (Monday).
        let result = get_date_week("2025-01-15", Weekday::Monday).unwrap();
        assert_eq!(result, "2025-01-13");
    }

    #[test]
    fn get_date_week_year_boundary() {
        // 2026-01-01 is a Thursday. With Sunday start, week starts on 2025-12-28.
        let result = get_date_week("2026-01-01", Weekday::Sunday).unwrap();
        assert_eq!(result, "2025-12-28");
    }

    #[test]
    fn get_date_week_invalid_date() {
        let result = get_date_week("not-a-date", Weekday::Sunday);
        assert!(result.is_err());
    }
}
