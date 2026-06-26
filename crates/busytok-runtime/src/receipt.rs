//! Pure assembly of the daily receipt DTO from store rows. Kept separate from
//! the supervisor so the shaping logic (cost-status mapping, cache-hit rate,
//! peak-hour TZ labelling, date label) is unit-testable without a DB.

use anyhow::Result;
use busytok_domain::ReportingTimezone;
use busytok_protocol::dto::{
    CostStatusDto, ReceiptBrandDto, ReceiptDailyDto, ReceiptMetricsDto, ReceiptModelSliceDto,
    ReceiptPeakHourDto,
};
use busytok_store::read_models::{PeakHourRow, ReceiptDailyTotalsRow, ReceiptModelSliceRow};

use crate::ui_models;

/// The four store reads bundled for one `receipt.daily` call.
pub struct ReceiptDailyData {
    pub totals: ReceiptDailyTotalsRow,
    pub models: Vec<ReceiptModelSliceRow>,
    pub session_count: i64,
    pub peak_hour: Option<PeakHourRow>,
}

const BRAND_NAME: &str = "BUSYTOK";
const BRAND_TAGLINE: &str = "AI CODING · TOKEN RECEIPT";
const BRAND_GITHUB: &str = "github.com/BalianWang/busytok";

/// Map store rows + the reporting timezone into the wire DTO. Pure: no I/O.
pub fn assemble_receipt_daily(
    data: ReceiptDailyData,
    rtz: &ReportingTimezone,
    date: &str,
    now_ms: i64,
) -> Result<ReceiptDailyDto> {
    let ReceiptDailyData {
        totals,
        models,
        session_count,
        peak_hour,
    } = data;

    let cost_status = ui_models::cost_status(totals.has_cost, totals.has_no_cost);
    let cache_hit_rate = cache_hit_rate(totals.cache_read_tokens, totals.input_tokens);
    let peak_hour_dto = peak_hour_label(peak_hour, rtz)?;

    let top_models = models
        .into_iter()
        .map(|m| ReceiptModelSliceDto {
            name: m.name,
            tokens: m.tokens,
            cost_usd: m.cost_usd,
            cost_status: ui_models::cost_status(m.has_cost, m.has_no_cost),
        })
        .collect();

    Ok(ReceiptDailyDto {
        date: date.to_string(),
        date_label: format_date_label(date)?,
        timezone: rtz.canonical_name().to_string(),
        metrics: ReceiptMetricsDto {
            total_tokens: totals.total_tokens,
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
            cache_read_tokens: totals.cache_read_tokens,
            cache_creation_tokens: totals.cache_creation_tokens,
            cache_hit_rate,
            cost_usd: totals.cost_usd,
            cost_status,
            event_count: totals.event_count,
            session_count,
            peak_hour: peak_hour_dto,
        },
        top_models,
        brand: ReceiptBrandDto {
            name: BRAND_NAME.to_string(),
            tagline: BRAND_TAGLINE.to_string(),
            github: BRAND_GITHUB.to_string(),
            generated_at_ms: now_ms,
        },
    })
}

fn cache_hit_rate(cache_read: i64, input: i64) -> Option<f64> {
    let denom = input + cache_read;
    if denom <= 0 {
        return None;
    }
    Some(cache_read as f64 / denom as f64)
}

/// Convert the peak UTC hour bucket to a reporting-TZ wall-clock label.
/// NOTE: hour = (bucket - local_midnight) / 3_600_000. Exact for whole-hour
/// offsets and for IANA zones except the single DST-transition hour per year
/// (where it may be ±1); acceptable for a secondary receipt metric.
fn peak_hour_label(
    peak: Option<PeakHourRow>,
    rtz: &ReportingTimezone,
) -> Result<Option<ReceiptPeakHourDto>> {
    let Some(p) = peak else {
        return Ok(None);
    };
    if p.tokens <= 0 {
        return Ok(None);
    }
    let local_date = rtz.local_date_for_timestamp_ms(p.bucket_start_ms)?;
    let local_midnight_ms = rtz.civil_date_to_utc_start_ms(&local_date)?;
    let local_hour = ((p.bucket_start_ms - local_midnight_ms) / 3_600_000).rem_euclid(24);
    Ok(Some(ReceiptPeakHourDto {
        label: format!("{local_hour:02}:00"),
        tokens: p.tokens,
    }))
}

