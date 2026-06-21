import { useCallback, useEffect, useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import type {
  ActivityListItemDto,
  ActivityStatusDto,
  CostStatusDto,
} from "@busytok/protocol-types";
import { useActivityList, useActivityDetail } from "../api/useBusytokData";
import { PageState } from "../components/PageState";
import { LedgerPagination } from "../components/desktop/LedgerPagination";
import { LedgerTable } from "../components/desktop/LedgerTable";
import { StatusPill } from "../components/desktop/StatusPill";
import type { StatusTone } from "../components/desktop/StatusPill";
import { DetailDrawer } from "../components/desktop/DetailDrawer";
import { useRefreshToolbar } from "../components/desktop/useRefreshToolbar";
import { useCursorPageStack } from "../hooks/useCursorPageStack";
import { safeReportEvent } from "../logging/reporter";
import { formatCost, formatCacheHitRate } from "../lib/formatters";

const PAGE_LIMIT = 100;

interface ActivityLedgerRow {
  id: string;
  happenedAt: string;
  clientLabel: string;
  projectLabel: string | null;
  modelLabel: string;
  tokens: string;
  cacheHitRate: string;
  costDisplay: string;
  status: ActivityStatusDto;
  detailAvailable: boolean;
  source: ActivityListItemDto;
}

function formatTimeOnly(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });
}

function toLedgerRow(item: ActivityListItemDto): ActivityLedgerRow {
  return {
    id: item.id,
    happenedAt: formatTimeOnly(item.happened_at_ms),
    clientLabel: item.client_label,
    projectLabel: item.project_label,
    modelLabel: item.model_label ?? "Unknown model",
    tokens: item.tokens.toLocaleString(),
    cacheHitRate: formatCacheHitRate(item.cache_hit_rate),
    costDisplay: formatCost(item.cost_usd, item.cost_status),
    status: item.status,
    detailAvailable: item.detail_available,
    source: item,
  };
}

const COLUMNS: Array<ColumnDef<ActivityLedgerRow>> = [
  { header: "Time", accessorKey: "happenedAt" },
  { header: "Client", accessorKey: "clientLabel" },
  { header: "Project", accessorFn: (row) => row.projectLabel ?? "--" },
  { header: "Model", accessorKey: "modelLabel" },
  { header: "Tokens", accessorKey: "tokens" },
  { header: "Cache", accessorKey: "cacheHitRate" },
  { header: "Cost", accessorKey: "costDisplay" },
  {
    header: "Status",
    accessorKey: "status",
    cell: (info) => <StatusPill tone={info.getValue<ActivityStatusDto>() as StatusTone} />,
  },
];

// ── Detail drawer content ─────────────────────────────────────────

function ActivityDetailContent({ itemId }: { itemId: string }) {
  const { data, isLoading } = useActivityDetail({ id: itemId });

  if (isLoading) {
    return <div className="detail-stack__loading">Loading detail...</div>;
  }

  const detail = data?.data;
  if (!detail) {
    return <div className="detail-stack__empty">No detail available.</div>;
  }

  const tk = detail.token_breakdown;

  return (
    <div className="detail-stack">
      <dl>
        <dt>Time</dt>
        <dd>{formatTimeOnly(detail.happened_at_ms)}</dd>

        <dt>Client</dt>
        <dd>{detail.client_label}</dd>
        {detail.source_label && (
          <>
            <dt>Source</dt>
            <dd>{detail.source_label}</dd>
          </>
        )}

        <dt>Project</dt>
        <dd>{detail.project_label ?? "--"}</dd>
        {detail.session_id && (
          <>
            <dt>Session</dt>
            <dd>{detail.session_id}</dd>
          </>
        )}

        <dt>Model</dt>
        <dd>{detail.model_label ?? "Unknown model"}</dd>
      </dl>

      {tk && (() => {
        const cacheHitRate = tk.input_tokens != null && tk.input_tokens > 0
          ? (tk.cached_input_tokens ?? 0) / tk.input_tokens
          : null;
        return (
        <section>
          <h3>Tokens</h3>
          <dl>
            <dt>Total</dt>
            <dd>{tk.total_tokens.toLocaleString()}</dd>
            {tk.input_tokens != null && (
              <>
                <dt>Input</dt>
                <dd>{tk.input_tokens.toLocaleString()}</dd>
              </>
            )}
            {tk.output_tokens != null && (
              <>
                <dt>Output</dt>
                <dd>{tk.output_tokens.toLocaleString()}</dd>
              </>
            )}
            {tk.cached_input_tokens != null && (
              <>
                <dt>Cached</dt>
                <dd>{tk.cached_input_tokens.toLocaleString()}</dd>
              </>
            )}
            <dt>Cache Hit</dt>
            <dd>{formatCacheHitRate(cacheHitRate)}</dd>
            {tk.reasoning_tokens != null && (
              <>
                <dt>Reasoning</dt>
                <dd>{tk.reasoning_tokens.toLocaleString()}</dd>
              </>
            )}
          </dl>
        </section>
        );
      })()}

      <section>
        <h3>Cost</h3>
        <p>{formatCost(detail.cost_usd, detail.cost_status)}</p>
      </section>

      <section>
        <h3>Technical Details</h3>
        <dl>
          <dt>Source ID</dt>
          <dd>{detail.technical_details.source_id ?? "--"}</dd>
          <dt>Provider</dt>
          <dd>{detail.technical_details.provider ?? "--"}</dd>
          <dt>Raw Model</dt>
          <dd>{detail.technical_details.raw_model ?? "--"}</dd>
          {detail.technical_details.notes.length > 0 && (
            <>
              <dt>Notes</dt>
              <dd>
                <ul>
                  {detail.technical_details.notes.map((note, i) => (
                    <li key={i}>{note}</li>
                  ))}
                </ul>
              </dd>
            </>
          )}
        </dl>
      </section>
    </div>
  );
}

