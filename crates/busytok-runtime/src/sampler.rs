//! Live throughput sampler — publishes fixed-interval `LiveSample` events.
//!
//! The sampler operates in two modes:
//!
//! * **Exact mode** (when a promoted generation is active): queries the
//!   `usage_buckets_2s` table for the most recently completed 2s bucket,
//!   publishing samples with `transient: false`.
//! * **Transient mode** (before the first promoted generation finishes, or
//!   during a rebuild): queries raw `usage_events` for the most recently
//!   completed 2s window, publishing samples with `transient: true`. These
//!   are also pushed into the in-memory ring buffer for `live.window`
//!   bootstrapping.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tracing::{debug, warn};

use busytok_events::{AppEvent, AppEventBus};
use busytok_protocol::dto::{LiveSampleDto, ReadinessStateDto};
use busytok_store::{live_queries, Database};

use crate::status::{ServiceStatusSnapshot, TRANSIENT_RING_BUFFER_CAPACITY};

/// Start the live sampler background task. Every 2s, checks the current
/// readiness state and active generation ID, then publishes a `LiveSample`
/// event.
///
/// * If the service is `ReadyExact` with an active generation, the sample
///   is sourced from `usage_buckets_2s` (exact, `transient: false`).
/// * Otherwise the sample is sourced from raw `usage_events` (approximate,
///   `transient: true`), and also pushed into the ring buffer carried in
///   the status snapshot.
///
/// Returns a shutdown sender; dropping the sender or sending `true` stops
/// the sampler.
pub fn start_sampler(
    db: Arc<Mutex<Database>>,
    status: Arc<tokio::sync::RwLock<ServiceStatusSnapshot>>,
    event_bus: Arc<AppEventBus>,
) -> tokio::sync::watch::Sender<bool> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(async move {
        let mut rx = shutdown_rx;
        // Local ring buffer so we don't need a write lock on every tick when
        // publishing transient samples.
        let mut local_buffer: VecDeque<LiveSampleDto> =
            VecDeque::with_capacity(TRANSIENT_RING_BUFFER_CAPACITY);

        loop {
            tokio::select! {
                _ = rx.changed() => {
                    debug!("Live sampler received shutdown signal");
                    break;
                }
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }

            // Determine whether we are in exact or transient mode.
            let (readiness, active_generation_id) = {
                let snap = status.read().await;
                (snap.readiness.clone(), snap.active_generation_id.clone())
            };

            let is_exact = matches!(readiness, ReadinessStateDto::ReadyExact)
                && active_generation_id.is_some();

            if is_exact {
                // Exact mode: snapshot the in-memory LiveBucket.
                // The writer is the sole mutator; sampler uses a read lock.
                let sample_opt = {
                    let snap = status.read().await;
                    let bucket = &snap.live_bucket;
                    let now = busytok_domain::now_ms();
                    // The most recently completed 2s window. The writer
                    // accumulates in the current window (one ahead).
                    let current_window = ((now - 2000) / 2000) * 2000;

                    // If the bucket was set by the writer and belongs to the
                    // current or just-completed window, publish its data.
                    // Otherwise the writer has been idle (no new events) for
                    // at least one full window — treat the bucket as expired
                    // and fall through to publishing a zero sample so the
                    // timeline keeps advancing.
                    if bucket.event_count > 0 && bucket.bucket_start_ms >= current_window {
                        Some(LiveSampleDto {
                            bucket_start_ms: bucket.bucket_start_ms,
                            tokens_per_sec: bucket.total_tokens as f64 / 2.0,
                            cost_per_sec: if bucket.cost_usd > 0.0 {
                                Some(bucket.cost_usd / 2.0)
                            } else {
                                None
                            },
                            events_per_sec: bucket.event_count as f64 / 2.0,
                        })
                    } else {
                        None
                    }
                };

                match sample_opt {
                    Some(dto) => {
                        let _ = event_bus.publish_ephemeral(AppEvent::LiveSample {
                            bucket_start_ms: dto.bucket_start_ms,
                            tokens_per_sec: dto.tokens_per_sec,
                            cost_per_sec: dto.cost_per_sec,
                            events_per_sec: dto.events_per_sec,
                            transient: false,
                        });
                    }
                    None => {
                        // No data in current window — publish zero sample.
                        let now_ms = busytok_domain::now_ms();
                        let bucket_start_ms = ((now_ms - 2000) / 2000) * 2000;
                        let _ = event_bus.publish_ephemeral(AppEvent::LiveSample {
                            bucket_start_ms,
                            tokens_per_sec: 0.0,
                            cost_per_sec: None,
                            events_per_sec: 0.0,
                            transient: false,
                        });
                    }
                }
            } else {
                // Transient mode — query raw usage_events (the legacy query)
                let sample = {
                    let db = match db.lock() {
                        Ok(db) => db,
                        Err(e) => {
                            warn!("Live sampler failed to acquire db lock: {e}");
                            continue;
                        }
                    };
                    live_queries::query_sample_window(db.conn())
                };

                match sample {
                    Ok(dto) => {
                        // Push into local ring buffer and flush to status
                        // snapshot periodically (on every tick).
                        if local_buffer.len() >= TRANSIENT_RING_BUFFER_CAPACITY {
                            local_buffer.pop_front();
                        }
                        local_buffer.push_back(dto.clone());

                        // Sync local buffer into the shared status snapshot.
                        {
                            let mut snap = status.write().await;
                            snap.transient_ring_buffer.clear();
                            for s in &local_buffer {
                                snap.transient_ring_buffer.push_back(s.clone());
                            }
                        }

                        let _ = event_bus.publish_ephemeral(AppEvent::LiveSample {
                            bucket_start_ms: dto.bucket_start_ms,
                            tokens_per_sec: dto.tokens_per_sec,
                            cost_per_sec: dto.cost_per_sec,
                            events_per_sec: dto.events_per_sec,
                            transient: true,
                        });
                    }
                    Err(e) => {
                        warn!("Live sampler transient query failed: {e}");
                    }
                }
            }
        }
    });

    shutdown_tx
}
