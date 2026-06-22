import { useCallback, useState } from "react";

interface CursorPageEntry {
  cursor: string | null;
  pageIndex: number;
  /** Total items on all pages before this one. Used for accurate
   * "Showing X–Y" display under cursor pagination, where page sizes
   * are not guaranteed to be uniform (last page is frequently short). */
  itemsBefore: number;
}

export interface CursorPageStack {
  /** Current cursor to pass to the API (null = first page). */
  cursor: string | null;
  /** 0-based page index. */
  pageIndex: number;
  /** Total items on all pages before the current one (0 on first page). */
  itemsBefore: number;
  hasPrev: boolean;
  /** Push the next_cursor from the API response.
   *  @param nextCursor — null signals "no more pages" (no-op).
   *  @param currentItemCount — items on the page being left behind. */
  goNext: (nextCursor: string | null, currentItemCount: number) => void;
  /** Pop the stack and return to previous page cursor. */
  goPrev: () => void;
  /** Clear stack back to first page. */
  reset: () => void;
}

/**
 * Frontend-maintained cursor page stack enabling Prev without backend
 * `prev_cursor` support. Pages push their cursors onto a history stack
 * on Next; Prev pops the stack and re-exposes the previous cursor.
 *
 * `itemsBefore` tracks the cumulative item count across pages so the
 * UI can display accurate "Showing X–Y" ranges without assuming every
 * page was exactly `pageSize` items long.
 *
 * `hasNext` is NOT tracked here — it's derived from the API response's
 * `next_cursor` field, which is the authoritative source.
 */
export function useCursorPageStack(): CursorPageStack {
  const [stack, setStack] = useState<CursorPageEntry[]>([
    { cursor: null, pageIndex: 0, itemsBefore: 0 },
  ]);

  const current = stack[stack.length - 1];

  const goNext = useCallback(
    (nextCursor: string | null, currentItemCount: number) => {
      if (nextCursor == null) return;
      setStack((prev) => {
        const last = prev[prev.length - 1];
        return [
          ...prev,
          {
            cursor: nextCursor,
            pageIndex: prev.length,
            itemsBefore: last.itemsBefore + currentItemCount,
          },
        ];
      });
    },
    [],
  );

  const goPrev = useCallback(() => {
    setStack((prev) => {
      if (prev.length <= 1) return prev;
      return prev.slice(0, -1);
    });
  }, []);

  const reset = useCallback(() => {
    setStack([{ cursor: null, pageIndex: 0, itemsBefore: 0 }]);
  }, []);

  return {
    cursor: current.cursor,
    pageIndex: current.pageIndex,
    itemsBefore: current.itemsBefore,
    hasPrev: stack.length > 1,
    goNext,
    goPrev,
    reset,
  };
}
