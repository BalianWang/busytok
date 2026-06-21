use busytok_protocol::dto::InvalidationScopeDto;
use tokio::sync::broadcast;

// ── AppEvent (the content payload) ─────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum AppEvent {
    UsageEventInserted {
        event_id: String,
        agent: String,
    },
    SummaryUpdated {
        keys_updated: Vec<String>,
    },
    ScanProgress {
        source_id: String,
        files_scanned: u64,
        events_ingested: u64,
    },
    Error {
        message: String,
        source: Option<String>,
    },
    DataInvalidated {
        datasets: Vec<InvalidationScopeDto>,
    },
    LiveSample {
        bucket_start_ms: i64,
        tokens_per_sec: f64,
        cost_per_sec: Option<f64>,
        events_per_sec: f64,
        transient: bool,
    },
    /// Emitted when a subscription client connects.
    SubscriptionConnected {
        client_id: Option<String>,
    },
    /// Emitted when a subscription client disconnects.
    SubscriptionDisconnected {
        client_id: Option<String>,
        reason: Option<String>,
    },
    /// Emitted when subscription reconnection fails after retries.
    SubscriptionReconnectFailed {
        attempts: u32,
        last_error: String,
    },
    /// Writer queue depth crossed a threshold (direction: "crossed_above" or "crossed_below").
    WriterQueueThreshold {
        queue_depth: i64,
        threshold: i64,
        severity: String,
    },
    /// Writer aggregate lag crossed a threshold.
    WriterLagThreshold {
        lag_ms: i64,
        threshold: i64,
        severity: String,
    },
}

impl AppEvent {
    /// Returns the event type string used for routing and display.
    pub fn event_type(&self) -> &'static str {
        match self {
            AppEvent::UsageEventInserted { .. } => "usage:event_inserted",
            AppEvent::SummaryUpdated { .. } => "usage:summary_updated",
            AppEvent::ScanProgress { .. } => "usage:scan_progress",
            AppEvent::Error { .. } => "usage:error",
            AppEvent::DataInvalidated { .. } => "data:invalidated",
            AppEvent::LiveSample { .. } => "live:sample",
            AppEvent::SubscriptionConnected { .. } => "subscription:connected",
            AppEvent::SubscriptionDisconnected { .. } => "subscription:disconnected",
            AppEvent::SubscriptionReconnectFailed { .. } => "subscription:reconnect_failed",
            AppEvent::WriterQueueThreshold { .. } => "diagnostic:writer_queue_threshold",
            AppEvent::WriterLagThreshold { .. } => "diagnostic:writer_lag_threshold",
        }
    }

    /// Whether this event is ephemeral (not checkpointed, not durable).
    pub fn is_ephemeral(&self) -> bool {
        match self {
            AppEvent::ScanProgress { .. }
            | AppEvent::LiveSample { .. }
            | AppEvent::SubscriptionConnected { .. }
            | AppEvent::SubscriptionDisconnected { .. }
            | AppEvent::SubscriptionReconnectFailed { .. }
            | AppEvent::WriterQueueThreshold { .. }
            | AppEvent::WriterLagThreshold { .. } => true,
            _ => false,
        }
    }

    /// Convenience helper for creating a `SummaryUpdated` event in tests.
    pub fn summary_updated_for_test() -> Self {
        AppEvent::SummaryUpdated {
            keys_updated: vec!["test".into()],
        }
    }
}

// ── PublishedEvent — envelope with durable sequencing metadata ─────────────

/// Envelope that wraps an `AppEvent` with durable sequencing metadata.
///
/// Published by the writer actor after commit and broadcast on the
/// `AppEventBus`. The metadata (`event_seq`, `generation_id`, etc.) is
/// used for ordering, recovery, and cache invalidation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PublishedEvent {
    /// The actual event payload.
    pub event: AppEvent,

    /// Global, monotonically increasing event sequence number.
    /// `None` for ephemeral events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_seq: Option<i64>,

    /// Active generation ID when this event was committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_id: Option<String>,

    /// Watermark timestamp of the generation when published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watermark_ms: Option<i64>,

    /// Invalidation scopes triggered by this event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<InvalidationScopeDto>,
}

impl PublishedEvent {
    /// Create a durable published event with full metadata.
    pub fn durable(
        event: AppEvent,
        event_seq: i64,
        generation_id: String,
        watermark_ms: i64,
        scopes: Vec<InvalidationScopeDto>,
    ) -> Self {
        Self {
            event,
            event_seq: Some(event_seq),
            generation_id: Some(generation_id),
            watermark_ms: Some(watermark_ms),
            scopes,
        }
    }

    /// Create an ephemeral published event (no durable metadata).
    pub fn ephemeral(event: AppEvent) -> Self {
        Self {
            event,
            event_seq: None,
            generation_id: None,
            watermark_ms: None,
            scopes: Vec::new(),
        }
    }

    /// The event type string, delegated to the inner `AppEvent`.
    pub fn event_type(&self) -> &'static str {
        self.event.event_type()
    }
}

