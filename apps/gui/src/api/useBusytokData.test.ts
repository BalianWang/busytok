import { describe, expect, it, vi, beforeEach } from "vitest";
import type { RangePresetDto, ReadinessStateDto } from "@busytok/protocol-types";

const { useQuerySpy, useMutationSpy, prefetchQuerySpy, useQueryClientSpy, invalidateQueriesSpy, mockClient } = vi.hoisted(() => {
  const useQuerySpy = vi.fn();
  const useMutationSpy = vi.fn();
  const prefetchQuerySpy = vi.fn();
  const invalidateQueriesSpy = vi.fn();
  // Return a shared `invalidateQueries` so tests can assert against it
  // regardless of how many times `useQueryClient()` is called.
  const useQueryClientSpy = vi.fn(() => ({
    invalidateQueries: invalidateQueriesSpy,
    prefetchQuery: prefetchQuerySpy,
  }));
  const mockClient = {
    shellStatus: vi.fn(),
    overviewSummary: vi.fn(),
    overviewTrend: vi.fn(),
    overviewHeatmap: vi.fn(),
    overviewRankings: vi.fn(),
    activityRecent: vi.fn(),
    activityList: vi.fn(),
    activityDetail: vi.fn(),
    breakdownList: vi.fn(),
    breakdownDetail: vi.fn(),
    settingsSnapshot: vi.fn(),
    settingsUpdate: vi.fn(),
    settingsDiagnostics: vi.fn(),
    settingsRecoveryAction: vi.fn(),
    promptsList: vi.fn(),
    promptsGet: vi.fn(),
    promptsCreate: vi.fn(),
    promptsUpdate: vi.fn(),
    promptsDelete: vi.fn(),
    promptsUse: vi.fn(),
    liveWindow: vi.fn(),
    modelList: vi.fn(),
    modelCreate: vi.fn(),
    modelUpdate: vi.fn(),
    modelDelete: vi.fn(),
    modelTagsUpdate: vi.fn(),
  };
  return { useQuerySpy, useMutationSpy, prefetchQuerySpy, useQueryClientSpy, invalidateQueriesSpy, mockClient };
});

vi.mock("@tanstack/react-query", () => ({
  useQuery: (options: unknown) => useQuerySpy(options),
  useMutation: (options: unknown) => useMutationSpy(options),
  useQueryClient: () => useQueryClientSpy(),
}));

vi.mock("./BusytokClientContext", () => ({
  useBusytokClient: () => mockClient,
}));

vi.mock("./busytokClient", () => ({
  busytokClient: mockClient,
  createBusytokClient: () => mockClient,
}));

import {
  envelopeQueryOptions,
  prefetchStartupQueries,
  useActivityList,
  useActivityRecent,
  useBreakdownDetail,
  useBreakdownList,
  useOverviewSummary,
  useOverviewTrend,
  useOverviewHeatmap,
  useOverviewRankings,
  useShellStatus,
  useModels,
  useModelMutations,
} from "./useBusytokData";
import { queryKeys } from "./queryKeys";

