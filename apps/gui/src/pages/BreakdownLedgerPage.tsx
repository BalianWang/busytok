import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import type {
  BreakdownKindDto,
  BreakdownListItemDto,
  RangePresetDto,
} from "@busytok/protocol-types";
import { useBreakdownList } from "../api/useBusytokData";
import { PageState } from "../components/PageState";
import { SegmentedControl } from "../components/desktop/SegmentedControl";
import { LedgerPagination } from "../components/desktop/LedgerPagination";
import { LedgerTable } from "../components/desktop/LedgerTable";
import { DetailDrawer } from "../components/desktop/DetailDrawer";
import { useRefreshToolbar } from "../components/desktop/useRefreshToolbar";
import { useCursorPageStack } from "../hooks/useCursorPageStack";
import { safeReportEvent } from "../logging/reporter";

const RANGE_OPTIONS: Array<{ value: RangePresetDto; label: string }> = [
  { value: "day", label: "Day" },
  { value: "week", label: "Week" },
  { value: "month", label: "Month" },
  { value: "year", label: "Year" },
];

const PAGE_LIMIT = 100;

interface BreakdownLedgerPageProps<T extends { id: string }> {
  kind: BreakdownKindDto;
  title: string;
  columns: Array<ColumnDef<T>>;
  toRows: (items: BreakdownListItemDto[]) => T[];
  emptyTitle?: string;
  emptyMessage?: string;
  drawerTitle: string;
  renderDetail: (itemId: string, range: RangePresetDto) => ReactNode;
}

export function BreakdownLedgerPage<T extends { id: string }>({
  kind,
  title,
  columns,
  toRows,
  emptyTitle = "No data",
  emptyMessage = "Try a different range to widen the view.",
  drawerTitle,
  renderDetail,
}: BreakdownLedgerPageProps<T>) {
  const [range, setRange] = useState<RangePresetDto>("month");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [pageSize, setPageSize] = useState(PAGE_LIMIT);
  const pageStack = useCursorPageStack();

  const { data, isLoading, isError, isFetching, refetch } =
    useBreakdownList({
      kind,
      range,
      cursor: pageStack.cursor,
      limit: pageSize,
    });

  useRefreshToolbar({
    surface: kind,
    onRefresh: refetch,
    isFetching,
  });

  const clearSelection = useCallback(() => setSelectedId(null), []);

  useEffect(() => {
    pageStack.reset();
    clearSelection();
  }, [range, pageSize, pageStack.reset, clearSelection]);

  const items = data?.data.items ?? [];
  const nextCursor = data?.data.next_cursor ?? null;

  const rows = useMemo(() => toRows(items), [items, toRows]);

  const handleNextPage = useCallback(() => {
    if (nextCursor != null && !isFetching) {
      clearSelection();
      pageStack.goNext(nextCursor, items.length);
      safeReportEvent("gui.pagination.next_requested", "Pagination next requested", {
        surface: kind, page_index: pageStack.pageIndex + 1, page_size: pageSize,
      });
    }
  }, [nextCursor, isFetching, pageStack, pageSize, items.length, kind, clearSelection]);

  const handlePrevPage = useCallback(() => {
    if (!isFetching) {
      clearSelection();
      pageStack.goPrev();
      safeReportEvent("gui.pagination.prev_requested", "Pagination prev requested", {
        surface: kind, page_index: pageStack.pageIndex - 1, page_size: pageSize,
      });
    }
  }, [pageStack, pageSize, kind, isFetching, clearSelection]);

  const handlePageSizeChange = useCallback((size: number) => {
    clearSelection();
    setPageSize(size);
    safeReportEvent("gui.pagination.page_size_changed", "Pagination page size changed", {
      surface: kind, page_size: size,
    });
  }, [kind, clearSelection]);

  // Accurate range using cumulative itemsBefore (not pageIndex * pageSize)
  const visibleStart = items.length > 0 ? pageStack.itemsBefore + 1 : null;
  const visibleEnd = items.length > 0 ? pageStack.itemsBefore + items.length : null;

  const handleRowSelect = useCallback((row: T) => {
    setSelectedId((prev) => (prev === row.id ? null : row.id));
  }, []);

  if (isLoading && !data) {
    return (
      <div className="breakdown-page">
        <PageState kind="loading" title={title} message={`Loading ${title.toLocaleLowerCase()} data...`} />
      </div>
    );
  }

  if (isError && !data) {
    return (
      <div className="breakdown-page">
        <PageState
          kind="error"
          title={`${title} unavailable`}
          message={`Could not load ${title.toLocaleLowerCase()} data.`}
          actionLabel="Retry"
          onAction={() => refetch()}
        />
      </div>
    );
  }

  return (
    <div className="breakdown-page">
      <section className="activity-page__range">
        <SegmentedControl label="Range" value={range} options={RANGE_OPTIONS} onChange={(v) => setRange(v)} />
      </section>

      {rows.length === 0 && !isFetching ? (
        <PageState kind="empty" title={emptyTitle} message={emptyMessage} />
      ) : (
        <>
          <section className="activity-page__table-shell page-surface">
            <LedgerTable
              ariaLabel={`${title} ledger`}
              columns={columns}
              rows={rows}
              selectedId={selectedId}
              onSelect={handleRowSelect}
            />
          </section>

          <LedgerPagination
            ariaLabel={`${title} pagination`}
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
        title={drawerTitle}
        onClose={() => setSelectedId(null)}
      >
        {selectedId && renderDetail(selectedId, range)}
      </DetailDrawer>
    </div>
  );
}
