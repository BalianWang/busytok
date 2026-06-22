//! Reporting timezone abstraction with IANA support via jiff.
//!
//! `ReportingTimezone` is the single point of truth for converting UTC
//! timestamps into civil dates for aggregation boundaries. It does NOT
//! affect raw event storage — all events remain UTC `timestamp_ms`.

use anyhow::{bail, Context, Result};
use jiff::tz::TimeZone;
use time::UtcOffset;

/// The reporting timezone for all aggregation and display boundaries.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportingTimezone {
    canonical: String,
    inner: TimezoneKind,
}

#[derive(Debug, Clone, PartialEq)]
enum TimezoneKind {
    /// IANA timezone with full DST support.
    Iana(TimeZone),
    /// Fixed UTC offset — no DST transitions.
    Fixed(UtcOffset),
}

impl ReportingTimezone {
    /// Parse from a timezone string. Accepts:
    ///   - IANA names: "Asia/Shanghai", "America/New_York", "Europe/Berlin"
    ///   - "UTC"
    ///   - Fixed offsets: "+08:00", "-05:30", "+05:45"
    ///   - "local" — resolved to system IANA name immediately
    pub fn parse(input: &str) -> Result<Self> {
        match input {
            "UTC" => Ok(Self {
                canonical: "UTC".to_string(),
                inner: TimezoneKind::Fixed(UtcOffset::UTC),
            }),
            "local" => {
                let iana = detect_system_iana_timezone();
                let tz = TimeZone::get(&iana).map_err(|e| {
                    anyhow::anyhow!("failed to parse system IANA timezone '{}': {}", iana, e)
                })?;
                Ok(Self {
                    canonical: iana,
                    inner: TimezoneKind::Iana(tz),
                })
            }
            other => {
                // Try fixed offset first: ±HH:MM, ±HHMM, ±HH
                if let Ok(offset) = parse_fixed_offset(other) {
                    return Ok(Self {
                        canonical: other.to_string(),
                        inner: TimezoneKind::Fixed(offset),
                    });
                }
                // Try IANA name
                let tz = TimeZone::get(other)
                    .map_err(|e| anyhow::anyhow!("unsupported timezone '{other}': {e}"))?;
                Ok(Self {
                    canonical: other.to_string(),
                    inner: TimezoneKind::Iana(tz),
                })
            }
        }
    }

    /// A UTC-anchored instance.
    pub fn utc() -> Self {
        Self {
            canonical: "UTC".to_string(),
            inner: TimezoneKind::Fixed(UtcOffset::UTC),
        }
    }

    /// The canonical string form, suitable for persistence.
    pub fn canonical_name(&self) -> &str {
        &self.canonical
    }

    /// True for IANA timezones (DST-aware, uses daily_usage materialized path).
    pub fn is_iana(&self) -> bool {
        matches!(&self.inner, TimezoneKind::Iana(_))
    }

    /// True for fixed-offset timezones (offset is constant, no DST).
    ///
    /// Note: this is necessary but not sufficient for the SQL hour-bucket fast
    /// path. See [`Self::is_whole_hour_offset`].
    pub fn is_fixed_offset(&self) -> bool {
        matches!(&self.inner, TimezoneKind::Fixed(_))
    }

    /// True when the offset (for fixed-offset zones) is a whole number of hours
    /// from UTC — e.g. UTC, `+08:00`, `-05:00`. Returns `false` for IANA zones
    /// (which go through the `daily_usage` path regardless, because DST makes
    /// the offset time-dependent) and for non-whole-hour fixed offsets like
    /// `+05:30`, `+05:45`, `+09:30`.
    ///
    /// The SQL hour-bucket fast path stores rows keyed by UTC hour boundaries.
    /// That only aligns with local-day boundaries when the offset is a whole
    /// number of hours; a `+05:30` day starts at `18:30 UTC` of the previous
    /// day, splitting the `18:00–19:00 UTC` hour bucket across two local days,
    /// so hour-bucket attribution is impossible. Non-whole-hour fixed offsets
    /// must route through `daily_usage` (event-level date attribution).
    pub fn is_whole_hour_offset(&self) -> bool {
        match &self.inner {
            TimezoneKind::Fixed(offset) => offset.whole_seconds().rem_euclid(3_600) == 0,
            TimezoneKind::Iana(_) => false,
        }
    }

