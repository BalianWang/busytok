import { act, cleanup, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  PageToolbarProvider,
  usePageToolbar,
} from "../../components/desktop/PageToolbarContext";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useReceiptToolbar } from "./useReceiptToolbar";

// Mock useDailyReceipt so the dialog doesn't fire real Tauri invoke calls
// (which fail in jsdom and would hang React Query's exponential retry).
vi.mock("../../api/useBusytokData", () => ({
  useDailyReceipt: () => ({
    data: {
      data: {
        date: "2026-06-26",
        date_label: "FRI · JUN 26, 2026",
        timezone: "UTC",
        metrics: {
          total_tokens: 100, input_tokens: 40, output_tokens: 60, cache_read_tokens: 10,
          cache_creation_tokens: 1, cache_hit_rate: 0.2, cost_usd: 1.0, cost_status: "exact",
          event_count: 3, session_count: 1, peak_hour: { label: "10:00", tokens: 100 },
        },
        top_models: [{ name: "m", tokens: 100, cost_usd: 1.0, cost_status: "exact" }],
        brand: { name: "BUSYTOK", tagline: "x", github: "x", generated_at_ms: 0 },
      },
    },
    isLoading: false,
    isError: false,
  }),
}));

// Module-level stable onRefresh — a per-render `vi.fn()` would destabilize
// the `handleRefresh`/`toolbar` memos in useReceiptToolbar (the
// useRefreshClickHandler useCallback deps include `onRefresh`), and
// useRegisterPageToolbar's effect would setToolbar() on every new toolbar
// reference → infinite render loop → JS heap OOM. Same pattern as
// useRefreshToolbar.test.tsx's NOOP_ON_REFRESH.
const STABLE_ON_REFRESH = vi.fn();

// The PageToolbarProvider stores the registered toolbar in state but does
// not render it; in production, AppShell renders `{toolbarContext?.toolbar}`.
// This minimal renderer mirrors that contract so the Share button is
// actually present in the DOM for the test to click.
function ToolbarSlot() {
  const ctx = usePageToolbar();
  return <>{ctx?.toolbar ?? null}</>;
}

// HarnessInner must live INSIDE <PageToolbarProvider> so that
// useRegisterPageToolbar (called from useReceiptToolbar) resolves a real
// setToolbar via context. If the hook is called from the same component
// that renders the provider, usePageToolbar() reads the OUTER context
// (null) and the toolbar is never registered.
function HarnessInner() {
  const dialog = useReceiptToolbar({
    surface: "overview",
    onRefresh: STABLE_ON_REFRESH,
    isFetching: false,
    today: "2026-06-26",
  });
  return (
    <>
      <ToolbarSlot />
      {dialog}
    </>
  );
}

function Harness() {
  return (
    <QueryClientProvider client={new QueryClient()}>
      <PageToolbarProvider>
        <HarnessInner />
      </PageToolbarProvider>
    </QueryClientProvider>
  );
}

afterEach(() => cleanup());

describe("useReceiptToolbar", () => {
  it("renders a Share button that opens the dialog", async () => {
    render(<Harness />);
    // findByRole waits for the toolbar's useEffect (which calls setToolbar)
    // to commit before the button is present in the DOM.
    const share = await screen.findByRole("button", { name: /share daily receipt/i });
    expect(share).toBeDefined();
    await act(async () => {
      share.click();
    });
    expect(await screen.findByText("Daily receipt")).toBeDefined();
  });
});
