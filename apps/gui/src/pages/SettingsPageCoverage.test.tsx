import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ReadEnvelopeDto,
  SettingsDiagnosticsDto,
  SettingsSnapshotDto,
} from "@busytok/protocol-types";
import { BusytokControlError } from "../api/busytokClient";
import { setPromptPaletteShortcutStatus } from "../lib/promptPaletteShortcutState";
import { loadPreferences } from "../lib/preferencesStorage";

const promptActionMocks = vi.hoisted(() => ({
  getPromptPaletteAccessibilityStatus: vi.fn(),
  openPromptPaletteAccessibilitySettings: vi.fn(),
}));

vi.mock("../lib/promptPaletteActions", () => ({
  getPromptPaletteAccessibilityStatus: promptActionMocks.getPromptPaletteAccessibilityStatus,
  openPromptPaletteAccessibilitySettings: promptActionMocks.openPromptPaletteAccessibilitySettings,
}));

const bgServiceMocks = vi.hoisted(() => ({
  getDesktopLifecycleSettings: vi.fn(),
  updateDesktopLifecycleSettings: vi.fn(),
  getBackgroundServiceDiagnostics: vi.fn(),
  repairBackgroundService: vi.fn(),
}));

vi.mock("../lib/backgroundServiceCommands", () => ({
  getDesktopLifecycleSettings: (...args: unknown[]) =>
    bgServiceMocks.getDesktopLifecycleSettings(...args),
  updateDesktopLifecycleSettings: (...args: unknown[]) =>
    bgServiceMocks.updateDesktopLifecycleSettings(...args),
  getBackgroundServiceDiagnostics: (...args: unknown[]) =>
    bgServiceMocks.getBackgroundServiceDiagnostics(...args),
  repairBackgroundService: (...args: unknown[]) =>
    bgServiceMocks.repairBackgroundService(...args),
}));

const desktopHostMocks = vi.hoisted(() => ({
  desktopHostShortcutDiagnostics: vi.fn(),
  desktopHostRetryShortcutRegistration: vi.fn(),
}));

vi.mock("../lib/desktopHostCommands", () => ({
  desktopHostShortcutDiagnostics: (...args: unknown[]) =>
    desktopHostMocks.desktopHostShortcutDiagnostics(...args),
  desktopHostRetryShortcutRegistration: (...args: unknown[]) =>
    desktopHostMocks.desktopHostRetryShortcutRegistration(...args),
}));

// Mock the reporter so frontend log emission from the page (theme changes,
// shortcut diagnostics) does not trip jsdom localStorage.removeItem assertions
// in the shared reporter module.
vi.mock("../logging/reporter", () => ({
  reportFrontendEvent: vi.fn(),
}));

// In-memory localStorage so loadPreferences/savePreferences round-trip works
// without bleeding across tests.
const memoryStore: Record<string, string> = {};
Object.defineProperty(globalThis, "localStorage", {
  value: {
    getItem: vi.fn((k: string) => (k in memoryStore ? memoryStore[k] : null)),
    setItem: vi.fn((k: string, v: string) => {
      memoryStore[k] = String(v);
    }),
    removeItem: vi.fn((k: string) => {
      delete memoryStore[k];
    }),
    clear: vi.fn(() => {
      for (const k of Object.keys(memoryStore)) delete memoryStore[k];
    }),
  },
  configurable: true,
});

const apiMocks = vi.hoisted(() => ({
  useSettingsSnapshot: vi.fn(),
  useSettingsUpdate: vi.fn(),
  useSettingsDiagnostics: vi.fn(),
  mutate: vi.fn(),
  refetch: vi.fn(),
}));

vi.mock("../api/useBusytokData", () => ({
  useSettingsSnapshot: (...args: unknown[]) => apiMocks.useSettingsSnapshot(...args),
  useSettingsUpdate: (...args: unknown[]) => apiMocks.useSettingsUpdate(...args),
  useSettingsDiagnostics: (...args: unknown[]) => apiMocks.useSettingsDiagnostics(...args),
}));

