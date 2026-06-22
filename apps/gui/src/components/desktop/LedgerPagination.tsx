const PAGE_SIZE_OPTIONS = [25, 50, 100] as const;

export interface LedgerPaginationProps {
  pageSize: number;
  pageSizeOptions?: readonly number[];
  hasPrev: boolean;
  hasNext: boolean;
  /** null = unknown (cursor-only backend without total_count). */
  totalCount: number | null;
  /** 1-based index of the first visible item, null = unknown. */
  visibleStart: number | null;
  /** 1-based index of the last visible item, null = unknown. */
  visibleEnd: number | null;
  /** 0-based page index from the cursor stack. Authoritative source for
   *  the page indicator — NOT re-derived from visibleStart, which is
   *  inaccurate under non-uniform page sizes. */
  pageIndex: number;
  loading: boolean;
  onPrev: () => void;
  onNext: () => void;
  onPageSizeChange: (size: number) => void;
  /** Accessible label for the nav region, e.g. "Activity pagination". */
  ariaLabel?: string;
}

function summaryText(
  totalCount: number | null,
  visibleStart: number | null,
  visibleEnd: number | null,
): string {
  if (visibleStart != null && visibleEnd != null && totalCount != null) {
    return `Showing ${visibleStart.toLocaleString()}–${visibleEnd.toLocaleString()} of ${totalCount.toLocaleString()}`;
  }
  if (visibleStart != null && visibleEnd != null) {
    return `Showing ${visibleStart.toLocaleString()}–${visibleEnd.toLocaleString()}`;
  }
  if (visibleEnd != null) {
    return `Showing ${visibleEnd.toLocaleString()} items`;
  }
  return "";
}

function pageIndicator(
  totalCount: number | null,
  pageIndex: number,
  pageSize: number,
): string {
  const n = pageIndex + 1;
  if (totalCount != null) {
    const total = Math.ceil(totalCount / pageSize);
    return `${n} / ${total}`;
  }
  return `Page ${n}`;
}

export function LedgerPagination({
  pageSize,
  pageSizeOptions = PAGE_SIZE_OPTIONS,
  hasPrev,
  hasNext,
  totalCount,
  visibleStart,
  visibleEnd,
  pageIndex,
  loading,
  onPrev,
  onNext,
  onPageSizeChange,
  ariaLabel = "Pagination",
}: LedgerPaginationProps) {
  const summary = summaryText(totalCount, visibleStart, visibleEnd);

  return (
    <nav className="activity-pagination" aria-label={ariaLabel}>
      {/* Left: rows-per-page selector */}
      <div className="activity-pagination__meta">
        <span className="activity-pagination__label">Rows</span>
        <div
          className="activity-pagination__rows-group"
          role="group"
          aria-label="Rows per page"
        >
          {pageSizeOptions.map((size) => (
            <button
              key={size}
              type="button"
              className={
                "activity-pagination__rows-btn" +
                (size === pageSize ? " is-active" : "")
              }
              aria-pressed={size === pageSize}
              title="Changes the number of rows and resets to the first page"
              onClick={() => onPageSizeChange(size)}
            >
              {size}
            </button>
          ))}
        </div>
      </div>

      {/* Center: summary */}
      <span className="activity-pagination__summary" aria-live="polite">
        {summary}
      </span>

      {/* Right: prev / page / next */}
      <div className="activity-pagination__nav">
        <button
          className="activity-pagination__nav-btn"
          disabled={!hasPrev || loading}
          onClick={onPrev}
          type="button"
          aria-label="Previous page"
        >
          Prev
        </button>
        <span className="activity-pagination__page-indicator" aria-label="Current page position">
          {pageIndicator(totalCount, pageIndex, pageSize)}
        </span>
        <button
          className="activity-pagination__nav-btn"
          disabled={!hasNext || loading}
          onClick={onNext}
          type="button"
          aria-label="Next page"
        >
          Next
        </button>
      </div>
    </nav>
  );
}
