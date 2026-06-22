import { useMemo } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import type {
  BreakdownListItemDto,
  BreakdownDetailDto,
  RangePresetDto,
  BreakdownMiniItemDto,
  SessionBreakdownListItemDto,
  ProjectBreakdownListItemDto,
  ProjectBreakdownDetailDto,
  ActivityListItemDto,
} from "@busytok/protocol-types";
import { useBreakdownDetail } from "../api/useBusytokData";
import { BreakdownLedgerPage } from "./BreakdownLedgerPage";
import { NivoTimelineChart } from "../components/charts/NivoTimelineChart";
import { StatusPill } from "../components/desktop/StatusPill";
import { buildTimelineBars } from "../lib/chartGrammar";
import { formatRelativeTime, formatDateTime, formatCost } from "../lib/formatters";

// ── Ledger row type and mapping ─────────────────────────────────────────────

interface ProjectLedgerRow {
  id: string;
  label: string;
  tokens: string;
  costDisplay: string;
  eventCount: string;
  lastActive: string;
  topModel: string;
}

function toProjectRows(items: BreakdownListItemDto[]): ProjectLedgerRow[] {
  return (items as Array<ProjectBreakdownListItemDto & { kind: "project" }>).map(
    (item) => ({
      id: item.id,
      label: item.label,
      tokens: item.tokens.toLocaleString(),
      costDisplay: formatCost(item.cost_usd, item.cost_status),
      eventCount: item.event_count.toLocaleString(),
      lastActive:
        item.last_active_at_ms != null
          ? formatRelativeTime(item.last_active_at_ms)
          : "--",
      topModel: item.top_model_label ?? "--",
    }),
  );
}

const COLUMNS: Array<ColumnDef<ProjectLedgerRow>> = [
  { header: "Project", accessorKey: "label" },
  { header: "Tokens", accessorKey: "tokens" },
  { header: "Cost", accessorKey: "costDisplay" },
  { header: "Events", accessorKey: "eventCount" },
  { header: "Last Active", accessorKey: "lastActive" },
  { header: "Top Model", accessorKey: "topModel" },
];

// ── Detail drawer content ───────────────────────────────────────────────────

function ProjectDetailContent({
  itemId,
  range,
}: {
  itemId: string;
  range: RangePresetDto;
}) {
  const { data, isLoading } = useBreakdownDetail({
    kind: "project",
    id: itemId,
    range,
  });

  if (isLoading) {
    return <div className="detail-stack__loading">Loading detail...</div>;
  }

  const d = data?.data;
  if (!d || d.kind !== "project") {
    return <div className="detail-stack__empty">No detail available.</div>;
  }

  const timelineBars = useMemo(
    () => buildTimelineBars(d.trend.buckets),
    [d.trend.buckets],
  );

  return (
    <div className="detail-stack">
      {/* Project metadata */}
      <dl>
        {d.project_path && (
          <>
            <dt>Project Path</dt>
            <dd>{d.project_path}</dd>
          </>
        )}
        <dt>Project Hash</dt>
        <dd className="detail-stack__mono">{d.project_hash}</dd>
      </dl>

      {/* Metrics cards */}
      {d.metrics.length > 0 && (
        <section>
          <h3>Metrics</h3>
          <div className="breakdown-metrics">
            {d.metrics.map((m) => (
              <div
                key={m.id}
                className={`metric-card metric-card--${m.tone}`}
              >
                <div className="metric-card__label">
                  {m.label.toUpperCase()}
                </div>
                <div className="metric-card__value">{m.value}</div>
                {m.helper && (
                  <div className="metric-card__helper">{m.helper}</div>
                )}
              </div>
            ))}
          </div>
        </section>
      )}

      {/* Trend chart */}
      {timelineBars.length > 0 && (
        <section>
          <h3>Usage Trend</h3>
          <div className="breakdown-chart">
            <NivoTimelineChart
              bars={timelineBars}
              range={d.trend.range}
            />
          </div>
        </section>
      )}

      {/* Model mix */}
      {d.model_mix.length > 0 && (
        <section>
          <h3>Models ({d.model_mix.length})</h3>
          <ul className="detail-stack__activity-list">
            {d.model_mix.map((m: BreakdownMiniItemDto) => (
              <li key={m.id} className="detail-stack__activity-item">
                <span style={{ flex: 1 }}>{m.label}</span>
                <span className="detail-stack__activity-time">
                  {m.tokens.toLocaleString()} tokens
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}

      {/* Top sessions */}
      {d.sessions.length > 0 && (
        <section>
          <h3>Top Sessions ({d.sessions.length})</h3>
          <ul className="detail-stack__activity-list">
            {d.sessions.map((s: SessionBreakdownListItemDto) => (
              <li key={s.id} className="detail-stack__activity-item">
                <span style={{ flex: 1 }}>{s.label}</span>
                <span className="detail-stack__activity-time">
                  {s.tokens.toLocaleString()} tokens
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}

      {/* Recent activity */}
      {d.recent_activity.length > 0 && (
        <section>
          <h3>Recent Activity ({d.recent_activity.length})</h3>
          <ul className="detail-stack__activity-list">
            {d.recent_activity.map((a: ActivityListItemDto) => (
              <li key={a.id} className="detail-stack__activity-item">
                <span className="detail-stack__activity-time">
                  {formatRelativeTime(a.happened_at_ms)}
                </span>
                <span className="detail-stack__activity-source">
                  {a.model_label ?? "Unknown model"}
                </span>
                <StatusPill tone={a.status} />
              </li>
            ))}
          </ul>
        </section>
      )}

      {/* Technical details */}
      {d.technical_details.length > 0 && (
        <section>
          <h3>Technical Details</h3>
          <dl>
            {d.technical_details.map((td, i) => (
              <span key={i} style={{ display: "contents" }}>
                <dt>{td.label}</dt>
                <dd>{td.value}</dd>
              </span>
            ))}
          </dl>
        </section>
      )}
    </div>
  );
}

// ── Main page component ─────────────────────────────────────────────────────

export function ProjectsPage() {
  return (
    <BreakdownLedgerPage
      kind="project"
      title="Projects"
      columns={COLUMNS}
      toRows={toProjectRows}
      drawerTitle="Project Detail"
      renderDetail={(itemId, range) => (
        <ProjectDetailContent itemId={itemId} range={range} />
      )}
    />
  );
}
