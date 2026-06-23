import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NivoTimelineChart, TimelineTooltipContent } from "./NivoTimelineChart";
import type { ChartBarDatum } from "../../lib/chartGrammar";
import { buildPlaceholderBars } from "../../lib/chartGrammar";
import { chartTokens } from "../../lib/chartTokens";

let resizeObserverWidth = 960;

// ResizeObserver is not available in jsdom
globalThis.ResizeObserver = class ResizeObserver {
  callback: ResizeObserverCallback;

  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
  }

  observe() {
    this.callback([
      {
        contentRect: { width: resizeObserverWidth },
      } as ResizeObserverEntry,
    ], this as unknown as ResizeObserver);
  }
  unobserve() {}
  disconnect() {}
};

const responsiveBarProps = vi.hoisted(() => [] as Array<Record<string, any>>);

vi.mock("@nivo/bar", () => ({
  ResponsiveBar: (props: Record<string, any>) => {
    responsiveBarProps.push(props);
    return <div data-testid="mock-responsive-bar">chart</div>;
  },
}));

afterEach(() => {
  cleanup();
  responsiveBarProps.length = 0;
  resizeObserverWidth = 960;
});

function bars(overrides: Partial<ChartBarDatum> = {}): ChartBarDatum[] {
  return [
    {
      key: "2026-05-01",
      label: "May 1",
      value: 1500,
      costUsd: 12.34,
      costStatus: "exact",
      eventCount: 2,
      isCurrent: false,
      ...overrides,
    },
    {
      key: "2026-05-02",
      label: "May 2",
      value: 2500,
      costUsd: null,
      costStatus: "unavailable",
      eventCount: 1,
      isCurrent: true,
    },
  ];
}

describe("TimelineTooltipContent", () => {
  it("formats token, cost, unavailable cost, and singular event values", () => {
    const { rerender } = render(
      <TimelineTooltipContent
        label="Today"
        value={1_500_000}
        metric="tokens"
        costUsd={2.5}
        costStatus="exact"
        eventCount={2}
      />,
    );

    expect(screen.getByText("1.5M tokens")).toBeDefined();
    expect(screen.getByText("$2.50")).toBeDefined();
    expect(screen.getByText("2 events")).toBeDefined();

    rerender(
      <TimelineTooltipContent
        label="Yesterday"
        value={1250}
        metric="cost"
        costUsd={null}
        costStatus="unavailable"
        eventCount={1}
      />,
    );

    expect(screen.getByText("$1.3K")).toBeDefined();
    expect(screen.getByText("1 event")).toBeDefined();

    rerender(
      <TimelineTooltipContent
        label="Small"
        value={42}
        metric="tokens"
        costUsd={null}
        costStatus={null}
      />,
    );

    expect(screen.getByText("42 tokens")).toBeDefined();

    rerender(
      <TimelineTooltipContent
        label="Small cost"
        value={42}
        metric="cost"
        costUsd={null}
        costStatus={null}
      />,
    );

    expect(screen.getByText("$42.00")).toBeDefined();
  });
});

