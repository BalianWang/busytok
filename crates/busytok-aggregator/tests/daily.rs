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
use busytok_domain::ReportingTimezone;

// ── date_from_timestamp_ms edge cases ──────────────────────────────────

#[test]
fn date_from_epoch_zero_is_1970_01_01_utc() {
    use busytok_aggregator::daily::date_from_timestamp_ms;
    let date = date_from_timestamp_ms(0, &ReportingTimezone::parse("UTC").unwrap()).unwrap();
    assert_eq!(date, "1970-01-01");
}

#[test]
fn date_from_epoch_zero_with_timezone_offset() {
    use busytok_aggregator::daily::date_from_timestamp_ms;
    // 1970-01-01 00:00 UTC = 1970-01-01 08:00 +08:00 (same date)
    let date_plus8 =
        date_from_timestamp_ms(0, &ReportingTimezone::parse("+08:00").unwrap()).unwrap();
    assert_eq!(date_plus8, "1970-01-01");
    // 1970-01-01 00:00 UTC = 1969-12-31 19:00 -05:00 (previous date)
    let date_minus5 =
        date_from_timestamp_ms(0, &ReportingTimezone::parse("-05:00").unwrap()).unwrap();
    assert_eq!(date_minus5, "1969-12-31");
}

#[test]
fn date_from_large_positive_timestamp() {
    use busytok_aggregator::daily::date_from_timestamp_ms;
    // Year 2038 boundary test
    let ts_ms = 4_000_000_000_000i64; // around 2096-09-22
    let date = date_from_timestamp_ms(ts_ms, &ReportingTimezone::parse("UTC").unwrap()).unwrap();
    // Just verify it doesn't panic and returns a plausible date string
    assert!(date.starts_with("2096-"), "expected 2096-x-x, got {date}");
}

#[test]
fn date_with_max_positive_offset() {
    use busytok_aggregator::daily::date_from_timestamp_ms;
    // +14:00 (largest civil timezone offset, e.g. Kiribati)
    let ts_ms = 1_700_000_000_000i64;
    let date = date_from_timestamp_ms(ts_ms, &ReportingTimezone::parse("+14:00").unwrap()).unwrap();
    // Just verify it parses and returns a valid date
    assert!(date.contains('-'), "expected YYYY-MM-DD, got {date}");
    assert_eq!(date.len(), 10);
}

#[test]
fn date_with_max_negative_offset() {
    use busytok_aggregator::daily::date_from_timestamp_ms;
    // -12:00 (largest negative civil timezone offset, e.g. Baker Island)
    let ts_ms = 1_700_000_000_000i64;
    let date = date_from_timestamp_ms(ts_ms, &ReportingTimezone::parse("-12:00").unwrap()).unwrap();
    assert!(date.contains('-'), "expected YYYY-MM-DD, got {date}");
    assert_eq!(date.len(), 10);
}