    /// Convert a UTC timestamp (ms) to a local date string "YYYY-MM-DD".
    pub fn local_date_for_timestamp_ms(&self, ts_ms: i64) -> Result<String> {
        match &self.inner {
            TimezoneKind::Fixed(offset) => {
                let secs = ts_ms / 1000;
                let utc_dt = time::OffsetDateTime::from_unix_timestamp(secs)
                    .with_context(|| format!("timestamp_ms {ts_ms} out of range"))?;
                let local_dt = utc_dt.to_offset(*offset);
                Ok(format!(
                    "{:04}-{:02}-{:02}",
                    local_dt.year(),
                    local_dt.month() as u8,
                    local_dt.day()
                ))
            }
            TimezoneKind::Iana(tz) => {
                let ts = jiff::Timestamp::from_millisecond(ts_ms)
                    .map_err(|e| anyhow::anyhow!("invalid timestamp {ts_ms}: {e}"))?;
                let zdt = ts.to_zoned(tz.clone());
                Ok(zdt.strftime("%Y-%m-%d").to_string())
            }
        }
    }

    /// Get the current civil date in this timezone as "YYYY-MM-DD".
    pub fn today_local_date(&self) -> Result<String> {
        match &self.inner {
            TimezoneKind::Fixed(offset) => {
                let now = time::OffsetDateTime::now_utc().to_offset(*offset);
                Ok(format!(
                    "{:04}-{:02}-{:02}",
                    now.year(),
                    now.month() as u8,
                    now.day()
                ))
            }
            TimezoneKind::Iana(tz) => {
                let now = jiff::Zoned::now().with_time_zone(tz.clone());
                Ok(now.strftime("%Y-%m-%d").to_string())
            }
        }
    }

    /// The current year, month, and day-of-month in this timezone.
    pub fn today_civil_ymd(&self) -> Result<(i32, u8, u8)> {
        match &self.inner {
            TimezoneKind::Fixed(offset) => {
                let now = time::OffsetDateTime::now_utc().to_offset(*offset);
                Ok((now.year(), now.month() as u8, now.day()))
            }
            TimezoneKind::Iana(tz) => {
                let now = jiff::Zoned::now().with_time_zone(tz.clone());
                Ok((i32::from(now.year()), now.month() as u8, now.day() as u8))
            }
        }
    }

    /// Convert a civil date string "YYYY-MM-DD" to UTC epoch ms at midnight local.
    pub fn civil_date_to_utc_start_ms(&self, date: &str) -> Result<i64> {
        match &self.inner {
            TimezoneKind::Fixed(offset) => {
                // Parse date manually to avoid needing `time` parsing feature
                let civil: jiff::civil::Date = date
                    .parse()
                    .map_err(|e| anyhow::anyhow!("invalid date '{date}': {e}"))?;
                let dt = time::Date::from_calendar_date(
                    i32::from(civil.year()),
                    time::Month::try_from(civil.month() as u8)
                        .with_context(|| format!("invalid month in date '{date}'"))?,
                    civil.day() as u8,
                )
                .with_context(|| format!("invalid date '{date}'"))?;
                let start = dt.with_time(time::Time::MIDNIGHT).assume_offset(*offset);
                Ok(start.unix_timestamp() * 1000)
            }
            TimezoneKind::Iana(tz) => {
                let civil: jiff::civil::Date = date
                    .parse()
                    .map_err(|e| anyhow::anyhow!("invalid date '{date}': {e}"))?;
                let zdt = civil
                    .at(0, 0, 0, 0)
                    .to_zoned(tz.clone())
                    .map_err(|e| anyhow::anyhow!("ambiguous civil time '{date}': {e}"))?;
                Ok(zdt.timestamp().as_millisecond())
            }
        }
    }

    /// Get the next civil date after `date` ("YYYY-MM-DD").
    pub fn next_civil_date(&self, date: &str) -> Result<String> {
        let civil: jiff::civil::Date = date
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid date '{date}': {e}"))?;
        let next = civil
            .tomorrow()
            .map_err(|e| anyhow::anyhow!("failed to compute next date after '{date}': {e}"))?;
        Ok(format!(
            "{:04}-{:02}-{:02}",
            next.year(),
            next.month(),
            next.day()
        ))
    }

