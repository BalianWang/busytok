//! OverviewPage — Surge overview dashboard.
//!
//! Composes five independently-loading panels: summary metrics, real-time
//! throughput (LiveCurve), usage trend chart, token activity heatmap, and
//! ranking bars.  Each panel owns its own data-fetching hook so that fast
//! panels render immediately while slower panels show their own loading
//! indicators — no single point of failure blocks the whole page.

import { useState, useMemo } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import type {
  RangePresetDto,
  ActivityListItemDto,
  ActivityStatusDto,
} from "@busytok/protocol-types";
import {
  DEFAULT_OVERVIEW_RANGE,
  useOverviewSummary,
  useActivityRecent,
} from "../api/useBusytokData";
import { PageState } from "../components/PageState";
import { useReceiptToolbar } from "../features/receipt/useReceiptToolbar";
import { LedgerTable } from "../components/desktop/LedgerTable";
import { StatusPill } from "../components/desktop/StatusPill";
import type { StatusTone } from "../components/desktop/StatusPill";
import { formatCacheHitRate, formatCost } from "../lib/formatters";
import { OverviewSummaryPanel } from "../components/overview/OverviewSummaryPanel";
import { OverviewTrendPanel } from "../components/overview/OverviewTrendPanel";
import { OverviewHeatmapPanel } from "../components/overview/OverviewHeatmapPanel";
import { OverviewRankingsPanel } from "../components/overview/OverviewRankingsPanel";
import { LiveCurvePanel } from "../components/overview/LiveCurvePanel";
import { PanelSkeleton } from "../components/overview/PanelSkeleton";

// ── Recent activity row type and column definitions ────────────────────

interface RecentActivityRow {
  id: string;
  time: string;
  source: string;
  model: string;
  tokens: string;
  cache: string;
  cost: string;
  status: ActivityStatusDto;
}

function formatTime(ms: number): string {
  const date = new Date(ms);
  const now = new Date();
  const isToday =
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate();

  if (isToday) {
    return date.toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
    });
  }
  return date.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function toRecentRow(item: ActivityListItemDto): RecentActivityRow {
  return {
    id: item.id,
    time: formatTime(item.happened_at_ms),
    source: [item.client_label, item.source_label].filter(Boolean).join(" / "),
    model: item.model_label ?? "Unknown model",
    tokens: item.tokens.toLocaleString(),
    cache: formatCacheHitRate(item.cache_hit_rate),
    cost: formatCost(item.cost_usd, item.cost_status),
    status: item.status,
  };
}

const RECENT_COLUMNS: Array<ColumnDef<RecentActivityRow>> = [
  { header: "Time", accessorKey: "time" },
  { header: "Source", accessorKey: "source" },
  { header: "Model", accessorKey: "model" },
  { header: "Tokens", accessorKey: "tokens" },
  { header: "Cache", accessorKey: "cache" },
  { header: "Cost", accessorKey: "cost" },
  {
    header: "Status",
    accessorKey: "status",
    cell: (info) => <StatusPill tone={info.getValue<ActivityStatusDto>() as StatusTone} />,
  },
];

// ── Recent activity section ──────────────────────────────────────────

function OverviewRecentActivity({ range }: { range: RangePresetDto }) {
  const { data: envelope, isLoading, isError } = useActivityRecent(range);

  if (isLoading && !envelope) {
    return (
      <section className="overview-console__recent" aria-label="Recent activity">
        <h3>Recent Activity</h3>
        <PanelSkeleton variant="table" rows={5} />
      </section>
    );
  }

  if (isError && !envelope) {
    return (
      <section className="overview-console__recent" aria-label="Recent activity">
        <h3>Recent Activity</h3>
        <p className="overview-panel__error">Recent activity unavailable</p>
      </section>
    );
  }

  const rows = isError
    ? []
    : envelope!.data.recent_activity.slice(0, 5).map(toRecentRow);

  return (
    <section className="overview-console__recent" aria-label="Recent activity">
      <h3>Recent Activity</h3>
      <LedgerTable
        readOnly
        ariaLabel="Recent activity"
        columns={RECENT_COLUMNS}
        rows={rows}
      />
    </section>
  );
}

// ── Page ──────────────────────────────────────────────────────────────────

export function OverviewPage() {
  const [range, setRange] = useState<RangePresetDto>(DEFAULT_OVERVIEW_RANGE);

  // Summary hook is our primary readiness indicator for the page header.
  const {
    data: summaryEnvelope,
    isLoading: summaryLoading,
    isError: summaryError,
    isFetching: summaryFetching,
    refetch: refetchSummary,
  } = useOverviewSummary(range);

  const today = new Date().toISOString().slice(0, 10);
  const receiptDialog = useReceiptToolbar({
    surface: "overview",
    onRefresh: refetchSummary,
    isFetching: summaryFetching,
    today,
  });

  // ── Catastrophic loading: summary is the most critical panel ─────────
  if (summaryLoading && !summaryEnvelope) {
    return (
      <div className="overview-console">
        <PageState
          kind="loading"
          title="Overview"
          message="Loading overview data..."
        />
      </div>
    );
  }

  // ── Catastrophic error ───────────────────────────────────────────────
  if (summaryError && !summaryEnvelope) {
    return (
      <div className="overview-console">
        <PageState
          kind="error"
          title="Overview unavailable"
          message="Could not load usage data."
          actionLabel="Retry"
          onAction={() => {
            refetchSummary();
          }}
        />
      </div>
    );
  }

  // ── Degraded banner (shown when envelope indicates non-exact/stale) ──
  const showDegraded =
    summaryEnvelope &&
    (!summaryEnvelope.is_exact || summaryEnvelope.is_stale);
  const degradedReason = summaryEnvelope?.degraded_reason ?? null;

  // ── Render ───────────────────────────────────────────────────────────
  return (
    <div className="overview-console">
      {/* Degraded ribbon (non-blocking) */}
      {showDegraded && (
        <div className="overview-console__degraded-ribbon" role="status">
          <span className="overview-console__degraded-ribbon-dot" aria-hidden="true" />
          <span>{degradedReason ?? (summaryEnvelope?.is_stale ? "Showing stale data — refresh in progress" : "Data is approximate — exact aggregates not yet available")}</span>
        </div>
      )}

      {/* 1. Summary metric cards */}
      <OverviewSummaryPanel range={range} />

      {/* 2-3. Trend (left) + Live curve (right) */}
      <div className="overview-console__charts-row">
        <OverviewTrendPanel range={range} onRangeChange={setRange} />
        <LiveCurvePanel />
      </div>

      {/* 4. Token Activity heatmap */}
      <OverviewHeatmapPanel range={range} />

      {/* 5. Ranking panels */}
      <OverviewRankingsPanel range={range} />

      {/* 6. Recent Activity */}
      <OverviewRecentActivity range={range} />

      {/* Receipt preview dialog (portaled by Radix Dialog; rendered here so
          the share button in the toolbar can open it once data is loaded). */}
      {receiptDialog}
    </div>
  );
}