describe("useBusytokData", () => {
  beforeEach(() => {
    useQuerySpy.mockReset();
    prefetchQuerySpy.mockReset();
    useMutationSpy.mockReset();
    invalidateQueriesSpy.mockReset();
    // Clear call history on mockClient methods so tests that invoke
    // queryFn/mutationFn start from a clean slate.
    vi.clearAllMocks();
  });

  it("exports shared envelope query options that retain stale data and skip polling", () => {
    const queryFn = vi.fn();
    const options = envelopeQueryOptions({
      queryKey: ["overview", "summary", "day"],
      queryFn,
    });

    const previousEnvelope = {
      data: { total: 1 },
      generated_at_ms: 1,
      generation_id: "gen-1",
      readiness: "ready_exact" as const,
      is_exact: true,
      is_stale: false,
      watermark_ms: null,
      progress: null,
      degraded_reason: null,
    };
    expect(options.queryKey).toEqual(["overview", "summary", "day"]);
    expect(options.queryFn).toBe(queryFn);
    expect(options.staleTime).toBe(30_000);
    expect("refetchInterval" in options).toBe(false);
    expect(options.placeholderData?.(previousEnvelope)).toBe(previousEnvelope);
  });

  it("does not prefetch overview startup queries (scan-window guard)", () => {
    // prefetchStartupQueries is intentionally a no-op: prefetching overview
    // data at GUI startup can poison the TanStack Query cache with an error
    // state during the scan window, which invalidateQueries does not reliably
    // reset. Dashboard components fetch their own data on mount instead.
    const queryClient = useQueryClientSpy();

    prefetchStartupQueries(queryClient);

    expect(prefetchQuerySpy).not.toHaveBeenCalled();
  });

  // ── Shell ────────────────────────────────────────────────────────

  // Safety net for the startup race documented in
  // docs/bugs/2026-06-24-startup-status-stale-on-fresh-install.md: the
  // titlebar chip is driven SOLELY by shell.status readiness. The
  // event-driven refresh can be delayed by the bootstrap one-shot gap
  // (lib.rs emits Unavailable then returns) or the subscription-bridge
  // backoff, and on a fresh install the runtime-event latch never fires
  // (lightweight register, no scan). Polling while not ready lets the chip
  // recover within the interval once the service is actually up; once ready,
  // steady state is owned by event-driven invalidation.

  it("configures shell status with a readiness-gated refetch interval", () => {
    useShellStatus();
    const options = useQuerySpy.mock.calls[0][0];
    expect(options.queryKey).toEqual(["shell", "status"]);
    expect(options.staleTime).toBe(5_000);
    expect(options.placeholderData).toBeUndefined();
    expect(typeof options.refetchInterval).toBe("function");
    expect(options.refetchIntervalInBackground).toBe(false);
  });

  it("shell status polls only for startup transients and stops on any healthy readiness", () => {
    useShellStatus();
    const options = useQuerySpy.mock.calls[0][0];
    const refetch = options.refetchInterval as (q: {
      state: { data?: { readiness?: ReadinessStateDto } | null };
    }) => unknown;

    // Startup/rebuild transients, and before the first successful fetch (data
    // undefined), keep polling — the chip must self-heal during the startup race.
    expect(refetch({ state: { data: undefined } })).toBe(5_000);
    expect(refetch({ state: { data: { readiness: "starting" } } })).toBe(5_000);
    expect(refetch({ state: { data: { readiness: "rebuilding" } } })).toBe(5_000);

    // Both healthy steady states stop polling — runtime events keep shell.status
    // fresh in steady state (zero perpetual RPC load). ready_degraded is a
    // legitimate steady state (service up, partially degraded), not a transient.
    expect(refetch({ state: { data: { readiness: "ready_exact" } } })).toBe(false);
    expect(refetch({ state: { data: { readiness: "ready_degraded" } } })).toBe(false);
  });

  // ── Overview — modular envelopes ─────────────────────────────────

  it("configures overview summary with stale time and no polling", () => {
    useOverviewSummary("day" as RangePresetDto);
    const options = useQuerySpy.mock.calls[0][0];
    expect(options.queryKey).toEqual(["overview", "summary", "day"]);
    expect(options.staleTime).toBe(30_000);
    expect("refetchInterval" in options).toBe(false);
    expect(options.placeholderData).toBeTypeOf("function");
  });

  it("configures overview trend with correct query key and no polling", () => {
    useOverviewTrend("week" as RangePresetDto);
    const options = useQuerySpy.mock.calls[0][0];
    expect(options.queryKey).toEqual(["overview", "trend", "week"]);
    expect("refetchInterval" in options).toBe(false);
  });

  it("configures overview heatmap with correct query key and no polling", () => {
    useOverviewHeatmap("month" as RangePresetDto);
    const options = useQuerySpy.mock.calls[0][0];
    expect(options.queryKey).toEqual(["overview", "heatmap", "month"]);
    expect("refetchInterval" in options).toBe(false);
  });

  it("configures overview rankings with correct query key and no polling", () => {
    useOverviewRankings("year" as RangePresetDto);
    const options = useQuerySpy.mock.calls[0][0];
    expect(options.queryKey).toEqual(["overview", "rankings", "year"]);
    expect("refetchInterval" in options).toBe(false);
  });

  // ── Activity ─────────────────────────────────────────────────────

  it("configures activity recent with correct query key and no polling", () => {
    useActivityRecent("day" as RangePresetDto);
    const options = useQuerySpy.mock.calls[0][0];
    expect(options.queryKey).toEqual(["activity", "recent", "day"]);
    expect("refetchInterval" in options).toBe(false);
  });

  it("configures activity list with correct query key and no polling", () => {
    useActivityList({
      range: "day",
      cursor: null,
      limit: 100,
      client_id: null,
      source_id: null,
      project_hash: null,
      model_id: null,
    });
    const options = useQuerySpy.mock.calls[0][0];
    expect(options.queryKey[0]).toBe("activity");
    expect(options.queryKey[1]).toBe("list");
    expect("refetchInterval" in options).toBe(false);
  });

  // ── Breakdown ─────────────────────────────────────────────────────

  it("configures breakdown queries without polling", () => {
    useBreakdownList({
      kind: "project",
      range: "day",
      cursor: null,
      limit: 100,
    });
    const options = useQuerySpy.mock.calls[0][0];
    expect(options.queryKey[0]).toBe("breakdown");
    expect("refetchInterval" in options).toBe(false);
  });

  // ── Detail drawers ──────────────────────────────────────────────

  it("configures breakdown detail queries without polling", () => {
    useBreakdownDetail({
      kind: "project",
      id: "project-1",
      range: "day",
    });
    const options = useQuerySpy.mock.calls[0][0];
    expect(options.queryKey[0]).toBe("breakdown");
    expect(options.queryKey[1]).toBe("detail");
    expect("refetchInterval" in options).toBe(false);
  });

  // ── Models (SQL catalog, Task 9 Step 2) ──────────────────────────

  it("useModels builds the request from the filter and gates on `enabled`", () => {
    useModels({ providerId: "deepseek", tags: ["chat"], includeDisabled: true, enabled: true });
    const options = useQuerySpy.mock.calls[0][0];
    expect(options.queryKey).toEqual(
      queryKeys.modelsList({
        provider_id: "deepseek",
        tags: ["chat"],
        include_disabled: true,
        sort: null,
        reasoning: null,
      }),
    );
    expect(options.enabled).toBe(true);
    expect(options.staleTime).toBe(30_000);
    // queryFn invokes client.modelList with the assembled request.
    expect(mockClient.modelList).not.toHaveBeenCalled();
    options.queryFn();
    expect(mockClient.modelList).toHaveBeenCalledWith({
      provider_id: "deepseek",
      tags: ["chat"],
      include_disabled: true,
      sort: null,
      reasoning: null,
    });
  });

  it("useModels defaults: no providerId → null, empty tags → [], includeDisabled → false, enabled → true", () => {
    useModels();
    const options = useQuerySpy.mock.calls[0][0];
    expect(options.enabled).toBe(true);
    options.queryFn();
    expect(mockClient.modelList).toHaveBeenCalledWith({
      provider_id: null,
      tags: [],
      include_disabled: false,
      sort: null,
      reasoning: null,
    });
  });

  it("useModels respects enabled=false (no fetch)", () => {
    useModels({ providerId: "deepseek", enabled: false });
    const options = useQuerySpy.mock.calls[0][0];
    expect(options.enabled).toBe(false);
  });

  it("useModelMutations wires four mutations that each call the right client method and invalidate the catalog", () => {
    useModelMutations();

    expect(useMutationSpy).toHaveBeenCalledTimes(4);
    const calls = useMutationSpy.mock.calls.map((c) => c[0]);

    // createModel
    calls[0].mutationFn({ provider_id: "p", model_id: "m", enabled: true, tags: [] });
    expect(mockClient.modelCreate).toHaveBeenCalledWith({ provider_id: "p", model_id: "m", enabled: true, tags: [] });
    calls[0].onSuccess();
    expect(invalidateQueriesSpy).toHaveBeenCalledWith({ queryKey: queryKeys.models() });

    // updateModel
    calls[1].mutationFn({ id: "m-1", enabled: false });
    expect(mockClient.modelUpdate).toHaveBeenCalledWith({ id: "m-1", enabled: false });
    calls[1].onSuccess();

    // deleteModel
    calls[2].mutationFn("m-1");
    expect(mockClient.modelDelete).toHaveBeenCalledWith("m-1");
    calls[2].onSuccess();

    // tagsUpdate
    calls[3].mutationFn({ modelId: "model-db-1", tags: ["chat"] });
    expect(mockClient.modelTagsUpdate).toHaveBeenCalledWith("model-db-1", ["chat"]);
    calls[3].onSuccess();

    // All four onSuccess callbacks invalidate the models catalog.
    expect(invalidateQueriesSpy).toHaveBeenCalledTimes(4);
  });
});