    /// Generate daily boundary windows for a range of civil days.
    pub fn daily_boundaries(&self, from_date: &str, days: i64) -> Result<Vec<DayBoundary>> {
        let mut boundaries = Vec::with_capacity(days as usize);
        let mut cursor = from_date.to_string();
        for _ in 0..days {
            let start_ms = self.civil_date_to_utc_start_ms(&cursor)?;
            let next_date = self.next_civil_date(&cursor)?;
            let end_ms = self.civil_date_to_utc_start_ms(&next_date)?;
            boundaries.push(DayBoundary {
                date: cursor.clone(),
                start_ms,
                end_ms,
            });
            cursor = next_date;
        }
        Ok(boundaries)
    }
}

/// A single civil-day window with UTC epoch boundaries.
#[derive(Debug, Clone)]
pub struct DayBoundary {
    pub date: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

// ── System timezone detection ─────────────────────────────────────────

/// Detect the system timezone as an IANA name.
///
/// Fallback is "UTC" only — never degrades to a fixed offset.
pub fn detect_system_iana_timezone() -> String {
    let tz = TimeZone::system();
    if tz.is_unknown() {
        tracing::warn!(
            event_code = "timezone.system_detection_failed",
            "jiff::TimeZone::system() returned unknown, falling back to UTC"
        );
        return "UTC".to_string();
    }
    match tz.iana_name() {
        Some(name) => name.to_string(),
        None => {
            tracing::warn!(
                event_code = "timezone.system_iana_unavailable",
                "system timezone has no IANA name, falling back to UTC"
            );
            "UTC".to_string()
        }
    }
}

/// Resolve "local" to a canonical IANA name via system detection.
pub fn resolve_local_timezone() -> String {
    detect_system_iana_timezone()
}

// ── Internal ──────────────────────────────────────────────────────────

/// Parse a fixed offset string like "+08:00", "-05:30", or "+05:45".
fn parse_fixed_offset(s: &str) -> Result<UtcOffset> {
    if s.len() < 3 {
        bail!("too short for fixed offset: '{s}'");
    }
    let sign: i8 = if s.starts_with('+') {
        1
    } else if s.starts_with('-') {
        -1
    } else {
        bail!("fixed offset must start with + or -: '{s}'");
    };
    let rest = &s[1..];
    let (hours_str, minutes_str): (&str, &str) = if rest.contains(':') {
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() != 2 {
            bail!("invalid fixed offset format: '{s}'");
        }
        (parts[0], parts[1])
    } else if rest.len() == 4 {
        (&rest[0..2], &rest[2..4])
    } else if rest.len() == 2 {
        (rest, "00")
    } else {
        bail!("invalid fixed offset format: '{s}'");
    };
    let hours: i8 = hours_str
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid hours in offset: '{s}'"))?;
    let minutes: i8 = minutes_str
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid minutes in offset: '{s}'"))?;
    if !(0..=23).contains(&hours.abs()) || !(0..=59).contains(&minutes.abs()) {
        bail!("fixed offset out of range: '{s}'");
    }
    UtcOffset::from_hms(sign * hours, sign * minutes, 0)
        .map_err(|e| anyhow::anyhow!("invalid offset '{s}': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse ─────────────────────────────────────────────────────

    #[test]
    fn parse_utc() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        assert_eq!(rtz.canonical_name(), "UTC");
        assert!(rtz.is_fixed_offset());
        assert!(!rtz.is_iana());
    }

    #[test]
    fn parse_iana_shanghai() {
        let rtz = ReportingTimezone::parse("Asia/Shanghai").unwrap();
        assert_eq!(rtz.canonical_name(), "Asia/Shanghai");
        assert!(rtz.is_iana());
        assert!(!rtz.is_fixed_offset());
    }

    #[test]
    fn parse_iana_new_york() {
        let rtz = ReportingTimezone::parse("America/New_York").unwrap();
        assert_eq!(rtz.canonical_name(), "America/New_York");
        assert!(rtz.is_iana());
    }

    #[test]
    fn parse_iana_berlin() {
        let rtz = ReportingTimezone::parse("Europe/Berlin").unwrap();
        assert_eq!(rtz.canonical_name(), "Europe/Berlin");
        assert!(rtz.is_iana());
    }

    #[test]
    fn parse_fixed_offset_positive_whole_hour() {
        let rtz = ReportingTimezone::parse("+08:00").unwrap();
        assert_eq!(rtz.canonical_name(), "+08:00");
        assert!(rtz.is_fixed_offset());
    }

    #[test]
    fn parse_fixed_offset_negative_whole_hour() {
        let rtz = ReportingTimezone::parse("-05:00").unwrap();
        assert_eq!(rtz.canonical_name(), "-05:00");
        assert!(rtz.is_fixed_offset());
    }

    #[test]
    fn parse_fixed_offset_half_hour() {
        let rtz = ReportingTimezone::parse("+05:30").unwrap();
        assert_eq!(rtz.canonical_name(), "+05:30");
        assert!(rtz.is_fixed_offset());
    }

    #[test]
    fn parse_fixed_offset_45_minutes() {
        let rtz = ReportingTimezone::parse("+05:45").unwrap();
        assert_eq!(rtz.canonical_name(), "+05:45");
        assert!(rtz.is_fixed_offset());
    }

    #[test]
    fn parse_local_resolves_to_iana_or_utc() {
        let rtz = ReportingTimezone::parse("local").unwrap();
        assert!(!rtz.canonical_name().is_empty());
        assert_ne!(rtz.canonical_name(), "local");
    }

    // ── is_whole_hour_offset ───────────────────────────────────────

    #[test]
    fn is_whole_hour_offset_true_for_utc_and_whole_hour_fixed_offsets() {
        for tz in ["UTC", "+00:00", "+08:00", "-05:00", "+14:00", "-10:00"] {
            let rtz = ReportingTimezone::parse(tz).unwrap();
            assert!(
                rtz.is_whole_hour_offset(),
                "{tz} should be whole-hour (fast-path eligible)"
            );
        }
    }

    #[test]
    fn is_whole_hour_offset_false_for_non_whole_hour_fixed_offsets() {
        // Local midnight for these falls mid-UTC-hour, so hour-bucket
        // attribution is impossible — they must use the daily_usage path.
        for tz in ["+05:30", "+05:45", "-03:30", "+09:30", "+12:45"] {
            let rtz = ReportingTimezone::parse(tz).unwrap();
            assert!(rtz.is_fixed_offset(), "{tz} should still be a fixed offset");
            assert!(
                !rtz.is_whole_hour_offset(),
                "{tz} should NOT be whole-hour (must use daily_usage)"
            );
        }
    }

    #[test]
    fn is_whole_hour_offset_false_for_iana_even_when_currently_whole_hour() {
        // IANA zones route through daily_usage regardless of their current
        // offset, because DST makes the offset time-dependent.
        for tz in ["Asia/Shanghai", "Europe/London", "America/New_York"] {
            let rtz = ReportingTimezone::parse(tz).unwrap();
            assert!(
                !rtz.is_whole_hour_offset(),
                "{tz} is IANA and must not claim whole-hour even if currently whole-hour"
            );
        }
    }

    #[test]
    fn parse_invalid_returns_err() {
        assert!(ReportingTimezone::parse("invalid").is_err());
        assert!(ReportingTimezone::parse("").is_err());
    }

    // ── local_date_for_timestamp_ms ────────────────────────────────

    #[test]
    fn local_date_utc_timezone() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        // 2026-06-12 00:00:00 UTC = 1781222400000 ms
        let ts_ms = 1781222400000i64;
        let date = rtz.local_date_for_timestamp_ms(ts_ms).unwrap();
        assert_eq!(date, "2026-06-12");
    }

    #[test]
    fn local_date_shanghai_timezone() {
        let rtz = ReportingTimezone::parse("Asia/Shanghai").unwrap();
        // 2026-06-11 16:00:00 UTC = 2026-06-12 00:00:00 CST
        let ts_ms = 1781193600000i64;
        let date = rtz.local_date_for_timestamp_ms(ts_ms).unwrap();
        assert_eq!(date, "2026-06-12");
    }

    #[test]
    fn local_date_different_timezones_same_utc() {
        // 2026-06-11 16:00:00 UTC = 2026-06-12 00:00:00 CST
        let ts_ms = 1781193600000i64;
        let shanghai_date = ReportingTimezone::parse("Asia/Shanghai")
            .unwrap()
            .local_date_for_timestamp_ms(ts_ms)
            .unwrap();
        let utc_date = ReportingTimezone::parse("UTC")
            .unwrap()
            .local_date_for_timestamp_ms(ts_ms)
            .unwrap();
        assert_eq!(shanghai_date, "2026-06-12");
        assert_eq!(utc_date, "2026-06-11");
    }

    // ── DST transitions ────────────────────────────────────────────

    #[test]
    fn dst_spring_forward_america_new_york() {
        let rtz = ReportingTimezone::parse("America/New_York").unwrap();
        // 2026-03-08: spring forward at 02:00 EST → 03:00 EDT
        // Before: 2026-03-08 06:59 UTC = 01:59 EST (Mar 8)
        // After:  2026-03-08 07:00 UTC = 03:00 EDT (Mar 8, skipped 02:00)
        let before_ts = 1772953140000i64;
        let after_ts = 1772953200000i64;
        let date_before = rtz.local_date_for_timestamp_ms(before_ts).unwrap();
        let date_after = rtz.local_date_for_timestamp_ms(after_ts).unwrap();
        assert_eq!(date_before, "2026-03-08");
        assert_eq!(date_after, "2026-03-08");
    }

    #[test]
    fn dst_fall_back_america_new_york() {
        let rtz = ReportingTimezone::parse("America/New_York").unwrap();
        // 2026-11-01: fall back, 02:00 EDT → 01:00 EST
        // Nov 1 05:00 UTC = Nov 1 01:00 EDT (still Nov 1)
        let ts_ms = 1793509200000i64;
        let date = rtz.local_date_for_timestamp_ms(ts_ms).unwrap();
        assert_eq!(date, "2026-11-01");
    }

    #[test]
    fn asia_shanghai_no_dst_consistent() {
        let rtz = ReportingTimezone::parse("Asia/Shanghai").unwrap();
        // Verify dates return without error (no DST transitions)
        let june = rtz.local_date_for_timestamp_ms(1781452800000).unwrap();
        let jan = rtz.local_date_for_timestamp_ms(1767225600000).unwrap();
        assert!(!june.is_empty());
        assert!(!jan.is_empty());
    }

    // ── civil_date_to_utc_start_ms ─────────────────────────────────

    #[test]
    fn civil_date_to_utc_start_utc() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let ms = rtz.civil_date_to_utc_start_ms("2026-06-12").unwrap();
        // 2026-06-12 00:00:00 UTC = 1781222400000 ms
        assert_eq!(ms, 1781222400000i64);
    }

