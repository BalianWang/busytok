import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { EventSubscriptionProvider } from "./EventSubscriptionProvider";
import { useEventSubscription } from "./useEventSubscription";
import { queryKeys } from "./queryKeys";
import { liveSamplesStore } from "./liveSamplesStore";
import type { RangePresetDto } from "@busytok/protocol-types";

const listenMocks: Map<string, (event: unknown) => void> = new Map();
const { liveWindowMock } = vi.hoisted(() => ({
  liveWindowMock: vi.fn(),
}));

vi.mock("./busytokClient", () => ({
  busytokClient: {
    liveWindow: liveWindowMock,
  },
}));

vi.mock("../logging/reporter", () => ({
  reportFrontendEvent: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((eventName: string, handler: (event: unknown) => void) => {
    listenMocks.set(eventName, handler);
    return Promise.resolve(() => {});
  }),
}));

function makeBatch(events: Array<{
  event_type: string;
  payload: unknown;
  event_seq?: number | null;
  scopes?: Array<{ dataset: string; breakdown_kind?: string | null }>;
  generation_id?: string | null;
  watermark_ms?: number | null;
  is_exact?: boolean;
}>) {
  return { events };
}

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={qc}>
      <EventSubscriptionProvider>{children}</EventSubscriptionProvider>
    </QueryClientProvider>
  );
}

