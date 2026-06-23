//! OverviewPage — page-level orchestration contract.
//!
//! Guards the Task 5 redesign: the page must read as one coherent monitoring
//! surface with summary metrics first, trend/heatmap as primary visual
//! anchors, and recent activity / rankings sitting one material layer
//! quieter. It also locks in the death of the legacy left-accent-bar
//! treatment for summary metric cards.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { OverviewMetricDto } from "@busytok/protocol-types";
import { OverviewPage } from "./OverviewPage";

// ── Mocks ───────────────────────────────────────────────────────────
//
// We mock the data hooks so the page renders in its populated form
// without requiring a live backend. We mock the heavy chart/heatmap
// children because their internal rendering is covered by their own
// unit tests; here we only care about page-level structure.

const mockUseOverviewSummary = vi.fn();
const mockUseOverviewTrend = vi.fn();
const mockUseOverviewHeatmap = vi.fn();
const mockUseOverviewRankings = vi.fn();
const mockUseActivityRecent = vi.fn();
const mockUseRefreshToolbar = vi.fn();

vi.mock("../api/useBusytokData", () => ({
  DEFAULT_OVERVIEW_RANGE: "day" as const,
  useOverviewSummary: (...args: unknown[]) => mockUseOverviewSummary(...args),
  useOverviewTrend: (...args: unknown[]) => mockUseOverviewTrend(...args),
  useOverviewHeatmap: (...args: unknown[]) => mockUseOverviewHeatmap(...args),
  useOverviewRankings: (...args: unknown[]) => mockUseOverviewRankings(...args),
  useActivityRecent: (...args: unknown[]) => mockUseActivityRecent(...args),
}));

vi.mock("../components/desktop/useRefreshToolbar", () => ({
  useRefreshToolbar: (...args: unknown[]) => mockUseRefreshToolbar(...args),
}));

vi.mock("../components/charts/NivoTimelineChart", () => ({
  NivoTimelineChart: () => <div data-testid="mock-trend-chart" />,
}));

vi.mock("../components/overview/OverviewTokenHeatmap", () => ({
  OverviewTokenHeatmap: () => <div data-testid="mock-heatmap" />,
}));

