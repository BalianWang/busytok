import { useId, type Ref } from "react";
import type { ReceiptViewModel } from "./viewModel";
import "./receipt.css";

interface ReceiptPaperProps {
  vm: ReceiptViewModel;
  /** Optional ref forwarded to the .receipt-paper element so callers
   * (e.g. the export pipeline) can capture it directly. */
  paperRef?: Ref<HTMLDivElement>;
}

export function ReceiptPaper({ vm, paperRef }: ReceiptPaperProps) {
  const empty = vm.totalTokensRaw === 0 && vm.items.length === 0;
  // Unique per-instance ID so preview + export root (both render <ReceiptPaper>)
  // don't collide on the same SVG pattern id in the DOM.
  const scallopId = `receipt-scallop-${useId()}`;
  return (
    <div className="receipt-paper" ref={paperRef}>
      {/* Rectangular brand stamp — red bordered "BUSYTOK" seal, tilted.
          Restored to the original pill-shaped div stamp (commit 351cfef),
          with content narrowed to just the wordmark. */}
      <div className="receipt-stamp" aria-hidden="true">BUSYTOK</div>

      <header className="receipt__header">
        <div className="receipt__brand">DAILY BILL</div>
        <div className="receipt__subtitle">AI CODING · TOKEN RECEIPT</div>
      </header>

      <div className="receipt__divider" />

      <div className="receipt__meta">
        <span>{vm.dateLabel}</span>
        <span>PRINTED {vm.generatedAtLabel}</span>
      </div>

      <div className="receipt__body">
        {empty ? (
          <div className="receipt-paper__empty">No usage recorded for this day.</div>
        ) : (
          <>
            <section className="receipt__items">
              <div className="receipt__items-header">
                <span>ITEM</span>
                <span>TOKENS</span>
                <span>COST</span>
              </div>
              {vm.items.map((item) => (
                <div key={item.name} className="receipt__item">
                  <span className="receipt__item-name">{item.name}</span>
                  <span className="receipt__item-tokens">{item.tokens}</span>
                  <span className="receipt__item-cost">{item.cost}</span>
                </div>
              ))}
              {vm.truncated && (
                <div className="receipt__items-truncated" aria-hidden="true">
                  · · ·
                </div>
              )}
            </section>

            <div className="receipt__total">
              <span className="receipt__total-label">TOTAL</span>
              <span className="receipt__total-tokens">{vm.total.tokens}</span>
              <span className="receipt__total-cost">{vm.total.cost}</span>
            </div>

            {/* Breakdown — auxiliary stats band below TOTAL, centered. */}
            <div className="receipt__breakdown">
              <div className="receipt__breakdown-stats">
                <span>{vm.summary}</span>
                <span>CACHE HIT {vm.cacheHitRate}</span>
              </div>
            </div>
          </>
        )}
      </div>

      {/* Footer: frameless QR conversion block (CTA → QR → short URL), then
          the two-line trust + signature. No white box — the previous dashed
          border read like a form card and broke the paper continuity. */}
      <footer className="receipt__footer">
        <div className="receipt__qr-block">
          <span className="receipt__qr-hint">SCAN TO TRY BUSYTOK</span>
          <img
            className="receipt__qr"
            src="/busytok-gh-qr.svg"
            alt="Scan to try busytok"
            width="56"
            height="56"
          />
        </div>
        <div className="receipt__footer-brand">
          LOCAL-FIRST · NO PROMPTS UPLOADED
        </div>
      </footer>

      {/* Capture-safe scalloped bottom edge (CSS mask is unreliable in
          foreignObject capture). */}
      <svg className="receipt__tear" width="420" height="14" aria-hidden="true">
        <defs>
          <pattern id={scallopId} width="20" height="14" patternUnits="userSpaceOnUse">
            <path d="M0,4 H20 a10,10 0 0,1 -20,0 Z" fill="#f6efe2" />
          </pattern>
        </defs>
        <rect width="420" height="14" fill={`url(#${scallopId})`} />
      </svg>
    </div>
  );
}