// ── AppEventBus ────────────────────────────────────────────────────────────

pub struct AppEventBus {
    sender: broadcast::Sender<PublishedEvent>,
}

impl AppEventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Publish a `PublishedEvent` onto the bus.
    pub fn publish(
        &self,
        event: PublishedEvent,
    ) -> Result<(), broadcast::error::SendError<PublishedEvent>> {
        self.sender.send(event)?;
        Ok(())
    }

    /// Convenience: publish an ephemeral event.
    pub fn publish_ephemeral(
        &self,
        event: AppEvent,
    ) -> Result<(), broadcast::error::SendError<PublishedEvent>> {
        self.publish(PublishedEvent::ephemeral(event))
    }

    /// Subscribe to the event bus.
    pub fn subscribe(&self) -> broadcast::Receiver<PublishedEvent> {
        self.sender.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use busytok_protocol::dto::{InvalidationDatasetDto, InvalidationScopeDto};

    #[test]
    fn data_invalidated_serde_roundtrip() {
        let event = AppEvent::DataInvalidated {
            datasets: vec![InvalidationScopeDto {
                dataset: InvalidationDatasetDto::Overview,
                breakdown_kind: None,
            }],
        };
        let json = serde_json::to_value(&event).unwrap();
        let roundtripped: AppEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event.event_type(), roundtripped.event_type());
    }

    #[test]
    fn live_sample_serde_roundtrip() {
        let event = AppEvent::LiveSample {
            bucket_start_ms: 1_740_000_000_000i64,
            tokens_per_sec: 150.5,
            cost_per_sec: Some(0.003),
            events_per_sec: 3.0,
            transient: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        let roundtripped: AppEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event.event_type(), roundtripped.event_type());
    }

    #[test]
    fn published_event_durable_metadata_roundtrips() {
        let event = AppEvent::UsageEventInserted {
            event_id: "evt-1".into(),
            agent: "claude_code".into(),
        };
        let published = PublishedEvent::durable(
            event,
            42,
            "gen-1".into(),
            1_700_000_000_000i64,
            vec![InvalidationScopeDto {
                dataset: InvalidationDatasetDto::Overview,
                breakdown_kind: None,
            }],
        );
        let json = serde_json::to_value(&published).unwrap();
        assert_eq!(json["event_seq"], 42);
        assert_eq!(json["generation_id"], "gen-1");
        assert_eq!(json["watermark_ms"], 1_700_000_000_000i64);
        assert_eq!(json["scopes"][0]["dataset"], "overview");
        // Event fields are nested under "event"
        assert_eq!(json["event"]["UsageEventInserted"]["event_id"], "evt-1");
    }

    #[test]
    fn published_event_ephemeral_omits_metadata() {
        let event = AppEvent::ScanProgress {
            source_id: "src-1".into(),
            files_scanned: 5,
            events_ingested: 100,
        };
        let published = PublishedEvent::ephemeral(event);
        let json = serde_json::to_value(&published).unwrap();
        assert!(json.get("event_seq").is_none());
        assert!(json.get("generation_id").is_none());
        // Event fields are nested under "event"
        assert_eq!(json["event"]["ScanProgress"]["source_id"], "src-1");
    }

    #[test]
    fn event_is_ephemeral_detection() {
        assert!(AppEvent::ScanProgress {
            source_id: "s".into(),
            files_scanned: 1,
            events_ingested: 1
        }
        .is_ephemeral());
        assert!(AppEvent::LiveSample {
            bucket_start_ms: 0,
            tokens_per_sec: 1.0,
            cost_per_sec: None,
            events_per_sec: 1.0,
            transient: false,
        }
        .is_ephemeral());
        assert!(!AppEvent::Error {
            message: "e".into(),
            source: None
        }
        .is_ephemeral());
    }

    #[test]
    fn subscription_diagnostic_events_have_correct_types() {
        assert_eq!(
            AppEvent::SubscriptionConnected {
                client_id: Some("c1".into())
            }
            .event_type(),
            "subscription:connected"
        );
        assert_eq!(
            AppEvent::SubscriptionDisconnected {
                client_id: Some("c1".into()),
                reason: Some("eof".into())
            }
            .event_type(),
            "subscription:disconnected"
        );
        assert_eq!(
            AppEvent::SubscriptionReconnectFailed {
                attempts: 3,
                last_error: "timeout".into()
            }
            .event_type(),
            "subscription:reconnect_failed"
        );
    }

    #[test]
    fn writer_threshold_events_have_correct_types() {
        assert_eq!(
            AppEvent::WriterQueueThreshold {
                queue_depth: 100,
                threshold: 80,
                severity: "warning".into()
            }
            .event_type(),
            "diagnostic:writer_queue_threshold"
        );
        assert_eq!(
            AppEvent::WriterLagThreshold {
                lag_ms: 5000,
                threshold: 3000,
                severity: "warning".into()
            }
            .event_type(),
            "diagnostic:writer_lag_threshold"
        );
    }
}
