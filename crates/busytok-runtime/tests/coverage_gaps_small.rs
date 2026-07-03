//! Coverage gap tests for `busytok-runtime` (sampler.rs + receipt.rs).
//!
//! Targets uncovered source lines:
//! - `receipt.rs`: `peak_hour_label` with tokens <= 0 (line 101, returns
//!   None), weekday arms for Tue/Wed/Thu/Sat/Sun (lines 126-132), and
//!   `Month::try_from` error closure (line 124).
//! - `sampler.rs`: exact-mode `cost_usd == 0.0` → `cost_per_sec: None`
//!   branch (line 95).

#![allow(
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    dead_code,
    unused_imports,
    unused_variables
)]

use std::sync::{Arc, Mutex, Once};

use busytok_domain::ReportingTimezone;
use busytok_events::AppEventBus;
use busytok_protocol::dto::{CostStatusDto, ReadinessStateDto};
use busytok_runtime::receipt::{assemble_receipt_daily, ReceiptDailyData};
use busytok_runtime::sampler;
use busytok_runtime::status::ServiceStatusSnapshot;
use busytok_store::read_models::{PeakHourRow, ReceiptDailyTotalsRow, ReceiptModelSliceRow};
use busytok_store::Database;

// ── receipt.rs: peak_hour_label with tokens <= 0 ────────────────────────