vi.mock("../components/overview/LiveCurvePanel", () => ({
  LiveCurvePanel: () => <div data-testid="mock-live-curve" />,
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

// ── Fixtures ────────────────────────────────────────────────────────

function envelope<T>(data: T, overrides: Record<string, unknown> = {}) {
  return {
    data,
    generated_at_ms: 1,
    generation_id: "gen-1",
    readiness: "ready_exact" as const,
    is_exact: true,
    is_stale: false,
    watermark_ms: null,
    progress: null,
    degraded_reason: null,
    ...overrides,
  };
}

function summaryMetrics(): Array<OverviewMetricDto> {
  return [
    { id: "tokens", label: "tokens", value: "12k", helper: "today", tone: "success" },
    { id: "events", label: "events", value: "3", helper: null, tone: "neutral" },
    { id: "cost", label: "cost", value: "$0.42", helper: null, tone: "warning" },
  ];
}

function stubAllPanelsPopulated() {
  mockUseOverviewSummary.mockReturnValue({
    data: envelope({
      timezone: "Asia/Shanghai",
      selected_range: "day",
      cost_status: "exact",
      generated_at_ms: 1,
      metrics: summaryMetrics(),
    }),
    isLoading: false,
    isError: false,
    isFetching: false,
  });

  mockUseOverviewTrend.mockReturnValue({
    data: envelope({
      trend: {
        range: "day",
        bucket_granularity: "hour",
        metric_options: ["tokens", "cost"],
        cost_status: "exact",
        buckets: [],
      },
    }),
    isLoading: false,
    isError: false,
    isFetching: false,
  });

  mockUseOverviewHeatmap.mockReturnValue({
    data: envelope({
      heatmap: {
        today: "2026-05-27",
        week_starts_on: 0,
        days: [],
      },
    }),
    isLoading: false,
    isError: false,
  });

  mockUseOverviewRankings.mockReturnValue({
    data: envelope({ rankings: [] }),
    isLoading: false,
    isError: false,
  });

  mockUseActivityRecent.mockReturnValue({
    data: envelope({ recent_activity: [] }),
    isLoading: false,
    isError: false,
  });
}

// ── Tests ───────────────────────────────────────────────────────────

describe("OverviewPage", () => {
  it("renders the summary surface before recent activity and supporting sections", () => {
    stubAllPanelsPopulated();
    render(<OverviewPage />);

    // Summary metric cards render their (uppercased) labels.
    expect(screen.getByText("TOKENS")).toBeDefined();

    // Recent activity is the trailing supporting section.
    expect(
      screen.getByRole("heading", { name: /recent activity/i }),
    ).toBeDefined();

    // Trend + heatmap are present as primary visual anchors.
    expect(screen.getByRole("heading", { name: /usage trend/i })).toBeDefined();
    expect(screen.getByTestId("mock-trend-chart")).toBeDefined();
    expect(screen.getByTestId("mock-heatmap")).toBeDefined();
  });

  it("does not rely on a left-accent-bar-only treatment for summary metrics", () => {
    stubAllPanelsPopulated();
    render(<OverviewPage />);

    // The legacy design encoded each metric card's tone purely as a 3px
    // left border strip on `.metric-card--<tone>`. The redesign moves the
    // semantic signal to (a) surface hierarchy and (b) a subtle top
    // accent / tinted wash expressed as a structural element so it is
    // observable in jsdom.
    //
    // Legacy regression guard: no element should claim the left-strip
    // role, and no card should still carry a `metric-card--<tone>` modifier
    // whose only purpose was to drive that left border.
    expect(
      document.querySelector(".metric-card__accent-strip"),
    ).toBeNull();
    expect(
      document.querySelector(".metric-card__left-accent"),
    ).toBeNull();

    // New direction guard (Phase 2 conditional top-accent model): the
    // top-accent flag is NOT universal. Neutral cards (the calm default —
    // including success, which renders as neutral) carry no accent bar;
    // only exception cards (warning/danger) get the 2px top flag. This is
    // the observable signal that tone is now carried by an exception flag
    // rather than a left border strip. Assert the conditional contract
    // against the rendered cards.
    const cards = document.querySelectorAll(".metric-card");
    expect(cards.length).toBeGreaterThan(0);
    cards.forEach((card) => {
      const isException =
        card.classList.contains("metric-card--warning") ||
        card.classList.contains("metric-card--danger");
      if (isException) {
        expect(card.querySelector(".metric-card__top-accent")).not.toBeNull();
      } else {
        expect(card.querySelector(".metric-card__top-accent")).toBeNull();
      }
    });
    // The fixture (success, neutral, warning) yields one warning card, so
    // exactly one top-accent flag should be present overall.
    expect(
      document.querySelectorAll(".metric-card__top-accent").length,
    ).toBe(1);
  });

  it("keeps metric values as neutral high-contrast text (helper copy may carry tone, the number must not)", () => {
    stubAllPanelsPopulated();
    render(<OverviewPage />);

    const valueNodes = document.querySelectorAll(".metric-card__value");
    expect(valueNodes.length).toBeGreaterThan(0);
    valueNodes.forEach((node) => {
      // The value must not adopt a semantic tone class — it stays neutral.
      const cls = node.getAttribute("class") ?? "";
      expect(cls).not.toMatch(/is-(success|warning|danger|info)/);
    });
  });

  it("uses the shared segmented control for trend range/metric selection", () => {
    stubAllPanelsPopulated();
    render(<OverviewPage />);

    // The shared SegmentedControl renders an aria-label matching the
    // panel's own labels ("Range", "Chart metric").
    expect(screen.getByRole("group", { name: "Range" })).toBeDefined();
    expect(screen.getByRole("group", { name: "Chart metric" })).toBeDefined();
  });

  it("renders exactly one degraded ribbon (page-level owner) when the envelope is degraded", () => {
    // Degraded is now a thin page-level ribbon (dot + line), not a centered
    // PageState card. Banner ownership stays on the page (not on the summary
    // panel) — duplicate banners on the same screen were a regression. Verify
    // exactly one ribbon surfaces for a degraded envelope.
    mockUseOverviewSummary.mockReturnValue({
      data: envelope(
        {
          timezone: "Asia/Shanghai",
          selected_range: "day",
          cost_status: "approximate",
          generated_at_ms: 1,
          metrics: summaryMetrics(),
        },
        { is_exact: false, degraded_reason: "building aggregates" },
      ),
      isLoading: false,
      isError: false,
      isFetching: false,
    });
    mockUseOverviewTrend.mockReturnValue({
      data: envelope({
        trend: {
          range: "day",
          bucket_granularity: "hour",
          metric_options: ["tokens", "cost"],
          cost_status: "exact",
          buckets: [],
        },
      }),
      isLoading: false,
      isError: false,
      isFetching: false,
    });
    mockUseOverviewHeatmap.mockReturnValue({
      data: envelope({ heatmap: { today: "2026-05-27", week_starts_on: 0, days: [] } }),
      isLoading: false,
      isError: false,
    });
    mockUseOverviewRankings.mockReturnValue({
      data: envelope({ rankings: [] }),
      isLoading: false,
      isError: false,
    });
    mockUseActivityRecent.mockReturnValue({
      data: envelope({ recent_activity: [] }),
      isLoading: false,
      isError: false,
    });

    render(<OverviewPage />);

    // Page-level degraded ribbon surfaces exactly once (no centered card).
    const degradedRibbons = document.querySelectorAll(
      ".overview-console__degraded-ribbon",
    );
    expect(degradedRibbons.length).toBe(1);
    // The legacy centered PageState degraded card must not appear.
    expect(
      document.querySelectorAll('.page-state[data-state-kind="degraded"]')
        .length,
    ).toBe(0);

    // And the panel-level duplicate role="alert" banner must not appear.
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
