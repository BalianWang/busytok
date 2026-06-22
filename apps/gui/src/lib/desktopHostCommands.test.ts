import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  desktopHostShowGui,
  desktopHostShortcutDiagnostics,
  desktopHostRetryShortcutRegistration,
} from "./desktopHostCommands";

const mockInvoke = vi.hoisted(() => vi.fn((_cmd: string, _args?: Record<string, unknown>) => Promise.resolve() as Promise<unknown>));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => mockInvoke(cmd, args),
}));

describe("desktopHostCommands", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(undefined);
  });

  it("desktopHostShowGui invokes the correct Tauri command", async () => {
    await desktopHostShowGui();
    expect(mockInvoke).toHaveBeenCalledWith("desktop_host_show_gui", undefined);
  });

  it("desktopHostShortcutDiagnostics invokes the correct Tauri command", async () => {
    const expected = { state: "registered", shortcut: "CommandOrControl+Shift+K", failure_reason: null, retry_count: 0 };
    mockInvoke.mockResolvedValueOnce(expected);
    const result = await desktopHostShortcutDiagnostics();
    expect(mockInvoke).toHaveBeenCalledWith("desktop_host_shortcut_diagnostics", undefined);
    expect(result).toEqual(expected);
  });

  it("desktopHostRetryShortcutRegistration invokes the correct Tauri command", async () => {
    await desktopHostRetryShortcutRegistration();
    expect(mockInvoke).toHaveBeenCalledWith("desktop_host_retry_shortcut_registration", undefined);
  });
});
