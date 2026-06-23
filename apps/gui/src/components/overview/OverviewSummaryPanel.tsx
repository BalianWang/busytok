//! OverviewSummaryPanel — metric cards powered by useOverviewSummary.
//!
//! Renders the header identity row and primary metric cards from the
//! overview.summary envelope. The degraded-state banner is owned at the
//! page level (OverviewPage) to avoid duplicate warnings on the same
//! screen.

import { useOverviewSummary } from "../../api/useBusytokData";
import { PanelSkeleton } from "./PanelSkeleton";
import type { RangePresetDto } from "@busytok/protocol-types";

interface OverviewSummaryPanelProps {
  range: RangePresetDto;
}

export function OverviewSummaryPanel({ range }: OverviewSummaryPanelProps) {
  const { data: envelope, isLoading, isError } = useOverviewSummary(range);

  // ── Error ────────────────────────────────────────────────────────────
  if (isError) {
    return (
      <section className="overview-panel overview-panel--error" aria-label="Overview summary">
        <p className="overview-panel__error">Summary unavailable</p>
      </section>
    );
  }

  // ── Loading ──────────────────────────────────────────────────────────
  if (isLoading || !envelope) {
    return (
      <section className="overview-panel" aria-label="Overview summary">
        <PanelSkeleton variant="metrics" />
      </section>
    );
  }

  const { data } = envelope;

  return (
    <section className="overview-panel" aria-label="Overview summary">
      {/* Metric cards */}
      <div className="overview-console__metrics" aria-label="Metric cards">
        {data.metrics.map((metric) => {
          // success reads as neutral (no semantic billboard); only
          // warning/danger carry the exception flag.
          const cardTone: "neutral" | "warning" | "danger" =
            metric.tone === "warning" || metric.tone === "danger" ? metric.tone : "neutral";
          const showsFlag = cardTone === "warning" || cardTone === "danger";
          return (
            <div key={metric.id} className={`metric-card metric-card--${cardTone}`}>
              {showsFlag ? <div className="metric-card__top-accent" aria-hidden="true" /> : null}
              <div className="metric-card__label">{metric.label.toUpperCase()}</div>
              <div className="metric-card__value">{metric.value}</div>
              {metric.helper ? <div className="metric-card__helper">{metric.helper}</div> : null}
            </div>
          );
        })}
      </div>
    </section>
  );
}
