import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import { createChart } from "lightweight-charts";
import { buildDisplayLiveCurveSamples } from "../../api/liveSmoothing";
import { chartTokens } from "../../lib/chartTokens";

afterEach(cleanup);

let mockLiveState = {
  samples: [
    { bucket_start_ms: 1000, tokens_per_sec: 10, cost_per_sec: null, events_per_sec: 2, is_exact: true },
    { bucket_start_ms: 3000, tokens_per_sec: 20, cost_per_sec: null, events_per_sec: 3, is_exact: true },
  ],
  smoothedSamples: [
    {
      bucket_start_ms: 1000,
      tokens_per_min: 600,
      cost_per_min: null,
      events_per_min: 120,
      raw_tokens_per_min: 600,
      raw_peak_tokens_per_min: 600,
      raw_event_count: 4,
      is_exact: true,
    },
    {
      bucket_start_ms: 3000,
      tokens_per_min: 900,
      cost_per_min: null,
      events_per_min: 150,
      raw_tokens_per_min: 1200,
      raw_peak_tokens_per_min: 1200,
      raw_event_count: 6,
      is_exact: true,
    },
  ],
  isLoading: false,
  hasTransient: false,
};

vi.mock("../../api/useLiveSamples", () => ({
  useLiveSamples: () => mockLiveState,
}));

vi.mock("../../api/useEventSubscription", () => ({
  useEventSubscription: () => ({
    connectionStatus: "connected" as const,
  }),
}));

const chartMocks = vi.hoisted(() => {
  const mockTokenSeries = {
    setData: vi.fn(),
    applyOptions: vi.fn(),
  };
  const mockTimeScale = {
    applyOptions: vi.fn(),
    setVisibleRange: vi.fn(),
  };
  const mockChart = {
    addSeries: vi.fn().mockReturnValue(mockTokenSeries),
    applyOptions: vi.fn(),
    remove: vi.fn(),
    timeScale: vi.fn(() => mockTimeScale),
    subscribeCrosshairMove: vi.fn(),
    unsubscribeCrosshairMove: vi.fn(),
  };
  return { mockTokenSeries, mockTimeScale, mockChart };
});

vi.mock("lightweight-charts", () => {
  return {
    createChart: vi.fn(() => chartMocks.mockChart),
    AreaSeries: {},
    ColorType: { Solid: "solid" },
    CrosshairMode: { Normal: 0 },
    LineType: { Curved: 2 },
    LineStyle: { Dotted: 1 },
  };
});

import { LiveCurvePanel } from "./LiveCurvePanel";

