import { useEffect, useMemo, useRef, useState } from "react";
import { ResponsiveBar } from "@nivo/bar";
import type { BarDatum, BarTooltipProps } from "@nivo/bar";
import { ACTIVE_BAR_GRADIENT, DEFAULT_BAR_GRADIENT, nivoTheme } from "../../lib/nivoTheme";
import { chartTokens } from "../../lib/chartTokens";
import { buildPlaceholderBars, buildTimelineAxisModel, getTimelineBarRadius, type ChartBarDatum } from "../../lib/chartGrammar";
import type { CostStatusDto } from "@busytok/protocol-types";
import { formatCompactNumber, formatCost } from "../../lib/formatters";

interface NivoTimelineChartProps {
  bars: ChartBarDatum[];
  activeKey?: string | null;
  range: "day" | "week" | "month" | "year";
  widthHint?: number;
  metric?: "cost" | "tokens";
}

// Extra fields attached to each datum (stored via the BarDatum index)
// and accessed through a tracked type alias rather than an interface
// to avoid conflicts with BarDatum's index signature.
type TimelineDatum = {
  key: string;
  label: string;
  value: number;
  costUsd: number | null;
  costStatus: CostStatusDto;
  eventCount: number;
  isCurrent: boolean;
  [key: string]: string | number | boolean | null;
};

function formatMetricValue(value: number, metric: "cost" | "tokens"): string {
  if (metric === "cost") {
    if (Math.abs(value) >= 1000) return `$${formatCompactNumber(value)}`;
    return formatCost(value, "exact");
  }
  return formatCompactNumber(value);
}

export function TimelineTooltipContent({
  label,
  value,
  metric = "tokens",
  costUsd,
  costStatus,
  eventCount,
}: {
  label: string;
  value: number;
  metric?: "cost" | "tokens";
  costUsd?: number | null;
  costStatus?: CostStatusDto | null;
  eventCount?: number;
}) {
  const display = formatMetricValue(value, metric);

  let costDisplay: string | null = null;
  if (costUsd != null && costStatus != null && costStatus !== "unavailable") {
    costDisplay = formatCost(costUsd, costStatus);
  } else if (costStatus === "unavailable") {
    costDisplay = "N/A";
  }

  return (
    <div className="chart-tooltip">
      <div className="chart-tooltip__label">{label}</div>
      <div className="chart-tooltip__value">{metric === "cost" ? display : `${display} tokens`}</div>
      {metric === "tokens" && costDisplay && (
        <div className="chart-tooltip__value">{costDisplay}</div>
      )}
      {eventCount != null && (
        <div className="chart-tooltip__value">{eventCount} event{eventCount !== 1 ? "s" : ""}</div>
      )}
    </div>
  );
}

const GRADIENT_DEFS = [DEFAULT_BAR_GRADIENT, ACTIVE_BAR_GRADIENT];
const CHART_MARGIN = { top: 8, right: 12, bottom: 42, left: 56 };

export function NivoTimelineChart({
  bars,
  activeKey,
  range,
  widthHint,
  metric = "tokens",
}: NivoTimelineChartProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [measuredWidth, setMeasuredWidth] = useState(widthHint ?? 0);
  const effectiveBars = bars.length > 0 ? bars : buildPlaceholderBars(range);
  const allZero = effectiveBars.every((bar) => bar.value === 0);

  useEffect(() => {
    const el = containerRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) => {
      setMeasuredWidth(Math.round(entry.contentRect.width));
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [widthHint]);

  const data = effectiveBars.map((bar) => ({
    key: bar.key,
    label: bar.label,
    value: bar.value,
    costUsd: bar.costUsd,
    costStatus: bar.costStatus,
    eventCount: bar.eventCount,
    isCurrent: bar.isCurrent,
  })) as unknown as BarDatum[];

  const TooltipComponent = useMemo(
    () =>
      function TimelineTooltip({ data: tooltipDatum }: BarTooltipProps<BarDatum>) {
        const d = tooltipDatum as unknown as TimelineDatum;
        return (
          <TimelineTooltipContent
            label={d.label}
            value={d.value}
            metric={metric}
            costUsd={d.costUsd}
            costStatus={d.costStatus}
            eventCount={d.eventCount}
          />
        );
      },
    [metric],
  );

  const axis = buildTimelineAxisModel({
    range,
    bars: effectiveBars,
    width: widthHint ?? 960,
  });
  const primaryLabels = new Map(axis.primaryLabels.map((item) => [item.key, item.text]));
  const renderedTickKeys =
    axis.secondaryTickKeys.length > 0
      ? axis.secondaryTickKeys
      : axis.primaryLabels.map((item) => item.key);

  const figureStyle = {
    margin: 0,
    height: "100%",
    "--chart-mb": `${CHART_MARGIN.bottom}px`,
    "--chart-ml": `${CHART_MARGIN.left}px`,
    "--chart-mr": `${CHART_MARGIN.right}px`,
  } as React.CSSProperties;

  return (
    <figure
      aria-label="Usage over time"
      aria-description={allZero ? "No usage data for this period" : undefined}
      role="figure"
      style={figureStyle}
    >
      <div ref={containerRef} style={{ height: "100%" }}>
        <ResponsiveBar
          data={data}
          keys={["value"]}
          indexBy="key"
          layout="vertical"
          margin={CHART_MARGIN}
          theme={nivoTheme}
          defs={GRADIENT_DEFS}
          fill={
            activeKey
              ? [
                  {
                    match: (d) => (d.data as unknown as TimelineDatum).key === activeKey,
                    id: "activeBarGradient",
                  },
                  { match: "*", id: "defaultBarGradient" },
                ]
              : [{ match: "*", id: "defaultBarGradient" }]
          }
          borderRadius={getTimelineBarRadius(effectiveBars.length, measuredWidth)}
          enableLabel={false}
          tooltip={TooltipComponent}
          axisBottom={{
            tickSize: axis.secondaryTickKeys.length > 0 ? 6 : 0,
            tickPadding: 8,
            tickValues: renderedTickKeys,
            format: (key) => primaryLabels.get(String(key)) ?? "",
          }}
          axisLeft={{
            tickSize: 0,
            tickPadding: 8,
            format: (v) => {
              const num = Number(v);
              if (metric === "cost") return `$${formatCompactNumber(num)}`;
              return formatCompactNumber(num);
            },
          }}
          gridXValues={[]}
          gridYValues={4}
          colors={[chartTokens.linePrimary]}
          motionConfig="gentle"
          padding={0.42}
        />
      </div>
    </figure>
  );
}
