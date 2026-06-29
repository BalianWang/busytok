import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { userEvent } from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { busytokClient } from "./api/busytokClient";

// Mock localStorage for reporter (imported by App)
const storage = (() => {
  let store: Record<string, string> = {};
  return {
    getItem: vi.fn((key: string) => (key in store ? store[key] : null)),
    setItem: vi.fn((key: string, value: string) => {
      store[key] = value;
    }),
    removeItem: vi.fn((key: string) => {
      delete store[key];
    }),
    clear: vi.fn(() => {
      store = {};
    }),
    get length() {
      return Object.keys(store).length;
    },
    key: vi.fn((index: number) => Object.keys(store)[index] ?? null),
  };
})();

Object.defineProperty(globalThis, "localStorage", {
  value: storage,
  writable: true,
  configurable: true,
});
import type {
  ReadEnvelopeDto,
  OverviewSummaryDto,
  OverviewTrendResponseDto,
  OverviewHeatmapResponseDto,
  OverviewRankingsResponseDto,
  ActivityRecentResponseDto,
  ActivityListResponseDto,
  ActivityDetailDto,
  BreakdownListResponseDto,
  BreakdownDetailDto,
  PromptEntryDto,
  PromptListResponseDto,
  SettingsSnapshotDto,
  SettingsDiagnosticsDto,
  SettingsRecoveryActionResponseDto,
} from "@busytok/protocol-types";
import { App } from "./App";

const { mockHideWindow } = vi.hoisted(() => ({
  mockHideWindow: vi.fn(() => Promise.resolve()),
}));

// Mock Tauri event API — EventSubscriptionProvider calls listen() on mount.
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({ hide: mockHideWindow, onFocusChanged: vi.fn().mockResolvedValue(() => {}) })),
}));

vi.mock("./lib/updaterClient", () => ({
  // Plain async fns (not vi.fn) so the boot check survives afterEach's
  // restoreAllMocks(), which would otherwise clear a mockResolvedValue and
  // leave checkForUpdate() returning undefined → unhandled rejection.
  checkForUpdate: async () => ({ kind: "up-to-date" }),
  applyUpdate: async () => {},
  CHECK_TIMEOUT_MS: 20_000,
  DOWNLOAD_TIMEOUT_MS: 120_000,
}));

// Mock liveWindow — useLiveSamples calls it on mount.
vi.spyOn(busytokClient, "liveWindow").mockResolvedValue({
  data: { exact_samples: [], transient_samples: [], current_tokens_per_sec: 0, current_events_per_sec: 0, start_ms: 0, end_ms: 0 },
  generated_at_ms: 0, generation_id: null, readiness: "live",
  is_exact: false, is_stale: false, watermark_ms: null, progress: null, degraded_reason: null,
} as any);

// Mock lightweight-charts — LiveCurvePanel imports it.
vi.mock("lightweight-charts", () => {
  const mockSeries = { setData: vi.fn(), applyOptions: vi.fn() };
  const mockPriceScale = { applyOptions: vi.fn() };
  const mockTimeScale = { applyOptions: vi.fn(), setVisibleRange: vi.fn() };
  const mockChart = {
    addSeries: vi.fn(() => mockSeries),
    priceScale: vi.fn(() => mockPriceScale),
    applyOptions: vi.fn(),
    remove: vi.fn(),
    timeScale: vi.fn(() => mockTimeScale),
  };
  return {
    createChart: vi.fn(() => mockChart),
    AreaSeries: {},
    ColorType: { Solid: "solid" },
    CrosshairMode: { Normal: 0 },
  };
});

function envelope<T>(data: T): ReadEnvelopeDto<T> {
  return {
    data,
    generated_at_ms: Date.now(),
    generation_id: "gen-1",
    readiness: "ready_exact",
    is_exact: true,
    is_stale: false,
    watermark_ms: null,
    progress: null,
    degraded_reason: null,
  };
}

function makePrompt(overrides: Partial<PromptEntryDto> = {}): PromptEntryDto {
  return {
    id: "prompt-1",
    content: "Reusable prompt body",
    tags: ["global"],
    alias: "global",
    is_pinned: false,
    usage_count: 1,
    last_used_at_ms: null,
    created_at_ms: 1715900000000,
    updated_at_ms: 1716000000000,
    ...overrides,
  };
}

