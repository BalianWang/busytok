//! Chart grammar: map backend trend buckets into chart-ready bar data and
//! compute axis models that adapt to the viewport width and time range.

import type { OverviewTrendBucketDto, CostStatusDto } from "@busytok/protocol-types";

// ─── Public types ─────────────────────────────────────────────────────────────

export interface ChartBarDatum {
  key: string;
  label: string;
  value: number;
  costUsd: number | null;
  costStatus: CostStatusDto;
  eventCount: number;
  isCurrent: boolean;
}

export interface TimelineAxisLabel {
  key: string;
  text: string;
}

export interface TimelineAxisTerminalLabel {
  align: "start" | "end";
  text: string;
}

export interface TimelineAxisModel {
  primaryLabels: TimelineAxisLabel[];
  primaryGuideKeys: string[];
  secondaryTickKeys: string[];
  terminalLabels?: TimelineAxisTerminalLabel[];
  density: "compact" | "regular" | "expanded";
}

// ─── Density helper ────────────────────────────────────────────────────────────

function getTimelineAxisDensity(width: number): TimelineAxisModel["density"] {
  if (width >= 1200) return "expanded";
  if (width < 720) return "compact";
  return "regular";
}

export function getTimelineBarRadius(barCount: number, plotWidth = 960): number {
  if (barCount === 0) return 0;
  const base = barCount > 20 ? 3 : barCount > 12 ? 6 : 9;
  const marginH = 56 + 12;
  const usable = plotWidth - marginH;
  if (usable <= 0) return base;
  const estBarW = usable / barCount;
  return Math.min(base, Math.floor(estBarW / 2));
}

// ─── Axis item picking helpers ─────────────────────────────────────────────────

function pickAxisItems(
  bars: ChartBarDatum[],
  step: number,
  includeLast = false,
): ChartBarDatum[] {
  const items = bars.filter((_, index) => index % step === 0);
  if (!includeLast || bars.length === 0) return items;
  const lastBar = bars[bars.length - 1];
  if (items.some((item) => item.key === lastBar.key)) return items;
  return [...items, lastBar];
}

function parseClockHourLabel(label: string): number | null {
  const match = label.match(/^(\d{1,2}):00$/);
  if (!match) return null;
  const hour = Number(match[1]);
  return Number.isInteger(hour) && hour >= 0 && hour <= 23 ? hour : null;
}

function parseEpochHourKey(key: string): number | null {
  const numericKey = Number(key);
  if (!Number.isFinite(numericKey)) return null;
  const isMilliseconds = Math.abs(numericKey) >= 1_000_000_000_000;
  const isSeconds = Math.abs(numericKey) >= 1_000_000_000;
  if (!isMilliseconds && !isSeconds) return null;
  const date = new Date(isMilliseconds ? numericKey : numericKey * 1000);
  return Number.isNaN(date.getTime()) ? null : date.getHours();
}

function pickDayAxisItems(
  bars: ChartBarDatum[],
  density: TimelineAxisModel["density"],
): ChartBarDatum[] {
  const interval = density === "compact" ? 12 : density === "expanded" ? 3 : 6;
  const selectedHours = new Set<number>();
  for (let hour = 0; hour < 24; hour += interval) {
    selectedHours.add(hour);
  }
  const items = bars.filter((bar) => {
    const hour = parseClockHourLabel(bar.label) ?? parseEpochHourKey(bar.key);
    return hour !== null && selectedHours.has(hour);
  });
  return items.length > 0 ? items : pickAxisItems(bars, interval);
}

// ─── Public API ────────────────────────────────────────────────────────────────