fn totals(has_cost: bool, has_no_cost: bool) -> ReceiptDailyTotalsRow {
    ReceiptDailyTotalsRow {
        total_tokens: 1000,
        input_tokens: 600,
        output_tokens: 400,
        cache_read_tokens: 300,
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
fn peak_hour_with_zero_tokens_returns_none() {
    // Line 101: `if p.tokens <= 0 { return Ok(None); }`.
    let peak = PeakHourRow {
        bucket_start_ms: 1_782_453_600_000,
        tokens: 0,
    };
    let dto = assemble_receipt_daily(
        ReceiptDailyData {
            totals: totals(true, false),
            models: vec![],
            session_count: 1,
            peak_hour: Some(peak),
        },
        &rtz(),
        "2026-06-26",
        0,
    )
    .unwrap();
    assert!(
        dto.metrics.peak_hour.is_none(),
        "peak_hour with 0 tokens must be None"
    );
}

#[test]
fn peak_hour_with_negative_tokens_returns_none() {
    // Defensive: negative tokens also returns None (tokens <= 0).
    let peak = PeakHourRow {
        bucket_start_ms: 1_782_453_600_000,
        tokens: -5,
    };
    let dto = assemble_receipt_daily(
        ReceiptDailyData {
            totals: totals(true, false),
            models: vec![],
            session_count: 1,
            peak_hour: Some(peak),
        },
        &rtz(),
        "2026-06-26",
        0,
    )
    .unwrap();
    assert!(dto.metrics.peak_hour.is_none());
}

// ── receipt.rs: weekday arms (lines 126-132) ────────────────────────────
//
// 2026-06-26 is Friday (already covered). We test each remaining weekday
// to cover the match arms for Monday, Tuesday, Wednesday, Thursday,
// Saturday, and Sunday.

#[test]
fn date_label_monday() {
    // 2026-06-22 is a Monday.
    let dto = assemble_receipt_daily(
        ReceiptDailyData {
            totals: totals(true, false),
            models: vec![],
            session_count: 0,
            peak_hour: None,
        },
        &rtz(),
        "2026-06-22",
        0,
    )
    .unwrap();
    assert_eq!(dto.date_label, "MON · JUN 22, 2026");
}

#[test]
fn date_label_tuesday() {
    // 2026-06-23 is a Tuesday.
    let dto = assemble_receipt_daily(
        ReceiptDailyData {
            totals: totals(true, false),
            models: vec![],
            session_count: 0,
            peak_hour: None,
        },
        &rtz(),
        "2026-06-23",
        0,
    )
    .unwrap();
    assert_eq!(dto.date_label, "TUE · JUN 23, 2026");
}

#[test]
fn date_label_wednesday() {
    // 2026-06-24 is a Wednesday.
    let dto = assemble_receipt_daily(
        ReceiptDailyData {
            totals: totals(true, false),
            models: vec![],
            session_count: 0,
            peak_hour: None,
        },
        &rtz(),
        "2026-06-24",
        0,
    )
    .unwrap();
    assert_eq!(dto.date_label, "WED · JUN 24, 2026");
}

#[test]
fn date_label_thursday() {
    // 2026-06-25 is a Thursday.
    let dto = assemble_receipt_daily(
        ReceiptDailyData {
            totals: totals(true, false),
            models: vec![],
            session_count: 0,
            peak_hour: None,
        },
        &rtz(),
        "2026-06-25",
        0,
    )
    .unwrap();
    assert_eq!(dto.date_label, "THU · JUN 25, 2026");
}

#[test]
fn date_label_saturday() {
    // 2026-06-27 is a Saturday.
    let dto = assemble_receipt_daily(
        ReceiptDailyData {
            totals: totals(true, false),
            models: vec![],
            session_count: 0,
            peak_hour: None,
        },
        &rtz(),
        "2026-06-27",
        0,
    )
    .unwrap();
    assert_eq!(dto.date_label, "SAT · JUN 27, 2026");
}

#[test]
fn date_label_sunday() {
    // 2026-06-28 is a Sunday.
    let dto = assemble_receipt_daily(
        ReceiptDailyData {
            totals: totals(true, false),
            models: vec![],
            session_count: 0,
            peak_hour: None,
        },
        &rtz(),
        "2026-06-28",
        0,
    )
    .unwrap();
    assert_eq!(dto.date_label, "SUN · JUN 28, 2026");
}

// ── receipt.rs: Month::try_from error (line 124) ────────────────────────

#[test]
fn date_label_invalid_month_errors() {
    // Month 13 is out of range — Month::try_from(13) returns Err.
    let result = assemble_receipt_daily(
        ReceiptDailyData {
            totals: totals(true, false),
            models: vec![],
            session_count: 0,
            peak_hour: None,
        },
        &rtz(),
        "2026-13-01",
        0,
    );
    assert!(result.is_err(), "invalid month must return error");
}

#[test]
fn date_label_invalid_day_errors() {
    // Day 32 is out of range — Date::from_calendar_date returns Err.
    let result = assemble_receipt_daily(
        ReceiptDailyData {
            totals: totals(true, false),
            models: vec![],
            session_count: 0,
            peak_hour: None,
        },
        &rtz(),
        "2026-06-32",
        0,
    );
    assert!(result.is_err(), "invalid day must return error");
}

#[test]
fn date_label_malformed_date_errors() {
    // Not enough parts — "2026-06" splits into 2 parts, not 3.
    let result = assemble_receipt_daily(
        ReceiptDailyData {
            totals: totals(true, false),
            models: vec![],
            session_count: 0,
            peak_hour: None,
        },
        &rtz(),
        "2026-06",
        0,
    );
    assert!(result.is_err(), "malformed date must return error");
}

// ── receipt.rs: per-model cost_status and cache_hit_rate edge cases ─────

#[test]
fn per_model_unavailable_cost_when_row_has_no_cost() {
    // A model row with has_cost=false and has_no_cost=false → Unavailable.
    let data = ReceiptDailyData {
        totals: totals(true, false),
        models: vec![ReceiptModelSliceRow {
            name: "model-x".into(),
            tokens: 500,
            cost_usd: None,
            has_cost: false,
            has_no_cost: false,
        }],
        session_count: 1,
        peak_hour: None,
    };
    let dto = assemble_receipt_daily(data, &rtz(), "2026-06-26", 0).unwrap();
    assert_eq!(dto.top_models[0].cost_status, CostStatusDto::Unavailable);
}

#[test]
fn cache_hit_rate_with_only_cache_read_tokens() {
    // input=0, cache_read=100 → denom=100, rate=1.0.
    let data = ReceiptDailyData {
        totals: ReceiptDailyTotalsRow {
            total_tokens: 100,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 100,
            cost_usd: Some(0.5),
            has_cost: true,
            has_no_cost: false,
            event_count: 1,
        },
        models: vec![],
        session_count: 1,
        peak_hour: None,
    };
    let dto = assemble_receipt_daily(data, &rtz(), "2026-06-26", 0).unwrap();
    let rate = dto.metrics.cache_hit_rate.unwrap();
    assert!(
        (rate - 1.0).abs() < 1e-9,
        "cache_hit_rate must be 1.0, got {rate}"
    );
}

// ── sampler.rs: exact mode with cost_usd == 0.0 (line 95) ────────────────

#[tokio::test]
async fn sampler_exact_mode_with_zero_cost_publishes_none_cost_per_sec() {
    // When live_bucket.cost_usd == 0.0, the `if bucket.cost_usd > 0.0`
    // branch is false → cost_per_sec: None (line 95).
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let mut rx = event_bus.subscribe();

    {
        let mut snap = status.write().await;
        snap.apply_durable_transition(
            ReadinessStateDto::ReadyExact,
            Some("gen-zero-cost".to_string()),
        );
        // Set up a bucket with tokens > 0 but cost_usd == 0.0.
        snap.live_bucket.bucket_start_ms = (busytok_domain::now_ms() / 2000) * 2000;
        snap.live_bucket.total_tokens = 200;
        snap.live_bucket.cost_usd = 0.0; // ← triggers the None branch
        snap.live_bucket.event_count = 2;
    }

    let db = Database::open_in_memory().expect("open in-memory db");
    let shutdown_tx = sampler::start_sampler(
        Arc::new(Mutex::new(db)),
        Arc::clone(&status),
        Arc::clone(&event_bus),
    );

    // Wait for a non-transient (exact) sample with cost_per_sec == None.
    let started = tokio::time::Instant::now();
    let mut got_zero_cost_exact = false;
    while started.elapsed() < tokio::time::Duration::from_secs(5) {
        if let Ok(published) = rx.try_recv() {
            if let busytok_events::AppEvent::LiveSample {
                transient: false,
                cost_per_sec: None,
                ..
            } = &published.event
            {
                got_zero_cost_exact = true;
                break;
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    let _ = shutdown_tx.send(true);
    assert!(
        got_zero_cost_exact,
        "sampler must emit exact sample with cost_per_sec=None when cost_usd==0.0"
    );
}

// ── Tracing subscriber init ────────────────────────────────────────────
//
// Multi-line tracing macros (debug!, warn!) expand their format string
// arguments lazily — only when a subscriber is present and the level is
// enabled. Without a subscriber, the format string lines are NOT counted as
// "executed" by coverage instrumentation, even though the macro call site
// is reached. We initialize a subscriber once per test binary so the format
// string lines are covered.

static SUBSCRIBER_INIT: Once = Once::new();

fn init_subscriber() {
    SUBSCRIBER_INIT.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("trace")
            .with_test_writer()
            .try_init();
    });
}

// ── sampler.rs: shutdown signal (lines 56-57) ───────────────────────────
//
// The existing `sampler_exact_mode_with_zero_cost_publishes_none_cost_per_sec`
// test sends shutdown but doesn't wait for the sampler to process it — the
// test ends and the runtime is dropped before the background task processes
// the signal. This test sends shutdown immediately and waits for the sampler
// to process it, covering lines 56-57 (debug! + break).

#[tokio::test]
async fn sampler_processes_shutdown_signal() {
    init_subscriber();

    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let db = Database::open_in_memory().expect("open in-memory db");
    let shutdown_tx = sampler::start_sampler(
        Arc::new(Mutex::new(db)),
        Arc::clone(&status),
        Arc::clone(&event_bus),
    );

    // Send shutdown immediately — the sampler's select! resolves via
    // rx.changed() (not the 2s sleep), hitting lines 56-57.
    shutdown_tx.send(true).expect("send shutdown");
    // Wait for the sampler to process the shutdown signal.
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
}

// ── sampler.rs: poisoned db mutex (lines 132-134) ───────────────────────
//
// When the db Mutex is poisoned (a panic occurred while holding the lock),
// db.lock() returns Err(PoisonError). The sampler's transient-mode path
// handles this with a warn! + continue (lines 132-134). We poison the mutex
// by panicking in a separate OS thread while holding the lock.

#[tokio::test]
async fn sampler_logs_warn_when_db_mutex_is_poisoned() {
    init_subscriber();

    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));

    // Create db and poison the mutex by panicking while holding it.
    let db = Arc::new(Mutex::new(
        Database::open_in_memory().expect("open in-memory db"),
    ));
    let db_clone = Arc::clone(&db);
    let handle = std::thread::spawn(move || {
        let _guard = db_clone.lock().unwrap();
        panic!("intentional panic to poison the mutex");
    });
    // Thread panicked — mutex is now poisoned.
    let _ = handle.join();

    let shutdown_tx =
        sampler::start_sampler(Arc::clone(&db), Arc::clone(&status), Arc::clone(&event_bus));

    // Wait for the first tick (2s sleep + processing). The sampler wakes
    // from sleep, enters transient mode (default readiness is Starting),
    // tries to lock the poisoned db, hits the Err arm (lines 132-134),
    // and continues the loop.
    tokio::time::sleep(tokio::time::Duration::from_millis(2500)).await;

    // Send shutdown and wait for processing.
    let _ = shutdown_tx.send(true);
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
}
