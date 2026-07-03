//! TanStack Query hooks for the Busytok API — modular envelope edition.
//!
//! Modular read endpoints return `ReadEnvelopeDto<T>` so callers can
//! inspect `readiness`, `is_stale`, `generated_at_ms`, and other
//! diagnostics alongside the data.  Hooks that target legacy endpoints
//! (marked for removal in Task 15) are preserved with their original
//! signatures.

import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import type { QueryClient, QueryKey } from "@tanstack/react-query";
import { useBusytokClient } from "./BusytokClientContext";
import type { BusytokClient } from "./busytokClient";
import { queryKeys } from "./queryKeys";
import type {
  ReadEnvelopeDto,
  RangePresetDto,
  ActivityListRequestDto,
  ActivityDetailRequestDto,
  BreakdownListRequestDto,
  BreakdownDetailRequestDto,
  SettingsUpdateRequestDto,
  SettingsRecoveryActionRequestDto,
  OverviewSummaryDto,
  OverviewTrendResponseDto,
  OverviewHeatmapResponseDto,
  OverviewRankingsResponseDto,
  ActivityRecentResponseDto,
  ActivityListResponseDto,
  ActivityDetailDto,
  BreakdownListResponseDto,
  BreakdownDetailDto,
  PromptCreateRequestDto,
  PromptDeleteRequestDto,
  PromptDeleteResultDto,
  PromptEntryDto,
  PromptGetRequestDto,
  PromptListQueryDto,
  PromptListResponseDto,
  PromptUpdateRequestDto,
  PromptUseRequestDto,
  PromptUseResultDto,
  PromptSuggestTagsResponseDto,
  ProviderCreateRequestDto,
  ProviderUpdateRequestDto,
  ProfileCreateRequestDto,
  ProfileUpdateRequestDto,
  ReceiptDailyDto,
  SettingsSnapshotDto,
  SettingsDiagnosticsDto,
  SettingsRecoveryActionResponseDto,
  ShellStatusDto,
  SubagentRuntimeStatusDto,
} from "@busytok/protocol-types";

const SHELL_STALE_MS = 5_000;
// Poll cadence for `useShellStatus` while the service is not yet ready.
// Caps titlebar-chip staleness during the startup race; see
// docs/bugs/2026-06-24-startup-status-stale-on-fresh-install.md.
const SHELL_REFETCH_MS = 5_000;
const ENVELOPE_STALE_TIME_MS = 30_000;
export const DEFAULT_OVERVIEW_RANGE: RangePresetDto = "day";

/**
 * Envelope-aware `placeholderData` factory.
 *
 * Keeps the previous envelope visible during refetch so the UI never
 * flashes to a loading skeleton.  The `keepPreviousData`-style behaviour
 * is mapped onto `placeholderData` (the current TanStack Query convention).
 */
function envelopePlaceholder<T>(prev: ReadEnvelopeDto<T> | undefined): ReadEnvelopeDto<T> | undefined {
  return prev;
}

interface EnvelopeQueryOptionsInput<TData, TKey extends QueryKey = QueryKey> {
  queryKey: TKey;
  queryFn: () => Promise<ReadEnvelopeDto<TData>>;
}

export function envelopeQueryOptions<TData, TKey extends QueryKey = QueryKey>({
  queryKey,
  queryFn,
}: EnvelopeQueryOptionsInput<TData, TKey>) {
  return {
    queryKey,
    queryFn,
    staleTime: ENVELOPE_STALE_TIME_MS,
    placeholderData: (prev: ReadEnvelopeDto<TData> | undefined) => envelopePlaceholder(prev),
    // The service scan window (~8-12s at startup) can cause the first
    // fetch attempt to fail; use exponential backoff (0ms, 1s, 2s, 4s)
    // to cover the scan window. The global default retry: 3 is the floor;
    // retryDelay is set explicitly here so envelope queries always benefit
    // regardless of Tauri/TanStack-global config changes.
    retry: 4,
    retryDelay: (attemptIndex: number) => Math.min(1000 * 2 ** attemptIndex, 10000),
  };
}

