import type { StatusChipDto } from "@busytok/protocol-types";
import { reportFrontendEventSafely } from "../../logging/safeReporter";

export const AGGREGATE_LAG_WARNING_THRESHOLD_MS = 5_000;
export const AGGREGATE_LAG_CRITICAL_THRESHOLD_MS = 30_000;

export type AggregateLagSeverity = "warning" | "critical";

let lastObservedAggregateLagSeverity: AggregateLagSeverity | null = null;
let aggregateLagTelemetryInitialized = false;

function detailForSeverity(severity: AggregateLagSeverity): string {
  return severity === "critical"
    ? "Processing delay is severely elevated. Recent totals may be noticeably behind."
    : "Processing delay is elevated. Recent totals may take a moment to catch up.";
}

function visibleEventCodeForSeverity(severity: AggregateLagSeverity): string {
  return severity === "critical"
    ? "gui.shell.aggregate_lag_critical_visible"
    : "gui.shell.aggregate_lag_warning_visible";
}

function visibleEventMessageForSeverity(severity: AggregateLagSeverity): string {
  return severity === "critical"
    ? "Aggregate lag reached the critical threshold in the desktop shell."
    : "Aggregate lag reached the warning threshold in the desktop shell.";
}

export function formatAggregateLagLabel(aggregateLagMs: number): string {
  return aggregateLagMs >= 1_000
    ? `Lag ${(aggregateLagMs / 1_000).toFixed(1)}s`
    : `Lag ${aggregateLagMs}ms`;
}

export function aggregateLagSeverity(aggregateLagMs: number | null): AggregateLagSeverity | null {
  if (aggregateLagMs == null || aggregateLagMs < AGGREGATE_LAG_WARNING_THRESHOLD_MS) {
    return null;
  }
  return aggregateLagMs >= AGGREGATE_LAG_CRITICAL_THRESHOLD_MS ? "critical" : "warning";
}

export function aggregateLagStatusChip(aggregateLagMs: number | null): StatusChipDto | null {
  const severity = aggregateLagSeverity(aggregateLagMs);
  if (severity == null || aggregateLagMs == null) {
    return null;
  }

  return {
    id: "aggregate_lag",
    label: formatAggregateLagLabel(aggregateLagMs),
    tone: severity === "critical" ? "danger" : "warning",
    detail: detailForSeverity(severity),
    action: null,
  };
}

export function syncAggregateLagTelemetry(aggregateLagMs: number | null): void {
  const currentSeverity = aggregateLagSeverity(aggregateLagMs);
  const previousSeverity = aggregateLagTelemetryInitialized
    ? lastObservedAggregateLagSeverity
    : null;

  aggregateLagTelemetryInitialized = true;
  lastObservedAggregateLagSeverity = currentSeverity;

  if (previousSeverity === currentSeverity) {
    return;
  }

  if (currentSeverity != null) {
    reportFrontendEventSafely({
      level: "WARN",
      event_code: visibleEventCodeForSeverity(currentSeverity),
      message: visibleEventMessageForSeverity(currentSeverity),
      details: {
        aggregate_lag_ms: aggregateLagMs,
        previous_severity: previousSeverity,
        current_severity: currentSeverity,
      },
    });
    return;
  }

  if (previousSeverity != null) {
    reportFrontendEventSafely({
      level: "INFO",
      event_code: "gui.shell.aggregate_lag_recovered",
      message: "Aggregate lag recovered to a healthy level in the desktop shell.",
      details: {
        aggregate_lag_ms: aggregateLagMs,
        previous_severity: previousSeverity,
        current_severity: currentSeverity,
      },
    });
  }
}

export function resetAggregateLagTelemetryStateForTests(): void {
  aggregateLagTelemetryInitialized = false;
  lastObservedAggregateLagSeverity = null;
}
