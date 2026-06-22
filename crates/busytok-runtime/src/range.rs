//! Range resolution for Surge UI pages.
//!
//! Computes time windows (day/week/month/year) in a given timezone and
//! generates trend-bucket subdivisions suitable for store aggregation.
//!
//! All timezone handling goes through `ReportingTimezone` from
//! `busytok-domain`, which supports both IANA names and fixed UTC offsets.

use anyhow::Result;
use busytok_domain::ReportingTimezone;
use busytok_protocol::dto::{RangePresetDto, WeekdayIndexDto};
use time::{Date, Duration, Month, Time, UtcOffset};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A resolved time range with both millisecond and date-string representations.
#[derive(Debug, Clone)]
pub struct ResolvedRange {
    pub start_ms: i64,
    pub end_ms: i64,
    pub start_date: String,
    pub end_date: String,
}

/// A 12-calendar-month heatmap window expressed as inclusive date strings.
#[derive(Debug, Clone)]
pub struct HeatmapWindow {
    pub start: String,
    pub end_inclusive: String,
}

/// A single trend bucket window computed by the runtime.
///
/// The store accepts slices of these for per-bucket aggregation.
/// The store does NOT depend on `busytok-runtime`, so matching read-model
/// windows are defined in `busytok-store`.
#[derive(Debug, Clone)]
pub struct TrendBucketWindow {
    pub start_ms: i64,
    pub end_ms: i64,
    pub key: String,
    pub is_current: bool,
}