export function prefetchStartupQueries(queryClient: Pick<QueryClient, "prefetchQuery">, _client?: BusytokClient) {
  // The service may not be ready at GUI startup (scan window). Prefetching
  // overview data here poisons the TanStack Query cache with an error state
  // that the Dashboard then inherits — and invalidateQueries on the ready
  // transition doesn't reliably reset it. Let the Dashboard components
  // fetch their own data when they mount (the retry config handles the
  // scan window).
  //
  // Settings, diagnostics, and other non-overview queries can be added
  // here if needed.
}

// ── Shell ────────────────────────────────────────────────────────────

export function useShellStatus() {
  const client = useBusytokClient();
  return useQuery<ShellStatusDto>({
    queryKey: queryKeys.shellStatus(),
    queryFn: () => client.shellStatus(),
    staleTime: SHELL_STALE_MS,
    // Startup-race safety net: the titlebar chip is driven solely by
    // shell.status readiness. The event-driven refresh can be delayed by the
    // bootstrap one-shot gap (lib.rs emits Unavailable then returns) or the
    // subscription-bridge backoff, and on a fresh install the runtime-event
    // latch never fires (lightweight register, no scan). Poll only while the
    // service is NOT yet in a healthy steady state — i.e. for the
    // starting/rebuilding transients and before the first successful fetch
    // (data still undefined) — so the chip self-heals within SHELL_REFETCH_MS.
    // Stop on BOTH healthy steady states (ready_exact AND ready_degraded): in
    // steady state runtime events keep shell.status fresh, so polling would be
    // perpetual load for no benefit. ready_degraded is a legitimate steady
    // state (service up, partially degraded), not a transient to poll out of.
    // refetchIntervalInBackground:false keeps polling tied to a visible window.
    // See docs/bugs/2026-06-24-startup-status-stale-on-fresh-install.md.
    refetchInterval: (query) => {
      const readiness = query.state.data?.readiness;
      return readiness === "ready_exact" || readiness === "ready_degraded"
        ? false
        : SHELL_REFETCH_MS;
    },
    refetchIntervalInBackground: false,
  });
}

// ── Overview — modular envelopes ─────────────────────────────────────

export function useOverviewSummary(range: RangePresetDto) {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<OverviewSummaryDto>>(
    envelopeQueryOptions({
      queryKey: queryKeys.overviewSummary(range),
      queryFn: () => client.overviewSummary({ range }),
    }),
  );
}

export function useOverviewTrend(range: RangePresetDto) {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<OverviewTrendResponseDto>>(
    envelopeQueryOptions({
      queryKey: queryKeys.overviewTrend(range),
      queryFn: () => client.overviewTrend({ range, granularity: null }),
    }),
  );
}

export function useOverviewHeatmap(range: RangePresetDto) {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<OverviewHeatmapResponseDto>>(
    envelopeQueryOptions({
      queryKey: queryKeys.overviewHeatmap(range),
      queryFn: () => client.overviewHeatmap({ range }),
    }),
  );
}

export function useOverviewRankings(range: RangePresetDto) {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<OverviewRankingsResponseDto>>(
    envelopeQueryOptions({
      queryKey: queryKeys.overviewRankings(range),
      queryFn: () => client.overviewRankings({ range }),
    }),
  );
}

// ── Receipt — daily share-image data ─────────────────────────────────

export function useDailyReceipt(date: string) {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<ReceiptDailyDto>>(
    envelopeQueryOptions({
      queryKey: queryKeys.receiptDaily(date),
      queryFn: () => client.receiptDaily({ date }),
    }),
  );
}

// ── Activity ─────────────────────────────────────────────────────────

