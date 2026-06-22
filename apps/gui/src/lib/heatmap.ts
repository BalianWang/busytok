//! Heatmap model builder: converts the dense OverviewHeatmapDto into a calendar
//! grid model with quartile-based intensity bins, streaks, and rich tooltip values.
//! All date arithmetic is UTC-based so strings like "2025-06-15" are treated as
//! pure calendar dates — no timezone drift.

import type { OverviewHeatmapDto, CostStatusDto } from "@busytok/protocol-types";
import { formatCompactNumber, formatCost } from "./formatters";

// ─── Public model types ────────────────────────────────────────────────────────

export interface OverviewHeatmapDay {
  key: string;
  date: string | null;
  tokens: bigint | null;
  costUsd: number | null;
  costStatus: CostStatusDto | null;
  eventCount: number | null;
  intensity: 0 | 1 | 2 | 3 | 4;
  isPlaceholder: boolean;
  isActive: boolean;
  tooltipDateLabel: string | null;
  tooltipTokensLabel: string | null;
  tooltipCostLabel: string | null;
  tooltipEventsLabel: string | null;
}

export interface OverviewHeatmapColumn {
  key: string;
  monthLabel: string | null;
  days: OverviewHeatmapDay[];
}

export interface OverviewHeatmapModel {
  totalTokens: bigint;
  totalTokensLabel: string;
  legendLevels: Array<0 | 1 | 2 | 3 | 4>;
  columns: OverviewHeatmapColumn[];
  summary: {
    mostActiveMonthLabel: string;
    mostActiveDayLabel: string;
    longestActiveStreakLabel: string;
    currentActiveStreakLabel: string;
  };
}

// ─── Constants ─────────────────────────────────────────────────────────────────

const FULL_DAYS = [
  "Sunday",
  "Monday",
  "Tuesday",
  "Wednesday",
  "Thursday",
  "Friday",
  "Saturday",
] as const;

const FULL_MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
] as const;

const SHORT_MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
] as const;

// ─── Date helpers (UTC-based for pure calendar arithmetic) ───────────────────

function parseUTCDate(dateStr: string): Date {
  const [y, m, d] = dateStr.split("-").map(Number);
  return new Date(Date.UTC(y, m - 1, d));
}

function formatUTCDate(d: Date): string {
  const y = d.getUTCFullYear();
  const m = String(d.getUTCMonth() + 1).padStart(2, "0");
  const day = String(d.getUTCDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

function addDays(dateStr: string, days: number): string {
  const d = parseUTCDate(dateStr);
  d.setUTCDate(d.getUTCDate() + days);
  return formatUTCDate(d);
}

function subtractCalendarMonths(dateStr: string, months: number): string {
  const [y, m, d] = dateStr.split("-").map(Number);
  let newMonth = m - months;
  let newYear = y;
  while (newMonth <= 0) {
    newMonth += 12;
    newYear--;
  }
  const daysInMonth = new Date(Date.UTC(newYear, newMonth, 0)).getUTCDate();
  const newDay = Math.min(d, daysInMonth);
  return `${newYear}-${String(newMonth).padStart(2, "0")}-${String(newDay).padStart(2, "0")}`;
}

function getDayOfWeek(dateStr: string): number {
  return parseUTCDate(dateStr).getUTCDay(); // 0=Sun … 6=Sat
}

function findWeekStart(dateStr: string, weekStartsOn: number): string {
  const dow = getDayOfWeek(dateStr);
  const offset = (dow - weekStartsOn + 7) % 7;
  return addDays(dateStr, -offset);
}

function findWeekEnd(dateStr: string, weekStartsOn: number): string {
  const dow = getDayOfWeek(dateStr);
  const offset = (weekStartsOn + 6 - dow + 7) % 7;
  return addDays(dateStr, offset);
}

function getISOWeekKey(dateStr: string): string {
  const d = parseUTCDate(dateStr);
  const target = new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate()));
  const dayNum = (target.getUTCDay() + 6) % 7; // Mon=0 … Sun=6
  target.setUTCDate(target.getUTCDate() + 3 - dayNum);
  const yearStart = new Date(Date.UTC(target.getUTCFullYear(), 0, 1));
  const weekNum = Math.ceil(
    ((target.getTime() - yearStart.getTime()) / 86_400_000 + 1) / 7,
  );
  return `${target.getUTCFullYear()}-W${String(weekNum).padStart(2, "0")}`;
}

// ─── Formatting helpers ─────────────────────────────────────────────────────

function normalizeTokenValue(tokens: number | bigint): bigint {
  return typeof tokens === "bigint" ? tokens : BigInt(tokens);
}

function formatTooltipDate(dateStr: string): string {
  const d = parseUTCDate(dateStr);
  const dayName = FULL_DAYS[d.getUTCDay()];
  const monthName = FULL_MONTHS[d.getUTCMonth()];
  return `${dayName}, ${monthName} ${d.getUTCDate()}, ${d.getUTCFullYear()}`;
}

function formatDayLabel(dateStr: string): string {
  const d = parseUTCDate(dateStr);
  const monthName = FULL_MONTHS[d.getUTCMonth()];
  return `${monthName} ${d.getUTCDate()}, ${d.getUTCFullYear()}`;
}

