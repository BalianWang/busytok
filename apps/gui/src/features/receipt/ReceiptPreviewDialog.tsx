import * as Dialog from "@radix-ui/react-dialog";
import { useRef } from "react";
import { Calendar as CalendarIcon, Download as DownloadIcon } from "lucide-react";
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
  // Capture the .receipt-paper element (the paper itself — no stage wrapper)
  // so the exported PNG is filled edge-to-edge by the receipt body. The ref
  // is read inside useReceiptExport's captureBytes at click time, so it
  // always points at the latest-mounted paper.
  const paperRef = useRef<HTMLDivElement>(null);
  const dateInputRef = useRef<HTMLInputElement>(null);
  const exportApi = useReceiptExport(paperRef, date);

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
            {/* Visually-hidden Title/Description satisfy Radix Dialog a11y
                (Dialog.Content requires a Title or aria-describedby). They
                occupy no visible space. */}
            <Dialog.Title className="receipt-preview__sr-only">
              Daily receipt
            </Dialog.Title>
            <Dialog.Description className="receipt-preview__sr-only">
              Preview your day as a shareable receipt.
            </Dialog.Description>

            <div className="receipt-preview__scroll">
              {vm ? (
                <ReceiptPaper vm={vm} key="preview" />
              ) : (
                <div className="receipt-preview__loading">Loading…</div>
              )}
            </div>

            <footer className="receipt-preview__toolbar">
              {/* Hidden native date input — opened via showPicker() on icon
                  click. showPicker() is supported on Chromium ≥99 and
                  WebKit ≥17, matching Tauri's webview floor. */}
              <input
                ref={dateInputRef}
                type="date"
                aria-label="Receipt date"
                className="receipt-preview__date-input"
                value={date}
                onChange={(e) => onDateChange(e.target.value)}
              />
              <button
                type="button"
                className="receipt-action-button"
                aria-label="Pick receipt date"
                title="Pick receipt date"
                onClick={() => dateInputRef.current?.showPicker()}
              >
                <CalendarIcon size={16} strokeWidth={1.75} aria-hidden="true" />
              </button>
              <button
                type="button"
                className="receipt-action-button"
                aria-label="Save PNG"
                title="Save PNG"
                disabled={exportApi.busy || !vm}
                onClick={exportApi.savePng}
              >
                <DownloadIcon size={16} strokeWidth={1.75} aria-hidden="true" />
              </button>
            </footer>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
      {/* Off-screen export root — OUTSIDE the dialog content so it is not
          focus-trapped or announced. Captures the .receipt-paper twin (no
          stage wrapper) for an edge-to-edge receipt image. */}
      <div className="receipt-export-root" aria-hidden="true">
        {vm && (
          <ReceiptPaper
            vm={vm}
            key="export"
            // Forward paperRef so captureBytes targets the paper element.
            paperRef={paperRef}
          />
        )}
      </div>
    </>
  );
}
