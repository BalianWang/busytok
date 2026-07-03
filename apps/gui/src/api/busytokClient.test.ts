import { describe, expect, it, vi } from "vitest";
import { BusytokControlError, createBusytokClient } from "./busytokClient";

describe("createBusytokClient", () => {
  it("requests shell status", async () => {
    const invoke = vi.fn().mockResolvedValue({ generated_at_ms: 0, status_chips: [] });
    const client = createBusytokClient({ invoke });
    await client.shellStatus();
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "shell.status",
      params: {},
    }));
  });

  it("requests overview summary (modular envelope)", async () => {
    const envelope = {
      data: { timezone: "UTC", selected_range: "day", cost_status: "unavailable", metrics: [], generated_at_ms: 1 },
      generated_at_ms: 1, generation_id: null, readiness: "starting", is_exact: false,
      is_stale: true, watermark_ms: null, progress: null, degraded_reason: null,
    };
    const invoke = vi.fn().mockResolvedValue(envelope);
    const client = createBusytokClient({ invoke });
    await client.overviewSummary({ range: "day" });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "overview.summary",
      params: { range: "day" },
    }));
  });

  it("requests overview trend (modular envelope)", async () => {
    const invoke = vi.fn().mockResolvedValue({});
    const client = createBusytokClient({ invoke });
    await client.overviewTrend({ range: "week", granularity: null });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "overview.trend",
      params: { range: "week", granularity: null },
    }));
  });

  it("requests overview heatmap (modular envelope)", async () => {
    const invoke = vi.fn().mockResolvedValue({});
    const client = createBusytokClient({ invoke });
    await client.overviewHeatmap({ range: "month" });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "overview.heatmap",
      params: { range: "month" },
    }));
  });

  it("requests overview rankings (modular envelope)", async () => {
    const invoke = vi.fn().mockResolvedValue({});
    const client = createBusytokClient({ invoke });
    await client.overviewRankings({ range: "year" });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "overview.rankings",
      params: { range: "year" },
    }));
  });

  it("requests activity recent (modular envelope)", async () => {
    const invoke = vi.fn().mockResolvedValue({});
    const client = createBusytokClient({ invoke });
    await client.activityRecent({ range: "day", limit: null });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "activity.recent",
      params: { range: "day", limit: null },
    }));
  });

  it("requests live window (modular envelope)", async () => {
    const invoke = vi.fn().mockResolvedValue({});
    const client = createBusytokClient({ invoke });
    await client.liveWindow({ window_seconds: null });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "live.window",
      params: { window_seconds: null },
    }));
  });

  it("requests activity list", async () => {
    const invoke = vi.fn().mockResolvedValue({ items: [], next_cursor: null, summary: {} });
    const client = createBusytokClient({ invoke });
    await client.activityList({
      range: "day", cursor: null, limit: null,
      client_id: null, source_id: null, project_hash: null, model_id: null,
    });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "activity.list",
      params: { range: "day", cursor: null, limit: null, client_id: null, source_id: null, project_hash: null, model_id: null },
    }));
  });

  it("requests activity detail", async () => {
    const invoke = vi.fn().mockResolvedValue({ id: "evt-1" });
    const client = createBusytokClient({ invoke });
    await client.activityDetail({ id: "evt-1" });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "activity.detail",
      params: { id: "evt-1" },
    }));
  });

  it("requests breakdown list", async () => {
    const invoke = vi.fn().mockResolvedValue({ items: [], next_cursor: null, kind: "project", summary: {} });
    const client = createBusytokClient({ invoke });
    await client.breakdownList({ kind: "project", range: "week", cursor: null, limit: null });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "breakdown.list",
      params: { kind: "project", range: "week", cursor: null, limit: null },
    }));
  });

  it("requests breakdown detail", async () => {
    const invoke = vi.fn().mockResolvedValue({ kind: "project", id: "proj-1" });
    const client = createBusytokClient({ invoke });
    await client.breakdownDetail({ kind: "project", id: "proj-1", range: "week" });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "breakdown.detail",
      params: { kind: "project", id: "proj-1", range: "week" },
    }));
  });

  it("requests settings snapshot", async () => {
    const invoke = vi.fn().mockResolvedValue({ timezone: "UTC" });
    const client = createBusytokClient({ invoke });
    await client.settingsSnapshot();
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "settings.snapshot",
      params: {},
    }));
  });

  it("requests settings update", async () => {
    const invoke = vi.fn().mockResolvedValue({ timezone: "America/New_York" });
    const client = createBusytokClient({ invoke });
    await client.settingsUpdate({ timezone: "America/New_York", week_starts_on: null, discovery: null, privacy: null, prompt_palette_default_action: null });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "settings.update",
      params: { timezone: "America/New_York", week_starts_on: null, discovery: null, privacy: null, prompt_palette_default_action: null },
    }));
  });

  it("requests settings diagnostics", async () => {
    const invoke = vi.fn().mockResolvedValue({ db_healthy: true });
    const client = createBusytokClient({ invoke });
    await client.settingsDiagnostics();
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "settings.diagnostics",
      params: {},
    }));
  });

  it("requests settings recovery action", async () => {
    const invoke = vi.fn().mockResolvedValue({ id: "rescan_all", accepted: true });
    const client = createBusytokClient({ invoke });
    await client.settingsRecoveryAction({ id: "rescan_all" });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "settings.recovery_action",
      params: { id: "rescan_all" },
    }));
  });

  it("requests prompt list", async () => {
    const invoke = vi.fn().mockResolvedValue({ data: { entries: [], total_count: 0 } });
    const client = createBusytokClient({ invoke });
    await client.promptsList({ query: "review", tag: null, sort: "smart", limit: 50 });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "prompts.list",
      params: { query: "review", tag: null, sort: "smart", limit: 50 },
    }));
  });

  it("records prompt use", async () => {
    const invoke = vi.fn().mockResolvedValue({ usage_count: 1, last_used_at_ms: 123 });
    const client = createBusytokClient({ invoke });
    await client.promptsUse({
      id: "prompt-1",
      action: "CopyAndPaste",
      surface: "overlay",
      outcome: "paste_attempted",
      failure_reason: null,
    });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "prompts.use",
      params: {
        id: "prompt-1",
        action: "CopyAndPaste",
        surface: "overlay",
        outcome: "paste_attempted",
        failure_reason: null,
      },
    }));
  });

  it("requests prompt tag suggestions", async () => {
    const invoke = vi.fn().mockResolvedValue({ tags: ["review", "release"] });
    const client = createBusytokClient({ invoke });
    await client.promptsSuggestTags({ query: "re", limit: null });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "prompts.suggest_tags",
      params: { query: "re", limit: null },
    }));
  });

  it("settingsUpdate throws BusytokControlError with code and payload on structured Tauri error", async () => {
    const invoke = vi.fn().mockRejectedValue(
      new Error('[settings_validation_failed] timezone is invalid | payload: {"errors":[{"code":"invalid","field_path":"timezone","message":"Unknown timezone"}]}')
    );
    const client = createBusytokClient({ invoke });
    await expect(client.settingsUpdate({ timezone: "Nowhere/Nowhere", week_starts_on: null, discovery: null, privacy: null, prompt_palette_default_action: null })).rejects.toThrow(BusytokControlError);
    try {
      await client.settingsUpdate({ timezone: "Nowhere/Nowhere", week_starts_on: null, discovery: null, privacy: null, prompt_palette_default_action: null });
    } catch (e: unknown) {
      expect(e).toBeInstanceOf(BusytokControlError);
      const err = e as BusytokControlError;
      expect(err.code).toBe("settings_validation_failed");
      expect(err.payload).toEqual({
        errors: [{ code: "invalid", field_path: "timezone", message: "Unknown timezone" }],
      });
    }
  });

  it("settingsUpdate throws BusytokControlError without payload on simple error", async () => {
    const invoke = vi.fn().mockRejectedValue(new Error("[internal_error] something went wrong"));
    const client = createBusytokClient({ invoke });
    try {
      await client.settingsUpdate({ timezone: "UTC", week_starts_on: null, discovery: null, privacy: null, prompt_palette_default_action: null });
    } catch (e: unknown) {
      expect(e).toBeInstanceOf(BusytokControlError);
      const err = e as BusytokControlError;
      expect(err.code).toBe("internal_error");
      expect(err.payload).toBeNull();
    }
  });

  it("settingsRecoveryAction throws BusytokControlError on structured Tauri error", async () => {
    const invoke = vi.fn().mockRejectedValue(
      new Error('[action_failed] recovery could not be started | payload: {"reason":"already_running"}')
    );
    const client = createBusytokClient({ invoke });
    try {
      await client.settingsRecoveryAction({ id: "rescan_all" });
    } catch (e: unknown) {
      expect(e).toBeInstanceOf(BusytokControlError);
      const err = e as BusytokControlError;
      expect(err.code).toBe("action_failed");
      expect(err.payload).toEqual({ reason: "already_running" });
    }
  });

  // ── Model catalog RPCs (Task 9 Step 1) ───────────────────────────

  it("requests model list", async () => {
    const invoke = vi.fn().mockResolvedValue({ models: [] });
    const client = createBusytokClient({ invoke });
    await client.modelList({ provider_id: "deepseek", tags: ["chat"], include_disabled: false });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "model.list",
      params: { provider_id: "deepseek", tags: ["chat"], include_disabled: false },
    }));
  });

  it("requests model create", async () => {
    const invoke = vi.fn().mockResolvedValue({ model_db_id: "m-1" });
    const client = createBusytokClient({ invoke });
    await client.modelCreate({ provider_id: "deepseek", model_id: "deepseek-chat", enabled: true, tags: ["chat"] });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "model.create",
      params: { provider_id: "deepseek", model_id: "deepseek-chat", enabled: true, tags: ["chat"] },
    }));
  });

  it("requests model update", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    const client = createBusytokClient({ invoke });
    await client.modelUpdate({ id: "m-1", enabled: false });
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "model.update",
      params: { id: "m-1", enabled: false },
    }));
  });

  it("requests model delete", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    const client = createBusytokClient({ invoke });
    await client.modelDelete("m-1");
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "model.delete",
      params: { id: "m-1" },
    }));
  });

  it("requests model tags update", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    const client = createBusytokClient({ invoke });
    await client.modelTagsUpdate("deepseek-chat", ["chat", "reasoning"]);
    expect(invoke).toHaveBeenCalledWith("invoke_busytok", expect.objectContaining({
      method: "model.tags.update",
      params: { model_id: "deepseek-chat", tags: ["chat", "reasoning"] },
    }));
  });

});
