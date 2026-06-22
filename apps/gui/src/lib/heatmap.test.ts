import { describe, it, expect } from "vitest";
import type { OverviewHeatmapDto, OverviewHeatmapDayDto } from "@busytok/protocol-types";
import { buildOverviewHeatmapModel, summarizeHeatmapForDiagnostics, diagnosticsSignature } from "./heatmap";
import type { HeatmapDiagnostics } from "./heatmap";

// ─── buildOverviewHeatmapModel ────────────────────────────────────────────────

function singleDayDto(overrides?: Partial<OverviewHeatmapDayDto>): OverviewHeatmapDayDto {
  return {
    date: "2025-06-01",
    tokens: 1000,
    cost_usd: 0.10,
    cost_status: "exact",
    event_count: 3,
    ...overrides,
  };
}

describe("buildOverviewHeatmapModel", () => {
  it("builds a grid with 12 months of columns", () => {
    const dayCount = 30; // one month of daily data
    const days: OverviewHeatmapDayDto[] = Array.from({ length: dayCount }, (_, i) => {
      const day = String(i + 1).padStart(2, "0");
      return {
        date: `2025-06-${day}`,
        tokens: (i + 1) * 100,
        cost_usd: (i + 1) * 0.01,
        cost_status: "exact" as const,
        event_count: i + 1,
      };
    });

    const dto: OverviewHeatmapDto = {
      today: "2025-06-30",
      week_starts_on: 1, // Monday
      days,
    };

    const model = buildOverviewHeatmapModel(dto);
    expect(model.columns.length).toBeGreaterThan(0);
    expect(model.totalTokens).toBeGreaterThan(0n);
    expect(model.totalTokensLabel).toBeTruthy();

    // Each column should have 7 days
    for (const col of model.columns) {
      expect(col.days).toHaveLength(7);
    }
  });

  it("handles empty data (all zero tokens)", () => {
    const dto: OverviewHeatmapDto = {
      today: "2025-06-15",
      week_starts_on: 0, // Sunday
      days: [
        { date: "2025-06-01", tokens: 0, cost_usd: null, cost_status: "unavailable", event_count: 0 },
        { date: "2025-06-02", tokens: 0, cost_usd: null, cost_status: "unavailable", event_count: 0 },
      ],
    };

    const model = buildOverviewHeatmapModel(dto);
    expect(model.totalTokens).toBe(0n);
    expect(model.legendLevels).toEqual([0]);
    expect(model.summary.mostActiveMonthLabel).toBe("No activity");
    expect(model.summary.mostActiveDayLabel).toBe("No activity");
    expect(model.summary.longestActiveStreakLabel).toBe("0d");
    expect(model.summary.currentActiveStreakLabel).toBe("0d");
  });

  it("assigns intensity levels based on quartiles", () => {
    // 12 days with increasing tokens
    const days: OverviewHeatmapDayDto[] = Array.from({ length: 12 }, (_, i) => ({
      date: `2025-06-${String(i + 1).padStart(2, "0")}`,
      tokens: (i + 1) * 1000,
      cost_usd: (i + 1) * 0.10,
      cost_status: "exact" as const,
      event_count: i + 1,
    }));

    const dto: OverviewHeatmapDto = {
      today: "2025-06-12",
      week_starts_on: 1,
      days,
    };

    const model = buildOverviewHeatmapModel(dto);
    const realDays = model.columns.flatMap((c) => c.days).filter((d) => !d.isPlaceholder);
    const intensities = realDays.map((d) => d.intensity);

    // Should have variety in intensities
    expect(new Set(intensities).size).toBeGreaterThan(1);
    expect(Math.max(...intensities)).toBe(4);
    // Non-zero intensities should all be >= 1
    const nonZero = intensities.filter((i: number) => i > 0);
    expect(nonZero.every((i: number) => i >= 1)).toBe(true);
  });

  it("computes streaks correctly", () => {
    const days: OverviewHeatmapDayDto[] = [
      { date: "2025-06-01", tokens: 100, cost_usd: 0.01, cost_status: "exact", event_count: 1 },
      { date: "2025-06-02", tokens: 200, cost_usd: 0.02, cost_status: "exact", event_count: 1 },
      { date: "2025-06-03", tokens: 0,   cost_usd: null, cost_status: "unavailable", event_count: 0 },
      { date: "2025-06-04", tokens: 300, cost_usd: 0.03, cost_status: "exact", event_count: 1 },
      { date: "2025-06-05", tokens: 400, cost_usd: 0.04, cost_status: "exact", event_count: 1 },
      { date: "2025-06-06", tokens: 500, cost_usd: 0.05, cost_status: "exact", event_count: 1 },
    ];

    const dto: OverviewHeatmapDto = {
      today: "2025-06-06",
      week_starts_on: 1,
      days,
    };

    const model = buildOverviewHeatmapModel(dto);
    expect(model.summary.longestActiveStreakLabel).toBe("3d");
    expect(model.summary.currentActiveStreakLabel).toBe("3d");
  });

  it("assigns month labels correctly", () => {
    const days: OverviewHeatmapDayDto[] = [
      { date: "2025-05-30", tokens: 100, cost_usd: 0.01, cost_status: "exact", event_count: 1 },
      { date: "2025-05-31", tokens: 200, cost_usd: 0.02, cost_status: "exact", event_count: 1 },
      { date: "2025-06-01", tokens: 300, cost_usd: 0.03, cost_status: "exact", event_count: 1 },
      { date: "2025-06-02", tokens: 400, cost_usd: 0.04, cost_status: "exact", event_count: 1 },
    ];

    const dto: OverviewHeatmapDto = {
      today: "2025-06-02",
      week_starts_on: 0, // Sunday
      days,
    };

    const model = buildOverviewHeatmapModel(dto);
    const monthLabels = model.columns.map((c) => c.monthLabel).filter(Boolean);
    expect(monthLabels.length).toBeGreaterThanOrEqual(1);
  });

  it("includes cost and event data in day model", () => {
    // Provide data covering the full 12-month grid window so the
    // first non-placeholder day maps to a DTO entry.
    const dto: OverviewHeatmapDto = {
      today: "2025-06-01",
      week_starts_on: 0,
      days: [
        { date: "2024-06-01", tokens: 0, cost_usd: null, cost_status: "unavailable", event_count: 0 },
        { date: "2025-06-01", tokens: 1000, cost_usd: 0.10, cost_status: "exact", event_count: 3 },
      ],
    };

    const model = buildOverviewHeatmapModel(dto);
    // Find the day that has the data we need
    const day = model.columns
      .flatMap((c) => c.days)
      .find((d) => d.date === "2025-06-01");

    expect(day).toBeDefined();
    expect(day!.costUsd).toBe(0.10);
    expect(day!.costStatus).toBe("exact");
    expect(day!.eventCount).toBe(3);
    expect(day!.tooltipCostLabel).toBe("$0.10");
    expect(day!.tooltipEventsLabel).toBe("3 events");
  });

  it("shows N/A for cost when unavailable", () => {
    const dto: OverviewHeatmapDto = {
      today: "2025-06-01",
      week_starts_on: 0,
      days: [
        { date: "2024-06-01", tokens: 0, cost_usd: null, cost_status: "unavailable", event_count: 0 },
        { date: "2025-06-01", tokens: 1000, cost_usd: null, cost_status: "unavailable", event_count: 3 },
      ],
    };

    const model = buildOverviewHeatmapModel(dto);
    const day = model.columns
      .flatMap((c) => c.days)
      .find((d) => d.date === "2025-06-01");
    expect(day).toBeDefined();
    expect(day!.tooltipCostLabel).toBe("N/A");
  });
});

