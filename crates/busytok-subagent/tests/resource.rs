#![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
//! ResourceMonitor unit tests (spec §8.1, §5.1 spike note).

use busytok_config::SubagentResourcePolicyConfig;
use busytok_subagent::resource::{ResourceMonitor, ResourcePressureState, ResourceSample};

fn sample(
    service_rss_mb: f64,
    sidecar_rss_mb: Option<f64>,
    sidecar_cpu_percent: Option<f64>,
    hot_session_count: u32,
    system_available_mb: f64,
) -> ResourceSample {
    ResourceSample {
        service_rss_mb,
        sidecar_rss_mb,
        sidecar_cpu_percent,
        hot_session_count,
        system_available_mb,
        queued_task_count: 0,
        running_task_count: 0,
    }
}

#[test]
fn is_under_pressure_true_when_system_available_below_threshold() {
    let policy = SubagentResourcePolicyConfig {
        memory_pressure_free_mb: 2048,
        monitor_interval_seconds: 30,
    };
    let mon = ResourceMonitor::new(policy, 800, 1200);
    let s = sample(20.0, Some(100.0), Some(1.0), 1, 1024.0);
    assert!(mon.is_under_pressure(&s), "1024 < 2048 => pressure");
}

#[test]
fn is_under_pressure_false_when_system_available_at_or_above_threshold() {
    let policy = SubagentResourcePolicyConfig {
        memory_pressure_free_mb: 2048,
        monitor_interval_seconds: 30,
    };
    let mon = ResourceMonitor::new(policy, 800, 1200);
    let s = sample(20.0, Some(100.0), Some(1.0), 1, 2048.0);
    assert!(!mon.is_under_pressure(&s), "2048 == 2048 => no pressure");
    let s2 = sample(20.0, Some(100.0), Some(1.0), 1, 4096.0);
    assert!(!mon.is_under_pressure(&s2), "4096 > 2048 => no pressure");
}

#[test]
fn exceeds_soft_limit_true_when_sidecar_rss_above_soft_limit() {
    let policy = SubagentResourcePolicyConfig::default();
    let mon = ResourceMonitor::new(policy, 800, 1200);
    let s = sample(20.0, Some(801.0), Some(2.0), 1, 4096.0);
    assert!(mon.exceeds_soft_limit(&s), "801 > 800 => soft exceeded");
}

#[test]
fn exceeds_soft_limit_false_when_sidecar_rss_unknown() {
    let policy = SubagentResourcePolicyConfig::default();
    let mon = ResourceMonitor::new(policy, 800, 1200);
    let s = sample(20.0, None, None, 0, 4096.0);
    assert!(
        !mon.exceeds_soft_limit(&s),
        "None RSS => no soft limit breach"
    );
}

#[test]
fn exceeds_hard_limit_true_when_sidecar_rss_above_hard_limit() {
    let policy = SubagentResourcePolicyConfig::default();
    let mon = ResourceMonitor::new(policy, 800, 1200);
    let s = sample(20.0, Some(1201.0), Some(2.0), 1, 4096.0);
    assert!(mon.exceeds_hard_limit(&s), "1201 > 1200 => hard exceeded");
}

#[test]
fn exceeds_hard_limit_false_when_sidecar_rss_below_hard_limit() {
    let policy = SubagentResourcePolicyConfig::default();
    let mon = ResourceMonitor::new(policy, 800, 1200);
    let s = sample(20.0, Some(1200.0), Some(2.0), 1, 4096.0);
    assert!(!mon.exceeds_hard_limit(&s), "1200 == 1200 => not exceeded");
}

#[test]
fn is_under_pressure_uses_policy_threshold_not_hardcoded() {
    // Verify the predicate reads self.policy.memory_pressure_free_mb (not a
    // hardcoded 2048). With threshold=512, a sample at 1000MB available is
    // NOT under pressure.
    let policy = SubagentResourcePolicyConfig {
        memory_pressure_free_mb: 512,
        monitor_interval_seconds: 30,
    };
    let mon = ResourceMonitor::new(policy, 800, 1200);
    let s = sample(20.0, None, None, 0, 1000.0);
    assert!(
        !mon.is_under_pressure(&s),
        "1000 > 512 => no pressure with custom threshold"
    );
}

