import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  getDesktopLifecycleSettings,
  updateDesktopLifecycleSettings,
  getBackgroundServiceDiagnostics,
  repairBackgroundService,
} from "./backgroundServiceCommands";
import type {
  DesktopLifecycleSettings,
  DesktopBackgroundServiceDiagnostics,
} from "./backgroundServiceCommands";

const mockInvoke = vi.hoisted(() =>
  vi.fn((_cmd: string, _args?: Record<string, unknown>) =>
    Promise.resolve() as Promise<unknown>,
  ),
);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) =>
    mockInvoke(cmd, args),
}));

describe("backgroundServiceCommands", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(undefined);
  });

  it("getDesktopLifecycleSettings returns the settings snapshot", async () => {
    const expected: DesktopLifecycleSettings = {
      launch_busytok_desktop_at_login: true,
    };
    mockInvoke.mockResolvedValueOnce(expected);

    const result = await getDesktopLifecycleSettings();

    expect(mockInvoke).toHaveBeenCalledWith(
      "desktop_lifecycle_settings_snapshot", undefined,
    );
    expect(result).toEqual(expected);
  });

  it("updateDesktopLifecycleSettings sends the settings and invokes the command", async () => {
    const settings: DesktopLifecycleSettings = {
      launch_busytok_desktop_at_login: false,
    };

    await updateDesktopLifecycleSettings(settings);

    expect(mockInvoke).toHaveBeenCalledWith(
      "desktop_lifecycle_settings_update",
      { settings },
    );
  });

  it("getBackgroundServiceDiagnostics returns the diagnostics payload", async () => {
    const expected: DesktopBackgroundServiceDiagnostics = {
      state: "running",
      actionable: false,
      gui_build_identity: "0.1.0",
      service_build_identity: "0.1.0",
      version_skew: false,
    };
    mockInvoke.mockResolvedValueOnce(expected);

    const result = await getBackgroundServiceDiagnostics();

    expect(mockInvoke).toHaveBeenCalledWith(
      "desktop_background_service_diagnostics", undefined,
    );
    expect(result).toEqual(expected);
  });

  it("getBackgroundServiceDiagnostics returns stopped state as non-actionable", async () => {
    const expected: DesktopBackgroundServiceDiagnostics = {
      state: "stopped_for_this_session",
      actionable: false,
      gui_build_identity: "0.1.0",
      service_build_identity: null,
      version_skew: false,
    };
    mockInvoke.mockResolvedValueOnce(expected);

    const result = await getBackgroundServiceDiagnostics();

    expect(result.state).toBe("stopped_for_this_session");
    expect(result.actionable).toBe(false);
  });

  it("repairBackgroundService invokes the repair command", async () => {
    await repairBackgroundService();

    expect(mockInvoke).toHaveBeenCalledWith(
      "desktop_background_service_repair", undefined,
    );
  });

  it("repairBackgroundService propagates errors", async () => {
    mockInvoke.mockRejectedValueOnce(new Error("repair failed"));

    await expect(repairBackgroundService()).rejects.toThrow("repair failed");
  });
});