// ── Main page component ────────────────────────────────────────────

export function ActivityPage() {
  // Activity data is retained for 24 hours server-side, so "day" is the
  // only meaningful range. This is intentional — a range selector would
  // show empty results for longer periods.
  const range = "day" as const;
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [pageSize, setPageSize] = useState(PAGE_LIMIT);
  const pageStack = useCursorPageStack();

  const { data, isLoading, isError, isFetching, refetch } = useActivityList({
    range,
    cursor: pageStack.cursor,
    limit: pageSize,
    client_id: null,
    source_id: null,
    project_hash: null,
    model_id: null,
  });

  useRefreshToolbar({
    surface: "activity",
    onRefresh: refetch,
    isFetching,
  });

  const clearSelection = useCallback(() => setSelectedId(null), []);

  // Reset cursor on page size change
  useEffect(() => {
    pageStack.reset();
    clearSelection();
  }, [pageSize, pageStack.reset, clearSelection]);

  const items = data?.data.items ?? [];
  const nextCursor = data?.data.next_cursor ?? null;

  const rows = useMemo(() => items.map(toLedgerRow), [items]);

  const handleNextPage = useCallback(() => {
    if (nextCursor != null && !isFetching) {
      clearSelection();
      // Pass current item count so the stack tracks cumulative itemsBefore accurately
      pageStack.goNext(nextCursor, items.length);
      safeReportEvent("gui.pagination.next_requested", "Pagination next requested", {
        surface: "activity", page_index: pageStack.pageIndex + 1, page_size: pageSize,
      });
    }
  }, [nextCursor, isFetching, pageStack, pageSize, items.length, clearSelection]);

  const handlePrevPage = useCallback(() => {
    if (!isFetching) {
      clearSelection();
      pageStack.goPrev();
      safeReportEvent("gui.pagination.prev_requested", "Pagination prev requested", {
        surface: "activity", page_index: pageStack.pageIndex - 1, page_size: pageSize,
      });
    }
  }, [pageStack, pageSize, isFetching, clearSelection]);

  const handlePageSizeChange = useCallback((size: number) => {
    clearSelection();
    setPageSize(size);
    safeReportEvent("gui.pagination.page_size_changed", "Pagination page size changed", {
      surface: "activity", page_size: size,
    });
  }, [clearSelection]);

  // Accurate range using cumulative itemsBefore (not pageIndex * pageSize)
  const visibleStart = items.length > 0 ? pageStack.itemsBefore + 1 : null;
  const visibleEnd = items.length > 0 ? pageStack.itemsBefore + items.length : null;

  if (isLoading && !data) {
    return (
      <div className="activity-page">
        <PageState kind="loading" title="Activity" message="Loading activity data..." />
      </div>
    );
  }

  if (isError && !data) {
    return (
      <div className="activity-page">
        <PageState
          kind="error"
          title="Activity unavailable"
          message="Could not load activity data."
          actionLabel="Retry"
          onAction={() => refetch()}
        />
      </div>
    );
  }

  return (
    <div className="activity-page">
      {items.length === 0 && !isFetching ? (
        <PageState kind="empty" title="No activity" message="No activity found in the last 24 hours." />
      ) : (
        <>
          <section className="activity-page__range">
            <span className="activity-page__range-hint">
              Today's activity — retained for 24 hours
            </span>
          </section>

          <section className="activity-page__table-shell page-surface">
            <LedgerTable
              ariaLabel="Activity ledger"
              columns={COLUMNS}
              rows={rows}
              selectedId={selectedId}
              onSelect={(row) => setSelectedId(row.id)}
              disabledPredicate={(row) => !row.detailAvailable}
            />
          </section>

          <LedgerPagination
            ariaLabel="Activity pagination"
            pageSize={pageSize}
            pageIndex={pageStack.pageIndex}
            hasPrev={pageStack.hasPrev}
            hasNext={nextCursor != null}
            totalCount={null}
            visibleStart={visibleStart}
            visibleEnd={visibleEnd}
            loading={isFetching}
            onPrev={handlePrevPage}
            onNext={handleNextPage}
            onPageSizeChange={handlePageSizeChange}
          />
        </>
      )}

      <DetailDrawer
        open={selectedId !== null}
        title="Activity Detail"
        onClose={() => setSelectedId(null)}
      >
        {selectedId && <ActivityDetailContent itemId={selectedId} />}
      </DetailDrawer>
    </div>
  );
}
