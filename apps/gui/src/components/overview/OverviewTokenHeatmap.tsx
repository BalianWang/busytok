import { useState } from "react";
import type { OverviewHeatmapModel, OverviewHeatmapDay } from "../../lib/heatmap";

// ─── Weekday label derivation ─────────────────────────────────────────────────

const ALL_DAY_LABELS_2 = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"] as const;

/**
 * Derive 7 two-letter weekday abbreviation labels from the first non-placeholder
 * day found in the model columns. Returns labels ordered from the first day
 * of the week (as determined by weekStartsOn).
 */
function deriveWeekdayLabels(
  model: OverviewHeatmapModel,
): readonly string[] {
  for (const col of model.columns) {
    for (let i = 0; i < col.days.length; i++) {
      const day = col.days[i];
      if (!day.isPlaceholder && day.date !== null) {
        const dow = new Date(day.date + "T00:00:00Z").getUTCDay(); // 0=Sun
        const weekStart = (dow - i + 7) % 7;
        return Array.from(
          { length: 7 },
          (_, idx) => ALL_DAY_LABELS_2[(weekStart + idx) % 7],
        );
      }
    }
  }
  // Fallback: Monday-first
  return ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
}

// ─── Tooltip component ─────────────────────────────────────────────────────────

function HeatmapTooltip({ day }: { day: OverviewHeatmapDay }) {
  return (
    <div className="overview-heatmap__tooltip">
      {day.tooltipDateLabel && (
        <div className="overview-heatmap__tooltip-date">
          {day.tooltipDateLabel}
        </div>
      )}
      {day.tooltipTokensLabel && (
        <div className="overview-heatmap__tooltip-value">
          {day.tooltipTokensLabel}
        </div>
      )}
      {day.tooltipCostLabel && (
        <div className="overview-heatmap__tooltip-value">
          Cost: {day.tooltipCostLabel}
        </div>
      )}
      {day.tooltipEventsLabel && (
        <div className="overview-heatmap__tooltip-value">
          {day.tooltipEventsLabel}
        </div>
      )}
    </div>
  );
}

// ─── Component ────────────────────────────────────────────────────────────────

export function OverviewTokenHeatmap({
  model,
}: {
  model: OverviewHeatmapModel;
}) {
  const [tooltip, setTooltip] = useState<{
    day: OverviewHeatmapDay;
    x: number;
    y: number;
  } | null>(null);

  const weekdayLabels = deriveWeekdayLabels(model);

  const handleCellEnter = (
    day: OverviewHeatmapDay,
    e: React.MouseEvent<HTMLSpanElement>,
  ) => {
    if (!day.isPlaceholder) {
      setTooltip({ day, x: e.clientX, y: e.clientY });
    }
  };

  const handleCellLeave = () => {
    setTooltip(null);
  };

  return (
    <div className="overview-heatmap">
      {/* ── Header ─────────────────────────────────────────────────── */}
      <div className="overview-heatmap__header">
        <h2 className="overview-heatmap__title">Token Activity</h2>
        <div className="overview-heatmap__total">
          <span className="overview-heatmap__total-label">Total tokens</span>
          <span className="overview-heatmap__total-value">{model.totalTokensLabel}</span>
        </div>
      </div>

      {/* ── Month label row ────────────────────────────────────────── */}
      <div className="overview-heatmap__months">
        <span className="overview-heatmap__month-spacer" />
        {model.columns.map((col) => (
          <span key={col.key} className="overview-heatmap__month-label">
            {col.monthLabel ?? ""}
          </span>
        ))}
      </div>

      {/* ── Grid body (weekday markers + grid) ─────────────────────── */}
      <div className="overview-heatmap__grid-body">
        {/* Weekday column */}
        <div className="overview-heatmap__weekdays">
          {weekdayLabels.map((label, i) => (
            <span key={i} className="overview-heatmap__weekday">
              {label}
            </span>
          ))}
        </div>

        {/* ARIA grid */}
        <div role="grid" className="overview-heatmap__grid">
          {Array.from({ length: 7 }, (_, rowIdx) => (
            <div key={rowIdx} role="row" className="overview-heatmap__row">
              {model.columns.map((col) => {
                const day = col.days[rowIdx];

                // Missing cell: render empty placeholder
                if (!day) {
                  return (
                    <span
                      key={`${col.key}-empty-${rowIdx}`}
                      className="overview-heatmap__cell overview-heatmap__cell--placeholder"
                    />
                  );
                }

                // Explicit placeholder day
                if (day.isPlaceholder) {
                  return (
                    <span
                      key={day.key}
                      data-testid={`heatmap-placeholder-${day.key}`}
                      className="overview-heatmap__cell overview-heatmap__cell--placeholder"
                      onMouseEnter={() => setTooltip(null)}
                    />
                  );
                }

                // In-range day
                return (
                  <span
                    key={day.key}
                    role="gridcell"
                    aria-label={day.tooltipDateLabel ?? undefined}
                    className={`overview-heatmap__cell overview-heatmap__cell--${day.intensity}`}
                    onMouseEnter={(e) => handleCellEnter(day, e)}
                    onMouseLeave={handleCellLeave}
                  />
                );
              })}
            </div>
          ))}
        </div>
      </div>

      {/* ── Summary ────────────────────────────────────────────────── */}
      <div className="overview-heatmap__summary">
        <div className="overview-heatmap__summary-item">
          <span className="overview-heatmap__summary-label">
            Most Active Month
          </span>
          <span className="overview-heatmap__summary-value">
            {model.summary.mostActiveMonthLabel}
          </span>
        </div>
        <div className="overview-heatmap__summary-item">
          <span className="overview-heatmap__summary-label">
            Most Active Day
          </span>
          <span className="overview-heatmap__summary-value">
            {model.summary.mostActiveDayLabel}
          </span>
        </div>
        <div className="overview-heatmap__summary-item">
          <span className="overview-heatmap__summary-label">
            Longest Active Streak
          </span>
          <span className="overview-heatmap__summary-value">
            {model.summary.longestActiveStreakLabel}
          </span>
        </div>
        <div className="overview-heatmap__summary-item">
          <span className="overview-heatmap__summary-label">
            Current Active Streak
          </span>
          <span className="overview-heatmap__summary-value">
            {model.summary.currentActiveStreakLabel}
          </span>
        </div>
      </div>

      {/* ── Legend ─────────────────────────────────────────────────── */}
      <div className="overview-heatmap__legend">
        <span className="overview-heatmap__legend-label">Less</span>
        {model.legendLevels.map((level) => (
          <span
            key={level}
            className={`overview-heatmap__legend-swatch overview-heatmap__cell--${level}`}
          />
        ))}
        <span className="overview-heatmap__legend-label">More</span>
      </div>

      {/* ── Tooltip ────────────────────────────────────────────────── */}
      {tooltip && (
        <div
          className="overview-heatmap__tooltip"
          style={{
            position: "fixed",
            left: tooltip.x + 10,
            top: tooltip.y + 10,
          }}
        >
          <HeatmapTooltip day={tooltip.day} />
        </div>
      )}
    </div>
  );
}