function formatMonthYearLabel(yearMonth: string): string {
  const [y, m] = yearMonth.split("-").map(Number);
  return `${FULL_MONTHS[m - 1]} ${y}`;
}

// ─── Main builder ────────────────────────────────────────────────────────────

export function buildOverviewHeatmapModel(dto: OverviewHeatmapDto): OverviewHeatmapModel {
  const { today, week_starts_on: weekStartsOn, days: dailyTimeline } = dto;

  // ── 1. Build lookup map ────────────────────────────────────────────────
  const tokenMap = new Map<string, { tokens: bigint; costUsd: number | null; costStatus: CostStatusDto; eventCount: number }>();
  for (const bucket of dailyTimeline) {
    tokenMap.set(bucket.date, {
      tokens: normalizeTokenValue(bucket.tokens),
      costUsd: bucket.cost_usd,
      costStatus: bucket.cost_status,
      eventCount: bucket.event_count,
    });
  }

  // ── 2. Determine in-range window ───────────────────────────────────────
  const startDate = subtractCalendarMonths(today, 12);

  // ── 3. Extend to full week boundaries ──────────────────────────────────
  const gridStart = findWeekStart(startDate, weekStartsOn);
  const gridEnd = findWeekEnd(today, weekStartsOn);

  // ── 4. Build columns with days ─────────────────────────────────────────
  const columns: OverviewHeatmapColumn[] = [];
  let totalTokens = 0n;
  let placeholderIndex = 0;

  let currentColumnStart = gridStart;
  while (currentColumnStart <= gridEnd) {
    const days: OverviewHeatmapDay[] = [];

    for (let offset = 0; offset < 7; offset++) {
      const date = addDays(currentColumnStart, offset);
      const inRange = date >= startDate && date <= today;

      if (inRange) {
        const entry = tokenMap.get(date);
        const tokens = entry?.tokens ?? 0n;
        if (tokens > 0n) totalTokens += tokens;

        days.push({
          key: date,
          date,
          tokens,
          costUsd: entry?.costUsd ?? null,
          costStatus: entry?.costStatus ?? null,
          eventCount: entry?.eventCount ?? null,
          intensity: 0,
          isPlaceholder: false,
          isActive: tokens > 0n,
          tooltipDateLabel: formatTooltipDate(date),
          tooltipTokensLabel: tokens > 0n ? `${formatCompactNumber(tokens)} tokens` : "0 tokens",
          tooltipCostLabel: entry ? formatCost(entry.costUsd, entry.costStatus) : null,
          tooltipEventsLabel: entry != null ? `${entry.eventCount} events` : null,
        });
      } else {
        days.push({
          key: `placeholder-${placeholderIndex++}`,
          date: null,
          tokens: null,
          costUsd: null,
          costStatus: null,
          eventCount: null,
          intensity: 0,
          isPlaceholder: true,
          isActive: false,
          tooltipDateLabel: null,
          tooltipTokensLabel: null,
          tooltipCostLabel: null,
          tooltipEventsLabel: null,
        });
      }
    }

    columns.push({
      key: getISOWeekKey(currentColumnStart),
      monthLabel: null,
      days,
    });

    currentColumnStart = addDays(currentColumnStart, 7);
  }

  // ── 5. Assign month labels ─────────────────────────────────────────────
  const seenMonths = new Set<string>();
  for (const column of columns) {
    for (const day of column.days) {
      if (day.isPlaceholder || day.date === null) continue;
      const month = day.date.substring(0, 7);
      if (!seenMonths.has(month)) {
        seenMonths.add(month);
        column.monthLabel = SHORT_MONTHS[parseInt(month.substring(5, 7), 10) - 1];
        break;
      }
    }
  }

  // ── 6. Compute intensity bins ──────────────────────────────────────────
  const allInRangeDays = columns
    .flatMap((c) => c.days)
    .filter((d) => !d.isPlaceholder);

  const nonZeroDays = allInRangeDays.filter((d) => d.isActive);

  let legendLevels: Array<0 | 1 | 2 | 3 | 4>;

  if (nonZeroDays.length === 0) {
    legendLevels = [0];
  } else if (nonZeroDays.length < 4) {
    legendLevels = [0, 1];
    for (const day of nonZeroDays) {
      day.intensity = 1;
    }
  } else {
    legendLevels = [0, 1, 2, 3, 4];
    const values = nonZeroDays.map((d) => d.tokens!);
    values.sort((a, b) => Number(a - b));

    const n = values.length;
    const q1 = quartileValue(values, 0.25);
    const q2 = quartileValue(values, 0.5);
    const q3 = quartileValue(values, 0.75);

    for (const day of nonZeroDays) {
      const v = day.tokens!;
      if (v <= q1) {
        day.intensity = 1;
      } else if (v <= q2) {
        day.intensity = 2;
      } else if (v <= q3) {
        day.intensity = 3;
      } else {
        day.intensity = 4;
      }
    }
  }

  // ── 7. Compute summary ─────────────────────────────────────────────────
  const summary = computeSummary(allInRangeDays, totalTokens);

  return {
    totalTokens,
    totalTokensLabel: formatCompactNumber(totalTokens),
    legendLevels,
    columns,
    summary,
  };
}