export function buildTimelineAxisModel({
  range,
  bars,
  width,
}: {
  range: "day" | "week" | "month" | "year";
  bars: ChartBarDatum[];
  width: number;
}): TimelineAxisModel {
  const density = getTimelineAxisDensity(width);

  if (bars.length === 0) {
    return {
      primaryLabels: [],
      primaryGuideKeys: [],
      secondaryTickKeys: [],
      density,
    };
  }

  if (range === "day") {
    const items = pickDayAxisItems(bars, density);
    return {
      primaryLabels: items.map((item) => ({ key: item.key, text: item.label })),
      primaryGuideKeys: items.map((item) => item.key),
      secondaryTickKeys: bars.map((bar) => bar.key),
      density,
    };
  }

  if (range === "week") {
    return {
      primaryLabels: bars.map((bar) => ({ key: bar.key, text: bar.label })),
      primaryGuideKeys: bars.map((bar) => bar.key),
      secondaryTickKeys: bars.map((bar) => bar.key),
      density,
    };
  }

  if (range === "month") {
    const step = density === "expanded" ? 5 : 7;
    const items = pickAxisItems(bars, step, density === "expanded");
    return {
      primaryLabels: items.map((item) => ({ key: item.key, text: item.label })),
      primaryGuideKeys: items.map((item) => item.key),
      secondaryTickKeys: bars.map((bar) => bar.key),
      density,
    };
  }

  // year
  return {
    primaryLabels: bars.map((bar) => ({ key: bar.key, text: bar.label })),
    primaryGuideKeys: bars.map((bar) => bar.key),
    secondaryTickKeys: [],
    density,
  };
}

// ─── Placeholder bars for empty-bucket axis skeleton ─────────────────────────

const MONTH_ABBR = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

function dateKey(d: Date): string {
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

function dateLabel(d: Date): string {
  return `${MONTH_ABBR[d.getMonth()]} ${d.getDate()}`;
}

const ZERO_BAR = {
  value: 0,
  costUsd: null,
  costStatus: "unavailable" as CostStatusDto,
  eventCount: 0,
  isCurrent: false,
};

export function buildPlaceholderBars(range: "day" | "week" | "month" | "year"): ChartBarDatum[] {
  if (range === "day") {
    return Array.from({ length: 24 }, (_, i) => ({
      key: `${i}`,
      label: `${i}:00`,
      ...ZERO_BAR,
    }));
  }

  if (range === "week") {
    const today = new Date();
    return Array.from({ length: 7 }, (_, i) => {
      const d = new Date(today);
      d.setDate(d.getDate() - (6 - i));
      return { key: dateKey(d), label: dateLabel(d), ...ZERO_BAR };
    });
  }

  if (range === "month") {
    const now = new Date();
    const year = now.getFullYear();
    const month = now.getMonth();
    const days = new Date(year, month + 1, 0).getDate();
    return Array.from({ length: days }, (_, i) => {
      const d = new Date(year, month, i + 1);
      return { key: dateKey(d), label: dateLabel(d), ...ZERO_BAR };
    });
  }

  // year
  const year = new Date().getFullYear();
  return Array.from({ length: 12 }, (_, i) => ({
    key: `${year}-${pad2(i + 1)}`,
    label: MONTH_ABBR[i],
    ...ZERO_BAR,
  }));
}

// ─── Metric value sanitisation ─────────────────────────────────────────────────

export function sanitizeMetricValue(value: unknown): number {
  const numeric =
    typeof value === "bigint"
      ? Number(value)
      : typeof value === "number"
        ? value
        : Number(value);

  if (!Number.isFinite(numeric) || numeric <= 0) return 0;
  return numeric;
}

// ─── Bar builder ───────────────────────────────────────────────────────────────

export function buildTimelineBars(
  buckets: OverviewTrendBucketDto[],
  metric: "cost" | "tokens" = "tokens",
): ChartBarDatum[] {
  return buckets.map((bucket) => {
    let value: number;
    if (metric === "cost") {
      if (bucket.cost_status === "unavailable") {
        value = 0;
      } else if (bucket.cost_usd != null) {
        value = bucket.cost_usd;
      } else {
        value = 0;
      }
    } else {
      value = sanitizeMetricValue(bucket.tokens);
    }
    return {
      key: bucket.key,
      label: bucket.label,
      value,
      costUsd: bucket.cost_usd,
      costStatus: bucket.cost_status,
      eventCount: bucket.event_count,
      isCurrent: bucket.is_current,
    };
  });
}
