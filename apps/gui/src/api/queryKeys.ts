//! Canonical query keys matching invalidation scopes broadcast by the
//! backend event plane.  Modules that invalidate these scopes use the
//! same key factories so that `queryClient.invalidateQueries` hits the
//! right caches.

import type {
  RangePresetDto,
  ActivityListRequestDto,
  ActivityDetailRequestDto,
  BreakdownListRequestDto,
  BreakdownDetailRequestDto,
  ModelListRequestDto,
  PromptGetRequestDto,
  PromptListQueryDto,
} from "@busytok/protocol-types";

export const queryKeys = {
  // ── Shell ─────────────────────────────────────────────────────────
  shellStatus: () => ["shell", "status"] as const,

  // ── Overview — modular envelopes ──────────────────────────────────
  overviewSummary: (range: RangePresetDto) =>
    ["overview", "summary", range] as const,
  overviewTrend: (range: RangePresetDto) =>
    ["overview", "trend", range] as const,
  overviewHeatmap: (range: RangePresetDto) =>
    ["overview", "heatmap", range] as const,
  overviewRankings: (range: RangePresetDto) =>
    ["overview", "rankings", range] as const,

  // ── Activity ──────────────────────────────────────────────────────
  activityRecent: (range: RangePresetDto) =>
    ["activity", "recent", range] as const,
  activityList: (request: ActivityListRequestDto) =>
    ["activity", "list", request] as const,
  activityDetail: (request: ActivityDetailRequestDto) =>
    ["activity", "detail", request] as const,

  // ── Breakdown ─────────────────────────────────────────────────────
  breakdownList: (request: BreakdownListRequestDto) =>
    ["breakdown", "list", request] as const,
  breakdownDetail: (request: BreakdownDetailRequestDto) =>
    ["breakdown", "detail", request] as const,

  // ── Settings ──────────────────────────────────────────────────────
  settingsSnapshot: () => ["settings", "snapshot"] as const,
  settingsDiagnostics: () => ["settings", "diagnostics"] as const,

  // ── Providers ─────────────────────────────────────────────────────
  providers: () => ["providers"] as const,

  // ── Models (SQL catalog) ──────────────────────────────────────────
  // `models()` is the invalidation root; `modelsList(request)` is the
  // per-filter read key. Both must be used together so mutations
  // invalidate every cached list regardless of filter.
  models: () => ["models"] as const,
  modelsList: (request: ModelListRequestDto) =>
    ["models", "list", request] as const,

  // ── Subagent runtime status (envelope) ────────────────────────────
  subagentRuntimeStatus: () => ["subagents", "runtime_status"] as const,

  // ── Prompts ───────────────────────────────────────────────────────
  promptsList: (request: PromptListQueryDto) =>
    ["prompts", "list", request] as const,
  promptDetail: (request: PromptGetRequestDto) =>
    ["prompts", "detail", request] as const,
  promptsRoot: () => ["prompts"] as const,
  promptSuggestTags: (request: { query: string | null }) =>
    ["prompts", "suggest_tags", request] as const,

  // ── Live ──────────────────────────────────────────────────────────
  liveWindow: (windowSeconds: number | null) =>
    ["live", "window", windowSeconds] as const,

  // ── Receipt ───────────────────────────────────────────────────────
  receiptDaily: (date: string) => ["receipt", "daily", date] as const,
};