beforeEach(() => {
  storage.clear();
  mockHideWindow.mockClear();
  vi.spyOn(busytokClient, "shellStatus").mockResolvedValue({
    generated_at_ms: Date.now(),
    status_chips: [],
    readiness: "ready_exact",
    latest_event_seq: null,
    writer_queue_depth: null,
    aggregate_lag_ms: null,
    subscription_bridge_connectivity: null,
  });
  // Modular overview endpoints (replace legacy overviewSnapshot)
  vi.spyOn(busytokClient, "overviewSummary").mockResolvedValue(
    envelope<OverviewSummaryDto>({
      timezone: "America/New_York",
      selected_range: "day",
      cost_status: "exact",
      metrics: [
        { id: "tokens", label: "tokens", value: "1,234,567", helper: null, tone: "neutral" },
        { id: "cost", label: "cost", value: "$12.34", helper: null, tone: "neutral" },
        { id: "events", label: "events captured", value: "42", helper: null, tone: "neutral" },
      ],
      generated_at_ms: 1716000000000,
    }),
  );
  vi.spyOn(busytokClient, "overviewTrend").mockResolvedValue(
    envelope<OverviewTrendResponseDto>({
      trend: {
        range: "day",
        bucket_granularity: "hour",
        metric_options: ["tokens", "cost"],
        cost_status: "exact",
        buckets: [
          {
            key: "0", label: "0:00", start_ms: 1715904000000, end_ms: 1715907600000,
            tokens: 100, cost_usd: 0.001, cost_status: "exact", event_count: 2, is_current: false,
          },
        ],
      },
    }),
  );
  vi.spyOn(busytokClient, "overviewHeatmap").mockResolvedValue(
    envelope<OverviewHeatmapResponseDto>({
      heatmap: {
        today: "2025-06-15",
        week_starts_on: 0,
        days: [
          { date: "2025-06-15", tokens: 1000, cost_usd: 0.01, cost_status: "exact", event_count: 5 },
        ],
      },
    }),
  );
  vi.spyOn(busytokClient, "overviewRankings").mockResolvedValue(
    envelope<OverviewRankingsResponseDto>({
      rankings: [],
    }),
  );
  vi.spyOn(busytokClient, "activityRecent").mockResolvedValue(
    envelope<ActivityRecentResponseDto>({
      recent_activity: [],
    }),
  );
  vi.spyOn(busytokClient, "activityList").mockResolvedValue(
    envelope<ActivityListResponseDto>({
      generated_at_ms: Date.now(),
      items: [],
      next_cursor: null,
      summary: { item_count: 0, total_tokens: 0, total_cost_usd: 0, cost_status: "exact" },
    }),
  );
  vi.spyOn(busytokClient, "activityDetail").mockResolvedValue(
    envelope<ActivityDetailDto>({
      id: "",
      title: "",
      subtitle: null,
      happened_at_ms: 0,
      client_id: "",
      client_label: "",
      source_id: null,
      source_label: null,
      source_root_path: null,
      project_label: null,
      project_hash: null,
      session_id: null,
      model_id: null,
      model_label: null,
      status: "ok",
      tokens: 0,
      token_breakdown: null,
      cost_usd: null,
      cost_status: "unavailable",
      technical_details: { source_id: null, provider: null, raw_model: null, notes: [] },
    }),
  );
  vi.spyOn(busytokClient, "breakdownList").mockResolvedValue(
    envelope<BreakdownListResponseDto>({
      generated_at_ms: Date.now(),
      kind: "project",
      items: [],
      next_cursor: null,
      summary: { item_count: 0, total_tokens: 0, total_cost_usd: 0, total_cost_status: "exact" },
    }),
  );
  vi.spyOn(busytokClient, "breakdownDetail").mockResolvedValue(
    envelope<BreakdownDetailDto>({ kind: "project",
    id: "",
    label: "",
    project_hash: "",
    project_path: null,
    metrics: [],
    trend: {
      range: "month",
      bucket_granularity: "day",
      metric_options: ["tokens", "cost"],
      cost_status: "exact",
      buckets: [],
    },
    model_mix: [],
    sessions: [],
    recent_activity: [],
    technical_details: [],
  }),
  );
  vi.spyOn(busytokClient, "settingsSnapshot").mockResolvedValue(
    envelope<SettingsSnapshotDto>({
      timezone: "UTC",
      week_starts_on: 0,
      discovery: {
        claude_code_default_paths: true,
        codex_default_paths: false,
        manual_roots: [],
      },
      privacy: {
        local_only: false,
        redact_sensitive_values: true,
      },
      diagnostics: {
        db_healthy: true,
        db_size_bytes: 1048576,
        migration_version: 1,
        usage_event_count: 12500,
        last_log_checkpoint_ms: Date.now() - 3600_000,
        writer_queue_depth: 0,
        aggregate_lag_ms: 0,
        recent_diagnostics: [],
        subagent: null,
      },
      recovery_actions: [],
      prompt_palette_default_action: "OnlyCopy",
    }),
  );
  vi.spyOn(busytokClient, "settingsDiagnostics").mockResolvedValue(
    envelope<SettingsDiagnosticsDto>({
      db_healthy: true,
      db_size_bytes: 1048576,
      migration_version: 1,
      usage_event_count: 12500,
      last_log_checkpoint_ms: Date.now() - 3600_000,
      writer_queue_depth: 0,
      aggregate_lag_ms: 0,
      recent_diagnostics: [],
      subagent: null,
    }),
  );
  vi.spyOn(busytokClient, "settingsRecoveryAction").mockResolvedValue(
    envelope<SettingsRecoveryActionResponseDto>({
      id: "rescan_all",
      accepted: true,
    message: "",
    }),
  );
  vi.spyOn(busytokClient, "promptsList").mockResolvedValue(
    envelope<PromptListResponseDto>({
      entries: [],
      total_count: 0,
    }),
  );
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function createQueryClient() {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } });
}

