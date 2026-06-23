import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";

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

const mocks = vi.hoisted(() => ({
  executePromptAction: vi.fn(),
  getPromptPaletteAccessibilityStatus: vi.fn(),
  pasteActiveApp: vi.fn(),
  writeSystemClipboard: vi.fn(),
  mutatePromptUse: vi.fn(),
  reportFrontendEvent: vi.fn(),
  flushBuffer: vi.fn(),
  hasBufferedLogs: vi.fn(),
  usePromptsList: vi.fn(),
  usePromptUse: vi.fn(),
  prefetchStartupQueries: vi.fn(),
  promptListRefetch: vi.fn(),
}));

vi.mock("./components/AppShell", () => ({
  AppShell: ({ children, onNavigate }: { children: React.ReactNode; onNavigate: (page: string) => void }) => (
    <div>
      <button type="button" onClick={() => onNavigate("overview")}>Overview nav</button>
      <button type="button" onClick={() => onNavigate("settings")}>Settings nav</button>
      {children}
    </div>
  ),
}));

vi.mock("./pages/OverviewPage", () => ({ OverviewPage: () => <main>Overview page</main> }));
vi.mock("./pages/UsagePage", () => ({ UsagePage: () => <main>Usage page</main> }));
vi.mock("./pages/ProjectsPage", () => ({ ProjectsPage: () => <main>Projects page</main> }));
vi.mock("./pages/ModelsPage", () => ({ ModelsPage: () => <main>Models page</main> }));
vi.mock("./pages/SessionsPage", () => ({ SessionsPage: () => <main>Sessions page</main> }));
vi.mock("./pages/PromptPalettePage", () => ({ PromptPalettePage: () => <main>Prompt Palette page</main> }));
vi.mock("./pages/SettingsPage", () => ({ SettingsPage: () => <main>Settings page</main> }));

vi.mock("./api/EventSubscriptionProvider", () => ({
  EventSubscriptionProvider: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  EventSubscriptionContext: {
    Provider: ({ children }: { children: React.ReactNode }) => <>{children}</>,
    Consumer: ({ children }: { children: (value: unknown) => React.ReactNode }) =>
      children({ connectionStatus: "connected", serviceStatus: "ready", bridgeStatus: "connected" }),
  },
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onFocusChanged: vi.fn().mockResolvedValue(() => {}) }),
}));

vi.mock("./lib/updaterClient", () => ({
  checkForUpdate: async () => ({ kind: "up-to-date" }),
  applyUpdate: async () => {},
  CHECK_TIMEOUT_MS: 20_000,
  DOWNLOAD_TIMEOUT_MS: 120_000,
}));

vi.mock("./api/busytokClient", () => ({
  busytokClient: {},
}));

vi.mock("./api/useBusytokData", () => ({
  usePromptsList: (...args: unknown[]) => mocks.usePromptsList(...args),
  usePromptUse: (...args: unknown[]) => mocks.usePromptUse(...args),
  prefetchStartupQueries: (...args: unknown[]) => mocks.prefetchStartupQueries(...args),
  useSettingsSnapshot: () => ({
    data: { data: { prompt_palette_default_action: "copy" } },
    isLoading: false,
    isError: false,
    isFetching: false,
    refetch: vi.fn(),
  }),
}));

vi.mock("./logging/reporter", () => ({
  reportFrontendEvent: (...args: unknown[]) => mocks.reportFrontendEvent(...args),
  flushBuffer: (...args: unknown[]) => mocks.flushBuffer(...args),
  hasBufferedLogs: (...args: unknown[]) => mocks.hasBufferedLogs(...args),
}));

vi.mock("./api/useEventSubscription", () => ({
  useEventSubscription: () => ({
    connectionStatus: "connected",
    serviceStatus: "ready",
    bridgeStatus: "connected",
  }),
}));

beforeEach(() => {
  storage.clear();
  mocks.prefetchStartupQueries.mockReset();
  mocks.hasBufferedLogs.mockReturnValue(false);
  mocks.mutatePromptUse.mockResolvedValue({ usage_count: 1, last_used_at_ms: 10 });
  mocks.usePromptUse.mockReturnValue({
    mutateAsync: mocks.mutatePromptUse,
    isPending: false,
  });
  mocks.usePromptsList.mockReturnValue({
    data: { data: { entries: [], total_count: 0 } },
    isLoading: false,
    isError: false,
    refetch: mocks.promptListRefetch,
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function renderApp() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>,
  );
}

describe("App prompt palette orchestration", () => {
  it("reports mount events and flushes buffered frontend logs", () => {
    mocks.hasBufferedLogs.mockReturnValue(true);

    renderApp();

    expect(mocks.reportFrontendEvent).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "gui.app_mounted" }),
    );
    expect(mocks.flushBuffer).toHaveBeenCalled();
  });

  it("restores the last desktop page after the main app remounts", async () => {
    const user = userEvent.setup();
    const firstRender = renderApp();

    await user.click(screen.getByRole("button", { name: "Settings nav" }));
    expect(screen.getByText("Settings page")).toBeDefined();
    await waitFor(() => {
      expect(localStorage.getItem("busytok.desktop.currentPage.v1")).toBe("settings");
    });

    firstRender.unmount();
    renderApp();

    expect(screen.getByText("Settings page")).toBeDefined();
    expect(screen.queryByText("Overview page")).toBeNull();
  });
});
