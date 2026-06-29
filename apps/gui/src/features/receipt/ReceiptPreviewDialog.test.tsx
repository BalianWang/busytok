import { cleanup, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ReceiptPreviewDialog } from "./ReceiptPreviewDialog";

vi.mock("../../api/useBusytokData", () => ({
  useDailyReceipt: () => ({
    data: {
      data: {
        date: "2026-06-26",
        date_label: "FRI · JUN 26, 2026",
        timezone: "UTC",
        metrics: {
          total_tokens: 100, input_tokens: 40, output_tokens: 60, cache_read_tokens: 10,
          cache_hit_rate: 0.2, cost_usd: 1.0, cost_status: "exact",
          event_count: 3, session_count: 1, peak_hour: { label: "10:00", tokens: 100 },
        },
        top_models: [{ name: "m", tokens: 100, cost_usd: 1.0, cost_status: "exact" }],
        brand: { name: "BUSYTOK", tagline: "x", github: "x", generated_at_ms: 1_781_600_000_000 },
      },
    },
    isLoading: false,
    isError: false,
  }),
}));

afterEach(() => cleanup());

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient();
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("ReceiptPreviewDialog", () => {
  it("renders the receipt preview and two icon action buttons when open", () => {
    wrap(
      <ReceiptPreviewDialog open date="2026-06-26" onDateChange={vi.fn()} onClose={vi.fn()} />,
    );
    // Both the live preview (inside the dialog) and the off-screen
    // capture root render <ReceiptPaper /> with the same vm.
    expect(screen.getAllByText("DAILY BILL").length).toBe(2);
    // The toolbar carries two icon buttons: calendar + save (copy removed).
    expect(screen.getByRole("button", { name: /pick receipt date/i })).toBeDefined();
    expect(screen.getByRole("button", { name: /save png/i })).toBeDefined();
    // Close button (X) in the top-right corner.
    expect(screen.getByRole("button", { name: /close receipt/i })).toBeDefined();
    // Hidden date input is in the DOM and labelled for a11y.
    expect(screen.getByLabelText(/^receipt date$/i)).toBeDefined();
  });

  it("does NOT render a Copy image button (feature removed)", () => {
    wrap(
      <ReceiptPreviewDialog open date="2026-06-26" onDateChange={vi.fn()} onClose={vi.fn()} />,
    );
    expect(screen.queryByRole("button", { name: /copy image/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /copy summary/i })).toBeNull();
  });

  it("does NOT render the visible title or description (sr-only per a11y)", () => {
    wrap(
      <ReceiptPreviewDialog open date="2026-06-26" onDateChange={vi.fn()} onClose={vi.fn()} />,
    );
    // The Radix Dialog.Title is present in the DOM (a11y requirement) but
    // visually hidden via .receipt-preview__sr-only.
    const title = screen.getByText("Daily receipt");
    expect(title).toBeDefined();
    expect(title.className).toContain("receipt-preview__sr-only");
  });

  it("renders the off-screen export root as a fragment sibling of the dialog", () => {
    const { container } = wrap(
      <ReceiptPreviewDialog open date="2026-06-26" onDateChange={vi.fn()} onClose={vi.fn()} />,
    );
    // The export root must NOT be inside .receipt-preview (the dialog content);
    // it is a fragment sibling so it escapes Radix's focus trap / aria-hidden.
    const exportRoot = container.querySelector(".receipt-export-root");
    expect(exportRoot).not.toBeNull();
    expect(exportRoot?.getAttribute("aria-hidden")).toBe("true");
    expect(exportRoot?.closest(".receipt-preview")).toBeNull();
  });

  it("renders nothing when closed", () => {
    const { container } = wrap(
      <ReceiptPreviewDialog open={false} date="2026-06-26" onDateChange={vi.fn()} onClose={vi.fn()} />,
    );
    expect(container.querySelector(".receipt-preview")).toBeNull();
  });

  it("calls onClose when the close button is clicked", () => {
    const onClose = vi.fn();
    wrap(
      <ReceiptPreviewDialog open date="2026-06-26" onDateChange={vi.fn()} onClose={onClose} />,
    );
    const closeBtn = screen.getByRole("button", { name: /close receipt/i });
    closeBtn.click();
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