/// "YYYY-MM-DD" → "FRI · JUN 26, 2026".
fn format_date_label(date: &str) -> Result<String> {
    let parts: Vec<&str> = date.split('-').collect();
    anyhow::ensure!(parts.len() == 3, "invalid date: {date}");
    let year: i32 = parts[0].parse()?;
    let month: u8 = parts[1].parse()?;
    let day: u8 = parts[2].parse()?;
    use time::{Date, Month};
    let d = Date::from_calendar_date(
        year,
        Month::try_from(month).map_err(|e| anyhow::anyhow!("{e}"))?,
        day,
    )?;
    let wd = match d.weekday() {
        time::Weekday::Monday => "MON",
        time::Weekday::Tuesday => "TUE",
        time::Weekday::Wednesday => "WED",
        time::Weekday::Thursday => "THU",
        time::Weekday::Friday => "FRI",
        time::Weekday::Saturday => "SAT",
        time::Weekday::Sunday => "SUN",
    };
    let mon = [
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ][(month as usize) - 1];
    Ok(format!("{wd} · {mon} {day}, {year}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn totals(has_cost: bool, has_no_cost: bool) -> ReceiptDailyTotalsRow {
        ReceiptDailyTotalsRow {
            total_tokens: 1000,
            input_tokens: 600,
            output_tokens: 400,
            cache_read_tokens: 300,
            cache_creation_tokens: 50,
            cost_usd: if has_cost { Some(2.5) } else { None },
            has_cost,
            has_no_cost,
            event_count: 9,
        }
    }

    fn rtz() -> ReportingTimezone {
        ReportingTimezone::parse("Asia/Shanghai").unwrap()
    }

    #[test]
    fn maps_aggregate_cost_status_and_cache_hit_rate() {
        let dto = assemble_receipt_daily(
            ReceiptDailyData {
                totals: totals(true, false),
                models: vec![],
                session_count: 3,
                peak_hour: None,
            },
            &rtz(),
            "2026-06-26",
            1_000,
        )
        .unwrap();
        assert_eq!(dto.metrics.cost_status, CostStatusDto::Exact);
        assert!((dto.metrics.cache_hit_rate.unwrap() - (300.0 / 900.0)).abs() < 1e-9);
        assert_eq!(dto.metrics.session_count, 3);
        assert!(dto.metrics.peak_hour.is_none());
        assert_eq!(dto.date_label, "FRI · JUN 26, 2026");
        assert_eq!(dto.brand.name, "BUSYTOK");
    }

    #[test]
    fn partial_cost_when_any_row_lacks_cost() {
        let dto = assemble_receipt_daily(
            ReceiptDailyData {
                totals: totals(true, true),
                models: vec![],
                session_count: 0,
                peak_hour: None,
            },
            &rtz(),
            "2026-06-26",
            0,
        )
        .unwrap();
        assert_eq!(dto.metrics.cost_status, CostStatusDto::Partial);
    }

    #[test]
    fn empty_day_is_unavailable_no_cache_rate() {
        let dto = assemble_receipt_daily(
            ReceiptDailyData {
                totals: ReceiptDailyTotalsRow {
                    total_tokens: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                    cost_usd: None,
                    has_cost: false,
                    has_no_cost: false,
                    event_count: 0,
                },
                models: vec![],
                session_count: 0,
                peak_hour: None,
            },
            &rtz(),
            "2026-06-26",
            0,
        )
        .unwrap();
        assert_eq!(dto.metrics.cost_status, CostStatusDto::Unavailable);
        assert!(dto.metrics.cache_hit_rate.is_none());
    }

    #[test]
    fn per_model_cost_status_independent_of_aggregate() {
        let data = ReceiptDailyData {
            totals: totals(true, false),
            models: vec![ReceiptModelSliceRow {
                name: "model-b".into(),
                tokens: 500,
                cost_usd: None,
                has_cost: false,
                has_no_cost: true,
                event_count: 1,
            }],
            session_count: 1,
            peak_hour: None,
        };
        let dto = assemble_receipt_daily(data, &rtz(), "2026-06-26", 0).unwrap();
        assert_eq!(dto.metrics.cost_status, CostStatusDto::Exact); // aggregate has full cost
        assert_eq!(dto.top_models[0].cost_status, CostStatusDto::Unavailable); // that row has none
    }

    #[test]
    fn cache_hit_rate_none_when_denominator_zero() {
        assert!(cache_hit_rate(0, 0).is_none());
        assert!(cache_hit_rate(10, -10).is_none()); // guarded against negative
    }
}
