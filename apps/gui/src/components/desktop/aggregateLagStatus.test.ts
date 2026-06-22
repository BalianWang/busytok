import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  AGGREGATE_LAG_CRITICAL_THRESHOLD_MS,
  AGGREGATE_LAG_WARNING_THRESHOLD_MS,
  aggregateLagSeverity,
  aggregateLagStatusChip,
  resetAggregateLagTelemetryStateForTests,
  syncAggregateLagTelemetry,
} from "./aggregateLagStatus";

const reportFrontendEvent = vi.fn();

vi.mock("../../logging/safeReporter", () => ({
  reportFrontendEventSafely: (entry: unknown) => reportFrontendEvent(entry),
}));

beforeEach(() => {
  reportFrontendEvent.mockReset();
  resetAggregateLagTelemetryStateForTests();
});

describe("aggregateLagStatus", () => {
  it("keeps healthy lag hidden below the warning threshold", () => {
    expect(aggregateLagSeverity(AGGREGATE_LAG_WARNING_THRESHOLD_MS - 1)).toBeNull();
    expect(aggregateLagStatusChip(AGGREGATE_LAG_WARNING_THRESHOLD_MS - 1)).toBeNull();
  });

  it("shows warning at the warning threshold and danger at the critical threshold", () => {
    expect(aggregateLagStatusChip(AGGREGATE_LAG_WARNING_THRESHOLD_MS)).toMatchObject({
      label: "Lag 5.0s",
      tone: "warning",
    });
    expect(aggregateLagStatusChip(AGGREGATE_LAG_CRITICAL_THRESHOLD_MS - 1)).toMatchObject({
      label: "Lag 30.0s",
      tone: "warning",
    });
    expect(aggregateLagStatusChip(AGGREGATE_LAG_CRITICAL_THRESHOLD_MS)).toMatchObject({
      label: "Lag 30.0s",
      tone: "danger",
    });
  });

  it("emits exact telemetry transitions without duplicate initial visibility", () => {
    syncAggregateLagTelemetry(31_200);
    syncAggregateLagTelemetry(31_200);
    syncAggregateLagTelemetry(5_200);
    syncAggregateLagTelemetry(0);

    expect(
      reportFrontendEvent.mock.calls.map(([entry]) => (
        entry as { event_code: string }
      ).event_code),
    ).toEqual([
      "gui.shell.aggregate_lag_critical_visible",
      "gui.shell.aggregate_lag_warning_visible",
      "gui.shell.aggregate_lag_recovered",
    ]);
  });
});
