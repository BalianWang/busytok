import { useMemo } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import type {
  BreakdownListItemDto,
  BreakdownDetailDto,
  RangePresetDto,
  BreakdownMiniItemDto,
  ProjectBreakdownListItemDto,
  ModelBreakdownListItemDto,
  ModelBreakdownDetailDto,
  ActivityListItemDto,
} from "@busytok/protocol-types";
import { useBreakdownDetail } from "../api/useBusytokData";
import { BreakdownLedgerPage } from "./BreakdownLedgerPage";
import { NivoTimelineChart } from "../components/charts/NivoTimelineChart";
import { StatusPill } from "../components/desktop/StatusPill";
import { buildTimelineBars } from "../lib/chartGrammar";
import { formatRelativeTime, formatCost, formatCacheHitRate } from "../lib/formatters";

// ── Ledger row type and mapping ─────────────────────────────────────────────

interface ModelLedgerRow {
  id: string;
  label: string;
  tokens: string;
  costDisplay: string;
  eventCount: string;
  clients: string;
  lastActive: string;
}

function toModelRows(items: BreakdownListItemDto[]): ModelLedgerRow[] {
  return (items as Array<ModelBreakdownListItemDto & { kind: "model" }>).map(
    (item) => ({
      id: item.id,
      label: item.label,
      tokens: item.tokens.toLocaleString(),
      costDisplay: formatCost(item.cost_usd, item.cost_status),
      eventCount: item.event_count.toLocaleString(),
      clients:
        item.client_labels.length > 0
          ? item.client_labels.join(", ")
          : "--",
      lastActive:
        item.last_active_at_ms != null
          ? formatRelativeTime(item.last_active_at_ms)
          : "--",
    }),
  );
}

const COLUMNS: Array<ColumnDef<ModelLedgerRow>> = [
  { header: "Model", accessorKey: "label" },
  { header: "Tokens", accessorKey: "tokens" },
  { header: "Cost", accessorKey: "costDisplay" },
  { header: "Events", accessorKey: "eventCount" },
  { header: "Clients", accessorKey: "clients" },
  { header: "Last Active", accessorKey: "lastActive" },
];

// ── Detail drawer content ───────────────────────────────────────────────────

function ModelDetailContent({
  itemId,
  range,
}: {
  itemId: string;
  range: RangePresetDto;
}) {
  const { data, isLoading } = useBreakdownDetail({
    kind: "model",
    id: itemId,
    range,
  });

  if (isLoading) {
    return <div className="detail-stack__loading">Loading detail...</div>;
  }

  const d = data?.data;
  if (!d || d.kind !== "model") {
    return <div className="detail-stack__empty">No detail available.</div>;
  }

  const timelineBars = useMemo(
    () => buildTimelineBars(d.trend.buckets),
    [d.trend.buckets],
  );

  const tk = d.token_breakdown;

  return (
    <div className="detail-stack">
      {/* Token breakdown */}
      <section>
        <h3>Token Breakdown</h3>
        <dl>
          <dt>Total</dt>
          <dd>{tk.total_tokens.toLocaleString()}</dd>
          {tk.prompt_input_total_tokens != null && (
            <>
              <dt>Prompt Input (Total)</dt>
              <dd>{tk.prompt_input_total_tokens.toLocaleString()}</dd>
            </>
          )}
          {tk.prompt_input_non_cached_tokens != null && (
            <>
              <dt>Prompt Input (Non-cached)</dt>
              <dd>{tk.prompt_input_non_cached_tokens.toLocaleString()}</dd>
            </>
          )}
          {tk.cache_read_tokens != null && (
            <>
              <dt>Cache Read</dt>
              <dd>{tk.cache_read_tokens.toLocaleString()}</dd>
            </>
          )}
          {tk.cache_write_tokens != null && (
            <>
              <dt>Cache Write</dt>
              <dd>{tk.cache_write_tokens.toLocaleString()}</dd>
            </>
          )}
          <dt>Cache Hit Rate</dt>
          <dd>{formatCacheHitRate(tk.cache_hit_rate)}</dd>
          {tk.output_tokens != null && (
            <>
              <dt>Output</dt>
              <dd>{tk.output_tokens.toLocaleString()}</dd>
            </>
          )}
          {tk.reasoning_tokens != null && (
            <>
              <dt>Reasoning</dt>
              <dd>{tk.reasoning_tokens.toLocaleString()}</dd>
            </>
          )}
        </dl>
      </section>

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

      {/* Client mix */}
      {d.client_mix.length > 0 && (
        <section>
          <h3>Clients ({d.client_mix.length})</h3>
          <ul className="detail-stack__activity-list">
            {d.client_mix.map((c: BreakdownMiniItemDto) => (
              <li key={c.id} className="detail-stack__activity-item">
                <span style={{ flex: 1 }}>{c.label}</span>
                <span className="detail-stack__activity-time">
                  {c.tokens.toLocaleString()} tokens
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}

      {/* Project mix */}
      {d.project_mix.length > 0 && (
        <section>
          <h3>Projects ({d.project_mix.length})</h3>
          <ul className="detail-stack__activity-list">
            {d.project_mix.map(
              (p: ProjectBreakdownListItemDto) => (
                <li key={p.id} className="detail-stack__activity-item">
                  <span style={{ flex: 1 }}>{p.label}</span>
                  <span className="detail-stack__activity-time">
                    {p.tokens.toLocaleString()} tokens
                  </span>
                </li>
              ),
            )}
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

export function ModelsPage() {
  return (
    <BreakdownLedgerPage
      kind="model"
      title="Models"
      columns={COLUMNS}
      toRows={toModelRows}
      drawerTitle="Model Detail"
      renderDetail={(itemId, range) => (
        <ModelDetailContent itemId={itemId} range={range} />
      )}
    />
  );
}
