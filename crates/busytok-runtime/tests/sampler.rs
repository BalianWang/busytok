use std::sync::{Arc, Mutex};
use std::time::Duration;

use busytok_domain::now_ms;
use busytok_events::AppEventBus;
use busytok_protocol::dto::ReadinessStateDto;
use busytok_runtime::sampler;
use busytok_runtime::status::ServiceStatusSnapshot;
use busytok_store::Database;

#[tokio::test]
async fn sampler_publishes_transient_samples_before_first_exact_generation() {
    let db = Database::open_in_memory().expect("open in-memory db");
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let mut rx = event_bus.subscribe();

    // Status starts in "Starting" readiness with no active generation —
    // the sampler must fall back to transient mode.
    assert_eq!(status.read().await.readiness, ReadinessStateDto::Starting);
    assert!(status.read().await.active_generation_id.is_none());

    let shutdown_tx =
        sampler::start_sampler(Arc::new(Mutex::new(db)), Arc::clone(&status), event_bus);

    let started = tokio::time::Instant::now();
    let mut got_transient_sample = false;
    while started.elapsed() < tokio::time::Duration::from_secs(5) {
        if let Ok(published) = rx.try_recv() {
            if let busytok_events::AppEvent::LiveSample { transient, .. } = &published.event {
                if *transient {
                    got_transient_sample = true;
                    break;
                }
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    let _ = shutdown_tx.send(true);
    assert!(
        got_transient_sample,
        "sampler should emit transient LiveSample when no promoted generation exists"
    );
}

#[tokio::test]
async fn sampler_publishes_exact_samples_from_live_bucket_when_writer_accumulates() {
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let mut rx = event_bus.subscribe();

    {
        let mut snap = status.write().await;
        snap.apply_durable_transition(ReadinessStateDto::ReadyExact, Some("gen-1".to_string()));
        snap.live_bucket.bucket_start_ms = (busytok_domain::now_ms() / 2000) * 2000;
        snap.live_bucket.total_tokens = 200;
        snap.live_bucket.cost_usd = 0.01;
        snap.live_bucket.event_count = 3;
    }

    let db = Database::open_in_memory().expect("open in-memory db");
    let shutdown_tx =
        sampler::start_sampler(Arc::new(Mutex::new(db)), Arc::clone(&status), event_bus);

    let started = tokio::time::Instant::now();
    let mut got_exact_sample = false;
    while started.elapsed() < tokio::time::Duration::from_secs(5) {
        if let Ok(published) = rx.try_recv() {
            if let busytok_events::AppEvent::LiveSample { transient, .. } = &published.event {
                if !*transient {
                    got_exact_sample = true;
                    break;
                }
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    let _ = shutdown_tx.send(true);
    assert!(
        got_exact_sample,
        "sampler should emit exact LiveSample from in-memory LiveBucket"
    );
}

#[tokio::test]
async fn sampler_publishes_zero_sample_when_live_bucket_is_empty() {
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let mut rx = event_bus.subscribe();

    {
        let mut snap = status.write().await;
        snap.apply_durable_transition(ReadinessStateDto::ReadyExact, Some("gen-1".to_string()));
    }

    let db = Database::open_in_memory().expect("open in-memory db");
    let shutdown_tx =
        sampler::start_sampler(Arc::new(Mutex::new(db)), Arc::clone(&status), event_bus);

    let started = tokio::time::Instant::now();
    let mut got_zero_sample = false;
    while started.elapsed() < tokio::time::Duration::from_secs(5) {
        if let Ok(published) = rx.try_recv() {
            if let busytok_events::AppEvent::LiveSample {
                tokens_per_sec,
                transient,
                ..
            } = &published.event
            {
                if !*transient && *tokens_per_sec == 0.0 {
                    got_zero_sample = true;
                    break;
                }
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    let _ = shutdown_tx.send(true);
    assert!(
        got_zero_sample,
        "sampler should emit zero-token exact sample when LiveBucket is empty"
    );
}

#[tokio::test]
async fn sampler_computes_per_second_rates_from_usage_events() {
    let db = Database::open_in_memory().expect("open in-memory db");
    let now = now_ms();
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let mut rx = event_bus.subscribe();

    // Insert usage events within the last 4s.
    {
        let conn = db.conn();
        for i in 0..4 {
            let ts = now - 1000 * i;
            conn.execute(
                "INSERT INTO usage_events (id, timestamp_ms, agent, client_kind, \
                 source_file_id, source_path, source_line, source_offset_start, \
                 source_offset_end, session_id, project_hash, model, total_tokens, \
                 raw_event_hash, created_at_ms, updated_at_ms) \
                 VALUES (?1, ?2, 'claude', 'claude_code', 'sf-1', '/tmp/test.jsonl', \
                 1, 0, 0, 'sess-sampler-test', '', '', 100, 'hash-dummy', ?2, ?2)",
                rusqlite::params![format!("evt-sampler-{}", i), ts],
            )
            .unwrap();
        }
    }

    let shutdown_tx =
        sampler::start_sampler(Arc::new(Mutex::new(db)), Arc::clone(&status), event_bus);

    let started = tokio::time::Instant::now();
    let mut got_sample = false;
    while started.elapsed() < tokio::time::Duration::from_secs(5) {
        if let Ok(published) = rx.try_recv() {
            if matches!(published.event, busytok_events::AppEvent::LiveSample { .. }) {
                got_sample = true;
                break;
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    let _ = shutdown_tx.send(true);
    assert!(
        got_sample,
        "sampler should emit a LiveSample event within 5 seconds"
    );
}

#[tokio::test]
async fn sampler_snapshots_live_bucket_without_resetting_during_writer_accumulation() {
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let mut rx = event_bus.subscribe();

    {
        let mut snap = status.write().await;
        snap.apply_durable_transition(ReadinessStateDto::ReadyExact, Some("gen-race".to_string()));
    }

    let db = Database::open_in_memory().unwrap();
    let shutdown_tx =
        sampler::start_sampler(Arc::new(Mutex::new(db)), Arc::clone(&status), event_bus);

    // Simulate writer accumulating events between sampler ticks.
    for round in 0..5 {
        {
            let mut snap = status.write().await;
            let now = busytok_domain::now_ms();
            let window = (now / 2000) * 2000;
            snap.live_bucket.bucket_start_ms = window;
            snap.live_bucket.total_tokens += 100;
            snap.live_bucket.cost_usd += 0.001;
            snap.live_bucket.event_count += 1;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let _ = shutdown_tx.send(true);

    // Sampler should have published at least one exact (non-transient) sample.
    let mut got_exact = false;
    while let Ok(published) = rx.try_recv() {
        if let busytok_events::AppEvent::LiveSample {
            transient: false, ..
        } = &published.event
        {
            got_exact = true;
        }
    }
    assert!(
        got_exact,
        "sampler should have read LiveBucket accumulated by writer"
    );
}

#[tokio::test]
async fn sampler_publishes_zero_sample_when_live_bucket_is_expired() {
    let status = Arc::new(tokio::sync::RwLock::new(ServiceStatusSnapshot::new()));
    let event_bus = Arc::new(AppEventBus::new(64));
    let mut rx = event_bus.subscribe();

    // Set up a stale bucket: bucket_start_ms is from an old window
    // (far enough in the past that it's < current_window).
    {
        let mut snap = status.write().await;
        snap.apply_durable_transition(ReadinessStateDto::ReadyExact, Some("gen-1".to_string()));
        snap.live_bucket.bucket_start_ms = 1000; // very old — will be < current_window
        snap.live_bucket.total_tokens = 500;
        snap.live_bucket.cost_usd = 0.05;
        snap.live_bucket.event_count = 5;
    }

    let db = Database::open_in_memory().expect("open in-memory db");
    let shutdown_tx =
        sampler::start_sampler(Arc::new(Mutex::new(db)), Arc::clone(&status), event_bus);

    let started = tokio::time::Instant::now();
    let mut got_zero_sample = false;
    while started.elapsed() < tokio::time::Duration::from_secs(5) {
        if let Ok(published) = rx.try_recv() {
            if let busytok_events::AppEvent::LiveSample {
                tokens_per_sec,
                transient,
                ..
            } = &published.event
            {
                if !*transient && *tokens_per_sec == 0.0 {
                    got_zero_sample = true;
                    break;
                }
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    let _ = shutdown_tx.send(true);
    assert!(
        got_zero_sample,
        "sampler should emit zero sample when LiveBucket is expired (bucket_start_ms < current_window)"
    );
}
