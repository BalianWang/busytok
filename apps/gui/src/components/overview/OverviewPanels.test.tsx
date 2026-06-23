import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { OverviewHeatmapPanel } from "./OverviewHeatmapPanel";
import { OverviewRankingsPanel } from "./OverviewRankingsPanel";
import { OverviewSummaryPanel } from "./OverviewSummaryPanel";

const mockUseOverviewSummary = vi.fn();
const mockUseOverviewHeatmap = vi.fn();
const mockUseOverviewRankings = vi.fn();

vi.mock("../../api/useBusytokData", () => ({
  useOverviewSummary: (...args: unknown[]) => mockUseOverviewSummary(...args),
  useOverviewHeatmap: (...args: unknown[]) => mockUseOverviewHeatmap(...args),
  useOverviewRankings: (...args: unknown[]) => mockUseOverviewRankings(...args),
}));

vi.mock("./OverviewTokenHeatmap", () => ({
  OverviewTokenHeatmap: ({ model }: { model: { columns: unknown[] } }) => (
    <div data-testid="mock-heatmap">columns:{model.columns.length}</div>
  ),
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function envelope<T>(data: T, overrides: Record<string, unknown> = {}) {
  return {
    data,
    generated_at_ms: 1,
    generation_id: "gen-1",
    readiness: "ready_exact",
    is_exact: true,
    is_stale: false,
    watermark_ms: null,
    progress: null,
    degraded_reason: null,
    ...overrides,
  };
}

describe("OverviewSummaryPanel", () => {
  it("renders loading, error, and metric states without a panel-level degraded banner", () => {
    mockUseOverviewSummary.mockReturnValue({ data: null, isLoading: true, isError: false });
    render(<OverviewSummaryPanel range="day" />);
    expect(screen.getByText("Loading summary data...")).toBeDefined();

    cleanup();
    mockUseOverviewSummary.mockReturnValue({ data: null, isLoading: false, isError: true });
    render(<OverviewSummaryPanel range="week" />);
    expect(screen.getByText("Summary unavailable")).toBeDefined();

    // Degraded envelope: the panel must render metric cards but must NOT
    // surface a degraded banner — banner ownership lives on the page
    // (OverviewPage) to avoid duplicate warnings on the same screen.
    cleanup();
    mockUseOverviewSummary.mockReturnValue({
      data: envelope(
        {
          timezone: "Asia/Shanghai",
          selected_range: "month",
          cost_status: "exact",
          generated_at_ms: 1,
          metrics: [
            {
              id: "tokens",
              label: "tokens",
              value: "12k",
              helper: "today",
              tone: "success",
            },
            {
              id: "events",
              label: "events",
              value: "3",
              helper: null,
              tone: "neutral",
            },
            {
              id: "errors",
              label: "errors",
              value: "1",
              helper: null,
              tone: "danger",
            },
          ],
        },
        { is_exact: false, degraded_reason: "building aggregates" },
      ),
      isLoading: false,
      isError: false,
    });
    render(<OverviewSummaryPanel range="month" />);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByText("TOKENS")).toBeDefined();
    expect(screen.getByText("12k")).toBeDefined();

    // Neutralized model: success renders as neutral (opaque surface, no
    // accent bar), neutral has no flag either, and only danger/warning
    // carry the 2px top-flag exception cue.
    const cards = screen.getAllByText(/TOKENS|EVENTS|ERRORS/).map((el) => el.closest(".metric-card")!);
    // success + neutral cards have no top-accent; the danger card does.
    const flagged = cards.filter((c) => c.querySelector(".metric-card__top-accent"));
    expect(flagged.length).toBe(1);
    expect(flagged[0].className).toContain("metric-card--danger");
    // success must never be emitted as a class (renders as neutral).
    const successCards = cards.filter((c) => c.className.includes("metric-card--success"));
    expect(successCards.length).toBe(0);
    const neutralCards = cards.filter((c) => c.className.includes("metric-card--neutral"));
    expect(neutralCards.length).toBe(2);

    cleanup();
    mockUseOverviewSummary.mockReturnValue({
      data: envelope(
        {
          timezone: "Asia/Shanghai",
          selected_range: "month",
          cost_status: "exact",
          generated_at_ms: 1,
          metrics: [],
        },
        { is_stale: true },
      ),
      isLoading: false,
      isError: false,
    });
    render(<OverviewSummaryPanel range="month" />);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("OverviewHeatmapPanel", () => {
  it("renders loading, error, empty, and heatmap states", () => {
    mockUseOverviewHeatmap.mockReturnValue({ data: null, isLoading: true, isError: false });
    render(<OverviewHeatmapPanel range="day" />);
    expect(screen.getByText("Loading heatmap...")).toBeDefined();

    cleanup();
    mockUseOverviewHeatmap.mockReturnValue({ data: null, isLoading: false, isError: true });
    render(<OverviewHeatmapPanel range="day" />);
    expect(screen.getByText("Heatmap unavailable")).toBeDefined();

    cleanup();
    mockUseOverviewHeatmap.mockReturnValue({
      data: envelope({ heatmap: null }),
      isLoading: false,
      isError: false,
    });
    render(<OverviewHeatmapPanel range="day" />);
    expect(screen.getByText("No heatmap data available.")).toBeDefined();

    cleanup();
    mockUseOverviewHeatmap.mockReturnValue({
      data: envelope({
        heatmap: {
          today: "2026-05-27",
          week_starts_on: 0,
          days: [
            {
              date: "2026-05-27",
              tokens: 100,
              cost_usd: 0.01,
              cost_status: "exact",
              event_count: 2,
            },
          ],
        },
      }),
      isLoading: false,
      isError: false,
    });
    render(<OverviewHeatmapPanel range="day" />);
    expect(screen.getByTestId("mock-heatmap")).toBeDefined();
  });
});

describe("OverviewRankingsPanel", () => {
  it("renders loading, error, empty, section, and per-section empty states", () => {
    mockUseOverviewRankings.mockReturnValue({ data: null, isLoading: true, isError: false });
    render(<OverviewRankingsPanel range="day" />);
    expect(screen.getByText("Loading rankings...")).toBeDefined();

    cleanup();
    mockUseOverviewRankings.mockReturnValue({ data: null, isLoading: false, isError: true });
    render(<OverviewRankingsPanel range="day" />);
    expect(screen.getByText("Rankings unavailable")).toBeDefined();

    cleanup();
    mockUseOverviewRankings.mockReturnValue({
      data: envelope({
        rankings: [
          { id: "costs", title: "Top Costs", items: [] },
          { id: "models", title: "Top Models", items: [] },
        ],
      }),
      isLoading: false,
      isError: false,
    });
    render(<OverviewRankingsPanel range="day" />);
    expect(screen.getByText("Top Costs")).toBeDefined();
    expect(screen.getByText("Top Models")).toBeDefined();
    expect(screen.getAllByText("No data").length).toBe(2);

    cleanup();
    mockUseOverviewRankings.mockReturnValue({
      data: envelope({
        rankings: [
          {
            id: "projects",
            title: "Projects",
            items: [
              {
                id: "p1",
                label: "Autoken",
                value: "10k",
                helper: null,
                bar_value: 80,
                action: null,
              },
            ],
          },
          { id: "models", title: "Models", items: [] },
        ],
      }),
      isLoading: false,
      isError: false,
    });
    render(<OverviewRankingsPanel range="day" />);
    expect(screen.getByText("Projects")).toBeDefined();
    expect(screen.getByText("Autoken")).toBeDefined();
    expect(screen.getByText("No data")).toBeDefined();
  });
});
