//! OverviewRankingsPanel — ranking bars powered by useOverviewRankings.
//!
//! Renders the ranking sections (projects, models, etc.) from the
//! overview.rankings envelope.  Each section renders independently
//! and the panel handles loading / error / empty states.

import { useOverviewRankings } from "../../api/useBusytokData";
import { PanelSkeleton } from "./PanelSkeleton";
import type { RangePresetDto } from "@busytok/protocol-types";

interface OverviewRankingsPanelProps {
  range: RangePresetDto;
}

export function OverviewRankingsPanel({ range }: OverviewRankingsPanelProps) {
  const { data: envelope, isLoading, isError } = useOverviewRankings(range);

  // ── Error ────────────────────────────────────────────────────────────
  if (isError) {
    return (
      <section className="overview-panel overview-panel--error" aria-label="Rankings">
        <p className="overview-panel__error">Rankings unavailable</p>
      </section>
    );
  }

  // ── Loading ──────────────────────────────────────────────────────────
  if (isLoading || !envelope) {
    return (
      <section className="overview-panel" aria-label="Rankings">
        <PanelSkeleton variant="list" rows={5} />
      </section>
    );
  }

  const { data } = envelope;

  return (
    <section className="overview-console__rankings" aria-label="Rankings">
      {data.rankings.map((section) => (
        <div key={section.id} className="ranking-section">
          <h3 className="ranking-section__title">{section.title}</h3>
          {section.items.length > 0 ? (
            <div className="ranking-section__items">
              {section.items.map((item, idx) => (
                <div
                  key={item.id}
                  data-rank={idx + 1}
                  className={`ranking-item${idx === 0 ? " ranking-item--leader" : ""}`}
                >
                  <span
                    className="ranking-item__bar"
                    style={{ width: `${item.bar_value}%` }}
                  />
                  <span className="ranking-item__label">{item.label}</span>
                  <span className="ranking-item__value">{item.value}</span>
                </div>
              ))}
            </div>
          ) : (
            <p className="ranking-section__empty">No data</p>
          )}
        </div>
      ))}
    </section>
  );
}
