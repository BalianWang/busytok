import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PromptPaletteOverlayController } from "./PromptPaletteOverlayController";

const mocks = vi.hoisted(() => ({
  mutateAsync: vi.fn(),
  refetch: vi.fn(),
  usePromptsList: vi.fn(),
  usePromptUse: vi.fn(),
  executePromptAction: vi.fn(),
  writeSystemClipboard: vi.fn(),
  pasteActiveApp: vi.fn(),
  getAccessibilityStatus: vi.fn(),
  requestPanelClose: vi.fn(() => Promise.resolve()),
  panelRuntime: { invoke: vi.fn(), subscribe: vi.fn(() => () => {}), requestClose: vi.fn() },
  createPasteBridge: vi.fn((_options?: unknown) => ({})),
}));

vi.mock("../../api/useBusytokData", () => ({
  usePromptsList: (...args: unknown[]) => mocks.usePromptsList(...args),
  usePromptUse: (...args: unknown[]) => mocks.usePromptUse(...args),
}));

vi.mock("../../lib/promptPaletteActions", () => ({
  executePromptAction: (...args: any[]) => (mocks.executePromptAction as any)(...args),
  PROMPT_ACTION_ERROR_MESSAGE: "Could not run prompt action. Try again.",
  promptActionStatusMessage: vi.fn(),
  writeSystemClipboard: (...args: any[]) => (mocks.writeSystemClipboard as any)(...args),
  readSystemClipboard: async () => "",
  pasteActiveApp: (...args: any[]) => (mocks.pasteActiveApp as any)(...args),
  getPromptPaletteAccessibilityStatus: (...args: any[]) => (mocks.getAccessibilityStatus as any)(...args),
}));

vi.mock("../../lib/promptPalettePasteBridge", () => ({
  createPromptPasteBridge: (...args: any[]) => (mocks.createPasteBridge as any)(...args),
}));

vi.mock("../../lib/panelWindowOps", () => ({
  getPanelRuntime: () => mocks.panelRuntime,
  requestPanelClose: () => mocks.requestPanelClose(),
}));

vi.mock("../../api/useEventSubscription", () => ({
  useEventSubscription: () => ({
    connectionStatus: "connected",
    serviceStatus: "ready",
    bridgeStatus: "connected",
  }),
}));

vi.mock("../../logging/reporter", () => ({
  reportFrontendEvent: vi.fn(),
}));

vi.mock("./PromptPaletteOverlay", () => ({
  PromptPaletteOverlay: ({
    query,
    onQueryChange,
    onExecute,
    defaultAction: _defaultAction,
  }: {
    query: string;
    onQueryChange: (query: string) => void;
    onExecute?: (entry: unknown, action: unknown) => void;
    defaultAction?: string;
  }) => (
    <section aria-label="Prompt Palette Controller">
      <p>query:{query}</p>
      <button type="button" onClick={() => onQueryChange("follow-up")}>
        Set query
      </button>
      {onExecute && (
        <button
          type="button"
          onClick={() => onExecute({ id: "test-entry", content: "hello" }, "Copy&Paste")}
        >
          Execute paste
        </button>
      )}
    </section>
  ),
}));

