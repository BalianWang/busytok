import { useMemo, useState } from "react";
import { RefreshButton } from "../../components/desktop/RefreshButton";
import { useRefreshClickHandler } from "../../components/desktop/useRefreshClickHandler";
import { useRegisterPageToolbar } from "../../components/desktop/PageToolbarContext";
import { ReceiptPreviewDialog } from "./ReceiptPreviewDialog";
import { ShareReceiptButton } from "./ShareReceiptButton";

export interface ReceiptToolbarOptions {
  surface: string;
  onRefresh: () => Promise<unknown> | unknown;
  isFetching: boolean;
  /** Initial receipt date (today, YYYY-MM-DD). */
  today: string;
}

export function useReceiptToolbar({ surface, onRefresh, isFetching, today }: ReceiptToolbarOptions) {
  const [open, setOpen] = useState(false);
  const [date, setDate] = useState(today);
  const handleRefresh = useRefreshClickHandler({ surface, onRefresh });

  const toolbar = useMemo(
    () => (
      <>
        <ShareReceiptButton onClick={() => setOpen(true)} />
        <RefreshButton onRefresh={handleRefresh} isFetching={isFetching} />
      </>
    ),
    [handleRefresh, isFetching],
  );
  useRegisterPageToolbar(toolbar);

  return (
    <ReceiptPreviewDialog
      open={open}
      date={date}
      onDateChange={setDate}
      onClose={() => setOpen(false)}
    />
  );
}
