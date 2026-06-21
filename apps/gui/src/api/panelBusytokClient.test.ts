import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";

const { invokeMock, subscribeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  subscribeMock: vi.fn().mockReturnValue(() => {}),
}));

vi.mock("../lib/paletteRuntime", () => ({
  createPanelBridgeRuntime: () => ({
    invoke: invokeMock,
    subscribe: subscribeMock,
    requestClose: vi.fn(),
  }),
}));

import { panelBusytokClient } from "./panelBusytokClient";

describe("panelBusytokClient", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue({ data: { entries: [], total_count: 0 } });
  });

  it("invokes prompts.list through bridge as invoke_busytok", async () => {
    invokeMock.mockResolvedValue({ data: { entries: [], total_count: 0 } });
    await panelBusytokClient.promptsList({ query: "test", tag: null, sort: "smart", limit: 50 });
    // createBusytokClient wraps all calls in invoke("invoke_busytok", { method, params, meta })
    // The panelBusytokClient adapter extracts method & params and calls runtime.invoke(method, params)
    expect(invokeMock).toHaveBeenCalledWith(
      "prompts.list",
      expect.objectContaining({ query: "test", tag: null, sort: "smart", limit: 50 }),
    );
  });

  it("handles service errors from bridge", async () => {
    invokeMock.mockRejectedValue(new Error("bridge disconnected"));
    await expect(panelBusytokClient.shellStatus()).rejects.toThrow("bridge disconnected");
  });

  it("routes invoke_busytok commands through bridge", async () => {
    invokeMock.mockResolvedValue({ generated_at_ms: 0, status_chips: [] });
    await panelBusytokClient.shellStatus();
    // shell.status method called via runtime.invoke
    expect(invokeMock).toHaveBeenCalledWith(
      "shell.status",
      expect.anything(),
    );
  });

  it("passes params from invoke_busytok to bridge invoke", async () => {
    invokeMock.mockResolvedValue({ data: {} });
    await panelBusytokClient.overviewSummary({ range: "day" });
    expect(invokeMock).toHaveBeenCalledWith(
      "overview.summary",
      expect.objectContaining({ range: "day" }),
    );
  });
});