/// A single heatmap day window with local-time boundaries.
///
/// The runtime precomputes these so the store can query each day's data
/// using local-timezone midnight-to-midnight boundaries, avoiding the
/// UTC day grouping of the raw SQL `date()` function.
#[derive(Debug, Clone)]
pub struct HeatmapDayWindow {
    pub date: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

// ---------------------------------------------------------------------------
// Public functions
// ---------------------------------------------------------------------------

/// Parse a timezone string and return a `ReportingTimezone`.
///
/// Accepts `"UTC"`, `"local"`, IANA names like `"Asia/Shanghai"`, and
/// fixed offsets like `"+08:00"` or `"-05:00"`.
pub fn parse_timezone(tz: &str) -> Result<ReportingTimezone> {
    ReportingTimezone::parse(tz)
}

/// Returns `true` when the SQL hour-bucket fast path can be used.
///
/// The fast path uses SQL's built-in `strftime('%Y-%m-%dT%H:00:00', ...)`
/// grouping, which is only correct for fixed-offset timezones whose offset
/// is a whole number of hours. Non-whole-hour fixed offsets (+05:30,
/// +05:45, +09:30) and IANA zones (DST transitions) must use the
/// `daily_usage` materialized path instead.
pub fn use_sql_fast_path(rtz: &ReportingTimezone) -> bool {
    rtz.is_fixed_offset() && rtz.is_whole_hour_offset()
}

/// Parse a `YYYY-MM-DD` date string to epoch milliseconds (start of day, UTC).
pub fn parse_date_to_ms(date: &str) -> Result<i64> {
    let dt = Date::parse(
        date,
        &time::format_description::parse("[year]-[month]-[day]")?,
    )
    .map_err(|e| anyhow::anyhow!("invalid date '{}': {}", date, e))?;
    let start = dt.with_time(Time::MIDNIGHT).assume_offset(UtcOffset::UTC);
    Ok(start.unix_timestamp() * 1000)
}

/// Parse a `YYYY-MM-DD` date string to epoch milliseconds (start of *next* day, UTC).
/// This gives an exclusive end bound for a date-inclusive range.
pub fn parse_date_to_ms_exclusive(date: &str) -> Result<i64> {
    let dt = Date::parse(
        date,
        &time::format_description::parse("[year]-[month]-[day]")?,
    )
    .map_err(|e| anyhow::anyhow!("invalid date '{}': {}", date, e))?;
    let end = (dt + time::Duration::DAY)
        .with_time(Time::MIDNIGHT)
        .assume_offset(UtcOffset::UTC);
    Ok(end.unix_timestamp() * 1000)
}

/// Format a `Date` as `YYYY-MM-DD`.
pub fn format_date(date: Date) -> String {
    let year = date.year();
    let month: u8 = date.month().into();
    let day = date.day();
    format!("{:04}-{:02}-{:02}", year, month, day)
}

/// Compute the resolved time range for a given preset and reference date.
///
/// `rtz` is the reporting timezone. `year`, `month`, `day` form the
/// civil date in that timezone. `week_starts_on` is only meaningful for
/// the `Week` preset.
pub fn resolve_range(
    rtz: &ReportingTimezone,
    year: i32,
    month: u8,
    day: u8,
    preset: RangePresetDto,
    week_starts_on: WeekdayIndexDto,
) -> ResolvedRange {
    // Build the civil date string for today.
    let today_str = format!("{:04}-{:02}-{:02}", year, month, day);
    let today_date =
        Date::from_calendar_date(year, Month::try_from(month).expect("valid month"), day)
            .expect("valid date");

    match preset {
        RangePresetDto::Day => {
            let start_ms = rtz
                .civil_date_to_utc_start_ms(&today_str)
                .expect("valid start");
            let next_date = rtz.next_civil_date(&today_str).expect("valid next date");
            let end_ms = rtz
                .civil_date_to_utc_start_ms(&next_date)
                .expect("valid end");
            ResolvedRange {
                start_ms,
                end_ms,
                start_date: today_str.clone(),
                end_date: today_str,
            }
        }
        RangePresetDto::Week => {
            let current_wd = today_date.weekday().number_days_from_sunday(); // 0=Sun
            let target_wd = week_starts_on.value(); // 0=Sun
            let days_back = (current_wd as i8 - target_wd as i8 + 7) as i64 % 7;
            let week_start_date = today_date - Duration::days(days_back);
            let week_end_date = week_start_date + Duration::days(7);
            let start_date_str = format_date(week_start_date);
            let end_date_str = format_date(week_end_date);
            let start_ms = rtz
                .civil_date_to_utc_start_ms(&start_date_str)
                .expect("valid start");
            let end_ms = rtz
                .civil_date_to_utc_start_ms(&end_date_str)
                .expect("valid end");
            let end_inclusive = format_date(week_start_date + Duration::days(6));
            ResolvedRange {
                start_ms,
                end_ms,
                start_date: start_date_str,
                end_date: end_inclusive,
            }
        }
        RangePresetDto::Month => {
            let month_start =
                Date::from_calendar_date(year, today_date.month(), 1).expect("valid month start");
            let next_month_date = if today_date.month() == Month::December {
                Date::from_calendar_date(year + 1, Month::January, 1).expect("valid january start")
            } else {
                Date::from_calendar_date(year, today_date.month().next(), 1)
                    .expect("valid next month start")
            };
            let start_date_str = format_date(month_start);
            let end_date_str = format_date(next_month_date);
            let start_ms = rtz
                .civil_date_to_utc_start_ms(&start_date_str)
                .expect("valid start");
            let end_ms = rtz
                .civil_date_to_utc_start_ms(&end_date_str)
                .expect("valid end");
            let days_in_month = (next_month_date - month_start).whole_days() as i64;
            let end_inclusive = format_date(month_start + Duration::days(days_in_month - 1));
            ResolvedRange {
                start_ms,
                end_ms,
                start_date: start_date_str,
                end_date: end_inclusive,
            }
        }
        RangePresetDto::Year => {
            let year_start =
                Date::from_calendar_date(year, Month::January, 1).expect("valid year start");
            let year_end = Date::from_calendar_date(year + 1, Month::January, 1)
                .expect("valid next year start");
            let start_date_str = format_date(year_start);
            let end_date_str = format_date(year_end);
            let start_ms = rtz
                .civil_date_to_utc_start_ms(&start_date_str)
                .expect("valid start");
            let end_ms = rtz
                .civil_date_to_utc_start_ms(&end_date_str)
                .expect("valid end");
            let end_inclusive =
                Date::from_calendar_date(year, Month::December, 31).expect("valid dec 31");
            ResolvedRange {
                start_ms,
                end_ms,
                start_date: start_date_str,
                end_date: format_date(end_inclusive),
            }
        }
    }
}

/// Compute the heatmap window: 12 calendar months back from the given date.
///
/// Returns `HeatmapWindow` with `start` and `end_inclusive` date strings.
/// Example: `heatmap_window_from_date(2026, 5, 20)` returns
/// `start = "2025-05-20"`, `end_inclusive = "2026-05-20"`.
pub fn heatmap_window_from_date(year: i32, month: u8, day: u8) -> HeatmapWindow {
    let month_enum = Month::try_from(month).expect("valid month (1-12)");
    let date = Date::from_calendar_date(year, month_enum, day).expect("valid date");

    // Go back 12 calendar months (1 year, same month/day).
    let start_year = year - 1;
    let start_date = Date::from_calendar_date(start_year, month_enum, day).unwrap_or_else(|_| {
        // If the day does not exist in the target month (e.g., Feb 29
        // in a non-leap year), fall back to the last day of that month.
        // Use the first of the next month minus one day.
        let next_month = if month_enum == Month::December {
            Month::January
        } else {
            month_enum.next()
        };
        let next_year = if month_enum == Month::December {
            year
        } else {
            start_year
        };
        Date::from_calendar_date(next_year, next_month, 1).expect("valid next month")
            - Duration::DAY
    });

    HeatmapWindow {
        start: format_date(start_date),
        end_inclusive: format_date(date),
    }
}

/// Generate daily heatmap windows in local time for the 12 months up to today.
///
/// Each window represents one local-timezone day with midnight-to-midnight
/// boundaries expressed as UTC epoch milliseconds.
pub fn heatmap_days(rtz: &ReportingTimezone) -> Vec<HeatmapDayWindow> {
    // Parse today's date for calendar arithmetic using the time crate.
    let (today_year, today_month, today_day) = rtz.today_civil_ymd().expect("today ymd");

    // Compute start date: 1 year ago, same month/day (with fallback for
    // invalid dates like Feb 29 in non-leap year).
    let month_enum = Month::try_from(today_month).expect("valid month");
    let start_year = today_year - 1;
    let start_date =
        Date::from_calendar_date(start_year, month_enum, today_day).unwrap_or_else(|_| {
            let next_month = if month_enum == Month::December {
                Month::January
            } else {
                month_enum.next()
            };
            let next_year = if month_enum == Month::December {
                today_year
            } else {
                start_year
            };
            Date::from_calendar_date(next_year, next_month, 1).expect("valid next month")
                - Duration::DAY
        });

    let today_date =
        Date::from_calendar_date(today_year, month_enum, today_day).expect("valid today date");

    let days = (today_date - start_date).whole_days() + 1; // inclusive range
    let mut windows = Vec::with_capacity(days as usize);

    for i in 0..days {
        let day = start_date + Duration::days(i);
        let date_str = format_date(day);
        let start_ms = rtz
            .civil_date_to_utc_start_ms(&date_str)
            .expect("valid start");
        let next_date = rtz.next_civil_date(&date_str).expect("valid next date");
        let end_ms = rtz
            .civil_date_to_utc_start_ms(&next_date)
            .expect("valid end");
        windows.push(HeatmapDayWindow {
            date: date_str,
            start_ms,
            end_ms,
        });
    }

    windows
}

/// Generate trend buckets for the given preset.
///
/// Returns a vector of `TrendBucketWindow` values, each representing one
/// subdivision of the range (hourly for day, daily for week/month, monthly
/// for year).
pub fn trend_buckets(
    rtz: &ReportingTimezone,
    preset: RangePresetDto,
    week_starts_on: WeekdayIndexDto,
) -> Vec<TrendBucketWindow> {
    match preset {
        RangePresetDto::Day => trend_buckets_hourly(rtz),
        RangePresetDto::Week => trend_buckets_daily_week(rtz, week_starts_on),
        RangePresetDto::Month => trend_buckets_daily_month(rtz),
        RangePresetDto::Year => trend_buckets_monthly(rtz),
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn now_ms_value() -> i64 {
    busytok_domain::now_ms()
}

fn is_current_bucket_ms(start_ms: i64, end_ms: i64) -> bool {
    let now_ms = now_ms_value();
    now_ms >= start_ms && now_ms < end_ms
}

fn trend_buckets_hourly(rtz: &ReportingTimezone) -> Vec<TrendBucketWindow> {
    let today_str = rtz.today_local_date().expect("today local date");
    let (year, month, day) = rtz.today_civil_ymd().expect("today ymd");

    let day_start_ms = rtz
        .civil_date_to_utc_start_ms(&today_str)
        .expect("valid start");
    // Next-day boundary via civil date arithmetic — DST-aware.
    let next_day_ms = rtz
        .next_civil_date(&today_str)
        .ok()
        .and_then(|d| rtz.civil_date_to_utc_start_ms(&d).ok())
        .unwrap_or_else(|| day_start_ms + 24 * 3_600_000);

    let mut buckets = Vec::with_capacity(24);
    for hour in 0..24 {
        let start_ms = day_start_ms + (hour as i64 * 3_600_000);
        let raw_end_ms = day_start_ms + ((hour + 1) as i64 * 3_600_000);
        // Cap the final bucket to the actual next-day boundary.
        let end_ms = if hour == 23 { next_day_ms } else { raw_end_ms };
        buckets.push(TrendBucketWindow {
            start_ms,
            end_ms,
            key: format!("{:04}-{:02}-{:02}T{:02}:00:00", year, month, day, hour),
            is_current: is_current_bucket_ms(start_ms, end_ms),
        });
    }
    buckets
}

fn trend_buckets_daily_week(
    rtz: &ReportingTimezone,
    week_starts_on: WeekdayIndexDto,
) -> Vec<TrendBucketWindow> {
    let (year, month, day) = rtz.today_civil_ymd().expect("today ymd");
    let today_date =
        Date::from_calendar_date(year, Month::try_from(month).expect("valid month"), day)
            .expect("valid date");

    let current_wd = today_date.weekday().number_days_from_sunday(); // 0=Sun
    let target_wd = week_starts_on.value(); // 0=Sun
    let days_back = (current_wd as i8 - target_wd as i8 + 7) as i64 % 7;
    let week_start_date = today_date - Duration::days(days_back);

    let mut buckets = Vec::with_capacity(7);
    for day_offset in 0..7 {
        let day_date = week_start_date + Duration::days(day_offset);
        let date_str = format_date(day_date);
        let start_ms = rtz
            .civil_date_to_utc_start_ms(&date_str)
            .expect("valid start");
        let next_date = rtz.next_civil_date(&date_str).expect("valid next date");
        let end_ms = rtz
            .civil_date_to_utc_start_ms(&next_date)
            .expect("valid end");
        buckets.push(TrendBucketWindow {
            start_ms,
            end_ms,
            key: date_str,
            is_current: is_current_bucket_ms(start_ms, end_ms),
        });
    }
    buckets
}

fn trend_buckets_daily_month(rtz: &ReportingTimezone) -> Vec<TrendBucketWindow> {
    let (year, month, _day) = rtz.today_civil_ymd().expect("today ymd");
    let month_enum = Month::try_from(month).expect("valid month");

    let month_start = Date::from_calendar_date(year, month_enum, 1).expect("valid month start");
    let next_month_date = if month_enum == Month::December {
        Date::from_calendar_date(year + 1, Month::January, 1).expect("valid january start")
    } else {
        Date::from_calendar_date(year, month_enum.next(), 1).expect("valid next month start")
    };

    let days_in_month = (next_month_date - month_start).whole_days();

    let mut buckets = Vec::with_capacity(days_in_month as usize);
    for day_offset in 0..days_in_month {
        let day_date = month_start + Duration::days(day_offset);
        let date_str = format_date(day_date);
        let start_ms = rtz
            .civil_date_to_utc_start_ms(&date_str)
            .expect("valid start");
        let next_date = rtz.next_civil_date(&date_str).expect("valid next date");
        let end_ms = rtz
            .civil_date_to_utc_start_ms(&next_date)
            .expect("valid end");
        buckets.push(TrendBucketWindow {
            start_ms,
            end_ms,
            key: date_str,
            is_current: is_current_bucket_ms(start_ms, end_ms),
        });
    }
    buckets
}

fn trend_buckets_monthly(rtz: &ReportingTimezone) -> Vec<TrendBucketWindow> {
    let (year, _month, _day) = rtz.today_civil_ymd().expect("today ymd");

    let mut buckets = Vec::with_capacity(12);
    let mut current = Month::January;

    for _ in 0..12 {
        let next_m = current.next();
        let month_end_year = if next_m == Month::January {
            year + 1
        } else {
            year
        };

        let month_start = Date::from_calendar_date(year, current, 1).expect("valid month start");
        let month_end =
            Date::from_calendar_date(month_end_year, next_m, 1).expect("valid month end");

        let start_date_str = format_date(month_start);
        let end_date_str = format_date(month_end);
        let start_ms = rtz
            .civil_date_to_utc_start_ms(&start_date_str)
            .expect("valid start");
        let end_ms = rtz
            .civil_date_to_utc_start_ms(&end_date_str)
            .expect("valid end");

        let month_num: u8 = current.into();
        buckets.push(TrendBucketWindow {
            start_ms,
            end_ms,
            key: format!("{:04}-{:02}", year, month_num),
            is_current: is_current_bucket_ms(start_ms, end_ms),
        });

        current = next_m;
    }
    buckets
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── heatmap_window_from_date ──────────────────────────────────────

    #[test]
    fn heatmap_window_uses_calendar_month_boundary() {
        let window = heatmap_window_from_date(2026, 5, 20);
        assert_eq!(window.start, "2025-05-20");
        assert_eq!(window.end_inclusive, "2026-05-20");
    }

    #[test]
    fn heatmap_window_january() {
        // Jan 15 -> previous year Jan 15
        let window = heatmap_window_from_date(2026, 1, 15);
        assert_eq!(window.start, "2025-01-15");
        assert_eq!(window.end_inclusive, "2026-01-15");
    }

    // ── trend_buckets (day) ───────────────────────────────────────────

    #[test]
    fn trend_buckets_day_returns_24_hourly_buckets() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let buckets = trend_buckets(&rtz, RangePresetDto::Day, WeekdayIndexDto::SUNDAY);
        assert_eq!(buckets.len(), 24);

        // First bucket starts at midnight UTC today
        let today_str = rtz.today_local_date().unwrap();
        let midnight = rtz.civil_date_to_utc_start_ms(&today_str).unwrap();
        assert_eq!(buckets[0].start_ms, midnight);

        // Exactly one bucket is current
        let current_count = buckets.iter().filter(|b| b.is_current).count();
        assert_eq!(current_count, 1);
    }

    #[test]
    fn trend_buckets_day_has_stable_keys() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let (y, m, d) = rtz.today_civil_ymd().unwrap();
        let buckets = trend_buckets(&rtz, RangePresetDto::Day, WeekdayIndexDto::SUNDAY);
        for (i, b) in buckets.iter().enumerate() {
            assert_eq!(
                b.key,
                format!("{:04}-{:02}-{:02}T{:02}:00:00", y, m, d, i),
                "key should be derivable from start_ms"
            );
        }
    }

    // ── trend_buckets (week) ──────────────────────────────────────────

    #[test]
    fn trend_buckets_week_returns_7_daily_buckets() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let buckets = trend_buckets(&rtz, RangePresetDto::Week, WeekdayIndexDto::MONDAY);
        assert_eq!(buckets.len(), 7);

        // Exactly one bucket is current
        let current_count = buckets.iter().filter(|b| b.is_current).count();
        assert_eq!(current_count, 1);
    }