describe("LiveCurvePanel", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
    document.documentElement.removeAttribute("data-theme");
    mockLiveState = {
      samples: [
        { bucket_start_ms: 1000, tokens_per_sec: 10, cost_per_sec: null, events_per_sec: 2, is_exact: true },
        { bucket_start_ms: 3000, tokens_per_sec: 20, cost_per_sec: null, events_per_sec: 3, is_exact: true },
      ],
      smoothedSamples: [
        {
          bucket_start_ms: 1000,
          tokens_per_min: 600,
          cost_per_min: null,
          events_per_min: 120,
          raw_tokens_per_min: 600,
          raw_peak_tokens_per_min: 600,
          raw_event_count: 4,
          is_exact: true,
        },
        {
          bucket_start_ms: 3000,
          tokens_per_min: 900,
          cost_per_min: null,
          events_per_min: 150,
          raw_tokens_per_min: 1200,
          raw_peak_tokens_per_min: 1200,
          raw_event_count: 6,
          is_exact: true,
        },
      ],
      isLoading: false,
      hasTransient: false,
    };
    chartMocks.mockChart.addSeries.mockReset().mockReturnValue(chartMocks.mockTokenSeries);
  });

  it("renders the panel title", () => {
    render(<LiveCurvePanel />);
    expect(screen.getByText("Real-time Throughput")).toBeDefined();
  });

  it("uses dedicated live-series tokens instead of the generic data.primary role", () => {
    const computedStyleSpy = vi
      .spyOn(window, "getComputedStyle")
      .mockImplementation(() => {
        return {
          getPropertyValue: (name: string) => {
            switch (name) {
              case "--color-data-live-primary":
                return "#4f63f6";
              case "--color-data-live-primary-soft":
                return "rgba(79, 99, 246, 0.08)";
              case "--color-text-muted":
                return "#6e7681";
              case "--color-border-subtle":
                return "rgba(255, 255, 255, 0.06)";
              default:
                return "";
            }
          },
        } as CSSStyleDeclaration;
      });

    render(<LiveCurvePanel />);

    expect(chartMocks.mockChart.addSeries).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({
        lineColor: "#4f63f6",
      }),
    );

    const livePill = screen.getByText("● Live");
    expect(livePill.style.color).toBe("var(--color-data-live-primary)");

    computedStyleSpy.mockRestore();
  });

  it("renders display-smoothed samples as the primary token series", () => {
    render(<LiveCurvePanel />);
    const display = buildDisplayLiveCurveSamples(mockLiveState.smoothedSamples);

    expect(chartMocks.mockTokenSeries.setData).toHaveBeenCalledWith([
      { time: 1, value: display[0].display_tokens_per_min },
      { time: 3, value: display[1].display_tokens_per_min },
    ]);
  });

  it("shows live indicator when connected and no transient samples", () => {
    render(<LiveCurvePanel />);
    expect(screen.getByText("● Live")).toBeDefined();
  });

  it("recomputes the live area fill when the document theme changes", async () => {
    document.documentElement.dataset.theme = "light";

    const computedStyleSpy = vi
      .spyOn(window, "getComputedStyle")
      .mockImplementation((element) => {
        const target = element as HTMLElement;
        const theme = target.dataset.theme ?? "light";
        const lightFill = "rgba(79, 99, 246, 0.08)";
        const darkFill = "rgba(167, 184, 255, 0.10)";

        return {
          getPropertyValue: (name: string) => {
            if (name === "--color-data-live-primary-soft") {
              return theme === "dark" ? darkFill : lightFill;
            }
            if (name === "--color-data-live-primary") {
              return theme === "dark" ? "#a7b8ff" : "#4f63f6";
            }
            if (name === "--color-text-muted") {
              return theme === "dark" ? "#8b949e" : "#6e7480";
            }
            if (name === "--color-border-subtle") {
              return theme === "dark"
                ? "rgba(255, 255, 255, 0.06)"
                : "rgba(17, 24, 39, 0.08)";
            }
            return "";
          },
        } as CSSStyleDeclaration;
      });

    render(<LiveCurvePanel />);

    expect(chartMocks.mockChart.addSeries).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({
        lineColor: "#4f63f6",
        topColor: "rgba(79, 99, 246, 0.08)",
      }),
    );

    chartMocks.mockTokenSeries.applyOptions.mockClear();

    await act(async () => {
      document.documentElement.dataset.theme = "dark";
      await Promise.resolve();
    });

    expect(chartMocks.mockTokenSeries.applyOptions).toHaveBeenCalledWith(
      expect.objectContaining({
        lineColor: "#a7b8ff",
        topColor: "rgba(167, 184, 255, 0.10)",
      }),
    );

    computedStyleSpy.mockRestore();
  });

  it("does not show transient banner when all samples are exact", () => {
    render(<LiveCurvePanel />);
    expect(screen.queryByText(/transient samples/)).toBeNull();
  });

  it("shows loading state when data is not yet available", () => {
    mockLiveState = { samples: [], smoothedSamples: [], isLoading: true, hasTransient: false };

    render(<LiveCurvePanel />);

    expect(screen.getByTestId("live-curve-chart-frame")).toBeDefined();
    expect(screen.getByText("Loading...")).toBeDefined();
  });

  it("hides the chart attribution logo, disables user time-axis panning, and uses autoSize", () => {
    render(<LiveCurvePanel />);

    expect(createChart).toHaveBeenCalledWith(
      expect.any(HTMLDivElement),
      expect.objectContaining({
        autoSize: true,
        handleScroll: false,
        handleScale: false,
        layout: expect.objectContaining({
          attributionLogo: false,
        }),
      }),
    );
  });

  it("locks the visible time range to the latest 15 minutes", () => {
    vi.useFakeTimers();
    const now = new Date("2026-05-26T02:00:00.000Z");
    const nowSeconds = Math.floor(now.getTime() / 1000);
    vi.setSystemTime(now);

    render(<LiveCurvePanel />);

    expect(chartMocks.mockTimeScale.setVisibleRange).toHaveBeenCalledWith({
      from: nowSeconds - 15 * 60,
      to: nowSeconds,
    });
  });

  it("uses autoSize instead of manual window.resize dimension syncing", () => {
    // autoSize: true (confirmed in the createChart assertion test above)
    // delegates dimension tracking to lightweight-charts' internal
    // ResizeObserver. No manual applyOptions({ width }) call after
    // window.resize — the chart library handles sizing internally.
    render(<LiveCurvePanel />);
    chartMocks.mockChart.applyOptions.mockClear();

    act(() => {
      window.dispatchEvent(new Event("resize"));
    });

    expect(chartMocks.mockChart.applyOptions).not.toHaveBeenCalled();
  });

  it("relocks the visible time range on data-update (smoothedSamples change)", () => {
    // With autoSize: true, window.resize no longer triggers a manual relock.
    // The relock must still happen — driven by the data-update useEffect
    // which fires each time displaySamples changes (every live-data tick).
    // This test proves subsequent relock, not just the initial render.
    vi.useFakeTimers();
    const now = new Date("2026-05-26T02:00:00.000Z");
    const nowSeconds = Math.floor(now.getTime() / 1000);
    vi.setSystemTime(now);

    const { rerender } = render(<LiveCurvePanel />);

    // Clear the initial-render lock call to isolate the data-update path.
    chartMocks.mockTimeScale.setVisibleRange.mockClear();

    // Change smoothedSamples so useMemo produces a different displaySamples
    // array, triggering the data-update useEffect's lockToLiveWindow call.
    mockLiveState = {
      ...mockLiveState,
      smoothedSamples: [
        {
          bucket_start_ms: 5000,
          tokens_per_min: 800,
          cost_per_min: null,
          events_per_min: 100,
          raw_tokens_per_min: 800,
          raw_peak_tokens_per_min: 800,
          raw_event_count: 4,
          is_exact: true,
        },
      ],
    };

    rerender(<LiveCurvePanel />);

    // The data-update effect must relock after smoothedSamples change.
    expect(chartMocks.mockTimeScale.setVisibleRange).toHaveBeenCalledTimes(1);
    expect(chartMocks.mockTimeScale.setVisibleRange).toHaveBeenCalledWith({
      from: nowSeconds - 15 * 60,
      to: nowSeconds,
    });
  });

  it("routes transient live state through chartTokens.lineAttention (data.attention, not status.warning)", () => {
    // Contract guard: transient live samples describe in-progress analytical
    // data and must surface via data.attention — never status.warning. Both
    // the connection pill color and the transient banner color must equal
    // chartTokens.lineAttention.
    mockLiveState = {
      ...mockLiveState,
      hasTransient: true,
    };

    render(<LiveCurvePanel />);

    // The connection pill switches to the Live (partial) label and its
    // inline color is exactly chartTokens.lineAttention.
    const partialPill = screen.getByText("Live (partial)");
    expect(partialPill).toBeDefined();
    expect(partialPill.style.color).toBe(chartTokens.lineAttention);

    // The transient banner also routes through chartTokens.lineAttention.
    const banner = screen.getByText(/transient samples/, { exact: false });
    expect(banner.style.color).toBe(chartTokens.lineAttention);

    // And the underlying token must be data.attention, not status.warning.
    expect(chartTokens.lineAttention).toBe("var(--color-data-attention)");
    expect(chartTokens.lineAttention).not.toContain("status-warning");
  });
});
