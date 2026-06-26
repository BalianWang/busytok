import * as Dialog from "@radix-ui/react-dialog";
import { useRef } from "react";
import { useDailyReceipt } from "../../api/useBusytokData";
import { ReceiptPaper } from "./ReceiptPaper";
import { toReceiptViewModel } from "./viewModel";
import { useReceiptExport } from "./useReceiptExport";

interface Props {
  open: boolean;
  date: string;
  onDateChange: (date: string) => void;
  onClose: () => void;
}

export function ReceiptPreviewDialog({ open, date, onDateChange, onClose }: Props) {
  const envelope = useDailyReceipt(date);
  const dto = envelope.data?.data ?? null;
  const vm = dto ? toReceiptViewModel(dto) : null;
  const exportRootRef = useRef<HTMLDivElement>(null);
  const exportApi = useReceiptExport(exportRootRef, vm ?? EMPTY_VM, date);

  return (
    <>
    <Dialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="receipt-preview__overlay" />
        <Dialog.Content className="receipt-preview">
          <Dialog.Title className="receipt-preview__title">Daily receipt</Dialog.Title>
          <Dialog.Description className="receipt-preview__desc">
            Preview your day as a shareable receipt.
          </Dialog.Description>

          <label className="receipt-preview__date">
            <span>Receipt date</span>
            <input
              type="date"
              aria-label="Receipt date"
              value={date}
              onChange={(e) => onDateChange(e.target.value)}
            />
          </label>

          <div className="receipt-preview__scroll">
            {vm ? (
              <div className="receipt-preview__paper">
                {/* Scaled live preview */}
                <ReceiptPaper vm={vm} key="preview" />
              </div>
            ) : (
              <div className="receipt-preview__loading">Loading…</div>
            )}
          </div>

          <footer className="receipt-preview__actions">
            <button type="button" className="btn btn--secondary" onClick={exportApi.copySummary}>
              Copy summary
            </button>
            <button type="button" className="btn btn--secondary" onClick={exportApi.savePng} disabled={exportApi.busy || !vm}>
              Save PNG
            </button>
            <button type="button" className="btn btn--primary" onClick={exportApi.copyImage} disabled={exportApi.busy || !vm}>
              Copy image
            </button>
            <Dialog.Close asChild>
              <button type="button" className="btn btn--ghost">Close</button>
            </Dialog.Close>
          </footer>

        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
      {/* Off-screen export root — OUTSIDE the dialog content so it is not
          focus-trapped or announced; only the capture reads it. */}
      <div className="receipt-export-root" aria-hidden="true">
        <div ref={exportRootRef}>{vm && <ReceiptPaper vm={vm} key="export" />}</div>
      </div>
    </>
  );
}

const EMPTY_VM = toReceiptViewModel({
  date: "1970-01-01",
  date_label: "",
  timezone: "UTC",
  metrics: {
    total_tokens: 0, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0,
    cache_creation_tokens: 0, cache_hit_rate: null, cost_usd: null,
    cost_status: "unavailable", event_count: 0, session_count: 0, peak_hour: null,
  },
  top_models: [],
  brand: { name: "BUSYTOK", tagline: "", github: "", generated_at_ms: 0 },
});
