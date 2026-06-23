import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";

// ── Module-level mocks ────────────────────────────────────────────────
vi.mock("../api/useBusytokData", () => ({
  useSettingsSnapshot: () => ({
    data: {
      data: {
        timezone: "UTC",
        week_starts_on: 0,
        discovery: null,
        privacy: null,
        prompt_palette_default_action: "paste",
        diagnostics: null,
      },
    },
    isLoading: false,
    isError: false,
    isFetching: false,
    refetch: vi.fn(),
  }),
  useSettingsDiagnostics: () => ({ data: null }),
  useSettingsUpdate: () => ({ mutate: vi.fn(), isPending: false }),
  prefetchStartupQueries: vi.fn(),
}));

vi.mock("../hooks/usePreferences", () => ({
  usePreferences: () => ({
    preferences: { themePreference: "system" },
    updatePreference: vi.fn(),
  }),
}));

vi.mock("../components/desktop/useRefreshToolbar", () => ({
  useRefreshToolbar: vi.fn(),
}));

vi.mock("../hooks/useUpdater", () => ({
  useUpdater: vi.fn(),
}));

vi.mock("../lib/desktopHostCommands", () => ({
  desktopHostShortcutDiagnostics: vi.fn().mockResolvedValue({
    registered: false,
    shortcut: null,
    failure_reason: "unsupported_platform",
  }),
  desktopHostRetryShortcutRegistration: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../lib/backgroundServiceCommands", () => ({
  getDesktopLifecycleSettings: vi.fn().mockResolvedValue({ launch_busytok_desktop_at_login: false }),
  updateDesktopLifecycleSettings: vi.fn().mockResolvedValue(undefined),
  getBackgroundServiceDiagnostics: vi.fn().mockResolvedValue({ state: "running", pid: 1234 }),
  repairBackgroundService: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../lib/promptPaletteShortcutState", () => ({
  getPromptPaletteShortcutStatus: vi.fn(() => ({ state: "idle" })),
  subscribePromptPaletteShortcutStatus: vi.fn().mockReturnValue(() => {}),
}));

vi.mock("../lib/promptPaletteActions", () => ({
  getPromptPaletteAccessibilityStatus: vi.fn().mockResolvedValue({
    ok: false,
    failure_reason: "unsupported_platform",
  }),
  openPromptPaletteAccessibilitySettings: vi.fn(),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onFocusChanged: vi.fn().mockResolvedValue(() => {}),
  }),
}));

// Pull the mocked hook in AFTER the mock is registered.
import { useUpdater } from "../hooks/useUpdater";
import type { UpdaterContextValue } from "../hooks/useUpdater";
import { SettingsPage } from "./SettingsPage";

function mockUpdater(value: UpdaterContextValue) {
  vi.mocked(useUpdater).mockReturnValue(value);
}

describe("SettingsPage Updates section", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    cleanup();
  });

  it("shows Update now when an update is available and calls applyNow on click", () => {
    const applyNow = vi.fn();
    mockUpdater({
      status: { state: "available", version: "0.3.0", notes: "n", date: "d" },
      checkNow: vi.fn(),
      applyNow,
    });
    render(<SettingsPage />);
    const button = screen.getByRole("button", { name: /update now/i });
    expect(button).toBeTruthy();
    fireEvent.click(button);
    expect(applyNow).toHaveBeenCalledTimes(1);
    // description carries the version
    expect(screen.getByText(/0\.3\.0 is available/i)).toBeTruthy();
  });

  it("shows Up to date when up-to-date", () => {
    mockUpdater({
      status: { state: "up-to-date" },
      checkNow: vi.fn(),
      applyNow: vi.fn(),
    });
    render(<SettingsPage />);
    expect(screen.getByText(/up to date/i)).toBeTruthy();
  });

  it("shows Downloading update… with percent when downloading", () => {
    mockUpdater({
      status: { state: "downloading", percent: 42 },
      checkNow: vi.fn(),
      applyNow: vi.fn(),
    });
    render(<SettingsPage />);
    expect(screen.getByText(/downloading update.*42%/i)).toBeTruthy();
  });

  it("shows Downloading update… without percent when percent is null", () => {
    mockUpdater({
      status: { state: "downloading", percent: null },
      checkNow: vi.fn(),
      applyNow: vi.fn(),
    });
    render(<SettingsPage />);
    expect(screen.getByText(/downloading update/i)).toBeTruthy();
    expect(screen.queryByText(/%/)).toBeNull();
  });

  it("shows restarting message when installed-pending-restart", () => {
    mockUpdater({
      status: { state: "installed-pending-restart" },
      checkNow: vi.fn(),
      applyNow: vi.fn(),
    });
    render(<SettingsPage />);
    expect(screen.getByText(/update installed.*restarting/i)).toBeTruthy();
  });

  it("shows manual-restart message when installed-needs-manual-restart", () => {
    mockUpdater({
      status: { state: "installed-needs-manual-restart", version: "0.3.0" },
      checkNow: vi.fn(),
      applyNow: vi.fn(),
    });
    render(<SettingsPage />);
    expect(screen.getByText(/updated to v0\.3\.0.*restart busytok manually/i)).toBeTruthy();
  });

  it("shows error message + Retry button which calls checkNow", () => {
    const checkNow = vi.fn();
    mockUpdater({
      status: { state: "error", message: "network down" },
      checkNow,
      applyNow: vi.fn(),
    });
    render(<SettingsPage />);
    expect(screen.getByText(/update check failed: network down/i)).toBeTruthy();
    const retry = screen.getByRole("button", { name: /retry/i });
    expect(retry).toBeTruthy();
    fireEvent.click(retry);
    expect(checkNow).toHaveBeenCalledTimes(1);
  });

  it("shows default description + Check for updates when idle", () => {
    mockUpdater({
      status: { state: "idle" },
      checkNow: vi.fn(),
      applyNow: vi.fn(),
    });
    render(<SettingsPage />);
    expect(
      screen.getByText(/check for and install the latest version of busytok/i),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: /check for updates/i }),
    ).toBeTruthy();
  });

  it("hides the Windows note on macOS and shows it elsewhere", () => {
    vi.mocked(useUpdater).mockReturnValue({ status: { state: "up-to-date" }, checkNow: vi.fn(), applyNow: vi.fn() } as never);
    const platformGetter = vi.spyOn(navigator, "platform", "get");
    try {
      platformGetter.mockReturnValue("MacIntel");
      const { unmount } = render(<SettingsPage />);
      expect(screen.queryByText(/windows does not support/i)).toBeNull();
      unmount();
      platformGetter.mockReturnValue("Win32");
      render(<SettingsPage />);
      expect(screen.getByText(/windows does not support/i)).toBeTruthy();
    } finally {
      platformGetter.mockRestore();
    }
  });
});