// ─── summarizeHeatmapForDiagnostics ──────────────────────────────────────────

describe("summarizeHeatmapForDiagnostics", () => {
  it("reports legend [0], zero active days, and sparse=true when there is no activity", () => {
    const model = buildOverviewHeatmapModel({
      today: "2025-06-15",
      week_starts_on: 0,
      days: [],
    });
    const diag = summarizeHeatmapForDiagnostics(model);
    expect(diag.legend_levels).toEqual([0]);
    expect(diag.active_days).toBe(0);
    expect(diag.sparse).toBe(true);
    expect(diag.total_cells).toBeGreaterThan(0);
  });

  it("reports legend [0,1] and sparse=true for a few active days (sparse binning path)", () => {
    const model = buildOverviewHeatmapModel({
      today: "2025-06-15",
      week_starts_on: 0,
      days: [
        singleDayDto({ date: "2025-06-10", tokens: 500 }),
        singleDayDto({ date: "2025-06-11", tokens: 800 }),
      ],
    });
    const diag = summarizeHeatmapForDiagnostics(model);
    expect(diag.legend_levels).toEqual([0, 1]);
    expect(diag.active_days).toBe(2);
    expect(diag.sparse).toBe(true);
  });

  it("reports the full 5-tier ramp and sparse=false for dense activity", () => {
    const days: OverviewHeatmapDayDto[] = Array.from({ length: 26 }, (_, i) => {
      const d = String(i + 1).padStart(2, "0");
      return singleDayDto({ date: `2025-05-${d}`, tokens: (i + 1) * 100 });
    });
    const model = buildOverviewHeatmapModel({
      today: "2025-06-15",
      week_starts_on: 0,
      days,
    });
    const diag = summarizeHeatmapForDiagnostics(model);
    expect(diag.legend_levels).toEqual([0, 1, 2, 3, 4]);
    expect(diag.sparse).toBe(false);
    expect(diag.active_days).toBe(26);
    expect(diag.total_cells).toBeGreaterThanOrEqual(26);
  });
});

// ─── diagnosticsSignature ─────────────────────────────────────────────────────

describe("diagnosticsSignature", () => {
  it("is identical for distributions that differ only in total_cells (pure window-roll)", () => {
    const a: HeatmapDiagnostics = { legend_levels: [0, 1, 2, 3, 4], total_cells: 365, active_days: 26, sparse: false };
    const b: HeatmapDiagnostics = { legend_levels: [0, 1, 2, 3, 4], total_cells: 366, active_days: 26, sparse: false };
    expect(diagnosticsSignature(a)).toBe(diagnosticsSignature(b));
  });

  it("changes when active_days differs", () => {
    const a: HeatmapDiagnostics = { legend_levels: [0, 1], total_cells: 365, active_days: 2, sparse: true };
    const b: HeatmapDiagnostics = { legend_levels: [0, 1], total_cells: 365, active_days: 3, sparse: true };
    expect(diagnosticsSignature(a)).not.toBe(diagnosticsSignature(b));
  });

  it("changes when legend_levels differs (sparse vs full ramp)", () => {
    const a: HeatmapDiagnostics = { legend_levels: [0], total_cells: 365, active_days: 0, sparse: true };
    const b: HeatmapDiagnostics = { legend_levels: [0, 1, 2, 3, 4], total_cells: 365, active_days: 40, sparse: false };
    expect(diagnosticsSignature(a)).not.toBe(diagnosticsSignature(b));
  });
});