    #[test]
    fn trend_buckets_week_has_stable_keys() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let buckets = trend_buckets(&rtz, RangePresetDto::Week, WeekdayIndexDto::MONDAY);
        // Keys should be date strings in YYYY-MM-DD format
        for b in &buckets {
            assert!(
                b.key.len() == 10 && b.key.chars().nth(4) == Some('-'),
                "key '{}' should be YYYY-MM-DD",
                b.key
            );
        }
    }

    // ── trend_buckets (month) ─────────────────────────────────────────

    #[test]
    fn trend_buckets_month_returns_one_bucket_per_day() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let buckets = trend_buckets(&rtz, RangePresetDto::Month, WeekdayIndexDto::SUNDAY);
        // Current month could have 28-31 days
        assert!(buckets.len() >= 28);
        assert!(buckets.len() <= 31);

        // Exactly one bucket is current
        let current_count = buckets.iter().filter(|b| b.is_current).count();
        assert_eq!(current_count, 1);
    }

    // ── trend_buckets (year) ──────────────────────────────────────────

    #[test]
    fn trend_buckets_year_returns_12_monthly_buckets() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let buckets = trend_buckets(&rtz, RangePresetDto::Year, WeekdayIndexDto::SUNDAY);
        assert_eq!(buckets.len(), 12);

        // Exactly one bucket is current
        let current_count = buckets.iter().filter(|b| b.is_current).count();
        assert_eq!(current_count, 1);
    }

    #[test]
    fn trend_buckets_year_has_stable_keys() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let (year, _, _) = rtz.today_civil_ymd().unwrap();
        let buckets = trend_buckets(&rtz, RangePresetDto::Year, WeekdayIndexDto::SUNDAY);
        assert_eq!(buckets.len(), 12);
        let expected_keys: Vec<String> =
            (1..=12).map(|m| format!("{:04}-{:02}", year, m)).collect();
        for (i, b) in buckets.iter().enumerate() {
            assert_eq!(b.key, expected_keys[i]);
        }
    }

    // ── timezone boundaries ───────────────────────────────────────────

    #[test]
    fn trend_buckets_respects_positive_offset() {
        let rtz = ReportingTimezone::parse("+08:00").unwrap();
        let buckets = trend_buckets(&rtz, RangePresetDto::Day, WeekdayIndexDto::SUNDAY);
        assert_eq!(buckets.len(), 24);
        // Verify all 24 buckets exist
        for b in &buckets {
            assert!(!b.key.is_empty());
        }
    }

    #[test]
    fn trend_buckets_respects_iana_timezone() {
        let rtz = ReportingTimezone::parse("Asia/Shanghai").unwrap();
        let buckets = trend_buckets(&rtz, RangePresetDto::Day, WeekdayIndexDto::SUNDAY);
        assert_eq!(buckets.len(), 24);
    }

    // ── resolve_range ─────────────────────────────────────────────────

    #[test]
    fn resolve_day_range_is_midnight_to_midnight() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let range = resolve_range(
            &rtz,
            2026,
            5,
            20,
            RangePresetDto::Day,
            WeekdayIndexDto::SUNDAY,
        );
        let day_start = rtz.civil_date_to_utc_start_ms("2026-05-20").unwrap();
        let day_end = rtz.civil_date_to_utc_start_ms("2026-05-21").unwrap();
        assert_eq!(range.start_ms, day_start);
        assert_eq!(range.end_ms, day_end);
    }

    #[test]
    fn resolve_week_range_starts_on_monday() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        // 2026-05-20 is a Wednesday
        let range = resolve_range(
            &rtz,
            2026,
            5,
            20,
            RangePresetDto::Week,
            WeekdayIndexDto::MONDAY,
        );
        let week_start = rtz.civil_date_to_utc_start_ms("2026-05-18").unwrap();
        let week_end = rtz.civil_date_to_utc_start_ms("2026-05-25").unwrap();
        assert_eq!(range.start_ms, week_start);
        assert_eq!(range.end_ms, week_end);
        assert_eq!(range.start_date, "2026-05-18");
        assert_eq!(range.end_date, "2026-05-24");
    }

    #[test]
    fn resolve_month_range() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let range = resolve_range(
            &rtz,
            2026,
            5,
            20,
            RangePresetDto::Month,
            WeekdayIndexDto::SUNDAY,
        );
        let month_start = rtz.civil_date_to_utc_start_ms("2026-05-01").unwrap();
        let month_end = rtz.civil_date_to_utc_start_ms("2026-06-01").unwrap();
        assert_eq!(range.start_ms, month_start);
        assert_eq!(range.end_ms, month_end);
        assert_eq!(range.start_date, "2026-05-01");
        assert_eq!(range.end_date, "2026-05-31");
    }

    #[test]
    fn resolve_year_range() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let range = resolve_range(
            &rtz,
            2026,
            5,
            20,
            RangePresetDto::Year,
            WeekdayIndexDto::SUNDAY,
        );
        let year_start = rtz.civil_date_to_utc_start_ms("2026-01-01").unwrap();
        let year_end = rtz.civil_date_to_utc_start_ms("2027-01-01").unwrap();
        assert_eq!(range.start_ms, year_start);
        assert_eq!(range.end_ms, year_end);
        assert_eq!(range.start_date, "2026-01-01");
        assert_eq!(range.end_date, "2026-12-31");
    }

    // ── parse_timezone ────────────────────────────────────────────────

    #[test]
    fn parse_timezone_utc() {
        let rtz = parse_timezone("UTC").unwrap();
        assert_eq!(rtz.canonical_name(), "UTC");
    }

    #[test]
    fn parse_timezone_positive_offset() {
        let rtz = parse_timezone("+08:00").unwrap();
        assert_eq!(rtz.canonical_name(), "+08:00");
    }

    #[test]
    fn parse_timezone_negative_offset() {
        let rtz = parse_timezone("-05:00").unwrap();
        assert_eq!(rtz.canonical_name(), "-05:00");
    }

    #[test]
    fn parse_timezone_accepts_iana_names() {
        let rtz = parse_timezone("Asia/Shanghai").unwrap();
        assert_eq!(rtz.canonical_name(), "Asia/Shanghai");
        assert!(rtz.is_iana());
    }

    #[test]
    fn parse_timezone_rejects_empty_string() {
        let err = parse_timezone("").unwrap_err();
        assert!(err.to_string().contains("unsupported timezone"));
    }

    #[test]
    fn parse_timezone_rejects_bad_format() {
        let err = parse_timezone("invalid").unwrap_err();
        assert!(err.to_string().contains("unsupported timezone"));
    }

    // ── format_date ───────────────────────────────────────────────────

    #[test]
    fn format_date_produces_iso_string() {
        use time::macros::date;
        let d = date!(2026 - 05 - 20);
        assert_eq!(format_date(d), "2026-05-20");
    }

    #[test]
    fn format_date_pads_to_two_digits() {
        use time::macros::date;
        let d = date!(2026 - 01 - 03);
        assert_eq!(format_date(d), "2026-01-03");
    }

    // ── heatmap_days ───────────────────────────────────────────────────

    #[test]
    fn heatmap_days_returns_local_timezone_windows() {
        let rtz = ReportingTimezone::parse("+08:00").unwrap();
        let windows = heatmap_days(&rtz);
        // 12 months = roughly 365-366 days
        assert!(windows.len() >= 365);
        assert!(windows.len() <= 366);

        // Each window should span exactly 24 hours in UTC ms
        for w in &windows {
            assert_eq!(w.end_ms - w.start_ms, 24 * 60 * 60 * 1000);
        }
    }

    #[test]
    fn heatmap_days_utc_timezone() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let windows = heatmap_days(&rtz);
        assert!(!windows.is_empty());
    }

    // ── use_sql_fast_path ───────────────────────────────────────────

    #[test]
    fn use_sql_fast_path_for_whole_hour_fixed_offset() {
        for tz in ["UTC", "+00:00", "+08:00", "-05:00", "-10:00", "+14:00"] {
            let rtz = ReportingTimezone::parse(tz).unwrap();
            assert!(
                use_sql_fast_path(&rtz),
                "{tz} is whole-hour fixed → should use SQL fast path"
            );
        }
    }

    #[test]
    fn no_sql_fast_path_for_non_whole_hour_fixed_offset() {
        // Regression for the bug where +05:30 / +05:45 were routed through
        // the hour-bucket fast path despite their local-day boundary
        // (18:30 / 18:15 UTC of the previous day) splitting UTC hour buckets.
        // These must route through daily_usage instead.
        for tz in ["+05:30", "+05:45", "-03:30", "+09:30", "+12:45"] {
            let rtz = ReportingTimezone::parse(tz).unwrap();
            assert!(
                !use_sql_fast_path(&rtz),
                "{tz} is non-whole-hour fixed → must NOT use SQL fast path"
            );
        }
    }

    #[test]
    fn no_sql_fast_path_for_iana() {
        let rtz = ReportingTimezone::parse("Asia/Shanghai").unwrap();
        assert!(!use_sql_fast_path(&rtz));
    }
}