describe("PromptPaletteOverlayController", () => {
  beforeEach(() => {
    mocks.refetch.mockReset();
    mocks.refetch.mockResolvedValue(undefined);
    mocks.mutateAsync.mockReset();
    mocks.usePromptUse.mockReturnValue({
      mutateAsync: mocks.mutateAsync,
    });
    mocks.usePromptsList.mockReturnValue({
      data: { data: { entries: [], total_count: 0 } },
      isLoading: false,
      isError: false,
      refetch: mocks.refetch,
    });
    mocks.executePromptAction.mockReset();
    mocks.pasteActiveApp.mockResolvedValue({ ok: true, failure_reason: null });
    mocks.getAccessibilityStatus.mockResolvedValue({ ok: true, failure_reason: null });
    mocks.requestPanelClose.mockReset();
    mocks.createPasteBridge.mockReset();
    mocks.createPasteBridge.mockReturnValue({});
  });

  afterEach(() => {
    cleanup();
  });

  it("refetches when the standalone palette window regains focus", async () => {
    render(
      <PromptPaletteOverlayController
        open
        presentation="window"
        onClose={() => {}}
        onOpenPage={() => {}}
        onCreateNew={() => {}}
      />,
    );

    mocks.refetch.mockClear();
    window.dispatchEvent(new Event("focus"));

    await waitFor(() => {
      expect(mocks.refetch).toHaveBeenCalledOnce();
    });
  });

  it("resets local query state when the controller is remounted for a new palette session", async () => {
    const user = userEvent.setup();
    const view = render(
      <PromptPaletteOverlayController
        key="session-1"
        open
        presentation="window"
        onClose={() => {}}
        onOpenPage={() => {}}
        onCreateNew={() => {}}
      />,
    );

    expect(screen.getByText("query:")).toBeDefined();
    await user.click(screen.getByRole("button", { name: "Set query" }));
    expect(screen.getByText("query:follow-up")).toBeDefined();

    view.rerender(
      <PromptPaletteOverlayController
        key="session-2"
        open
        presentation="window"
        onClose={() => {}}
        onOpenPage={() => {}}
        onCreateNew={() => {}}
      />,
    );

    expect(screen.getByText("query:")).toBeDefined();
  });

  describe("window-mode paste routing", () => {
    it("passes runtime-aware functions to createPromptPasteBridge", async () => {
      const user = userEvent.setup();
      mocks.executePromptAction.mockResolvedValue({
        outcome: "paste_attempted",
        failure_reason: null,
      });

      render(
        <PromptPaletteOverlayController
          open
          presentation="window"
          onClose={() => {}}
          onOpenPage={() => {}}
          onCreateNew={() => {}}
        />,
      );

      await user.click(screen.getByRole("button", { name: "Execute paste" }));

      await waitFor(() => {
        expect(mocks.executePromptAction).toHaveBeenCalled();
      });

      expect(mocks.createPasteBridge).toHaveBeenCalled();
      const bridgeOptions = (mocks.createPasteBridge.mock.calls[0]?.[0] ?? {}) as Record<string, unknown>;

      // Window mode: close handled by hideWindowForPaste, not onWindowHidden
      expect(bridgeOptions.onWindowHidden).toBeUndefined();

      // Runtime-aware functions present
      expect(bridgeOptions.pasteIntoActiveApp).toBeInstanceOf(Function);
      expect(bridgeOptions.getAccessibilityStatus).toBeInstanceOf(Function);
      expect(bridgeOptions.hideWindowForPaste).toBeInstanceOf(Function);

      // pasteIntoActiveApp delegates to pasteActiveApp with the panel runtime
      await (bridgeOptions.pasteIntoActiveApp as () => Promise<unknown>)();
      expect(mocks.pasteActiveApp).toHaveBeenCalledWith(mocks.panelRuntime);

      // getAccessibilityStatus delegates to getPromptPaletteAccessibilityStatus with the panel runtime
      await (bridgeOptions.getAccessibilityStatus as () => Promise<unknown>)();
      expect(mocks.getAccessibilityStatus).toHaveBeenCalledWith(mocks.panelRuntime);

      // hideWindowForPaste calls requestPanelClose
      await (bridgeOptions.hideWindowForPaste as () => Promise<unknown>)();
      expect(mocks.requestPanelClose).toHaveBeenCalled();
    });

    it("does not double-close on paste success", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      mocks.executePromptAction.mockResolvedValue({
        outcome: "paste_attempted",
        failure_reason: null,
      });

      render(
        <PromptPaletteOverlayController
          open
          presentation="window"
          onClose={onClose}
          onOpenPage={() => {}}
          onCreateNew={() => {}}
        />,
      );

      await user.click(screen.getByRole("button", { name: "Execute paste" }));

      await waitFor(() => {
        expect(mocks.executePromptAction).toHaveBeenCalled();
      });

      // onClose should NOT be called on paste success — close was handled by hideWindowForPaste
      expect(onClose).not.toHaveBeenCalled();
    });

    it("calls onClose on paste fallback", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      mocks.executePromptAction.mockResolvedValue({
        outcome: "paste_fell_back_to_copy",
        failure_reason: "permission_missing",
      });

      render(
        <PromptPaletteOverlayController
          open
          presentation="window"
          onClose={onClose}
          onOpenPage={() => {}}
          onCreateNew={() => {}}
        />,
      );

      await user.click(screen.getByRole("button", { name: "Execute paste" }));

      await waitFor(() => {
        expect(onClose).toHaveBeenCalled();
      });
    });
  });

  describe("overlay-mode paste routing", () => {
    it("passes onWindowHidden to createPromptPasteBridge", async () => {
      const user = userEvent.setup();
      mocks.executePromptAction.mockResolvedValue({
        outcome: "copy",
        failure_reason: null,
      });

      render(
        <PromptPaletteOverlayController
          open
          presentation="overlay"
          onClose={() => {}}
          onOpenPage={() => {}}
          onCreateNew={() => {}}
        />,
      );

      await user.click(screen.getByRole("button", { name: "Execute paste" }));

      await waitFor(() => {
        expect(mocks.createPasteBridge).toHaveBeenCalled();
      });

      const bridgeOptions = (mocks.createPasteBridge.mock.calls[0]?.[0] ?? {}) as Record<string, unknown>;

      // Overlay mode: onWindowHidden is bound to onClose
      expect(bridgeOptions.onWindowHidden).toBeInstanceOf(Function);

      // Overlay mode: no runtime-aware functions
      expect(bridgeOptions.pasteIntoActiveApp).toBeUndefined();
      expect(bridgeOptions.getAccessibilityStatus).toBeUndefined();
      expect(bridgeOptions.hideWindowForPaste).toBeUndefined();
    });
  });
});