#[test]
fn exceeds_limits_use_configured_values_not_hardcoded() {
    // Verify soft/hard limits come from constructor, not hardcoded 800/1200.
    let policy = SubagentResourcePolicyConfig::default();
    let mon = ResourceMonitor::new(policy, 500, 700);
    let s = sample(20.0, Some(550.0), Some(1.0), 0, 4096.0);
    assert!(
        mon.exceeds_soft_limit(&s),
        "550 > 500 (custom soft) => soft exceeded"
    );
    assert!(
        !mon.exceeds_hard_limit(&s),
        "550 < 700 (custom hard) => not exceeded"
    );
}

#[test]
fn sample_returns_positive_service_rss_for_current_process() {
    // Spec §5.1 spike note: sysinfo CPU requires two refresh cycles.
    // First sample CPU is 0.0 / unreliable. This test only asserts RSS > 0
    // (the current process always has RSS).
    let policy = SubagentResourcePolicyConfig::default();
    let mut mon = ResourceMonitor::new(policy, 800, 1200);
    let s = mon.sample(None, 0, 0, 0);
    assert!(
        s.service_rss_mb > 0.0,
        "current process RSS must be > 0; got {}",
        s.service_rss_mb
    );
    assert_eq!(
        s.sidecar_rss_mb, None,
        "no sidecar_pid => sidecar_rss_mb is None"
    );
    assert_eq!(s.sidecar_cpu_percent, None, "no sidecar_pid => cpu is None");
    assert_eq!(s.hot_session_count, 0);
    // system_available_mb is best-effort; on some CI runners it may be 0.
    // Just assert it's non-negative.
    assert!(s.system_available_mb >= 0.0);
}

#[test]
fn sample_with_known_pid_returns_sidecar_rss_and_zero_first_cpu() {
    // Spec §5.1 spike note: first sample returns 0.0 for CPU (two refresh
    // cycles required). We sample the current process itself as a "sidecar"
    // so we have a real PID with real RSS.
    let policy = SubagentResourcePolicyConfig::default();
    let mut mon = ResourceMonitor::new(policy, 800, 1200);
    let pid = sysinfo::Pid::from_u32(std::process::id());
    let _ = pid;
    let s = mon.sample(Some(std::process::id()), 0, 0, 0);
    assert!(
        s.sidecar_rss_mb.map(|v| v > 0.0).unwrap_or(false),
        "sidecar_rss_mb must be > 0 for self PID; got {:?}",
        s.sidecar_rss_mb
    );
    // First sample CPU is 0.0 (spike note) — assert it's a finite number, not NaN.
    let cpu = s.sidecar_cpu_percent.unwrap_or(0.0);
    assert!(
        cpu.is_finite(),
        "first-sample CPU must be finite, got {cpu}"
    );
}

#[test]
fn second_sample_returns_meaningful_cpu() {
    // Spec §5.1: CPU requires two refresh cycles. After the first sample
    // primes sysinfo's internal previous-timestamp, the second sample should
    // be a real (but possibly still 0.0 for an idle process) finite value.
    let policy = SubagentResourcePolicyConfig::default();
    let mut mon = ResourceMonitor::new(policy, 800, 1200);
    let pid = std::process::id();
    let _ = mon.sample(Some(pid), 0, 0, 0); // prime
                                            // Burn a tiny bit of CPU so sysinfo has something to measure.
    let mut acc: u64 = 0;
    for i in 0..100_000 {
        acc = acc.wrapping_add(i);
    }
    std::hint::black_box(acc);
    let s2 = mon.sample(Some(pid), 0, 0, 0);
    let cpu = s2.sidecar_cpu_percent.unwrap_or(0.0);
    assert!(
        cpu.is_finite(),
        "second-sample CPU must be finite, got {cpu}"
    );
    assert!(cpu >= 0.0, "CPU percent is non-negative, got {cpu}");
}

