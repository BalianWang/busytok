//! Resource monitor — pure sampling module (spec §8.1, §5.1 spike note).
//!
//! `ResourceMonitor` is constructed by `PiSidecarSupervisor` and called from
//! the existing `supervision_loop` (no async runtime here — the loop is
//! already async and calls `sample()` synchronously between awaits).
//!
//! ## sysinfo CPU spike note (spec §5.1)
//!
//! sysinfo computes CPU percent from the delta between two refreshes. The
//! FIRST call to `sample()` after construction returns `0.0` for CPU because
//! there's no prior measurement to delta against. The supervision loop calls
//! `sample()` every `monitor_interval_seconds`, so by the second tick the
//! value is meaningful. Tests that assert on CPU behavior must call `sample()`
//! twice (prime + measure).

use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};

use busytok_config::SubagentResourcePolicyConfig;

/// One point-in-time snapshot of process + system resource usage.
/// Mirrors spec §8.1 collection fields. Written to `subagent_resource_events`
/// by `PiSidecarSupervisor::write_resource_event` (Task 2).
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceSample {
    /// busytok-service RSS in MB.
    pub service_rss_mb: f64,
    /// Pi sidecar RSS in MB (None when sidecar not running).
    pub sidecar_rss_mb: Option<f64>,
    /// Pi sidecar CPU percent (0–100). None when sidecar not running.
    /// First sample after construction is 0.0 (spike note).
    pub sidecar_cpu_percent: Option<f64>,
    /// Number of currently-hot sessions in the sidecar pool.
    pub hot_session_count: u32,
    /// System available memory in MB (macOS). Best-effort.
    pub system_available_mb: f64,
    /// Number of queued tasks (spec §8.1). Provided by the caller from the
    /// subagent_tasks table.
    pub queued_task_count: u32,
    /// Number of running tasks (spec §8.1). Provided by the caller.
    pub running_task_count: u32,
}

/// Edge-trigger latch for resource pressure state (spec §6.5: "lifecycle
/// boundaries only, not a metrics time-series table"). The supervisor
/// transitions between these states; DB events are written ONLY on
/// transitions, while `tracing` logs fire every sample tick for
/// observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResourcePressureState {
    /// No pressure, sidecar RSS under soft limit.
    #[default]
    Normal,
    /// System memory pressure OR sidecar RSS > soft limit (warning tier).
    Pressure,
    /// Sidecar RSS > hard limit (error tier — Plan 6 will force-kill).
    LimitExceeded,
}

impl ResourcePressureState {
    /// Returns the DB event type to write on a state transition, or `None`
    /// if no DB event should be written. This is a pure function — testable
    /// without async or sysinfo.
    ///
    /// Returns `Some` only for transitions INTO a pressure/limit state
    /// (escalations). Recovery transitions (Pressure→Normal,
    /// LimitExceeded→Normal) return `None` — the supervisor logs them to
    /// `tracing` (info level) but does NOT write a DB event, because
    /// `resource_recovered` is not in spec §3.2's event enum. The spec enum
    /// update + DB event for recovery are deferred to Plan 6.
    ///
    /// The latch state still updates on recovery (handled by the caller) so
    /// a re-pressurization later writes a fresh `memory_pressure` event.
    pub fn transition_event(old: Self, new: Self) -> Option<&'static str> {
        match (old, new) {
            (Self::Normal, Self::Pressure) => Some("memory_pressure"),
            (Self::Normal, Self::LimitExceeded) | (Self::Pressure, Self::LimitExceeded) => {
                Some("rss_limit_exceeded")
            }
            // Recovery transitions: no DB event (resource_recovered not in
            // spec §3.2 enum — deferred to Plan 6). Caller logs to tracing.
            (Self::Pressure, Self::Normal) | (Self::LimitExceeded, Self::Normal) => None,
            (Self::LimitExceeded, Self::Pressure) => None,
            _ => None, // same state — debounced
        }
    }

    /// Returns true if the transition is a recovery (pressure/limit → normal).
    /// The caller uses this to log the recovery to `tracing` even though no
    /// DB event is written.
    pub fn is_recovery(old: Self, new: Self) -> bool {
        matches!(
            (old, new),
            (Self::Pressure, Self::Normal) | (Self::LimitExceeded, Self::Normal)
        )
    }
}