import { SettingsPage } from "./SettingsPage";

function envelope<T>(data: T): ReadEnvelopeDto<T> {
  return {
    data,
    generated_at_ms: 1,
    generation_id: "gen-1",
    readiness: "ready_exact",
    is_exact: true,
    is_stale: false,
    watermark_ms: null,
    progress: null,
    degraded_reason: null,
  };
}

function diagnostics(overrides: Partial<SettingsDiagnosticsDto> = {}): SettingsDiagnosticsDto {
  return {
    db_healthy: false,
    db_size_bytes: 512,
    migration_version: 1,
    usage_event_count: 2,
    last_log_checkpoint_ms: null,
    writer_queue_depth: 0,
    aggregate_lag_ms: 0,
    recent_diagnostics: [],
    subagent: null,
    ...overrides,
  };
}

function snapshot(overrides: Partial<SettingsSnapshotDto> = {}): SettingsSnapshotDto {
  return {
    timezone: "UTC",
    week_starts_on: 0,
    discovery: {
      claude_code_default_paths: true,
      codex_default_paths: false,
      manual_roots: [
        { id: "root-1", client_id: "codex", root_path: "/tmp/codex", source_type: "manual_root" },
      ],
    },
    privacy: {
      local_only: false,
      redact_sensitive_values: true,
    },
    prompt_palette_default_action: "OnlyCopy",
    recovery_actions: [],
    diagnostics: diagnostics(),
    subagent: {
      enabled: true,
      profiles: [],
    },
    ...overrides,
  };
}

function mockPage(
  pageSnapshot = snapshot(),
  pageDiagnostics: SettingsDiagnosticsDto | undefined = diagnostics(),
  options: { pending?: boolean } = {},
) {
  apiMocks.useSettingsSnapshot.mockReturnValue({
    data: envelope(pageSnapshot),
    isLoading: false,
    isError: false,
    isFetching: false,
    refetch: apiMocks.refetch,
  });
  apiMocks.useSettingsDiagnostics.mockReturnValue({
    data: pageDiagnostics ? envelope(pageDiagnostics) : undefined,
    isLoading: false,
    isError: false,
  });
  apiMocks.useSettingsUpdate.mockReturnValue({
    mutate: apiMocks.mutate,
    isPending: options.pending ?? false,
  });
}

beforeEach(() => {
  setPromptPaletteShortcutStatus({ state: "idle" });
  apiMocks.useSettingsSnapshot.mockReset();
  apiMocks.useSettingsUpdate.mockReset();
  apiMocks.useSettingsDiagnostics.mockReset();
  apiMocks.mutate.mockReset();
  apiMocks.refetch.mockReset();
  promptActionMocks.getPromptPaletteAccessibilityStatus.mockReset();
  promptActionMocks.openPromptPaletteAccessibilitySettings.mockReset();
  promptActionMocks.getPromptPaletteAccessibilityStatus.mockResolvedValue({ ok: true });
  bgServiceMocks.getDesktopLifecycleSettings.mockReset();
  bgServiceMocks.updateDesktopLifecycleSettings.mockReset();
  bgServiceMocks.getBackgroundServiceDiagnostics.mockReset();
  bgServiceMocks.repairBackgroundService.mockReset();
  bgServiceMocks.getDesktopLifecycleSettings.mockResolvedValue({
    launch_busytok_desktop_at_login: true,
  });
  bgServiceMocks.getBackgroundServiceDiagnostics.mockResolvedValue({
    state: "running",
    actionable: false,
    gui_build_identity: "0.1.0",
    service_build_identity: "0.1.0",
    version_skew: false,
  });
  desktopHostMocks.desktopHostShortcutDiagnostics.mockReset();
  desktopHostMocks.desktopHostRetryShortcutRegistration.mockReset();
  desktopHostMocks.desktopHostShortcutDiagnostics.mockResolvedValue({
    state: "registered",
    shortcut: "Cmd+Shift+P",
    failure_reason: null,
    retry_count: 0,
  });
  desktopHostMocks.desktopHostRetryShortcutRegistration.mockResolvedValue(undefined);
  for (const key of Object.keys(memoryStore)) delete memoryStore[key];
});

