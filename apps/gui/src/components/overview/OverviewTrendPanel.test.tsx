import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { OverviewTrendPanel } from "./OverviewTrendPanel";

const mockUseOverviewTrend = vi.fn();

vi.mock("../../api/useBusytokData", () => ({
  useOverviewTrend: (...args: unknown[]) => mockUseOverviewTrend(...args),
}));

vi.mock("../charts/NivoTimelineChart", () => ({
  NivoTimelineChart: ({
    bars,
    activeKey,
    metric,
  }: {
    bars: unknown[];
    activeKey: string | null;
    metric: "cost" | "tokens";
  }) => (
    <div data-testid="mock-trend-chart">
      metric:{metric};active:{activeKey ?? "none"};bars:{bars.length}
    </div>
  ),
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function trendEnvelope(overrides: Record<string, unknown> = {}) {
  return {
    data: {
      trend: {
        range: "day",
        bucket_granularity: "hour",
        metric_options: ["tokens", "cost"],
        cost_status: "exact",
        buckets: [
          {
            key: "h1",
            label: "1 AM",
            start_ms: 1,
            end_ms: 2,
            tokens: 1200,
            cost_usd: 0.12,
            cost_status: "exact",
            event_count: 2,
            is_current: false,
          },
          {
            key: "h2",
            label: "2 AM",
            start_ms: 2,
            end_ms: 3,
            tokens: 2400,
            cost_usd: 0.24,
            cost_status: "exact",
            event_count: 3,
            is_current: true,
          },
        ],
      },
    },
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

describe("OverviewTrendPanel", () => {
  it("renders error before loading when the query failed without cached data", () => {
    mockUseOverviewTrend.mockReturnValue({
      data: null,
      isLoading: false,
      isError: true,
      isFetching: false,
    });

    render(<OverviewTrendPanel range="day" onRangeChange={() => {}} />);
    expect(screen.getByText("Trend data unavailable")).toBeDefined();
  });

  it("renders loading, stale, empty, and active chart states", () => {
    mockUseOverviewTrend.mockReturnValue({
      data: null,
      isLoading: true,
      isError: false,
      isFetching: false,
    });
    const { rerender } = render(<OverviewTrendPanel range="day" onRangeChange={() => {}} />);
    expect(screen.getByText("Loading trend data...")).toBeDefined();

    mockUseOverviewTrend.mockReturnValue({
      data: trendEnvelope({ is_stale: true }),
      isLoading: false,
      isError: false,
      isFetching: true,
    });
    rerender(<OverviewTrendPanel range="day" onRangeChange={() => {}} />);
    expect(screen.getByText("Refreshing trend data...")).toBeDefined();
    expect(screen.getByTestId("mock-trend-chart").textContent).toContain("active:none");

    mockUseOverviewTrend.mockReturnValue({
      data: trendEnvelope({
        data: {
          trend: {
            range: "day",
            bucket_granularity: "hour",
            metric_options: ["tokens", "cost"],
            cost_status: "exact",
            buckets: [],
          },
        },
      }),
      isLoading: false,
      isError: false,
      isFetching: false,
    });
    rerender(<OverviewTrendPanel range="day" onRangeChange={() => {}} />);
    // Empty state renders the chart component (which handles its own empty skeleton)
    expect(screen.getByTestId("mock-trend-chart")).not.toBeNull();
    expect(screen.getByTestId("mock-trend-chart").textContent).toContain("bars:0");
  });

  it("changes range and keeps unavailable cost data on the token metric", async () => {
    const user = userEvent.setup();
    const onRangeChange = vi.fn();

    mockUseOverviewTrend.mockReturnValue({
      data: trendEnvelope({
        data: {
          trend: {
            range: "day",
            bucket_granularity: "hour",
            metric_options: ["tokens", "cost"],
            cost_status: "unavailable",
            buckets: [
              {
                key: "h1",
                label: "1 AM",
                start_ms: 1,
                end_ms: 2,
                tokens: 1200,
                cost_usd: null,
                cost_status: "unavailable",
                event_count: 2,
                is_current: true,
              },
            ],
          },
        },
      }),
      isLoading: false,
      isError: false,
      isFetching: false,
    });

    render(<OverviewTrendPanel range="day" onRangeChange={onRangeChange} />);

    await user.click(screen.getByRole("button", { name: "Week" }));
    expect(onRangeChange).toHaveBeenCalledWith("week");

    await user.click(screen.getByRole("button", { name: "Cost" }));
    expect(screen.getByTestId("mock-trend-chart").textContent).toContain("metric:tokens");
  });

  it("switches to cost metrics when cost data is available", async () => {
    const user = userEvent.setup();

    mockUseOverviewTrend.mockReturnValue({
      data: trendEnvelope(),
      isLoading: false,
      isError: false,
      isFetching: false,
    });

    render(<OverviewTrendPanel range="day" onRangeChange={() => {}} />);
    await user.click(screen.getByRole("button", { name: "Cost" }));

    expect(screen.getByTestId("mock-trend-chart").textContent).toContain("metric:cost");
  });
});
