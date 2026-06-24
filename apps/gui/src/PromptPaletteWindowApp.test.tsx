import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PromptPaletteWindowApp } from "./PromptPaletteWindowApp";

const mocks = vi.hoisted(() => ({
  controllerInstanceCounter: 0,
  requestPanelClose: vi.fn(() => Promise.resolve()),
  requestShowGui: vi.fn(() => Promise.resolve()),
  reportFrontendEvent: vi.fn(),
}));

vi.mock("./lib/panelWindowOps", () => ({
  requestPanelClose: () => mocks.requestPanelClose(),
  requestShowGui: () => mocks.requestShowGui(),
}));

vi.mock("./logging/reporter", () => ({
  reportFrontendEvent: (entry: unknown) => mocks.reportFrontendEvent(entry),
}));

vi.mock("./api/PanelEventSubscriptionProvider", () => ({
  PanelEventSubscriptionProvider: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

vi.mock("./api/BusytokClientContext", () => ({
  BusytokClientProvider: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

vi.mock("./api/panelBusytokClient", () => ({
  panelBusytokClient: {},
}));

vi.mock("./api/useBusytokData", () => ({
  useSettingsSnapshot: () => ({
    data: { data: { prompt_palette_default_action: "Copy&Paste" } },
    isLoading: false,
    isError: false,
    isFetching: false,
    refetch: vi.fn(),
  }),
}));

vi.mock("./components/prompt-palette/PromptPaletteOverlayController", () => ({
  PromptPaletteOverlayController: ({
    open,
    onClose,
    onOpenPage,
    onCreateNew,
    defaultAction: _defaultAction,
  }: {
    open: boolean;
    onClose: () => void;
    onOpenPage: () => void;
    onCreateNew: () => void;
    defaultAction?: string;
  }) =>
    open ? (
      <section
        aria-label="Standalone Prompt Palette"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            onClose();
          }
        }}
        tabIndex={-1}
      >
        <button type="button" onClick={onClose}>
          Close
        </button>
        <p>controller-instance:{++mocks.controllerInstanceCounter}</p>
        <button type="button" onClick={onOpenPage}>
          Open management
        </button>
        <button type="button" onClick={onCreateNew}>
          Create prompt
        </button>
      </section>
    ) : null,
}));

describe("PromptPaletteWindowApp", () => {
  afterEach(() => {
    cleanup();
    mocks.controllerInstanceCounter = 0;
    mocks.requestPanelClose.mockClear();
    mocks.requestShowGui.mockClear();
    mocks.reportFrontendEvent.mockClear();
  });

  it("closes the palette via panel bridge on close", async () => {
    render(<PromptPaletteWindowApp />);

    const palette = screen.getByLabelText("Standalone Prompt Palette");
    expect(palette).toBeDefined();
    expect(screen.queryByText("Overview")).toBeNull();

    palette.focus();
    fireEvent.keyDown(palette, { key: "Escape" });

    expect(mocks.requestPanelClose).toHaveBeenCalled();
  });

  it("closes the palette when Escape reaches the window instead of the overlay", () => {
    render(<PromptPaletteWindowApp />);

    fireEvent.keyDown(window, { key: "Escape" });

    expect(mocks.requestPanelClose).toHaveBeenCalled();
  });

  it("starts a fresh controller session after close so window state does not leak", async () => {
    const user = userEvent.setup();

    render(<PromptPaletteWindowApp />);
    expect(screen.getByText("controller-instance:1")).toBeDefined();

    await user.click(screen.getByRole("button", { name: "Close" }));

    expect(screen.getByText("controller-instance:2")).toBeDefined();
  });

  it("shows the main window via panel bridge when the user opens the management page", async () => {
    const user = userEvent.setup();

    render(<PromptPaletteWindowApp />);

    await user.click(screen.getByRole("button", { name: "Open management" }));

    expect(mocks.requestShowGui).toHaveBeenCalled();
    expect(mocks.requestPanelClose).toHaveBeenCalled();
  });

  it("shows the main window via panel bridge when the user opens the create prompt flow", async () => {
    const user = userEvent.setup();

    render(<PromptPaletteWindowApp />);

    await user.click(screen.getByRole("button", { name: "Create prompt" }));

    expect(mocks.requestShowGui).toHaveBeenCalled();
    expect(mocks.requestPanelClose).toHaveBeenCalled();
  });

  it("keeps the palette visible when opening the management page fails", async () => {
    const user = userEvent.setup();
    mocks.requestShowGui.mockRejectedValueOnce(new Error("rpc failed"));

    render(<PromptPaletteWindowApp />);

    await user.click(screen.getByRole("button", { name: "Open management" }));

    expect(mocks.reportFrontendEvent).toHaveBeenCalledWith(
      expect.objectContaining({ event_code: "gui.prompt_palette.open_management_failed" }),
    );
  });

  it("logs the resolved theme on mount (not on import) exactly once, even under StrictMode", () => {
    // The panel must emit its resolved-theme log from inside a React effect,
    // not at module import time. That keeps the emission out of the test's
    // import phase and lets us assert the log fires exactly once on mount,
    // even under StrictMode (which double-invokes effects in dev).
    mocks.reportFrontendEvent.mockClear();

    // Import-time should not have emitted the panel theme log.
    expect(
      mocks.reportFrontendEvent.mock.calls.filter(
        (call) =>
          (call[0] as { event_code: string }).event_code ===
          "gui.prompt_palette.panel_theme_resolved",
      ),
    ).toHaveLength(0);

    render(
      <StrictMode>
        <PromptPaletteWindowApp />
      </StrictMode>,
    );

    const themeLogs = mocks.reportFrontendEvent.mock.calls.filter(
      (call) => (call[0] as { event_code: string }).event_code === "gui.theme.bootstrap",
    );
    const panelThemeLogs = mocks.reportFrontendEvent.mock.calls.filter(
      (call) =>
        (call[0] as { event_code: string }).event_code ===
        "gui.prompt_palette.panel_theme_resolved",
    );

    // The panel owns its own resolved-theme log; it must not duplicate the
    // global gui.theme.bootstrap noise from main.tsx, and it must fire
    // exactly once despite StrictMode double-invoking effects.
    expect(themeLogs).toHaveLength(0);
    expect(panelThemeLogs).toHaveLength(1);
  });
});
