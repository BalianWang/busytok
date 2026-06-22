import type { Page } from "@playwright/test";

export async function installGuiMocks(page: Page) {
  page.on("pageerror", (error) => {
    console.error("[e2e pageerror]", error.stack ?? error.message);
  });
  page.on("console", (message) => {
    if (message.type() === "error" && message.text().startsWith("[e2e ")) {
      console.error(message.text());
    }
  });

  await page.addInitScript(() => {
    function readEnvelope<T>(data: T) {
      return {
        data,
        generated_at_ms: Date.now(),
        generation_id: "e2e-gen-1",
        readiness: "ready_exact",
        is_exact: true,
        is_stale: false,
        watermark_ms: null,
        progress: null,
        degraded_reason: null,
      };
    }

    function activityItem(index: number) {
      return {
        id: `activity-${index}`,
        happened_at_ms: Date.now() - index * 60_000,
        client_id: "claude_code",
        client_label: `Claude Code ${index}`,
        source_id: "source-1",
        source_label: "/projects/autoken",
        source_root_path: "/projects/autoken",
        project_label: "Autoken",
        project_hash: "autoken",
        model_id: "claude-sonnet-4",
        model_label: "claude-sonnet-4",
        tokens: 1_000 + index,
        cost_usd: 0.01,
        cost_status: "exact",
        status: "ok",
        detail_available: true,
      };
    }

    const promptEntry = {
      id: "prompt-1",
      alias: ";;review",
      content: "Review this diff for bugs.",
      tags: ["Review"],
      is_pinned: true,
      usage_count: 3,
      last_used_at_ms: null,
      created_at_ms: Date.now(),
      updated_at_ms: Date.now(),
    };

    (window as any).__TAURI_INTERNALS__ = {
      callbacks: new Map<number, (...args: unknown[]) => unknown>(),
      transformCallback(callback: (...args: unknown[]) => unknown, _once?: boolean) {
        const id = Math.floor(Math.random() * 1_000_000);
        (this as any).callbacks.set(id, callback);
        return id;
      },
      unregisterCallback(id: number) {
        (this as any).callbacks.delete(id);
      },
      invoke: async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "log_frontend_event") {
          const entry = args?.entry as { event_code?: string; message?: string; details?: unknown } | undefined;
          if (entry?.event_code === "gui.render_error") {
            console.error("[e2e render_error]", entry.message, JSON.stringify(entry.details));
          }
          return null;
        }
        if (cmd === "invoke_busytok") {
          const method = args?.method as string;
          const params = (args?.params ?? {}) as Record<string, unknown>;

          switch (method) {
            case "shell.status":
              return {
                generated_at_ms: Date.now(),
                status_chips: [],
                readiness: "ready_exact",
                latest_event_seq: null,
                writer_queue_depth: null,
                aggregate_lag_ms: null,
                subscription_bridge_connectivity: "connected",
              };
            case "service.health":
              return { ok: true };
            case "service.status":
              return { state: "running", db_path: "/tmp/busytok/db", sources: 1 };
            case "overview.summary":
              return readEnvelope({
                timezone: "UTC",
                selected_range: "day",
                cost_status: "exact",
                metrics: [
                  { id: "tokens", label: "Total Tokens", value: "1,200", helper: "1.2k tokens today", tone: "success" },
                  { id: "cost", label: "Total Cost", value: "$12.50", helper: "$12.50 today", tone: "success" },
                  { id: "clients", label: "Active Clients", value: "1", helper: "Claude Code", tone: "neutral" },
                  { id: "events", label: "Events Captured", value: "42", helper: "42 events today", tone: "neutral" },
                ],
                generated_at_ms: Date.now(),
              });
            case "overview.trend":
              return readEnvelope({
                trend: {
                  range: "day",
                  bucket_granularity: "hour",
                  metric_options: ["tokens", "cost"],
                  cost_status: "exact",
                  buckets: Array.from({ length: 12 }, (_, i) => ({
                    key: `h${i}`,
                    label: `${i}:00`,
                    start_ms: Date.now() - (12 - i) * 3_600_000,
                    end_ms: Date.now() - (11 - i) * 3_600_000,
                    tokens: 100 + i * 20,
                    cost_usd: 0.01 + i * 0.001,
                    cost_status: "exact",
                    event_count: i + 1,
                    is_current: i === 11,
                  })),
                },
              });
            case "overview.heatmap":
              return readEnvelope({
                heatmap: {
                  today: "2026-05-26",
                  week_starts_on: 0,
                  days: Array.from({ length: 14 }, (_, i) => ({
                    date: `2026-05-${String(13 + i).padStart(2, "0")}`,
                    tokens: 500 + i * 75,
                    cost_usd: 0.01 + i * 0.002,
                    cost_status: "exact",
                    event_count: i + 1,
                  })),
                },
              });
            case "overview.rankings":
              return readEnvelope({
                rankings: [
                  {
                    id: "projects",
                    title: "Top Projects",
                    items: [
                      { id: "autoken", label: "Autoken", value: "80k", helper: "80k tokens", bar_value: 100, action: null },
                    ],
                  },
                ],
              });
            case "activity.recent":
              return readEnvelope({
                recent_activity: [activityItem(1), activityItem(2)],
              });
            case "activity.list": {
              const cursor = params.cursor as string | null | undefined;
              const start = cursor === "page-2" ? 101 : 1;
              return readEnvelope({
                generated_at_ms: Date.now(),
                items: Array.from({ length: 100 }, (_, i) => activityItem(start + i)),
                next_cursor: cursor === "page-2" ? "page-3" : "page-2",
                summary: {
                  item_count: 210,
                  total_tokens: 210_000,
                  total_cost_usd: 2.1,
                  cost_status: "exact",
                },
              });
            }
            case "activity.detail":
              return readEnvelope({
                ...activityItem(1),
                title: "Claude Code usage",
                subtitle: null,
                session_id: null,
                token_breakdown: null,
                technical_details: {
                  source_id: "source-1",
                  provider: "anthropic",
                  raw_model: "claude-sonnet-4",
                  notes: [],
                },
              });
            case "live.window":
              return readEnvelope({
                exact_samples: Array.from({ length: 12 }, (_, i) => ({
                  bucket_start_ms: Date.now() - (12 - i) * 60_000,
                  tokens_per_sec: 4 + i,
                  cost_per_sec: 0.0001 + i * 0.00001,
                  events_per_sec: 1,
                })),
                transient_samples: [],
                current_tokens_per_sec: 15,
                current_events_per_sec: 1,
                start_ms: Date.now() - 15 * 60_000,
                end_ms: Date.now(),
              });
            case "usage.dashboard":
              return (window as any).__BUSYTOK_E2E_DASHBOARD__;
            case "usage.timeline":
              return { entries: [] };
            case "usage.events": {
              const limit = (params.limit as number) ?? 100;
              return Array.from({ length: Math.min(limit, 5) }, (_, i) => ({
                id: `evt_${i + 1}`,
                agent: "claude_code",
                model: "claude-sonnet-4-20250514",
                total_tokens: 100 + i,
                cost_usd: 0.01,
                timestamp_ms: Date.now() - i * 60000,
              }));
            }
            case "usage.projects":
              return [];
            case "usage.models":
              return [];
            case "usage.sessions":
              return [];
            case "sources.list":
              return [];
            case "sources.status":
              return { id: params.id, agent: "claude_code", status: "active" };
            case "settings.get":
              return { timezone: "UTC", discovery: { claude_code_default_paths: true, codex_default_paths: true } };
            case "diagnostics.scan_status":
              return { state: "idle", last_scan_at_ms: null };
            case "diagnostics.store_health":
              return { ok: true, tables: 8 };
            case "prompts.list":
              const promptQuery = typeof params.query === "string" ? params.query.toLowerCase() : "";
              const promptTag = typeof params.tag === "string" ? params.tag.toLowerCase() : "";
              const promptMatchesQuery =
                promptQuery.length === 0 ||
                promptEntry.alias.toLowerCase().includes(promptQuery) ||
                promptEntry.content.toLowerCase().includes(promptQuery) ||
                promptEntry.tags.some((tag) => tag.toLowerCase().includes(promptQuery));
              const promptMatchesTag =
                promptTag.length === 0 ||
                promptEntry.tags.some((tag) => tag.toLowerCase() === promptTag);
              const promptEntries = promptMatchesQuery && promptMatchesTag ? [promptEntry] : [];
              return readEnvelope({
                entries: promptEntries,
                total_count: promptEntries.length,
              });
            case "prompts.use":
              return { usage_count: 4, last_used_at_ms: Date.now() };
            case "prompts.delete":
              return { deleted: true };
            default:
              return {};
          }
        }
        if (cmd === "prompt_palette_accessibility_status") {
          return { ok: true, failure_reason: null };
        }
        if (cmd === "prompt_palette_paste_active_app") {
          return { ok: true, failure_reason: null };
        }
        if (cmd === "prompt_palette_open_accessibility_settings") return null;
        if (cmd === "plugin:event|listen") return Math.floor(Math.random() * 1000);
        if (cmd === "plugin:event|unlisten") return null;
        return {};
      },
    };
    (window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: async (_event: string, _eventId: number) => undefined,
    };
    (window as any).__BUSYTOK_E2E_DASHBOARD__ = {
      today_total_tokens: 1200,
      today_total_cost_usd: 12.5,
      active_agents: ["claude_code"],
      top_projects: [],
      top_models: [{ model: "claude-sonnet-4-20250514", total_tokens: 1200, total_cost_usd: 12.5 }],
      recent_events: [],
    };
  });
}