// ─── Quartile helper ─────────────────────────────────────────────────────────

function quartileValue(sorted: bigint[], p: number): bigint {
  const n = sorted.length;
  const pos = (n - 1) * p;
  const lo = Math.floor(pos);
  const hi = Math.ceil(pos);
  if (lo === hi) return sorted[lo];
  const frac = pos - lo;
  const loVal = Number(sorted[lo]);
  const hiVal = Number(sorted[hi]);
  return BigInt(Math.round(loVal + frac * (hiVal - loVal)));
}

// ─── Summary computation ─────────────────────────────────────────────────────

function computeSummary(
  allDays: OverviewHeatmapDay[],
  totalTokens: bigint,
): OverviewHeatmapModel["summary"] {
  if (totalTokens === 0n) {
    return {
      mostActiveMonthLabel: "No activity",
      mostActiveDayLabel: "No activity",
      longestActiveStreakLabel: "0d",
      currentActiveStreakLabel: "0d",
    };
  }

  // Most Active Month
  const monthTotals = new Map<string, bigint>();
  for (const day of allDays) {
    if (!day.isActive || day.date === null) continue;
    const month = day.date.substring(0, 7);
    monthTotals.set(month, (monthTotals.get(month) ?? 0n) + day.tokens!);
  }

  let bestMonth = "";
  let bestMonthTokens = -1n;
  for (const [month, tokens] of monthTotals) {
    if (tokens > bestMonthTokens || (tokens === bestMonthTokens && month > bestMonth)) {
      bestMonthTokens = tokens;
      bestMonth = month;
    }
  }
  const mostActiveMonthLabel = formatMonthYearLabel(bestMonth);

  // Most Active Day
  let bestDay = "";
  let bestDayTokens = -1n;
  for (const day of allDays) {
    if (!day.isActive || day.date === null) continue;
    if (day.tokens! > bestDayTokens || (day.tokens === bestDayTokens && day.date > bestDay)) {
      bestDayTokens = day.tokens!;
      bestDay = day.date;
    }
  }
  const mostActiveDayLabel = formatDayLabel(bestDay);

  // Longest streak
  let longest = 0;
  let currentRun = 0;
  for (const day of allDays) {
    if (day.isActive) {
      currentRun++;
      if (currentRun > longest) longest = currentRun;
    } else {
      currentRun = 0;
    }
  }

  // Current streak (consecutive active days ending on today)
  let currentStreak = 0;
  for (let i = allDays.length - 1; i >= 0; i--) {
    if (allDays[i].isActive) {
      currentStreak++;
    } else {
      break;
    }
  }

  return {
    mostActiveMonthLabel,
    mostActiveDayLabel,
    longestActiveStreakLabel: `${longest}d`,
    currentActiveStreakLabel: `${currentStreak}d`,
  };
}

// ─── Diagnostics (observability) ─────────────────────────────────────────────

/** Summarized heatmap distribution emitted to the logging system when the
 *  model is built, so "user can't see activity" reports can be triaged:
 *  was the year sparse (few legend tiers), all-zero, or fully binned?
 *
 *  Pure derivation over the model — no side effects — so it is unit-testable
 *  independently of the React/logging layers. */
export interface HeatmapDiagnostics {
  legend_levels: Array<0 | 1 | 2 | 3 | 4>;
  total_cells: number;
  active_days: number;
  sparse: boolean;
  // Index signature: this is a structured log payload consumed by the
  // generic reporter (Record<string, unknown>), so it must satisfy that
  // contract while keeping the typed keys above for callers and tests.
  [key: string]: unknown;
}

export function summarizeHeatmapForDiagnostics(model: OverviewHeatmapModel): HeatmapDiagnostics {
  let totalCells = 0;
  let activeDays = 0;
  for (const column of model.columns) {
    for (const day of column.days) {
      if (day.isPlaceholder) continue;
      totalCells += 1;
      if (day.isActive) activeDays += 1;
    }
  }
  return {
    legend_levels: model.legendLevels,
    total_cells: totalCells,
    active_days: activeDays,
    // <=2 distinct tiers means the no-activity ([0]) or sparse-active ([0,1])
    // path — the scenario where empty-vs-level-1 contrast matters most.
    sparse: model.legendLevels.length <= 2,
  };
}

/**
 * Stable signature of the heatmap distribution's user-meaningful fields.
 *
 * Used by the panel to suppress duplicate observability emissions when a
 * refetch returns an identical distribution (window focus / reconnect hand
 * back new object references even when the data is unchanged). Excludes
 * `total_cells` deliberately: a pure window-roll that adds an inactive day
 * changes the cell count but not the activity distribution the user sees, so
 * it should not re-trigger a log. Pure so it is unit-testable in isolation.
 */
export function diagnosticsSignature(diag: HeatmapDiagnostics): string {
  return `${diag.legend_levels.join(",")}|${diag.active_days}|${diag.sparse}`;
}