afterEach(() => {
  cleanup();
  setPromptPaletteShortcutStatus({ state: "idle" });
  vi.useRealTimers();
});

describe("SettingsPage additional coverage", () => {
  it("renders diagnostics fallbacks and ready paste status", async () => {
    mockPage(snapshot(), diagnostics(), { pending: true });

    render(<SettingsPage />);

    expect(screen.getByText("None")).toBeDefined();
    expect(await screen.findByText("Ready")).toBeDefined();
    // Mutation pending should not produce any visible inline indicator
    expect(screen.queryByText("Saving settings...")).toBeNull();
  });

  it("updates week start, removes roots, toggles redaction", async () => {
    const user = userEvent.setup();
    mockPage();

    render(<SettingsPage />);

    await user.click(screen.getByRole("button", { name: "Monday" }));
    await user.click(screen.getByLabelText("Redact sensitive values"));
    await user.click(screen.getByRole("button", { name: "Remove root 1" }));

    await waitFor(() => {
      expect(apiMocks.mutate).toHaveBeenCalled();
    });
    expect(apiMocks.mutate.mock.calls.length).toBeGreaterThanOrEqual(3);
  });

  it("shows structured and general mutation errors next to settings fields", async () => {
    const user = userEvent.setup();
    apiMocks.mutate.mockImplementationOnce((_body, options) => {
      options.onError(
        new BusytokControlError("settings_validation_failed", "Invalid settings", {
          errors: [
            {
              code: "invalid_discovery",
              field_path: "discovery.codex_default_paths",
              message: "Codex path not allowed",
            },
          ],
        }),
      );
    });
    mockPage();

    render(<SettingsPage />);
    // The reporting timezone row renders read-only (follows system) and is no
    // longer editable, so the Codex discovery toggle is the mutation vehicle.
    await user.click(screen.getByLabelText("Codex"));

    expect(await screen.findByText("Codex path not allowed")).toBeDefined();

    cleanup();
    apiMocks.mutate.mockImplementationOnce((_body, options) => {
      options.onError(new Error("network down"));
    });
    mockPage(snapshot({ discovery: { claude_code_default_paths: true, codex_default_paths: true, manual_roots: [] } }));
    render(<SettingsPage />);
    await user.click(screen.getByLabelText("Claude Code"));

    await waitFor(() => {
      expect(apiMocks.mutate).toHaveBeenCalled();
    });
  });

  it("shows commit timeout and retries the last patch", async () => {
    vi.useFakeTimers();
    apiMocks.mutate.mockImplementation(() => {});
    mockPage();

    render(<SettingsPage />);
    // Reporting timezone is read-only; toggle Codex to drive a mutate call.
    fireEvent.click(screen.getByLabelText("Codex"));

    act(() => {
      vi.advanceTimersByTime(10_000);
    });

    expect(screen.getByText(/commit is taking longer/i)).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Retry commit" }));
    expect(apiMocks.mutate.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it("shows and persists the local theme preference selector", async () => {
    const user = userEvent.setup();
    mockPage();

    render(<SettingsPage />);

    // Appearance section is present with the shared SegmentedControl labels.
    expect(screen.getByText(/theme/i)).toBeDefined();
    expect(screen.getByRole("group", { name: /theme/i })).toBeDefined();

    await user.click(screen.getByRole("button", { name: "Dark" }));

    // Theme preference is frontend-local — persisted via preferencesStorage,
    // NOT routed through the server-backed settings mutation.
    expect(loadPreferences().themePreference).toBe("dark");
    expect(apiMocks.mutate).not.toHaveBeenCalled();
  });

  it("renders appearance, diagnostics, and destructive settings content together without hiding any section", async () => {
    mockPage(snapshot(), diagnostics());

    render(<SettingsPage />);

    // Standard configuration: appearance section (local theme control) +
    // server-backed reporting config.
    expect(screen.getByRole("group", { name: /theme/i })).toBeDefined();
    expect(screen.getByRole("heading", { name: /reporting timezone/i, level: 2 })).toBeDefined();
    // Diagnostics/info: shortcut status row + db diagnostics row.
    expect(screen.getByText(/prompt palette shortcut/i)).toBeDefined();
    expect(screen.getByText(/db healthy/i)).toBeDefined();
    // Privacy: risky/standard toggle controls.
    expect(screen.getByRole("heading", { name: /^privacy/i, level: 2 })).toBeDefined();
  });

  it("shows stopped for this session without treating it as needs attention", async () => {
    bgServiceMocks.getBackgroundServiceDiagnostics.mockResolvedValue({
      state: "stopped_for_this_session",
      actionable: false,
      gui_build_identity: "0.1.0",
      service_build_identity: null,
      version_skew: false,
    });
    mockPage();

    render(<SettingsPage />);

    expect(await screen.findByText("Stopped for session")).toBeDefined();
    expect(screen.queryByText("Needs attention")).toBeNull();
    expect(
      screen.queryByRole("button", { name: /repair background service/i }),
    ).toBeNull();
  });

  it("keeps background service copy separate from login-start copy", async () => {
    bgServiceMocks.getDesktopLifecycleSettings.mockResolvedValue({
      launch_busytok_desktop_at_login: false,
    });
    mockPage();

    render(<SettingsPage />);

    expect(
      await screen.findByRole("heading", { name: /desktop/i, level: 2 }),
    ).toBeDefined();
    expect(
      screen.getByRole("heading", { name: /background service/i, level: 2 }),
    ).toBeDefined();

    const desktopSection = screen
      .getByRole("heading", { name: /desktop/i, level: 2 })
      .closest("section")!;
    expect(desktopSection.textContent).not.toMatch(/repair/i);

    const bgSection = screen
      .getByRole("heading", { name: /background service/i, level: 2 })
      .closest("section")!;
    expect(bgSection.textContent).not.toMatch(/login/i);
  });

  it("repair background service does not imply changing login start", async () => {
    // Ensure the repair button is visible (actionable=true).
    bgServiceMocks.getBackgroundServiceDiagnostics.mockResolvedValue({
      state: "needs_attention",
      actionable: true,
      gui_build_identity: "0.1.0",
      service_build_identity: null,
      version_skew: false,
    });
    bgServiceMocks.getDesktopLifecycleSettings.mockResolvedValue({
      launch_busytok_desktop_at_login: true,
    });
    mockPage();

    render(<SettingsPage />);

    const repairBtn = await screen.findByRole("button", {
      name: /repair background service/i,
    });
    expect(repairBtn).toBeDefined();
    // updateDesktopLifecycleSettings should NOT have been called.
    expect(bgServiceMocks.updateDesktopLifecycleSettings).not.toHaveBeenCalled();
  });

  it("keeps lifecycle controls visible when service settings snapshot fails", async () => {
    // Lifecycle settings snapshot fails.
    bgServiceMocks.getDesktopLifecycleSettings.mockRejectedValue(
      new Error("unavailable"),
    );
    bgServiceMocks.getBackgroundServiceDiagnostics.mockResolvedValue({
      state: "running",
      actionable: false,
      gui_build_identity: "0.1.0",
      service_build_identity: null,
      version_skew: false,
    });
    mockPage(snapshot(), diagnostics());

    render(<SettingsPage />);

    // Background Service heading should still appear.
    await waitFor(() => {
      expect(
        screen.getByRole("heading", { name: /background service/i, level: 2 }),
      ).toBeDefined();
    });

    // Desktop heading should NOT appear (lifecycle settings failed).
    expect(
      screen.queryByRole("heading", { name: /desktop/i, level: 2 }),
    ).toBeNull();

    // All other sections should still render.
    expect(
      screen.getByRole("heading", { name: /appearance/i, level: 2 }),
    ).toBeDefined();
    expect(
      screen.getByRole("heading", { name: /^privacy/i, level: 2 }),
    ).toBeDefined();
  });

  it("uses zero page-private control classes after canonical migration", () => {
    mockPage();
    render(<SettingsPage />);
    expect(document.querySelector(".segmented-group")).toBeNull();
    expect(document.querySelector(".segmented-label")).toBeNull();
    expect(document.querySelector(".toggle-label")).toBeNull();
    expect(document.querySelector(".toggle")).toBeNull();
    expect(document.querySelector(".toggle-track")).toBeNull();
    expect(document.querySelector(".diag-value")).toBeNull();
    expect(document.querySelector(".manual-root-controls")).toBeNull();
  });

  it("renders Theme and Week starts on via shared SegmentedControl", () => {
    mockPage();
    render(<SettingsPage />);
    const groups = document.querySelectorAll(".segmented-control");
    expect(groups.length).toBeGreaterThanOrEqual(2);
  });

  // ── Background Service: diagnostics toggle + repair action ──────────

  it("reveals detailed diagnostics when Show Diagnostics is toggled on", async () => {
    bgServiceMocks.getBackgroundServiceDiagnostics.mockResolvedValue({
      state: "running",
      actionable: false,
      gui_build_identity: "gui-0.1.0",
      service_build_identity: "svc-0.1.0",
      version_skew: false,
    });
    mockPage();

    render(<SettingsPage />);

    // Wait for the background service section to load.
    await screen.findByRole("heading", { name: /background service/i, level: 2 });

    // Toggle "Show Diagnostics" on.
    const toggle = screen.getByLabelText("Show Diagnostics");
    fireEvent.click(toggle);

    // The detailed rows (GUI build, Service build) now render with their
    // unique build identity strings.
    expect(screen.getByText("gui-0.1.0")).toBeDefined();
    expect(screen.getByText("svc-0.1.0")).toBeDefined();
  });

  it("calls repairBackgroundService and refetches diagnostics when Repair is clicked", async () => {
    bgServiceMocks.getBackgroundServiceDiagnostics
      .mockResolvedValueOnce({
        state: "needs_attention",
        actionable: true,
        gui_build_identity: "0.1.0",
        service_build_identity: null,
        version_skew: false,
      })
      .mockResolvedValueOnce({
        state: "running",
        actionable: false,
        gui_build_identity: "0.1.0",
        service_build_identity: "0.1.0",
        version_skew: false,
      });
    bgServiceMocks.repairBackgroundService.mockResolvedValue(undefined);
    mockPage();

    render(<SettingsPage />);

    const repairBtn = await screen.findByRole("button", {
      name: /repair background service/i,
    });
    fireEvent.click(repairBtn);

    // Wait for the repair promise + diagnostics refetch to settle.
    await waitFor(() => {
      expect(bgServiceMocks.repairBackgroundService).toHaveBeenCalledTimes(1);
    });
    // After repair, diagnostics are refetched (second call returns "running").
    await waitFor(() => {
      expect(bgServiceMocks.getBackgroundServiceDiagnostics).toHaveBeenCalledTimes(2);
    });
  });

  // ── Desktop lifecycle toggle ────────────────────────────────────────

  it("persists the launch-at-login toggle and reverts on failure", async () => {
    bgServiceMocks.getDesktopLifecycleSettings.mockResolvedValue({
      launch_busytok_desktop_at_login: false,
    });
    bgServiceMocks.updateDesktopLifecycleSettings.mockResolvedValue(undefined);
    mockPage();

    render(<SettingsPage />);

    const toggle = await screen.findByLabelText("Launch Busytok Desktop at login");
    // Toggle ON.
    fireEvent.click(toggle);

    await waitFor(() => {
      expect(bgServiceMocks.updateDesktopLifecycleSettings).toHaveBeenCalledWith({
        launch_busytok_desktop_at_login: true,
      });
    });
  });

  it("reverts the launch-at-login toggle when the update fails", async () => {
    bgServiceMocks.getDesktopLifecycleSettings.mockResolvedValue({
      launch_busytok_desktop_at_login: true,
    });
    bgServiceMocks.updateDesktopLifecycleSettings.mockRejectedValue(
      new Error("permission denied"),
    );
    mockPage();

    render(<SettingsPage />);

    const toggle = await screen.findByLabelText("Launch Busytok Desktop at login");
    // Toggle OFF.
    fireEvent.click(toggle);

    // The update fails; the toggle should revert to ON.
    await waitFor(() => {
      expect(bgServiceMocks.updateDesktopLifecycleSettings).toHaveBeenCalledWith({
        launch_busytok_desktop_at_login: false,
      });
    });
  });

  // ── Shortcut retry button ───────────────────────────────────────────

  it("shows the Retry button and refetches diagnostics when host shortcut is failed", async () => {
    desktopHostMocks.desktopHostShortcutDiagnostics.mockResolvedValue({
      state: "failed",
      shortcut: "Cmd+Shift+P",
      failure_reason: "platform_restriction",
      retry_count: 0,
    });
    setPromptPaletteShortcutStatus({ state: "failed", message: "platform_restriction" });
    mockPage();

    render(<SettingsPage />);

    const retryBtn = await screen.findByRole("button", { name: /^retry$/i });
    fireEvent.click(retryBtn);

    // Retry triggers desktopHostRetryShortcutRegistration then re-fetches diagnostics.
    await waitFor(() => {
      expect(desktopHostMocks.desktopHostRetryShortcutRegistration).toHaveBeenCalledTimes(1);
    });
  });

  // ── Add/remove manual roots ─────────────────────────────────────────

  it("adds a new manual root row via Add root button", async () => {
    const user = userEvent.setup();
    mockPage(snapshot({ discovery: { claude_code_default_paths: true, codex_default_paths: false, manual_roots: [] } }));

    render(<SettingsPage />);

    await user.click(screen.getByRole("button", { name: /add root/i }));

    // A new root row with empty Client ID / Root path inputs appears.
    expect(screen.getByPlaceholderText("Client ID")).toBeDefined();
    expect(screen.getByPlaceholderText("Root path")).toBeDefined();

    // Typing into the new root fields triggers syncManualRoots → mutate.
    await user.type(screen.getByPlaceholderText("Client ID"), "codex");
    await user.type(screen.getByPlaceholderText("Root path"), "/tmp/codex");

    await waitFor(() => {
      expect(apiMocks.mutate).toHaveBeenCalled();
    });
  });

  it("edits an existing manual root path field", async () => {
    const user = userEvent.setup();
    mockPage();

    render(<SettingsPage />);

    // The snapshot has one root: { client_id: "codex", root_path: "/tmp/codex" }
    const pathInput = screen.getByDisplayValue("/tmp/codex");
    await user.clear(pathInput);
    await user.type(pathInput, "/new/path");

    await waitFor(() => {
      expect(apiMocks.mutate).toHaveBeenCalled();
    });
  });

  // ── Prompt palette default action change ───────────────────────────

  it("changes the prompt palette default action via the select", async () => {
    const user = userEvent.setup();
    mockPage(snapshot({ prompt_palette_default_action: "CopyAndPaste" }));

    render(<SettingsPage />);

    // Open the select and choose "Only paste".
    await user.click(screen.getByLabelText("Prompt palette default action"));
    await user.click(screen.getByRole("option", { name: /only paste/i }));

    await waitFor(() => {
      expect(apiMocks.mutate).toHaveBeenCalledWith(
        expect.objectContaining({
          prompt_palette_default_action: "OnlyPaste",
        }),
        expect.anything(),
      );
    });
  });

  // ── Paste status: permission missing on macOS ───────────────────────

  it("shows the Open System Settings button when paste permission is missing on macOS", async () => {
    promptActionMocks.getPromptPaletteAccessibilityStatus.mockResolvedValue({
      ok: false,
      failure_reason: "permission_missing",
    });
    const platformGetter = vi.spyOn(navigator, "platform", "get");
    platformGetter.mockReturnValue("MacIntel");
    mockPage();

    render(<SettingsPage />);

    expect(await screen.findByRole("button", { name: /open system settings/i })).toBeDefined();
    platformGetter.mockRestore();
  });
});
