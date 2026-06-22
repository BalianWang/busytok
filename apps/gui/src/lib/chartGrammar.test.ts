import { describe, it, expect } from "vitest";
import type { OverviewTrendBucketDto } from "@busytok/protocol-types";
import {
  buildTimelineBars,
  buildTimelineAxisModel,
  getTimelineBarRadius,
  sanitizeMetricValue,
} from "./chartGrammar";

// ─── Fixtures ─────────────────────────────────────────────────────────────────

const MOCK_BUCKETS: OverviewTrendBucketDto[] = [
  { key: "h0", label: "00:00", start_ms: 0, end_ms: 3_600_000, tokens: 500, cost_usd: 0.05, cost_status: "exact", event_count: 2, is_current: false },
  { key: "h1", label: "01:00", start_ms: 3_600_000, end_ms: 7_200_000, tokens: 1200, cost_usd: 0.12, cost_status: "exact", event_count: 3, is_current: false },
  { key: "h2", label: "02:00", start_ms: 7_200_000, end_ms: 10_800_000, tokens: 0, cost_usd: null, cost_status: "unavailable", event_count: 0, is_current: false },
  { key: "h3", label: "03:00", start_ms: 10_800_000, end_ms: 14_400_000, tokens: 3000, cost_usd: 0.30, cost_status: "exact", event_count: 5, is_current: true },
];

// ─── buildTimelineBars ────────────────────────────────────────────────────────

describe("buildTimelineBars", () => {
  it("returns bars with token values by default", () => {
    const bars = buildTimelineBars(MOCK_BUCKETS);
    expect(bars).toHaveLength(4);
    expect(bars[0]).toEqual({
      key: "h0", label: "00:00", value: 500,
      costUsd: 0.05, costStatus: "exact", eventCount: 2, isCurrent: false,
    });
    expect(bars[1].value).toBe(1200);
    expect(bars[2].value).toBe(0);
    expect(bars[3].value).toBe(3000);
  });

  it("returns bars with cost values when metric is cost", () => {
    const bars = buildTimelineBars(MOCK_BUCKETS, "cost");
    expect(bars[0].value).toBe(0.05);
    expect(bars[1].value).toBe(0.12);
    expect(bars[2].value).toBe(0); // null cost + unavailable → 0
    expect(bars[3].value).toBe(0.30);
  });

  it("returns empty array for empty buckets", () => {
    const bars = buildTimelineBars([]);
    expect(bars).toEqual([]);
  });

  it("preserves extra metadata in each bar", () => {
    const bars = buildTimelineBars(MOCK_BUCKETS);
    expect(bars[3].costUsd).toBe(0.30);
    expect(bars[3].costStatus).toBe("exact");
    expect(bars[3].eventCount).toBe(5);
    expect(bars[3].isCurrent).toBe(true);
  });
});

// ─── sanitizeMetricValue ──────────────────────────────────────────────────────

describe("sanitizeMetricValue", () => {
  it("returns 0 for negative values", () => {
    expect(sanitizeMetricValue(-5)).toBe(0);
  });

  it("returns 0 for NaN", () => {
    expect(sanitizeMetricValue(NaN)).toBe(0);
  });

  it("returns 0 for Infinity", () => {
    expect(sanitizeMetricValue(Infinity)).toBe(0);
  });

  it("returns the value for positive numbers", () => {
    expect(sanitizeMetricValue(100)).toBe(100);
    expect(sanitizeMetricValue(0)).toBe(0);
  });

  it("handles bigint values", () => {
    expect(sanitizeMetricValue(BigInt(5000))).toBe(5000);
  });

  it("handles string values", () => {
    expect(sanitizeMetricValue("300")).toBe(300);
  });
});

// ─── getTimelineBarRadius ──────────────────────────────────────────────────────

