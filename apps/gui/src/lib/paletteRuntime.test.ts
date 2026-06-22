import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { createPanelBridgeRuntime } from "./paletteRuntime";

describe("createPanelBridgeRuntime", () => {
  let originalBridge: unknown;

  beforeEach(() => {
    originalBridge = window.busytokPanelBridge;
  });

  afterEach(() => {
    // Restore or delete the bridge after each test.
    (window as any).busytokPanelBridge = originalBridge;
    if (originalBridge === undefined) {
      delete (window as any).busytokPanelBridge;
    }
  });

  it("invoke resolves when bridge returns ok", async () => {
    window.busytokPanelBridge = {
      invoke: vi.fn().mockResolvedValue({ ok: true, data: { status: "ready" } }),
      subscribe: vi.fn(),
    };
    const runtime = createPanelBridgeRuntime();
    const result = await runtime.invoke("shell.status");
    expect(result).toEqual({ status: "ready" });
  });

  it("invoke rejects when bridge returns error", async () => {
    window.busytokPanelBridge = {
      invoke: vi.fn().mockResolvedValue({ ok: false, error: "method not found" }),
      subscribe: vi.fn(),
    };
    const runtime = createPanelBridgeRuntime();
    await expect(runtime.invoke("unknown.method")).rejects.toThrow("method not found");
  });

  it("invoke rejects when bridge is missing", async () => {
    delete (window as any).busytokPanelBridge;
    const runtime = createPanelBridgeRuntime();
    await expect(runtime.invoke("shell.status")).rejects.toThrow("Panel bridge not available");
  });

  it("invoke rejects with default error message when bridge returns error without message", async () => {
    window.busytokPanelBridge = {
      invoke: vi.fn().mockResolvedValue({ ok: false }),
      subscribe: vi.fn(),
    };
    const runtime = createPanelBridgeRuntime();
    await expect(runtime.invoke("shell.status")).rejects.toThrow("Unknown error");
  });

  it("subscribe returns unsubscribe function", () => {
    const unsub = vi.fn();
    window.busytokPanelBridge = {
      invoke: vi.fn(),
      subscribe: vi.fn().mockReturnValue(unsub),
    };
    const runtime = createPanelBridgeRuntime();
    const result = runtime.subscribe("service:status", vi.fn());
    expect(result).toBe(unsub);
  });

  it("subscribe returns no-op when bridge is missing", () => {
    delete (window as any).busytokPanelBridge;
    const runtime = createPanelBridgeRuntime();
    const unsub = runtime.subscribe("service:status", vi.fn());
    expect(unsub).toBeInstanceOf(Function);
    // Calling it should not throw
    expect(() => unsub()).not.toThrow();
  });

  it("requestClose sends palette:close", () => {
    const invokeMock = vi.fn().mockResolvedValue({ ok: true });
    window.busytokPanelBridge = {
      invoke: invokeMock,
      subscribe: vi.fn(),
    };
    const runtime = createPanelBridgeRuntime();
    runtime.requestClose();
    expect(invokeMock).toHaveBeenCalledWith("palette:close");
  });

  it("requestClose is a no-op when bridge is missing", () => {
    delete (window as any).busytokPanelBridge;
    const runtime = createPanelBridgeRuntime();
    expect(() => runtime.requestClose()).not.toThrow();
  });
});