export function useActivityRecent(range: RangePresetDto) {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<ActivityRecentResponseDto>>(
    envelopeQueryOptions({
      queryKey: queryKeys.activityRecent(range),
      queryFn: () => client.activityRecent({ range, limit: null }),
    }),
  );
}

export function useActivityList(request: ActivityListRequestDto) {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<ActivityListResponseDto>>(
    envelopeQueryOptions({
      queryKey: queryKeys.activityList(request),
      queryFn: () => client.activityList(request),
    }),
  );
}

export function useActivityDetail(request: ActivityDetailRequestDto) {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<ActivityDetailDto>>(
    envelopeQueryOptions({
      queryKey: queryKeys.activityDetail(request),
      queryFn: () => client.activityDetail(request),
    }),
  );
}

// ── Breakdown ────────────────────────────────────────────────────────

export function useBreakdownList(request: BreakdownListRequestDto) {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<BreakdownListResponseDto>>(
    envelopeQueryOptions({
      queryKey: queryKeys.breakdownList(request),
      queryFn: () => client.breakdownList(request),
    }),
  );
}

export function useBreakdownDetail(request: BreakdownDetailRequestDto) {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<BreakdownDetailDto>>(
    envelopeQueryOptions({
      queryKey: queryKeys.breakdownDetail(request),
      queryFn: () => client.breakdownDetail(request),
    }),
  );
}

// ── Settings ─────────────────────────────────────────────────────────

export function useSettingsSnapshot() {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<SettingsSnapshotDto>>(
    envelopeQueryOptions({
      queryKey: queryKeys.settingsSnapshot(),
      queryFn: () => client.settingsSnapshot(),
    }),
  );
}

export function useSettingsUpdate() {
  const queryClient = useQueryClient();
  const client = useBusytokClient();
  return useMutation<ReadEnvelopeDto<SettingsSnapshotDto>, Error, SettingsUpdateRequestDto>({
    mutationFn: (req) => client.settingsUpdate(req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.settingsSnapshot() });
    },
  });
}

export function useSettingsDiagnostics() {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<SettingsDiagnosticsDto>>(
    envelopeQueryOptions({
      queryKey: queryKeys.settingsDiagnostics(),
      queryFn: () => client.settingsDiagnostics(),
    }),
  );
}

export function useSettingsRecoveryAction() {
  const queryClient = useQueryClient();
  const client = useBusytokClient();
  return useMutation<
    ReadEnvelopeDto<SettingsRecoveryActionResponseDto>,
    Error,
    SettingsRecoveryActionRequestDto
  >({
    mutationFn: (req) => client.settingsRecoveryAction(req),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.settingsSnapshot(),
      });
      queryClient.invalidateQueries({
        queryKey: queryKeys.settingsDiagnostics(),
      });
    },
  });
}

// ── Prompts ─────────────────────────────────────────────────────────

export function usePromptsList(request: PromptListQueryDto) {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<PromptListResponseDto>>({
    queryKey: queryKeys.promptsList(request),
    queryFn: () => client.promptsList(request),
    staleTime: 10_000,
    placeholderData: (prev) => envelopePlaceholder(prev),
  });
}

export function usePromptDetail(request: PromptGetRequestDto) {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<PromptEntryDto>>({
    queryKey: queryKeys.promptDetail(request),
    queryFn: () => client.promptsGet(request),
    staleTime: 10_000,
    placeholderData: (prev) => envelopePlaceholder(prev),
  });
}

export function usePromptCreate() {
  const queryClient = useQueryClient();
  const client = useBusytokClient();
  return useMutation<ReadEnvelopeDto<PromptEntryDto>, Error, PromptCreateRequestDto>({
    mutationFn: (req) => client.promptsCreate(req),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.promptsRoot() }),
  });
}

export function usePromptUpdate() {
  const queryClient = useQueryClient();
  const client = useBusytokClient();
  return useMutation<ReadEnvelopeDto<PromptEntryDto>, Error, PromptUpdateRequestDto>({
    mutationFn: (req) => client.promptsUpdate(req),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.promptsRoot() }),
  });
}