describe("getTimelineBarRadius", () => {
  it("returns small radius for high bar counts (day: 24, month: 30)", () => {
    expect(getTimelineBarRadius(24, 960)).toBe(3);
    expect(getTimelineBarRadius(30, 960)).toBe(3);
  });

  it("returns medium radius for mid-range bar counts", () => {
    expect(getTimelineBarRadius(15, 960)).toBe(6);
    expect(getTimelineBarRadius(20, 960)).toBe(6);
  });

  it("returns large radius for low bar counts in wide container", () => {
    expect(getTimelineBarRadius(7, 960)).toBe(9);
    expect(getTimelineBarRadius(12, 960)).toBe(9);
  });

  it("caps radius when bars are thin in a narrow container", () => {
    // 7 bars in 200px: usable = 132, estBarW ≈ 19, cap = 9 → base wins
    expect(getTimelineBarRadius(7, 200)).toBe(9);
    // 12 bars in 200px: usable = 132, estBarW ≈ 11, cap = 5
    expect(getTimelineBarRadius(12, 200)).toBe(5);
    // 7 bars in 150px: usable = 82, estBarW ≈ 11, cap = 5
    expect(getTimelineBarRadius(7, 150)).toBe(5);
  });

  it("returns 0 for zero bars", () => {
    expect(getTimelineBarRadius(0)).toBe(0);
  });
});

// ─── buildTimelineAxisModel ───────────────────────────────────────────────────

describe("buildTimelineAxisModel", () => {
  it("returns empty model for empty bars", () => {
    const model = buildTimelineAxisModel({ range: "day", bars: [], width: 960 });
    expect(model.primaryLabels).toEqual([]);
    expect(model.primaryGuideKeys).toEqual([]);
    expect(model.secondaryTickKeys).toEqual([]);
    expect(model.density).toBe("regular");
  });

  describe("day range", () => {
    const dayBars = Array.from({ length: 24 }, (_, i) => ({
      key: `h${i}`, label: `${String(i).padStart(2, "0")}:00`,
      value: i * 100, costUsd: null, costStatus: "unavailable" as const,
      eventCount: 0, isCurrent: false,
    }));

    it("returns hourly labels with regular density", () => {
      const model = buildTimelineAxisModel({ range: "day", bars: dayBars, width: 960 });
      expect(model.density).toBe("regular");
      // regular picks every 6 hours → 4 labels
      expect(model.primaryLabels.length).toBeGreaterThanOrEqual(3);
      expect(model.primaryLabels.length).toBeLessThanOrEqual(5);
      expect(model.secondaryTickKeys).toHaveLength(24);
    });

    it("returns compact labels when width < 720", () => {
      const model = buildTimelineAxisModel({ range: "day", bars: dayBars, width: 600 });
      expect(model.density).toBe("compact");
    });

    it("returns expanded labels when width >= 1200", () => {
      const model = buildTimelineAxisModel({ range: "day", bars: dayBars, width: 1400 });
      expect(model.density).toBe("expanded");
    });
  });

  describe("week range", () => {
    const weekBars = Array.from({ length: 7 }, (_, i) => ({
      key: `d${i}`, label: `Day ${i + 1}`,
      value: 100, costUsd: null, costStatus: "unavailable" as const,
      eventCount: 0, isCurrent: false,
    }));

    it("returns all days as primary labels", () => {
      const model = buildTimelineAxisModel({ range: "week", bars: weekBars, width: 960 });
      expect(model.primaryLabels).toHaveLength(7);
      expect(model.secondaryTickKeys).toHaveLength(7);
    });
  });

  describe("month range", () => {
    const monthBars = Array.from({ length: 30 }, (_, i) => ({
      key: `d${i + 1}`, label: `${i + 1}`,
      value: 100, costUsd: null, costStatus: "unavailable" as const,
      eventCount: 0, isCurrent: false,
    }));

    it("picks every 7th day for regular density", () => {
      const model = buildTimelineAxisModel({ range: "month", bars: monthBars, width: 960 });
      expect(model.density).toBe("regular");
      expect(model.primaryLabels.length).toBeGreaterThanOrEqual(4);
      expect(model.secondaryTickKeys).toHaveLength(30);
    });
  });

  describe("year range", () => {
    const yearBars = Array.from({ length: 12 }, (_, i) => ({
      key: `m${i + 1}`, label: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][i],
      value: 100, costUsd: null, costStatus: "unavailable" as const,
      eventCount: 0, isCurrent: false,
    }));

    it("returns all months as primary labels", () => {
      const model = buildTimelineAxisModel({ range: "year", bars: yearBars, width: 960 });
      expect(model.primaryLabels).toHaveLength(12);
      expect(model.secondaryTickKeys).toHaveLength(0);
    });
  });
});
