import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppShell } from "../AppShell";
import { PageToolbarProvider } from "./PageToolbarContext";
import { useRefreshToolbar } from "./useRefreshToolbar";

const reportFrontendEvent = vi.fn();

vi.mock("../../logging/safeReporter", () => ({
  reportFrontendEventSafely: (entry: unknown) => reportFrontendEvent(entry),
}));

vi.mock("../../api/useBusytokData", () => ({
  useShellStatus: () => ({
    data: {
      generated_at_ms: Date.now(),
      status_chips: [],
      readiness: "ready_exact" as const,
      latest_event_seq: 123,
      writer_queue_depth: null,
      aggregate_lag_ms: null,
      subscription_bridge_connectivity: "connected",
    },
    isLoading: false,
    isError: false,
  }),
}));

vi.mock("../../api/useEventSubscription", () => ({
  useEventSubscription: () => ({
    connectionStatus: "connected" as const,
  }),
}));

function Wrapper({ children }: { children: React.ReactNode }) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return (
    <QueryClientProvider client={queryClient}>
      <PageToolbarProvider>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          {children}
        </AppShell>
      </PageToolbarProvider>
    </QueryClientProvider>
  );
}

// Module-level stable default so `onRefresh` keeps a constant identity across
// renders. A per-render `async () => {}` default would destabilize the
// `handleRefresh`/`toolbar` memos in useRefreshToolbar, and
// useRegisterPageToolbar's effect would setToolbar() on every new toolbar
// reference → infinite render loop. (Production callers pass react-query's
// stable `refetch`, so this only manifests in tests with non-memoized defaults.)
const NOOP_ON_REFRESH = async (): Promise<void> => {};

function RefreshingPage({
  isFetching = false,
  onRefresh = NOOP_ON_REFRESH,
}: {
  isFetching?: boolean;
  onRefresh?: () => Promise<void>;
}) {
  useRefreshToolbar({
    surface: "overview",
    isFetching,
    onRefresh,
  });
  return <p>Page content</p>;
}

describe("useRefreshToolbar", () => {
  beforeEach(() => {
    reportFrontendEvent.mockReset();
  });

  afterEach(() => {
    // Manual cleanup — vitest.setup.ts does not register auto-cleanup
    // (globals are off), so without this each render() accumulates in the
    // same document and getByRole finds stale buttons from prior tests.
    cleanup();
    vi.clearAllMocks();
  });

  it("registers a titlebar refresh button", () => {
    render(
      <Wrapper>
        <RefreshingPage />
      </Wrapper>,
    );

    expect(screen.getByRole("button", { name: "Refresh data" })).toBeDefined();
  });

  it("logs requested and succeeded refresh events", async () => {
    const user = userEvent.setup();
    const onRefresh = vi.fn().mockResolvedValue(undefined);

    render(
      <Wrapper>
        <RefreshingPage onRefresh={onRefresh} />
      </Wrapper>,
    );

    await user.click(screen.getByRole("button", { name: "Refresh data" }));

    await waitFor(() => expect(onRefresh).toHaveBeenCalledOnce());
    expect(reportFrontendEvent).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "gui.refresh.requested" }),
    );
    expect(reportFrontendEvent).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "gui.refresh.succeeded" }),
    );
  });

  it("logs failed refresh events", async () => {
    const user = userEvent.setup();
    const onRefresh = vi.fn().mockRejectedValue(new Error("boom"));

    render(
      <Wrapper>
        <RefreshingPage onRefresh={onRefresh} />
      </Wrapper>,
    );

    await user.click(screen.getByRole("button", { name: "Refresh data" }));

    await waitFor(() => expect(onRefresh).toHaveBeenCalledOnce());
    expect(reportFrontendEvent).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "gui.refresh.failed" }),
    );
  });
});
