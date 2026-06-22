//! OverviewTrendPanel — usage trend bar chart powered by useOverviewTrend.
//!
//! Renders the segmented range/metric controls and the NivoTimelineChart
//! from the overview.trend envelope.  Handles loading, error, and empty
//! states independently so the rest of the overview page renders even
//! while trend data is still arriving.

import { useMemo, useState } from "react";
import { useOverviewTrend } from "../../api/useBusytokData";
import { SegmentedControl } from "../desktop/SegmentedControl";
import { NivoTimelineChart } from "../charts/NivoTimelineChart";
import { buildTimelineBars } from "../../lib/chartGrammar";
import type { RangePresetDto, CostStatusDto } from "@busytok/protocol-types";

const RANGE_OPTIONS: Array<{ value: RangePresetDto; label: string }> = [
  { value: "day", label: "Day" },
  { value: "week", label: "Week" },
  { value: "month", label: "Month" },
  { value: "year", label: "Year" },
];

const METRIC_OPTIONS: Array<{ value: "cost" | "tokens"; label: string }> = [
  { value: "cost", label: "Cost" },
  { value: "tokens", label: "Tokens" },
];

interface OverviewTrendPanelProps {
  range: RangePresetDto;
  onRangeChange: (range: RangePresetDto) => void;
}

export function OverviewTrendPanel({ range, onRangeChange }: OverviewTrendPanelProps) {
  const [chartMetric, setChartMetric] = useState<"cost" | "tokens">("tokens");

  const { data: envelope, isLoading, isError, isFetching } = useOverviewTrend(range);

  const costUnavailable = envelope?.data.trend.cost_status === "unavailable";
  const effectiveMetric = costUnavailable && chartMetric === "cost" ? "tokens" : chartMetric;

  // All hooks must be called unconditionally, before any early return,
  // so the hook call order is stable across renders.
  const timelineBars = useMemo(
    () =>
      envelope && !isError
        ? buildTimelineBars(envelope.data.trend.buckets, effectiveMetric)
        : [],
    [envelope, effectiveMetric, isError],
  );

  const activeBar = useMemo(
    () =>
      isFetching || timelineBars.length === 0
        ? null
        : (timelineBars.find((b) => b.isCurrent)?.key ?? null),
    [isFetching, timelineBars],
  );

  // ── Error ────────────────────────────────────────────────────────────
  if (isError) {
    return (
      <section className="overview-console__trend" aria-label="Usage trend">
        <div className="overview-console__trend-header">
          <h2>Usage Trend</h2>
        </div>
        <div className="overview-console__chart">
          <div className="overview-console__chart-empty">Trend data unavailable</div>
        </div>
      </section>
    );
  }

  // ── Loading ──────────────────────────────────────────────────────────
  if (isLoading || !envelope) {
    return (
      <section className="overview-console__trend" aria-label="Usage trend">
        <div className="overview-console__trend-header">
          <h2>Usage Trend</h2>
        </div>
        <div className="overview-console__chart">
          <span className="overview-console__chart-placeholder" />
          <div className="overview-console__chart-empty">Loading trend data...</div>
        </div>
      </section>
    );
  }

  // Stale banner
  const showStale = isFetching && envelope.is_stale;

  return (
    <section className="overview-console__trend" aria-label="Usage trend">
      {showStale && (
        <div className="overview-panel__stale-banner" role="status">
          Refreshing trend data...
        </div>
      )}

      <div className="overview-console__trend-header">
        <h2>Usage Trend</h2>
        <div className="overview-console__trend-controls">
          <SegmentedControl
            label="Range"
            value={range}
            options={RANGE_OPTIONS}
            onChange={(v) => onRangeChange(v)}
          />
          <SegmentedControl
            label="Chart metric"
            value={effectiveMetric}
            options={METRIC_OPTIONS}
            onChange={(v) => setChartMetric(v)}
          />
        </div>
      </div>

      <div className="overview-console__chart">
        <NivoTimelineChart
          bars={timelineBars}
          activeKey={activeBar}
          range={range}
          metric={effectiveMetric}
        />
      </div>
    </section>
  );
}
