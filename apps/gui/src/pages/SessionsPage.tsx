import { useMemo } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import type {
  BreakdownListItemDto,
  BreakdownDetailDto,
  RangePresetDto,
  BreakdownMiniItemDto,
  SessionBreakdownListItemDto,
  SessionBreakdownDetailDto,
  SessionTimelineItemDto,
  SourceContextItemDto,
} from "@busytok/protocol-types";
import { useBreakdownDetail } from "../api/useBusytokData";
import { BreakdownLedgerPage } from "./BreakdownLedgerPage";
import { formatRelativeTime, formatDateTime, formatCost, formatCacheHitRate } from "../lib/formatters";
import { StatusPill } from "../components/desktop/StatusPill";

// ── Ledger row type and mapping ─────────────────────────────────────────────

interface SessionLedgerRow {
  id: string;
  label: string;
  client: string;
  project: string;
  tokens: string;
  eventCount: string;
  lastActive: string;
}

function toSessionRows(items: BreakdownListItemDto[]): SessionLedgerRow[] {
  return (
    items as Array<
      SessionBreakdownListItemDto & { kind: "session" }
    >
  ).map((item) => ({
    id: item.id,
    label: item.label,
    client: item.client_label,
    project: item.project_label ?? "--",
    tokens: item.tokens.toLocaleString(),
    eventCount: item.event_count.toLocaleString(),
    lastActive:
      item.last_active_at_ms != null
        ? formatRelativeTime(item.last_active_at_ms)
        : "--",
  }));
}

const COLUMNS: Array<ColumnDef<SessionLedgerRow>> = [
  { header: "Session", accessorKey: "label" },
  { header: "Client", accessorKey: "client" },
  { header: "Project", accessorKey: "project" },
  { header: "Tokens", accessorKey: "tokens" },
  { header: "Events", accessorKey: "eventCount" },
  { header: "Last Active", accessorKey: "lastActive" },
];

// ── Detail drawer content ───────────────────────────────────────────────────

function SessionDetailContent({
  itemId,
  range,
}: {
  itemId: string;
  range: RangePresetDto;
}) {
  const { data, isLoading } = useBreakdownDetail({
    kind: "session",
    id: itemId,
    range,
  });

  if (isLoading) {
    return <div className="detail-stack__loading">Loading detail...</div>;
  }

  const d = data?.data;
  if (!d || d.kind !== "session") {
    return <div className="detail-stack__empty">No detail available.</div>;
  }

  const tk = d.token_breakdown;

  // ── Metadata section ────────────────────────────────────────────────
  return (
    <div className="detail-stack">
      {/* Session metadata */}
      <dl>
        <dt>Client</dt>
        <dd>{d.client_label}</dd>

        <dt>Project</dt>
        <dd>{d.project_label ?? "--"}</dd>

        {d.project_hash && (
          <>
            <dt>Project Hash</dt>
            <dd className="detail-stack__mono">{d.project_hash}</dd>
          </>
        )}

        {d.last_active_at_ms != null && (
          <>
            <dt>Last Active</dt>
            <dd>{formatDateTime(d.last_active_at_ms)}</dd>
          </>
        )}
      </dl>

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

      {/* Chronological timeline */}
      {d.timeline.length > 0 && (
        <section>
          <h3>Timeline ({d.timeline.length})</h3>
          <ul className="detail-stack__activity-list">
            {d.timeline.map((t: SessionTimelineItemDto) => (
              <li key={t.id} className="detail-stack__activity-item">
                <span className="detail-stack__activity-time">
                  {formatRelativeTime(t.happened_at_ms)}
                </span>
                <span style={{ flex: 1 }}>{t.label}</span>
                <span className="detail-stack__activity-tokens">
                  {t.tokens.toLocaleString()}
                </span>
                <StatusPill tone={t.status} />
              </li>
            ))}
          </ul>
        </section>
      )}

      {/* Models used */}
      {d.models_used.length > 0 && (
        <section>
          <h3>Models Used ({d.models_used.length})</h3>
          <ul className="detail-stack__activity-list">
            {d.models_used.map((m: BreakdownMiniItemDto) => (
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

      {/* Source context */}
      {d.source_context.length > 0 && (
        <section>
          <h3>Source Context</h3>
          <ul className="detail-stack__activity-list">
            {d.source_context.map((s: SourceContextItemDto) => (
              <li key={s.source_id} className="detail-stack__activity-item">
                <span className="detail-stack__activity-source">
                  {s.client_label}
                </span>
                <span style={{ color: "var(--color-text-muted)" }}>
                  {s.root_path}
                </span>
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

export function SessionsPage() {
  return (
    <BreakdownLedgerPage
      kind="session"
      title="Sessions"
      columns={COLUMNS}
      toRows={toSessionRows}
      drawerTitle="Session Detail"
      renderDetail={(itemId, range) => (
        <SessionDetailContent itemId={itemId} range={range} />
      )}
    />
  );
}