    #[test]
    fn civil_date_to_utc_start_shanghai() {
        let rtz = ReportingTimezone::parse("Asia/Shanghai").unwrap();
        let ms = rtz.civil_date_to_utc_start_ms("2026-06-12").unwrap();
        // 2026-06-12 00:00:00 CST = 2026-06-11 16:00:00 UTC = 1781193600000 ms
        assert_eq!(ms, 1781193600000i64);
    }

    // ── next_civil_date ────────────────────────────────────────────

    #[test]
    fn next_civil_date_normal() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        assert_eq!(rtz.next_civil_date("2026-06-12").unwrap(), "2026-06-13");
    }

    #[test]
    fn next_civil_date_month_boundary() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        assert_eq!(rtz.next_civil_date("2026-12-31").unwrap(), "2027-01-01");
    }

    #[test]
    fn next_civil_date_leap_year() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        assert_eq!(rtz.next_civil_date("2028-02-28").unwrap(), "2028-02-29");
    }

    // ── today_civil_ymd ────────────────────────────────────────────

    #[test]
    fn today_civil_ymd_returns_valid_date() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let (y, m, d) = rtz.today_civil_ymd().unwrap();
        assert!(y >= 2026);
        assert!((1..=12).contains(&m));
        assert!((1..=31).contains(&d));
    }

    // ── daily_boundaries ───────────────────────────────────────────

    #[test]
    fn daily_boundaries_single_day() {
        let rtz = ReportingTimezone::parse("UTC").unwrap();
        let boundaries = rtz.daily_boundaries("2026-06-12", 1).unwrap();
        assert_eq!(boundaries.len(), 1);
        assert_eq!(boundaries[0].date, "2026-06-12");
        assert_eq!(
            boundaries[0].end_ms - boundaries[0].start_ms,
            24 * 3600 * 1000
        );
    }

    #[test]
    fn daily_boundaries_respects_dst() {
        let rtz = ReportingTimezone::parse("America/New_York").unwrap();
        let boundaries = rtz.daily_boundaries("2026-03-08", 1).unwrap();
        assert_eq!(boundaries.len(), 1);
        let day_length_hours = (boundaries[0].end_ms - boundaries[0].start_ms) / 3600_000;
        assert_eq!(
            day_length_hours, 23,
            "spring-forward day should be 23h, got {day_length_hours}h"
        );
    }
}
