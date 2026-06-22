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
use busytok_events::{AppEvent, AppEventBus, PublishedEvent};

#[test]
fn publish_and_subscribe_receives_event() {
    let bus = AppEventBus::new(16);
    let mut receiver = bus.subscribe();
    bus.publish(PublishedEvent::ephemeral(
        AppEvent::summary_updated_for_test(),
    ))
    .unwrap();
    let event = receiver.try_recv().unwrap();
    assert_eq!(event.event_type(), "usage:summary_updated");
}

#[test]
fn multiple_subscribers_receive_same_event() {
    let bus = AppEventBus::new(16);
    let mut r1 = bus.subscribe();
    let mut r2 = bus.subscribe();
    bus.publish(PublishedEvent::ephemeral(AppEvent::Error {
        message: "test".into(),
        source: None,
    }))
    .unwrap();
    assert!(r1.try_recv().is_ok());
    assert!(r2.try_recv().is_ok());
}

#[test]
fn durable_event_carries_metadata() {
    let bus = AppEventBus::new(16);
    let mut rx = bus.subscribe();
    let event = PublishedEvent::durable(
        AppEvent::UsageEventInserted {
            event_id: "evt-1".into(),
            agent: "claude_code".into(),
        },
        42,
        "gen-1".into(),
        1_700_000_000_000i64,
        vec![],
    );
    bus.publish(event).unwrap();
    let received = rx.try_recv().unwrap();
    assert_eq!(received.event_type(), "usage:event_inserted");
    assert_eq!(received.event_seq, Some(42));
    assert_eq!(received.generation_id.as_deref(), Some("gen-1"));
}

#[test]
fn ephemeral_event_omits_durable_metadata() {
    let bus = AppEventBus::new(16);
    let mut rx = bus.subscribe();
    bus.publish_ephemeral(AppEvent::ScanProgress {
        source_id: "src-1".into(),
        files_scanned: 1,
        events_ingested: 10,
    })
    .unwrap();
    let received = rx.try_recv().unwrap();
    assert_eq!(received.event_type(), "usage:scan_progress");
    assert_eq!(received.event_seq, None);
    assert_eq!(received.generation_id, None);
}

#[test]
fn subscription_diagnostic_events_flow_through_bus() {
    let bus = AppEventBus::new(16);
    let mut rx = bus.subscribe();

    bus.publish_ephemeral(AppEvent::SubscriptionConnected {
        client_id: Some("sub-1".into()),
    })
    .unwrap();
    let evt = rx.try_recv().unwrap();
    assert_eq!(evt.event_type(), "subscription:connected");

    bus.publish_ephemeral(AppEvent::SubscriptionDisconnected {
        client_id: Some("sub-1".into()),
        reason: Some("eof".into()),
    })
    .unwrap();
    let evt = rx.try_recv().unwrap();
    assert_eq!(evt.event_type(), "subscription:disconnected");
}

#[test]
fn writer_threshold_events_flow_through_bus() {
    let bus = AppEventBus::new(16);
    let mut rx = bus.subscribe();

    bus.publish_ephemeral(AppEvent::WriterQueueThreshold {
        queue_depth: 90,
        threshold: 64,
        severity: "warning".into(),
    })
    .unwrap();
    let evt = rx.try_recv().unwrap();
    assert_eq!(evt.event_type(), "diagnostic:writer_queue_threshold");

    bus.publish_ephemeral(AppEvent::WriterLagThreshold {
        lag_ms: 10000,
        threshold: 5000,
        severity: "warning".into(),
    })
    .unwrap();
    let evt = rx.try_recv().unwrap();
    assert_eq!(evt.event_type(), "diagnostic:writer_lag_threshold");
}