/// Pure sampling + pressure-predicate module. Owns a `sysinfo::System` so
/// CPU deltas accumulate across `sample()` calls. Predicates are instance
/// methods that read the configured thresholds from `policy` / `soft_limit_mb`
/// / `hard_limit_mb` (no hardcoded values — spec §8.2 config flows through).
pub struct ResourceMonitor {
    system: System,
    policy: SubagentResourcePolicyConfig,
    soft_limit_mb: u32,
    hard_limit_mb: u32,
}

impl ResourceMonitor {
    /// Construct a new monitor. `soft_limit_mb` / `hard_limit_mb` come from
    /// `SubagentPiSidecarConfig`; `policy` comes from
    /// `SubagentResourcePolicyConfig`.
    pub fn new(
        policy: SubagentResourcePolicyConfig,
        soft_limit_mb: u32,
        hard_limit_mb: u32,
    ) -> Self {
        Self {
            system: System::new_all(),
            policy,
            soft_limit_mb,
            hard_limit_mb,
        }
    }

    /// Take a sample. `sidecar_pid` is the sidecar child PID (None when the
    /// sidecar is not running). `hot_session_count` is provided by the caller
    /// (the supervisor reads it from the sidecar's `adapter.health` response
    /// or tracks it via the executor). `queued_task_count` and
    /// `running_task_count` are provided by the caller from the
    /// `subagent_tasks` table (spec §8.1).
    ///
    /// First call after construction returns `sidecar_cpu_percent = Some(0.0)`
    /// (spike note) — sysinfo needs two refreshes to compute a delta.
    pub fn sample(
        &mut self,
        sidecar_pid: Option<u32>,
        hot_session_count: u32,
        queued_task_count: u32,
        running_task_count: u32,
    ) -> ResourceSample {
        // sysinfo 0.32 API: refresh_specifics takes a RefreshKind. We refresh
        // processes (with cpu+memory) and system memory in one call.
        self.system.refresh_specifics(
            RefreshKind::everything().with_processes(ProcessRefreshKind::everything()),
        );

        let service_pid = Pid::from_u32(std::process::id());
        let service_rss_mb = self
            .system
            .process(service_pid)
            .map(|p| bytes_to_mb(p.memory()))
            .unwrap_or(0.0);

        let (sidecar_rss_mb, sidecar_cpu_percent) = sidecar_pid
            .map(|pid| {
                let p = self.system.process(Pid::from_u32(pid));
                (
                    p.map(|proc_info| bytes_to_mb(proc_info.memory())),
                    p.map(|proc_info| proc_info.cpu_usage() as f64),
                )
            })
            .unwrap_or((None, None));

        let system_available_mb = bytes_to_mb(self.system.available_memory());

        ResourceSample {
            service_rss_mb,
            sidecar_rss_mb,
            sidecar_cpu_percent,
            hot_session_count,
            system_available_mb,
            queued_task_count,
            running_task_count,
        }
    }

    /// The configured sampling interval (spec §8.2). Read by the supervision
    /// loop to decide when to call `sample()`.
    pub fn monitor_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.policy.monitor_interval_seconds.max(1))
    }

    /// True when system available memory is below the pressure threshold
    /// (spec §8.3 step 1 threshold). Plan 5 only emits the
    /// `memory_pressure` event; the backpressure action is deferred to
    /// Plan 6. Reads `self.policy.memory_pressure_free_mb`.
    pub fn is_under_pressure(&self, sample: &ResourceSample) -> bool {
        sample.system_available_mb < self.policy.memory_pressure_free_mb as f64
    }

    /// True when sidecar RSS exceeds the soft limit (spec §8.3 step 3
    /// threshold). Plan 5 only emits a warning log; the graceful-restart
    /// action is deferred to Plan 6. Reads `self.soft_limit_mb`.
    pub fn exceeds_soft_limit(&self, sample: &ResourceSample) -> bool {
        sample
            .sidecar_rss_mb
            .map(|rss| rss > self.soft_limit_mb as f64)
            .unwrap_or(false)
    }

    /// True when sidecar RSS exceeds the hard limit (spec §8.3 step 5
    /// threshold). Plan 5 only emits the `rss_limit_exceeded` event;
    /// the force-kill action is deferred to Plan 6.
    /// Reads `self.hard_limit_mb`.
    pub fn exceeds_hard_limit(&self, sample: &ResourceSample) -> bool {
        sample
            .sidecar_rss_mb
            .map(|rss| rss > self.hard_limit_mb as f64)
            .unwrap_or(false)
    }
}

/// Convert bytes (sysinfo's `u64` memory) to megabytes as `f64`.
fn bytes_to_mb(bytes: u64) -> f64 {
    (bytes as f64) / (1024.0 * 1024.0)
}