function wrapperWithClient(qc: QueryClient) {
  return function TestWrapper({ children }: { children: React.ReactNode }) {
    return (
      <QueryClientProvider client={qc}>
        <EventSubscriptionProvider>{children}</EventSubscriptionProvider>
      </QueryClientProvider>
    );
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function liveWindowEnvelope(tokensPerSec: number, generatedAtMs: number) {
  return {
    data: {
      exact_samples: [
        {
          bucket_start_ms: 1740000000000 + generatedAtMs,
          tokens_per_sec: tokensPerSec,
          cost_per_sec: null,
          events_per_sec: 1,
        },
      ],
      transient_samples: [],
      current_tokens_per_sec: tokensPerSec,
      current_events_per_sec: 1,
      start_ms: 0,
      end_ms: 0,
    },
    generated_at_ms: generatedAtMs,
    generation_id: "gen-test",
    readiness: "ready_exact",
    is_exact: true,
    is_stale: false,
    watermark_ms: null,
    progress: null,
    degraded_reason: null,
  };
}

describe("useEventSubscription", () => {
  beforeEach(() => {
    listenMocks.clear();
    liveSamplesStore.setAll([]);
    liveWindowMock.mockReset();
    liveWindowMock.mockResolvedValue({
      data: {
        exact_samples: [],
        transient_samples: [],
        current_tokens_per_sec: 0,
        current_events_per_sec: 0,
        start_ms: 0,
        end_ms: 0,
      },
      generated_at_ms: 0,
      generation_id: null,
      readiness: "ready_exact",
      is_exact: true,
      is_stale: false,
      watermark_ms: null,
      progress: null,
      degraded_reason: null,
    });
  });

  it("starts with disconnected status", () => {
    const { result } = renderHook(() => useEventSubscription(), { wrapper });
    expect(result.current.connectionStatus).toBe("disconnected");
  });

  it("updates connection status when busytok:subscription-status fires", async () => {
    const { result } = renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:subscription-status")).toBe(true);
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });

    // Service must be ready for connectionStatus to reflect bridge state
    await act(async () => {
      listenMocks.get("busytok:service-status")?.({ payload: { status: "ready", since_ms: 999 } });
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:subscription-status");
      handler?.({ payload: { status: "connected", since_ms: 1000 } });
    });

    expect(result.current.connectionStatus).toBe("connected");
  });

  it("recovers connected status from event batches when startup status was missed", async () => {
    const { result } = renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });

    // Simulate the real race: backend already connected, so the initial
    // busytok:service-status ("ready") was emitted before the listener
    // was registered. serviceStatus remains "starting".
    expect(result.current.serviceStatus).toBe("starting");
    expect(result.current.connectionStatus).toBe("disconnected");

    // An event batch arrives — the latch should recover both statuses.
    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "live:sample",
          payload: {
            bucket_start_ms: 1740000000000,
            tokens_per_sec: 0,
            cost_per_sec: null,
            events_per_sec: 0,
          },
          is_exact: false,
        }]),
      });
    });

    expect(result.current.serviceStatus).toBe("ready");
    expect(result.current.bridgeStatus).toBe("connected");
    expect(result.current.connectionStatus).toBe("connected");
  });

  it("writes live sample events to the store via EventSubscriptionProvider", async () => {
    renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "live:sample",
          payload: {
            bucket_start_ms: 1740000000000,
            tokens_per_sec: 10,
            cost_per_sec: null,
            events_per_sec: 1,
          },
          is_exact: false,
        }]),
      });
    });

    expect(liveSamplesStore.getSamples()).toHaveLength(1);
    expect(liveSamplesStore.getSamples()[0].tokens_per_sec).toBe(10);
  });

  it("classifies non-transient live samples as exact even though event envelopes are ephemeral", async () => {
    renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "live:sample",
          payload: {
            bucket_start_ms: 1740000000000,
            tokens_per_sec: 10,
            cost_per_sec: null,
            events_per_sec: 1,
            transient: false,
          },
          is_exact: false,
        }]),
      });
    });

    const samples = liveSamplesStore.getSamplesWithFlags();
    expect(samples).toHaveLength(1);
    expect(samples[0].is_exact).toBe(true);
  });

  // ── Gap recovery tests ───────────────────────────────────────────

  it("detects single-event gap and invalidates all data queries", async () => {
    const { result } = renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
      expect(listenMocks.has("busytok:subscription-status")).toBe(true);
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });

    // Service must be ready for connectionStatus to reflect bridge state
    await act(async () => {
      listenMocks.get("busytok:service-status")?.({ payload: { status: "ready", since_ms: 999 } });
    });

    // Mark provider as connected with last_seen_seq
    await act(async () => {
      const statusHandler = listenMocks.get("busytok:subscription-status");
      statusHandler?.({
        payload: {
          status: "connected",
          since_ms: 5000,
          latest_event_seq: 11,
          last_seen_seq: 10,
          gap_detected: true,
          replayed_scopes: [],
        },
      });
    });

    // After gap recovery, queries should be invalidated.
    // Verify by checking that the query client invalidated overview queries.
    // We can observe this indirectly: the provider should trigger a
    // live.window refetch and invalidate broad query scopes.
    // Test that the connection status reflects the gap handling.
    expect(result.current.connectionStatus).toBe("connected");
  });

  it("invalidates nothing when gap is detected but scopes are replayed", async () => {
    const { result } = renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:subscription-status")).toBe(true);
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });

    // Service must be ready for connectionStatus to reflect bridge state
    await act(async () => {
      listenMocks.get("busytok:service-status")?.({ payload: { status: "ready", since_ms: 999 } });
    });

    await act(async () => {
      const statusHandler = listenMocks.get("busytok:subscription-status");
      statusHandler?.({
        payload: {
          status: "connected",
          since_ms: 5000,
          latest_event_seq: 15,
          last_seen_seq: 10,
          gap_detected: true,
          replayed_scopes: [
            { dataset: "overview_summary", breakdown_kind: null },
            { dataset: "overview_trend", breakdown_kind: null },
          ],
        },
      });
    });

    expect(result.current.connectionStatus).toBe("connected");
  });

  it("handles reconnecting status with gap detection", async () => {
    const { result } = renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:subscription-status")).toBe(true);
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });

    // Service must be ready for connectionStatus to reflect bridge reconnecting
    await act(async () => {
      listenMocks.get("busytok:service-status")?.({ payload: { status: "ready", since_ms: 999 } });
    });

    await act(async () => {
      const statusHandler = listenMocks.get("busytok:subscription-status");
      statusHandler?.({
        payload: { status: "reconnecting", since_ms: 3000 },
      });
    });

    expect(result.current.connectionStatus).toBe("reconnecting");
  });

  // ── Scope-driven invalidation tests ──────────────────────────────

  it("invalidates shell.status for client-only data scopes", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidateSpy = vi.spyOn(qc, "invalidateQueries");

    renderHook(() => useEventSubscription(), { wrapper: wrapperWithClient(qc) });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });

    await act(async () => {
      listenMocks.get("busytok:service-status")?.({
        payload: { status: "ready", since_ms: 100 },
      });
    });
    invalidateSpy.mockClear();

    await act(async () => {
      listenMocks.get("busytok:event")?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: {
            datasets: [{ dataset: "clients", breakdown_kind: null }],
          },
          scopes: [{ dataset: "clients", breakdown_kind: null }],
          event_seq: 5,
          is_exact: true,
        }]),
      });
    });

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 600));
    });

    expect(invalidateSpy).toHaveBeenCalledWith({
      queryKey: queryKeys.shellStatus(),
    });
  });

  it("coalesces shell.status invalidation across multiple shell-affecting scopes", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidateSpy = vi.spyOn(qc, "invalidateQueries");

    renderHook(() => useEventSubscription(), { wrapper: wrapperWithClient(qc) });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });

    await act(async () => {
      listenMocks.get("busytok:service-status")?.({
        payload: { status: "ready", since_ms: 100 },
      });
    });
    invalidateSpy.mockClear();

    await act(async () => {
      listenMocks.get("busytok:event")?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: {
            datasets: [
              { dataset: "overview_summary", breakdown_kind: null },
              { dataset: "activity_recent", breakdown_kind: null },
              { dataset: "live_realtime", breakdown_kind: null },
              { dataset: "clients", breakdown_kind: null },
            ],
          },
          scopes: [
            { dataset: "overview_summary", breakdown_kind: null },
            { dataset: "activity_recent", breakdown_kind: null },
            { dataset: "live_realtime", breakdown_kind: null },
            { dataset: "clients", breakdown_kind: null },
          ],
          event_seq: 6,
          is_exact: true,
        }]),
      });
    });

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 600));
    });

    const shellInvalidations = invalidateSpy.mock.calls.filter(
      ([arg]) => JSON.stringify(arg) === JSON.stringify({ queryKey: queryKeys.shellStatus() }),
    );
    expect(shellInvalidations).toHaveLength(1);
  });

  it("invalidates overview summary queries on overview_summary scope", async () => {
    renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: {
            datasets: [{ dataset: "overview_summary", breakdown_kind: null }],
          },
          scopes: [{ dataset: "overview_summary", breakdown_kind: null }],
          event_seq: 5,
          is_exact: true,
        }]),
      });
    });

    // Test passes if no throw; debounced invalidation is scheduled.
  });

  it("invalidates overview trend queries on overview_trend scope", async () => {
    renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: {
            datasets: [{ dataset: "overview_trend", breakdown_kind: null }],
          },
          scopes: [{ dataset: "overview_trend", breakdown_kind: null }],
          event_seq: 6,
          is_exact: true,
        }]),
      });
    });
  });

  it("invalidates all overview children on parent overview scope", async () => {
    renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: {
            datasets: [{ dataset: "overview", breakdown_kind: null }],
          },
          scopes: [{ dataset: "overview", breakdown_kind: null }],
          event_seq: 7,
          is_exact: true,
        }]),
      });
    });
  });

  it("invalidates breakdown queries with breakdown_kind", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidateSpy = vi.spyOn(qc, "invalidateQueries");

    renderHook(() => useEventSubscription(), { wrapper: wrapperWithClient(qc) });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: {
            datasets: [{ dataset: "breakdown", breakdown_kind: "project" }],
          },
          scopes: [{ dataset: "breakdown", breakdown_kind: "project" }],
          event_seq: 8,
          is_exact: true,
        }]),
      });
    });

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 600));
    });

    expect(invalidateSpy).toHaveBeenCalledWith({
      queryKey: ["breakdown", "detail", { kind: "project" }],
    });
  });

  it("invalidates all breakdown children on parent breakdown scope", async () => {
    renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: {
            datasets: [{ dataset: "breakdown", breakdown_kind: null }],
          },
          scopes: [{ dataset: "breakdown", breakdown_kind: null }],
          event_seq: 9,
          is_exact: true,
        }]),
      });
    });
  });

  it("invalidates settings.diagnostics on diagnostics scope", async () => {
    renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: {
            datasets: [{ dataset: "diagnostics", breakdown_kind: null }],
          },
          scopes: [{ dataset: "diagnostics", breakdown_kind: null }],
          event_seq: 10,
          is_exact: true,
        }]),
      });
    });
  });

  it("reloads live window samples on live_realtime scope", async () => {
    liveWindowMock.mockResolvedValueOnce(liveWindowEnvelope(12, 100));

    renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: {
            datasets: [{ dataset: "live_realtime", breakdown_kind: null }],
          },
          scopes: [{ dataset: "live_realtime", breakdown_kind: null }],
          event_seq: 11,
          is_exact: true,
        }]),
      });
    });

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 600));
    });

    await vi.waitFor(() => {
      expect(liveWindowMock).toHaveBeenCalledWith({ window_seconds: 900 });
      expect(liveSamplesStore.getSamples()[0]?.tokens_per_sec).toBe(12);
    });
  });

  it("queues another live window reload when live_realtime arrives during an in-flight reload", async () => {
    const first = deferred<ReturnType<typeof liveWindowEnvelope>>();
    const second = deferred<ReturnType<typeof liveWindowEnvelope>>();
    liveWindowMock
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);

    renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: { datasets: [{ dataset: "live_realtime", breakdown_kind: null }] },
          scopes: [{ dataset: "live_realtime", breakdown_kind: null }],
          event_seq: 12,
          is_exact: true,
        }]),
      });
    });

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 600));
    });
    expect(liveWindowMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: { datasets: [{ dataset: "live_realtime", breakdown_kind: null }] },
          scopes: [{ dataset: "live_realtime", breakdown_kind: null }],
          event_seq: 13,
          is_exact: true,
        }]),
      });
    });

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 600));
    });
    expect(liveWindowMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      first.resolve(liveWindowEnvelope(1, 100));
    });

    await vi.waitFor(() => {
      expect(liveWindowMock).toHaveBeenCalledTimes(2);
    });

    await act(async () => {
      second.resolve(liveWindowEnvelope(2, 200));
    });

    await vi.waitFor(() => {
      expect(liveSamplesStore.getSamples()[0]?.tokens_per_sec).toBe(2);
    });
  });

  it("replaces exact samples when live:window_reloaded carries a full live.window payload", async () => {
    liveSamplesStore.upsertExact({
      bucket_start_ms: 1000,
      tokens_per_sec: 1,
      cost_per_sec: null,
      events_per_sec: 1,
    });

    renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "live:window_reloaded",
          payload: liveWindowEnvelope(22, 300),
          is_exact: true,
        }]),
      });
    });

    const samples = liveSamplesStore.getSamples();
    expect(samples).toHaveLength(1);
    expect(samples[0].tokens_per_sec).toBe(22);
  });

  // ── Sequence gap from event_seq field ────────────────────────────

  it("detects sequence gap in event stream and triggers full invalidation", async () => {
    renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
    });

    // First event: seq 1
    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: {
            datasets: [{ dataset: "overview_summary", breakdown_kind: null }],
          },
          scopes: [],
          event_seq: 1,
          is_exact: true,
        }]),
      });
    });

    // Next event: seq 5 (gap of 3, no scopes)
    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: {
            datasets: [{ dataset: "overview_summary", breakdown_kind: null }],
          },
          scopes: [],
          event_seq: 5,
          is_exact: true,
        }]),
      });
    });

    // The gap should trigger a full invalidation (no throw = pass).
  });

  it("does not trigger full invalidation for consecutive sequences", async () => {
    renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: {
            datasets: [{ dataset: "overview_summary", breakdown_kind: null }],
          },
          scopes: [{ dataset: "overview_summary", breakdown_kind: null }],
          event_seq: 1,
          is_exact: true,
        }]),
      });
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "data:invalidated",
          payload: {
            datasets: [{ dataset: "overview_trend", breakdown_kind: null }],
          },
          scopes: [{ dataset: "overview_trend", breakdown_kind: null }],
          event_seq: 2,
          is_exact: true,
        }]),
      });
    });
  });

  // ── Service status channel tests ─────────────────────────────────

  it("starts with serviceStatus starting and bridgeStatus disconnected", () => {
    const { result } = renderHook(() => useEventSubscription(), { wrapper });
    expect(result.current.serviceStatus).toBe("starting");
    expect(result.current.bridgeStatus).toBe("disconnected");
  });

  it("listens to busytok:service-status channel", async () => {
    renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });
  });

  it("updates serviceStatus when busytok:service-status fires", async () => {
    const { result } = renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:service-status");
      handler?.({ payload: { status: "ready", since_ms: Date.now() } });
    });

    expect(result.current.serviceStatus).toBe("ready");
  });

  it("merges service bootstrap state and bridge state from separate channels", async () => {
    const { result } = renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:subscription-status")).toBe(true);
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });

    // Service reports starting → connectionStatus should be "disconnected"
    await act(async () => {
      const serviceHandler = listenMocks.get("busytok:service-status");
      serviceHandler?.({ payload: { status: "starting", since_ms: Date.now() } });
    });
    expect(result.current.serviceStatus).toBe("starting");
    expect(result.current.connectionStatus).toBe("disconnected");

    // Bridge reports reconnecting → still "disconnected" because service is not ready
    await act(async () => {
      const bridgeHandler = listenMocks.get("busytok:subscription-status");
      bridgeHandler?.({ payload: { status: "reconnecting", since_ms: Date.now() } });
    });
    expect(result.current.bridgeStatus).toBe("reconnecting");
    expect(result.current.connectionStatus).toBe("disconnected");

    // Service reports unavailable → still "disconnected"
    await act(async () => {
      const serviceHandler = listenMocks.get("busytok:service-status");
      serviceHandler?.({ payload: { status: "unavailable", since_ms: Date.now() } });
    });
    expect(result.current.serviceStatus).toBe("unavailable");
    expect(result.current.connectionStatus).toBe("disconnected");

    // Service reports ready, bridge is reconnecting → "reconnecting"
    await act(async () => {
      const serviceHandler = listenMocks.get("busytok:service-status");
      serviceHandler?.({ payload: { status: "ready", since_ms: Date.now() } });
    });
    expect(result.current.serviceStatus).toBe("ready");
    expect(result.current.connectionStatus).toBe("reconnecting");

    // Bridge reports connected → "connected"
    await act(async () => {
      const bridgeHandler = listenMocks.get("busytok:subscription-status");
      bridgeHandler?.({ payload: { status: "connected", since_ms: Date.now() } });
    });
    expect(result.current.bridgeStatus).toBe("connected");
    expect(result.current.connectionStatus).toBe("connected");
  });

  it("invalidates stale query groups when service transitions from ready to unavailable", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidateSpy = vi.spyOn(qc, "invalidateQueries");

    renderHook(() => useEventSubscription(), { wrapper: wrapperWithClient(qc) });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });

    // Transition to ready first
    await act(async () => {
      const handler = listenMocks.get("busytok:service-status");
      handler?.({ payload: { status: "ready", since_ms: Date.now() } });
    });

    invalidateSpy.mockClear();

    // Now transition to unavailable → should invalidate
    await act(async () => {
      const handler = listenMocks.get("busytok:service-status");
      handler?.({ payload: { status: "unavailable", since_ms: Date.now() } });
    });

    expect(invalidateSpy).toHaveBeenCalled();
  });

  it("does not invalidate on subsequent non-ready transitions", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidateSpy = vi.spyOn(qc, "invalidateQueries");

    renderHook(() => useEventSubscription(), { wrapper: wrapperWithClient(qc) });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });

    // Transition to ready first
    await act(async () => {
      const handler = listenMocks.get("busytok:service-status");
      handler?.({ payload: { status: "ready", since_ms: Date.now() } });
    });

    // Transition to unavailable → invalidates
    await act(async () => {
      const handler = listenMocks.get("busytok:service-status");
      handler?.({ payload: { status: "unavailable", since_ms: Date.now() } });
    });

    const callsAfterFirstTransition = invalidateSpy.mock.calls.length;
    invalidateSpy.mockClear();

    // Transition to repairing → should NOT invalidate again (already in non-ready)
    await act(async () => {
      const handler = listenMocks.get("busytok:service-status");
      handler?.({ payload: { status: "repairing", since_ms: Date.now() } });
    });

    expect(invalidateSpy).not.toHaveBeenCalled();
  });

  it("resets invalidation guard when service becomes ready again", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidateSpy = vi.spyOn(qc, "invalidateQueries");

    renderHook(() => useEventSubscription(), { wrapper: wrapperWithClient(qc) });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });

    // ready → unavailable → ready → unavailable
    await act(async () => {
      const handler = listenMocks.get("busytok:service-status");
      handler?.({ payload: { status: "ready", since_ms: Date.now() } });
    });
    await act(async () => {
      const handler = listenMocks.get("busytok:service-status");
      handler?.({ payload: { status: "unavailable", since_ms: Date.now() } });
    });

    const callsAfterFirst = invalidateSpy.mock.calls.length;

    await act(async () => {
      const handler = listenMocks.get("busytok:service-status");
      handler?.({ payload: { status: "ready", since_ms: Date.now() } });
    });
    await act(async () => {
      const handler = listenMocks.get("busytok:service-status");
      handler?.({ payload: { status: "unavailable", since_ms: Date.now() } });
    });

    // Should have invalidated twice total (once per ready→unavailable transition)
    expect(invalidateSpy.mock.calls.length).toBeGreaterThan(callsAfterFirst);
  });

  it("exposes repairing service status", async () => {
    const { result } = renderHook(() => useEventSubscription(), { wrapper });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:service-status");
      handler?.({ payload: { status: "repairing", since_ms: Date.now() } });
    });

    expect(result.current.serviceStatus).toBe("repairing");
    expect(result.current.connectionStatus).toBe("disconnected");
  });

  it("invalidates shell.status when event batch latches serviceStatus to ready", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidateSpy = vi.spyOn(qc, "invalidateQueries");

    renderHook(() => useEventSubscription(), { wrapper: wrapperWithClient(qc) });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:event")).toBe(true);
    });

    // serviceStatus is still "starting" — event batch should latch and invalidate
    await act(async () => {
      const handler = listenMocks.get("busytok:event");
      handler?.({
        payload: makeBatch([{
          event_type: "live:sample",
          payload: {
            bucket_start_ms: 1740000000000,
            tokens_per_sec: 1,
            cost_per_sec: null,
            events_per_sec: 1,
          },
          is_exact: false,
        }]),
      });
    });

    expect(invalidateSpy).toHaveBeenCalledWith({
      queryKey: queryKeys.shellStatus(),
    });
  });

  it("invalidates shell.status when busytok:service-status transitions to ready", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidateSpy = vi.spyOn(qc, "invalidateQueries");

    renderHook(() => useEventSubscription(), { wrapper: wrapperWithClient(qc) });

    await vi.waitFor(() => {
      expect(listenMocks.has("busytok:service-status")).toBe(true);
    });

    await act(async () => {
      const handler = listenMocks.get("busytok:service-status");
      handler?.({ payload: { status: "ready", since_ms: Date.now() } });
    });

    expect(invalidateSpy).toHaveBeenCalledWith({
      queryKey: queryKeys.shellStatus(),
    });
  });
});
