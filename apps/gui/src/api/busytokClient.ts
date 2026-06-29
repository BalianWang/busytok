//! Busytok API client — a Tauri invoke wrapper that calls `invoke_busytok`
//! through the Tauri core API.

import { getSessionId } from "../logging/reporter";

import type {
  LiveWindowDto,
  LiveWindowRequestDto,
  ActivityDetailDto,
  ActivityDetailRequestDto,
  ActivityListRequestDto,
  ActivityListResponseDto,
  ActivityRecentRequestDto,
  ActivityRecentResponseDto,
  BreakdownDetailDto,
  BreakdownDetailRequestDto,
  BreakdownListRequestDto,
  BreakdownListResponseDto,
  OverviewSummaryDto,
  OverviewSummaryRequestDto,
  OverviewTrendRequestDto,
  OverviewTrendResponseDto,
  OverviewHeatmapRequestDto,
  OverviewHeatmapResponseDto,
  OverviewRankingsRequestDto,
  OverviewRankingsResponseDto,
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
  PromptSuggestTagsRequestDto,
  PromptSuggestTagsResponseDto,
  ReceiptDailyDto,
  ReceiptDailyRequestDto,
  ReadEnvelopeDto,
  SettingsDiagnosticsDto,
  SettingsRecoveryActionRequestDto,
  SettingsRecoveryActionResponseDto,
  SettingsSnapshotDto,
  SettingsUpdateRequestDto,
  ShellStatusDto,
} from "@busytok/protocol-types";

/** Structured error from the control protocol, surviving the Tauri boundary. */
export class BusytokControlError extends Error {
  code: string;
  payload: unknown | null;

  constructor(code: string, message: string, payload: unknown | null) {
    super(message);
    this.name = 'BusytokControlError';
    this.code = code;
    this.payload = payload;
  }
}

/** Parse Tauri's stringified error back into a structured error if possible. */
function extractControlError(err: unknown): never {
  const msg = (err as any)?.message ?? String(err);
  // Tauri format: "[code] message" or "[code] message | payload: {...}"
  const match = msg.match(/^\[([^\]]+)\]\s+(.*?)(?:\s*\|\s*payload:\s*(.+))?$/s);
  if (match) {
    let payload: unknown = null;
    if (match[3]) {
      try { payload = JSON.parse(match[3]); } catch {}
    }
    throw new BusytokControlError(match[1], match[2], payload);
  }
  throw err instanceof Error ? err : new Error(msg);
}

/** Function signature matching Tauri's `invoke` — injectable for testing. */
export type InvokeFn = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

/** Instance type returned by `createBusytokClient`. */
export type BusytokClient = ReturnType<typeof createBusytokClient>;

/** Creates a Busytok client bound to a given `invoke` implementation. */
export function createBusytokClient(deps: { invoke: InvokeFn }) {
  const { invoke } = deps;

  async function call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    const correlationId = crypto.randomUUID();
    const result = await invoke("invoke_busytok", {
      method,
      params,
      meta: {
        session_id: getSessionId(),
        correlation_id: correlationId,
      },
    }) as T;
    return result;
  }

  return {
    // Shell
    shellStatus: () => call<ShellStatusDto>("shell.status"),

    // Overview — modular envelope methods
    overviewSummary: (request: OverviewSummaryRequestDto) =>
      call<ReadEnvelopeDto<OverviewSummaryDto>>("overview.summary", { ...request }),
    receiptDaily: (request: ReceiptDailyRequestDto) =>
      call<ReadEnvelopeDto<ReceiptDailyDto>>("receipt.daily", { ...request }),
    overviewTrend: (request: OverviewTrendRequestDto) =>
      call<ReadEnvelopeDto<OverviewTrendResponseDto>>("overview.trend", { ...request }),
    overviewHeatmap: (request: OverviewHeatmapRequestDto) =>
      call<ReadEnvelopeDto<OverviewHeatmapResponseDto>>("overview.heatmap", { ...request }),
    overviewRankings: (request: OverviewRankingsRequestDto) =>
      call<ReadEnvelopeDto<OverviewRankingsResponseDto>>("overview.rankings", { ...request }),

    // Activity — modular envelope method
    activityRecent: (request: ActivityRecentRequestDto) =>
      call<ReadEnvelopeDto<ActivityRecentResponseDto>>("activity.recent", { ...request }),

    // Activity — modular envelopes
    activityList: (request: ActivityListRequestDto) =>
      call<ReadEnvelopeDto<ActivityListResponseDto>>("activity.list", { ...request }),
    activityDetail: (request: ActivityDetailRequestDto) =>
      call<ReadEnvelopeDto<ActivityDetailDto>>("activity.detail", { ...request }),

    // Breakdown — modular envelopes
    breakdownList: (request: BreakdownListRequestDto) =>
      call<ReadEnvelopeDto<BreakdownListResponseDto>>("breakdown.list", { ...request }),
    breakdownDetail: (request: BreakdownDetailRequestDto) =>
      call<ReadEnvelopeDto<BreakdownDetailDto>>("breakdown.detail", { ...request }),

    // Settings — modular envelopes
    settingsSnapshot: () => call<ReadEnvelopeDto<SettingsSnapshotDto>>("settings.snapshot"),
    settingsUpdate: async (request: SettingsUpdateRequestDto) => {
      try {
        return await call<ReadEnvelopeDto<SettingsSnapshotDto>>("settings.update", { ...request });
      } catch (e) {
        extractControlError(e);
      }
    },
    settingsDiagnostics: () => call<ReadEnvelopeDto<SettingsDiagnosticsDto>>("settings.diagnostics"),
    settingsRecoveryAction: async (request: SettingsRecoveryActionRequestDto) => {
      try {
        return await call<ReadEnvelopeDto<SettingsRecoveryActionResponseDto>>("settings.recovery_action", { ...request });
      } catch (e) {
        extractControlError(e);
      }
    },

    // Prompts — modular envelopes
    promptsList: (request: PromptListQueryDto) =>
      call<ReadEnvelopeDto<PromptListResponseDto>>("prompts.list", { ...request }),
    promptsGet: (request: PromptGetRequestDto) =>
      call<ReadEnvelopeDto<PromptEntryDto>>("prompts.get", { ...request }),
    promptsCreate: (request: PromptCreateRequestDto) =>
      call<ReadEnvelopeDto<PromptEntryDto>>("prompts.create", { ...request }),
    promptsUpdate: (request: PromptUpdateRequestDto) =>
      call<ReadEnvelopeDto<PromptEntryDto>>("prompts.update", { ...request }),
    promptsDelete: (request: PromptDeleteRequestDto) =>
      call<PromptDeleteResultDto>("prompts.delete", { ...request }),
    promptsUse: (request: PromptUseRequestDto) =>
      call<PromptUseResultDto>("prompts.use", { ...request }),
    promptsSuggestTags: (request: PromptSuggestTagsRequestDto) =>
      call<PromptSuggestTagsResponseDto>("prompts.suggest_tags", { ...request }),

    // Live
    liveWindow: (request: LiveWindowRequestDto) =>
      call<ReadEnvelopeDto<LiveWindowDto>>("live.window", { ...request }),
  };
}

/** Default Busytok client using the real Tauri `invoke` from `@tauri-apps/api`. */
export const busytokClient = createBusytokClient({
  invoke: async (cmd: string, args?: Record<string, unknown>) => {
    // Dynamic import so the module works in non-Tauri test environments too.
    const { invoke: tauriInvoke } = await import("@tauri-apps/api/core");
    return tauriInvoke(cmd, args);
  },
});