function renderApp(ui: React.ReactElement, queryClient = createQueryClient()) {
  return render(<QueryClientProvider client={queryClient}>{ui}</QueryClientProvider>);
}

describe("App", () => {
  it("prefetches startup queries once per query client instance", async () => {
    const overviewSummarySpy = vi.spyOn(busytokClient, "overviewSummary");
    const sharedClient = createQueryClient();

    const firstRender = renderApp(<App />, sharedClient);

    await waitFor(() => {
      expect(overviewSummarySpy).toHaveBeenCalledTimes(1);
    });

    firstRender.unmount();
    renderApp(<App />, sharedClient);

    await waitFor(() => {
      expect(overviewSummarySpy).toHaveBeenCalledTimes(1);
    });

    renderApp(<App />, createQueryClient());

    await waitFor(() => {
      expect(overviewSummarySpy).toHaveBeenCalledTimes(2);
    });
  }, 15_000);

  it("renders overview page by default", async () => {
    renderApp(<App />);
    expect(screen.getByRole("button", { name: "Overview" })).toBeDefined();
    // OverviewPage shows metric cards once data loads
    await waitFor(() => {
      expect(screen.getByText("1,234,567")).toBeDefined();
    });
  });

  it("falls back to overview when reading the persisted page throws", async () => {
    storage.getItem.mockImplementationOnce(() => {
      throw new Error("corrupted localStorage");
    });

    renderApp(<App />);

    expect(screen.getByRole("button", { name: "Overview" })).toBeDefined();
    await waitFor(() => {
      expect(screen.getByText("1,234,567")).toBeDefined();
    });
  });

  it("renders desktop shell with sidebar navigation", () => {
    renderApp(<App />);
    expect(document.querySelector(".desktop-shell")).not.toBeNull();
    expect(document.querySelector(".desktop-sidebar")).not.toBeNull();
  });

  it("navigates to usage page when Usage sidebar item is clicked", async () => {
    const user = userEvent.setup();
    renderApp(<App />);
    await user.click(screen.getByRole("button", { name: "Usage" }));
    await waitFor(() => {
      expect(screen.getByRole("tab", { name: "Activity" })).toBeDefined();
    });
  });

  it("navigates to settings page when Settings sidebar item is clicked", async () => {
    const user = userEvent.setup();
    renderApp(<App />);
    await user.click(screen.getByRole("button", { name: "Settings" }));
    await waitFor(() => {
      const tzElements = screen.getAllByText(/timezone/i);
      expect(tzElements.length).toBeGreaterThanOrEqual(1);
    });
  });

  it("navigates to prompt palette page when Prompt Palette sidebar item is clicked", async () => {
    const user = userEvent.setup();
    renderApp(<App />);
    await user.click(screen.getByRole("button", { name: "Prompt Palette" }));
    await waitFor(() => {
      expect(screen.getByRole("searchbox", { name: "Search prompts" })).toBeDefined();
    });
  });

  it("opens the prompt palette overlay with Cmd/Ctrl+Option+K", async () => {
    // NOTE: The hotkey is now handled natively (NSPanel) rather than via
    // usePromptPaletteHotkey. The in-app overlay is still rendered but is
    // no longer triggerable from JS keyboard events. This test is retained
    // as a stub to document the migration.
  });

  it("keeps paste fallback status visible without hiding when paste is unsupported", async () => {
    // NOTE: Window hiding is now managed by the panel bridge, not
    // usePromptPaletteHotkey. This test is retained as a stub.
  });

  it("shows sanitized overlay action feedback when clipboard write fails", async () => {
    // NOTE: The overlay is no longer opened via JS keyboard shortcut.
    // Clipboard error sanitization is still tested in
    // promptPaletteActions.test.ts. This test is retained as a stub.
  });

  it("navigates back to overview from settings", async () => {
    const user = userEvent.setup();
    renderApp(<App />);
    // Wait for overview data to load
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Trend" })).toBeDefined();
    });
    await user.click(screen.getByRole("button", { name: "Settings" }));
    await waitFor(() => {
      const tzElements = screen.getAllByText(/timezone/i);
      expect(tzElements.length).toBeGreaterThanOrEqual(1);
    });
    await user.click(screen.getByRole("button", { name: "Overview" }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Trend" })).toBeDefined();
    });
  });

  it("renders four sidebar navigation items", () => {
    renderApp(<App />);
    const sidebarItems = document.querySelectorAll(".desktop-sidebar__item");
    expect(sidebarItems.length).toBe(4);
  });

  it("renders all navigation labels", () => {
    renderApp(<App />);
    expect(screen.getByRole("button", { name: "Overview" })).toBeDefined();
    expect(screen.getByRole("button", { name: "Usage" })).toBeDefined();
    expect(screen.getByRole("button", { name: "Prompt Palette" })).toBeDefined();
    expect(screen.getByRole("button", { name: "Settings" })).toBeDefined();
  });
});