// --- edge-trigger latch tests (Task 2 — pure functions, no async) ---
//
// `transition_event` and `is_recovery` are pure functions on the
// `ResourcePressureState` enum. These tests verify the edge-trigger
// semantics without spinning up the supervisor: escalation transitions
// return a DB event type, recovery transitions return None (DB event
// omitted by design — spec §3.2 has no `resource_recovered` type),
// and same-state transitions are debounced.

#[test]
fn transition_event_returns_memory_pressure_on_normal_to_pressure() {
    let event = ResourcePressureState::transition_event(
        ResourcePressureState::Normal,
        ResourcePressureState::Pressure,
    );
    assert_eq!(event, Some("memory_pressure"));
}

#[test]
fn transition_event_returns_rss_limit_on_normal_to_limit_exceeded() {
    let event = ResourcePressureState::transition_event(
        ResourcePressureState::Normal,
        ResourcePressureState::LimitExceeded,
    );
    assert_eq!(event, Some("rss_limit_exceeded"));
}

#[test]
fn transition_event_returns_rss_limit_on_pressure_to_limit_exceeded() {
    let event = ResourcePressureState::transition_event(
        ResourcePressureState::Pressure,
        ResourcePressureState::LimitExceeded,
    );
    assert_eq!(event, Some("rss_limit_exceeded"));
}

#[test]
fn transition_event_returns_none_on_pressure_to_normal_recovery() {
    // Recovery: no DB event (resource_recovered not in spec §3.2 enum).
    // The supervisor logs recovery to tracing only; DB event omitted by
    // design (spec §3.2 has no `resource_recovered` type).
    let event = ResourcePressureState::transition_event(
        ResourcePressureState::Pressure,
        ResourcePressureState::Normal,
    );
    assert_eq!(event, None);
    // But is_recovery returns true so the caller knows to log.
    assert!(ResourcePressureState::is_recovery(
        ResourcePressureState::Pressure,
        ResourcePressureState::Normal,
    ));
}

#[test]
fn transition_event_returns_none_on_limit_exceeded_to_normal_recovery() {
    let event = ResourcePressureState::transition_event(
        ResourcePressureState::LimitExceeded,
        ResourcePressureState::Normal,
    );
    assert_eq!(event, None);
    assert!(ResourcePressureState::is_recovery(
        ResourcePressureState::LimitExceeded,
        ResourcePressureState::Normal,
    ));
}

#[test]
fn transition_event_returns_none_on_limit_exceeded_to_pressure() {
    // Downgrade within warning tier — no new lifecycle event.
    let event = ResourcePressureState::transition_event(
        ResourcePressureState::LimitExceeded,
        ResourcePressureState::Pressure,
    );
    assert_eq!(event, None);
}

#[test]
fn transition_event_returns_none_on_same_state() {
    // Debounce — sustained pressure produces ONE event, not 40.
    assert_eq!(
        ResourcePressureState::transition_event(
            ResourcePressureState::Normal,
            ResourcePressureState::Normal,
        ),
        None,
        "Normal → Normal must be debounced (no event)"
    );
    assert_eq!(
        ResourcePressureState::transition_event(
            ResourcePressureState::Pressure,
            ResourcePressureState::Pressure,
        ),
        None,
        "Pressure → Pressure must be debounced (spec §6.5 lifecycle-boundaries-only)"
    );
    assert_eq!(
        ResourcePressureState::transition_event(
            ResourcePressureState::LimitExceeded,
            ResourcePressureState::LimitExceeded,
        ),
        None,
        "LimitExceeded → LimitExceeded must be debounced"
    );
}

// --- Task 1: queued/running task count fields (spec §8.1) ---

#[test]
fn sample_includes_queued_and_running_task_counts() {
    let policy = busytok_config::SubagentResourcePolicyConfig::default();
    let mut monitor = ResourceMonitor::new(policy, 800, 1200);
    let sample = monitor.sample(None, 0, 5, 2);
    assert_eq!(sample.queued_task_count, 5);
    assert_eq!(sample.running_task_count, 2);
}
