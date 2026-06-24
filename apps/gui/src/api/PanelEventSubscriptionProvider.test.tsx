import { describe, expect, it, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { PanelEventSubscriptionProvider } from "./PanelEventSubscriptionProvider";
import { useEventSubscription } from "./useEventSubscription";
import { queryKeys } from "./queryKeys";

const subscribeHandlers: Map<string, (payload: unknown) => void> = new Map();

const { subscribeMock } = vi.hoisted(() => ({
  subscribeMock: vi.fn((event: string, handler: (payload: unknown) => void) => {
    subscribeHandlers.set(event, handler);
    return () => {
      subscribeHandlers.delete(event);
    };
  }),
}));

vi.mock("../lib/paletteRuntime", () => ({
  createPanelBridgeRuntime: () => ({
    invoke: vi.fn(),
    subscribe: subscribeMock,
    requestClose: vi.fn(),
  }),
}));

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={qc}>
      <PanelEventSubscriptionProvider>{children}</PanelEventSubscriptionProvider>
    </QueryClientProvider>
  );
}

describe("PanelEventSubscriptionProvider", () => {
  beforeEach(() => {
    subscribeHandlers.clear();
    subscribeMock.mockClear();
  });

  it("provides EventSubscriptionContext with starting state", () => {
    const { result } = renderHook(() => useEventSubscription(), { wrapper });
    expect(result.current.connectionStatus).toBe("disconnected");
    expect(result.current.serviceStatus).toBe("starting");
    expect(result.current.bridgeStatus).toBe("disconnected");
  });

  it("subscribes to service:status and prompts:invalidate", () => {
    renderHook(() => useEventSubscription(), { wrapper });
    expect(subscribeMock).toHaveBeenCalledWith("service:status", expect.any(Function));
    expect(subscribeMock).toHaveBeenCalledWith("prompts:invalidate", expect.any(Function));
  });

  it("unsubscribes on unmount", () => {
    const { unmount } = renderHook(() => useEventSubscription(), { wrapper });
    expect(subscribeHandlers.size).toBeGreaterThan(0);
    unmount();
    expect(subscribeHandlers.size).toBe(0);
  });

  it("updates serviceStatus when bridge pushes service:status", async () => {
    const { result } = renderHook(() => useEventSubscription(), { wrapper });

    await act(async () => {
      const handler = subscribeHandlers.get("service:status");
      handler?.({ status: "ready", since_ms: Date.now() });
    });

    expect(result.current.serviceStatus).toBe("ready");
    expect(result.current.connectionStatus).toBe("connected");
  });

  it("updates connectionStatus to disconnected when service is not ready", async () => {
    const { result } = renderHook(() => useEventSubscription(), { wrapper });

    await act(async () => {
      const handler = subscribeHandlers.get("service:status");
      handler?.({ status: "unavailable", since_ms: Date.now() });
    });

    expect(result.current.serviceStatus).toBe("unavailable");
    expect(result.current.connectionStatus).toBe("disconnected");
  });

  it("updates connectionStatus to connected when service becomes ready", async () => {
    const { result } = renderHook(() => useEventSubscription(), { wrapper });

    // First set bridge to connected (but service is starting, so overall disconnected)
    // Then set service to ready
    await act(async () => {
      const handler = subscribeHandlers.get("service:status");
      handler?.({ status: "ready", since_ms: Date.now() });
    });

    expect(result.current.serviceStatus).toBe("ready");
    // bridgeStatus is set to "connected" when service:status reports "ready"
    expect(result.current.connectionStatus).toBe("connected");
  });

  it("invalidates prompts on prompts:invalidate", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidateSpy = vi.spyOn(qc, "invalidateQueries");

    const customWrapper = ({ children }: { children: React.ReactNode }) => (
      <QueryClientProvider client={qc}>
        <PanelEventSubscriptionProvider>{children}</PanelEventSubscriptionProvider>
      </QueryClientProvider>
    );

    renderHook(() => useEventSubscription(), { wrapper: customWrapper });

    await act(async () => {
      const handler = subscribeHandlers.get("prompts:invalidate");
      handler?.({});
    });

    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: queryKeys.promptsRoot() });
  });

  it("latches serviceStatus to ready when a prompts query succeeds even if the service:status push was missed", async () => {
    // Race: the panel bridge subscribe has no retained-event replay, so if the
    // native service:status=ready push lands before React subscribes, it's
    // dropped and serviceStatus stays "starting". A successful prompts query
    // (a pull, race-free) proves the service is alive → latch ready.
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const customWrapper = ({ children }: { children: React.ReactNode }) => (
      <QueryClientProvider client={qc}>
        <PanelEventSubscriptionProvider>{children}</PanelEventSubscriptionProvider>
      </QueryClientProvider>
    );
    const { result } = renderHook(() => useEventSubscription(), { wrapper: customWrapper });
    expect(result.current.serviceStatus).toBe("starting"); // push was missed

    await act(async () => {
      qc.setQueryData(["prompts", "list", { query: null }], { data: { entries: [] } });
    });

    expect(result.current.serviceStatus).toBe("ready"); // latched — action gate would pass
  });

  it("does not latch on a non-prompts query success", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const customWrapper = ({ children }: { children: React.ReactNode }) => (
      <QueryClientProvider client={qc}>
        <PanelEventSubscriptionProvider>{children}</PanelEventSubscriptionProvider>
      </QueryClientProvider>
    );
    const { result } = renderHook(() => useEventSubscription(), { wrapper: customWrapper });

    await act(async () => {
      qc.setQueryData(["overview", "summary"], { data: {} });
    });

    expect(result.current.serviceStatus).toBe("starting"); // not latched by a non-prompts query
  });
});
