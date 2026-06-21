import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { OverviewTokenHeatmap } from "./OverviewTokenHeatmap";
import type { OverviewHeatmapDay, OverviewHeatmapModel } from "../../lib/heatmap";

afterEach(() => cleanup());

function day(overrides: Partial<OverviewHeatmapDay> = {}): OverviewHeatmapDay {
  return {
    key: "2026-05-26",
    date: "2026-05-26",
    tokens: 1200n,
    costUsd: 0.12,
    costStatus: "exact",
    eventCount: 3,
    intensity: 3,
    isPlaceholder: false,
    isActive: true,
    tooltipDateLabel: "Tue, May 26, 2026",
    tooltipTokensLabel: "1.2k tokens",
    tooltipCostLabel: "$0.12",
    tooltipEventsLabel: "3 events",
    ...overrides,
  };
}

function placeholder(key: string): OverviewHeatmapDay {
  return day({
    key,
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

function model(columns = [
  {
    key: "week-1",
    monthLabel: "May",
    days: [
      placeholder("p1"),
      day(),
      day({ key: "2026-05-27", date: "2026-05-27", tooltipDateLabel: "Wed, May 27, 2026", intensity: 1 }),
      placeholder("p2"),
      day({ key: "2026-05-29", date: "2026-05-29", tooltipDateLabel: "Fri, May 29, 2026", intensity: 4 }),
      day({ key: "2026-05-30", date: "2026-05-30", tooltipDateLabel: "Sat, May 30, 2026", intensity: 2 }),
      day({ key: "2026-05-31", date: "2026-05-31", tooltipDateLabel: "Sun, May 31, 2026", intensity: 0 }),
    ],
  },
  {
    key: "week-2",
    monthLabel: null,
    days: [day({ key: "2026-06-01", date: "2026-06-01", tooltipDateLabel: "Mon, Jun 1, 2026" })],
  },
]): OverviewHeatmapModel {
  return {
    totalTokens: 2400n,
    totalTokensLabel: "2.4k",
    legendLevels: [0, 1, 2, 3, 4],
    columns,
    summary: {
      mostActiveMonthLabel: "May",
      mostActiveDayLabel: "Tue, May 26",
      longestActiveStreakLabel: "3 days",
      currentActiveStreakLabel: "1 day",
    },
  };
}

describe("OverviewTokenHeatmap", () => {
  it("renders totals, labels, placeholders, missing cells, and summary values", () => {
    render(<OverviewTokenHeatmap model={model()} />);

    expect(screen.getByText("Token Activity")).toBeDefined();
    expect(screen.getByText("2.4k")).toBeDefined();
    expect(screen.getAllByText("May").length).toBeGreaterThan(0);
    expect(screen.getByText("Most Active Month")).toBeDefined();
    expect(screen.getByText("3 days")).toBeDefined();
    expect(screen.getByTestId("heatmap-placeholder-p1")).toBeDefined();
    expect(document.querySelector(".overview-heatmap__cell--placeholder")).toBeDefined();
  });

  it("shows and hides a tooltip for in-range cells but not placeholders", () => {
    render(<OverviewTokenHeatmap model={model()} />);

    fireEvent.mouseEnter(screen.getByLabelText("Tue, May 26, 2026"), {
      clientX: 20,
      clientY: 30,
    });
    expect(screen.getByText("1.2k tokens")).toBeDefined();
    expect(screen.getByText("Cost: $0.12")).toBeDefined();
    expect(screen.getByText("3 events")).toBeDefined();

    fireEvent.mouseLeave(screen.getByLabelText("Tue, May 26, 2026"));
    expect(screen.queryByText("Cost: $0.12")).toBeNull();

    fireEvent.mouseEnter(screen.getByTestId("heatmap-placeholder-p1"));
    expect(screen.queryByText("1.2k tokens")).toBeNull();
  });

  it("falls back to Monday-first weekday labels when no dated cells exist", () => {
    render(<OverviewTokenHeatmap model={model([{ key: "empty", monthLabel: null, days: [] }])} />);

    expect(screen.getByText("Mo")).toBeDefined();
    expect(screen.getByText("Su")).toBeDefined();
  });
});

// ── Heatmap CSS contract ───────────────────────────────────────────────
//
// Lock the spec guarantee that the heatmap separates two visual roles via
// dedicated tokens: a NEUTRAL empty substrate (--color-heatmap-empty, NOT a
// data color) and a discrete indigo intensity ramp (--color-heatmap-level-1..4).
// The prior design conflated both into a single data-primary alpha ramp, which
// collapsed empty-vs-level-1 contrast (the bug fixed here). We read pages.css
// from disk (same pattern as tokens.test.ts) and assert against the literal CSS.

const pagesCss = readFileSync(
  pathToFileURL("./src/styles/pages.css"),
  "utf8",
);

// Slice out only the heatmap block so contract assertions are scoped to the
// heatmap ramp rules and not to unrelated references elsewhere.
const heatmapBlockStart = pagesCss.indexOf("/* ── Overview heatmap");
const heatmapBlockEnd = pagesCss.indexOf("/* ── Live curve panel");
const heatmapCss = pagesCss.slice(heatmapBlockStart, heatmapBlockEnd);

describe("OverviewTokenHeatmap CSS contract (pages.css)", () => {
  it("encodes the empty substrate with the dedicated neutral heatmap token", () => {
    // cell--0 must use the neutral empty token — NOT a data color. This is the
    // structural fix: "no activity" is a quiet substrate, not a tinted data cell.
    expect(heatmapCss).toContain("var(--color-heatmap-empty)");
    expect(heatmapCss).not.toContain("--color-data-primary-soft");
  });

  it("encodes active intensity via dedicated level tokens, not a data-primary alpha ramp", () => {
    expect(heatmapCss).toContain("var(--color-heatmap-level-1)");
    expect(heatmapCss).toContain("var(--color-heatmap-level-2)");
    expect(heatmapCss).toContain("var(--color-heatmap-level-3)");
    expect(heatmapCss).toContain("var(--color-heatmap-level-4)");
    // The ramp must not fall back to the generic data-primary family or to
    // ad-hoc color-mix alpha blending — both were the failure mode.
    expect(heatmapCss).not.toContain("--color-data-primary");
    expect(heatmapCss).not.toContain("color-mix");
  });

  it("does not encode intensity with success-green semantics", () => {
    // No raw success-green rgba literal in heatmap rules.
    expect(heatmapCss).not.toContain("rgba(109, 186, 120");
    expect(heatmapCss).not.toContain("rgba(126, 201, 138");

    // No --color-status-success reference inside the heatmap block.
    expect(heatmapCss).not.toContain("--color-status-success");
  });
});