describe("NivoTimelineChart", () => {
  it("renders chart with axes when every bar is zero (no CSS shell)", () => {
    render(
      <NivoTimelineChart
        bars={bars().map((bar) => ({ ...bar, value: 0, costUsd: 0 }))}
        range="day"
      />,
    );

    // ResponsiveBar must render (producing real axes), not the old CSS shell.
    expect(screen.getByTestId("mock-responsive-bar")).toBeDefined();
    expect(document.querySelector(".timeline-chart__empty-shell")).toBeNull();

    // No visible text overlay — accessibility hint is on the figure element.
    expect(screen.queryByText("No usage data for this period")).toBeNull();

    // The axis model is passed to ResponsiveBar even with zero data.
    const props = responsiveBarProps.at(-1)!;
    expect(props.axisBottom).toBeDefined();
    expect(props.axisLeft).toBeDefined();
  });

  it("renders chart with axis ticks when bars is empty (no buckets from backend)", () => {
    render(
      <NivoTimelineChart
        bars={[]}
        range="day"
      />,
    );

    // Placeholder bars generated, ResponsiveBar renders with real axis model.
    expect(screen.getByTestId("mock-responsive-bar")).toBeDefined();
    const props = responsiveBarProps.at(-1)!;
    expect(props.data).toHaveLength(24); // day = 24 hourly placeholders

    // Axis tick values are populated (not empty arrays).
    expect(props.axisBottom.tickValues.length).toBeGreaterThan(0);
    // Vertical grid is intentionally off (empty); 4 horizontal reference lines.
    expect(props.gridXValues).toEqual([]);
    expect(props.gridYValues).toBe(4);
  });

  it("placeholder bars match range granularity and use date-based labels", () => {
    const { rerender } = render(<NivoTimelineChart bars={[]} range="week" />);
    const weekData = responsiveBarProps.at(-1)!.data;
    expect(weekData).toHaveLength(7);
    // Week labels should be date-style ("Jun 4"), not plain numbers.
    expect((weekData[0] as any).label).toMatch(/^\w{3} \d+$/);
    expect((weekData[0] as any).key).toMatch(/^\d{4}-\d{2}-\d{2}$/);

    rerender(<NivoTimelineChart bars={[]} range="month" />);
    const daysInMonth = new Date(new Date().getFullYear(), new Date().getMonth() + 1, 0).getDate();
    const monthData = responsiveBarProps.at(-1)!.data;
    expect(monthData).toHaveLength(daysInMonth);
    expect((monthData[0] as any).label).toMatch(/^\w{3} \d+$/);
    expect((monthData[0] as any).key).toMatch(/^\d{4}-\d{2}-\d{2}$/);

    rerender(<NivoTimelineChart bars={[]} range="year" />);
    const yearData = responsiveBarProps.at(-1)!.data;
    expect(yearData).toHaveLength(12);
    expect((yearData[0] as any).label).toBe("Jan");
    expect((yearData[0] as any).key).toMatch(/^\d{4}-\d{2}$/);
  });

  it("passes formatted axes, tooltip, and active fill rules to ResponsiveBar", () => {
    render(
      <NivoTimelineChart
        bars={bars()}
        activeKey="2026-05-02"
        range="week"
        widthHint={720}
        metric="cost"
      />,
    );

    const props = responsiveBarProps[0];
    expect(screen.getByTestId("mock-responsive-bar")).toBeDefined();
    expect(props.data).toHaveLength(2);
    expect(props.axisLeft.format(1500)).toBe("$1.5K");
    expect(props.axisLeft.format(42)).toBe("$42");
    expect(props.axisBottom.format("2026-05-01")).toBeDefined();
    expect(props.fill[0].match({ data: { key: "2026-05-02" } })).toBe(true);
    expect(props.fill[0].match({ data: { key: "2026-05-01" } })).toBe(false);

    const Tooltip = props.tooltip;
    render(<Tooltip data={props.data[0]} />);
    expect(screen.getByText("May 1")).toBeDefined();
    expect(screen.getByText("$1.5K")).toBeDefined();
  });

  it("uses token-axis labels and default fill when no active key is set", () => {
    render(
      <NivoTimelineChart
        bars={bars()}
        range="month"
        metric="tokens"
      />,
    );

    const props = responsiveBarProps[0];
    expect(props.axisLeft.format(1500)).toBe("1.5K");
    expect(props.axisLeft.format(500)).toBe("500");
    expect(props.fill).toEqual([{ match: "*", id: "defaultBarGradient" }]);
  });

  it("adapts border radius to bar count", () => {
    const manyBars = Array.from({ length: 24 }, (_, i) => ({
      key: `h${i}`, label: `${i}`, value: 100, costUsd: null,
      costStatus: "unavailable" as const, eventCount: 0, isCurrent: false,
    }));

    const { rerender } = render(
      <NivoTimelineChart bars={manyBars} range="day" />,
    );
    expect(responsiveBarProps.at(-1)!.borderRadius).toBe(3);

    rerender(<NivoTimelineChart bars={bars()} range="week" />);
    expect(responsiveBarProps.at(-1)!.borderRadius).toBe(9);
  });

  it("adapts border radius when transitioning from zero to non-zero data", () => {
    resizeObserverWidth = 150;
    const thinBars = Array.from({ length: 12 }, (_, i) => ({
      key: `d${i}`,
      label: `${i}`,
      value: 0,
      costUsd: null,
      costStatus: "unavailable" as const,
      eventCount: 0,
      isCurrent: false,
    }));

    const { rerender } = render(<NivoTimelineChart bars={thinBars} range="month" />);
    // Chart renders even with zero data (real axes, no CSS shell).
    expect(screen.getByTestId("mock-responsive-bar")).toBeDefined();

    rerender(
      <NivoTimelineChart
        bars={thinBars.map((bar) => ({ ...bar, value: 100 }))}
        range="month"
      />,
    );

    expect(responsiveBarProps.at(-1)!.borderRadius).toBe(3);
  });

  it("consumes chartTokens.linePrimary as the ResponsiveBar color prop (no hard-coded colors)", () => {
    // Contract guard: the timeline chart must pass its series color through
    // chartTokens.linePrimary (which maps to --color-data-primary), not a
    // literal. A regression that hard-codes a hex/rgba string here would
    // break theme adaptation silently.
    render(<NivoTimelineChart bars={bars()} range="week" />);

    const props = responsiveBarProps.at(-1)!;
    expect(props.colors).toEqual([chartTokens.linePrimary]);
    expect(chartTokens.linePrimary).toBe("var(--color-data-primary)");
    // Belt-and-braces: the value must look like a CSS var, not a hex/rgba literal.
    expect(chartTokens.linePrimary).toMatch(/^var\(/);
    expect(chartTokens.linePrimary).not.toMatch(/^#[0-9a-f]/i);
    expect(chartTokens.linePrimary).not.toMatch(/^rgba?\(/i);
  });
});