export function usePromptDelete() {
  const queryClient = useQueryClient();
  const client = useBusytokClient();
  return useMutation<PromptDeleteResultDto, Error, PromptDeleteRequestDto>({
    mutationFn: (req) => client.promptsDelete(req),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.promptsRoot() }),
  });
}

export function usePromptUse() {
  const queryClient = useQueryClient();
  const client = useBusytokClient();
  return useMutation<PromptUseResultDto, Error, PromptUseRequestDto>({
    mutationFn: (req) => client.promptsUse(req),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.promptsRoot() }),
  });
}

export function useSuggestTags(query: string | null) {
  const client = useBusytokClient();
  return useQuery<PromptSuggestTagsResponseDto>({
    queryKey: queryKeys.promptSuggestTags({ query }),
    queryFn: () => client.promptsSuggestTags({ query, limit: null }),
    staleTime: 5_000,
    enabled: query !== null,
  });
}

// ── Providers ───────────────────────────────────────────────────────

export function useProviders() {
  const client = useBusytokClient();
  return useQuery({
    queryKey: queryKeys.providers(),
    queryFn: () => client.providerList(),
    staleTime: 30_000,
  });
}

export function useProviderMutations() {
  const client = useBusytokClient();
  const queryClient = useQueryClient();
  const invalidate = () => queryClient.invalidateQueries({ queryKey: queryKeys.providers() });

  const createProvider = useMutation({
    mutationFn: (req: ProviderCreateRequestDto) => client.providerCreate(req),
    onSuccess: invalidate,
  });
  const updateProvider = useMutation({
    mutationFn: (req: ProviderUpdateRequestDto) => client.providerUpdate(req),
    onSuccess: invalidate,
  });
  const deleteProvider = useMutation({
    mutationFn: (id: string) => client.providerDelete(id),
    onSuccess: invalidate,
  });
  const testConnection = useMutation({
    mutationFn: (id: string) => client.providerTestConnection(id),
  });

  return { createProvider, updateProvider, deleteProvider, testConnection };
}

// ── Profiles (Phase 4) ───────────────────────────────────────────────

/**
 * Profile mutations. All three invalidate `settingsSnapshot` on success
 * because profiles are READ via settings.snapshot (not a dedicated
 * profile.list RPC). This keeps the read+write paths consistent.
 */
export function useProfileMutations() {
  const client = useBusytokClient();
  const queryClient = useQueryClient();
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: queryKeys.settingsSnapshot() });

  const createProfile = useMutation({
    mutationFn: (req: ProfileCreateRequestDto) => client.profileCreate(req),
    onSuccess: invalidate,
  });
  const updateProfile = useMutation({
    mutationFn: (req: ProfileUpdateRequestDto) => client.profileUpdate(req),
    onSuccess: invalidate,
  });
  const deleteProfile = useMutation({
    mutationFn: (id: string) => client.profileDelete(id),
    onSuccess: invalidate,
  });

  return { createProfile, updateProfile, deleteProfile };
}

// ── Subagent runtime status ─────────────────────────────────────────

// Poll cadence for the read-only Subagents monitoring page (spec §4 Phase 2:
// 5s poll). `refetchIntervalInBackground: false` ties polling to a visible
// window (matches `useShellStatus` pattern).
const SUBAGENT_REFETCH_MS = 5_000;

export function useSubagentRuntimeStatus() {
  const client = useBusytokClient();
  return useQuery<ReadEnvelopeDto<SubagentRuntimeStatusDto>>({
    ...envelopeQueryOptions({
      queryKey: queryKeys.subagentRuntimeStatus(),
      queryFn: () => client.subagentRuntimeStatus(),
    }),
    refetchInterval: SUBAGENT_REFETCH_MS,
    refetchIntervalInBackground: false,
  });
}
