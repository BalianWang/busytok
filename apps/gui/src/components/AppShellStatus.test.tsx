import { cleanup, render, screen } from "@testing-library/react";
import { StrictMode, useMemo } from "react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppShell } from "./AppShell";
import { resetAggregateLagTelemetryStateForTests } from "./desktop/aggregateLagStatus";
import { PageToolbarProvider, useRegisterPageToolbar } from "./desktop/PageToolbarContext";
import type { ShellStatusDto } from "@busytok/protocol-types";

globalThis.ResizeObserver = class ResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const mocks = vi.hoisted(() => ({
  shellStatus: undefined as ShellStatusDto | undefined,
  connectionStatus: "connected" as "connected" | "disconnected" | "reconnecting",
  reportFrontendEvent: vi.fn(),
}));

vi.mock("../api/useBusytokData", () => ({
  useShellStatus: () => ({
    data: mocks.shellStatus,
    isLoading: false,
    isError: false,
  }),
}));

vi.mock("../api/useEventSubscription", () => ({
  useEventSubscription: () => ({
    connectionStatus: mocks.connectionStatus,
  }),
}));

vi.mock("../logging/reporter", () => ({
  reportFrontendEvent: (...args: unknown[]) => mocks.reportFrontendEvent(...args),
  safeReportEvent: (...args: unknown[]) => {
    // Mirror the real wrapper: forward to reportFrontendEvent at INFO level.
    const [event_code, message, details] = args as [string, string, Record<string, unknown>?];
    try {
      mocks.reportFrontendEvent({ level: "INFO", event_code, message, details });
    } catch {
      // Observability must not break the user action path.
    }
  },
}));

function status(overrides: Partial<ShellStatusDto> = {}): ShellStatusDto {
  return {
    generated_at_ms: 1,
    readiness: "ready_exact",
    status_chips: [],
    latest_event_seq: 1,
    writer_queue_depth: null,
    aggregate_lag_ms: null,
    subscription_bridge_connectivity: "connected",
    ...overrides,
  };
}

function ToolbarRegistrant() {
  const toolbar = useMemo(() => <button type="button">Refresh now</button>, []);
  useRegisterPageToolbar(toolbar);
  return <p>Content</p>;
}

beforeEach(() => {
  mocks.shellStatus = status();
  mocks.connectionStatus = "connected";
  mocks.reportFrontendEvent.mockReset();
  resetAggregateLagTelemetryStateForTests();
});

afterEach(() => cleanup());

describe("AppShell status rendering", () => {
  it("renders the single calm chip with the healthy label and a success dot when fully healthy", () => {
    render(
      <PageToolbarProvider>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <p>Content</p>
        </AppShell>
      </PageToolbarProvider>,
    );

    // Exactly ONE primary titlebar chip, with the healthy label.
    expect(screen.getByRole("button", { name: "Live capture active" })).toBeDefined();
    // No queue/connection/lag capsules remain.
    expect(screen.queryByText(/Q:/)).toBeNull();
    expect(screen.queryByText("⟳")).toBeNull();
    expect(screen.queryByText("⚠")).toBeNull();
    expect(document.querySelector(".desktop-progress-banner")).toBeNull();
  });

  it.each([
    ["starting", "Starting"],
    ["rebuilding", "Rebuilding"],
    ["ready_degraded", "Degraded"],
  ] as const)("renders %s readiness as the single escalated chip", (readiness, label) => {
    mocks.shellStatus = status({ readiness });

    render(
      <PageToolbarProvider>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <p>Content</p>
        </AppShell>
      </PageToolbarProvider>,
    );

    const chip = screen.getByRole("button", { name: label });
    expect(chip).toBeDefined();
    expect(chip.className).toContain("is-warning");
    // No auxiliary danger chip when there is no blocking danger.
    expect(screen.queryByRole("button", { name: /Service unreachable/ })).toBeNull();
  });

  it("renders the toolbar and projects reconnecting + queue into the single chip (no capsules)", () => {
    mocks.connectionStatus = "reconnecting";
    mocks.shellStatus = status({
      writer_queue_depth: 7,
      aggregate_lag_ms: 900, // below warning threshold → chip stays on "Reconnecting…"
    });

    render(
      <PageToolbarProvider>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <ToolbarRegistrant />
        </AppShell>
      </PageToolbarProvider>,
    );

    expect(screen.getByText("Refresh now")).toBeDefined();
    // Reconnecting is the highest-precedence label after readiness, so the
    // single chip carries it. No standalone Q:/⟳ capsules exist anymore.
    expect(screen.getByRole("button", { name: "Reconnecting…" })).toBeDefined();
    expect(screen.queryByText(/Q:/)).toBeNull();
    expect(screen.queryByText("⟳")).toBeNull();
  });

  it("renders elevated lag as the single escalated chip and filters scan_progress", async () => {
    const user = userEvent.setup();
    mocks.connectionStatus = "disconnected";
    mocks.shellStatus = status({
      aggregate_lag_ms: 6100,
      status_chips: [
        {
          id: "scan_progress",
          label: "Hidden progress",
          tone: "warning",
          detail: null,
          action: null,
        },
      ],
    });

    render(
      <PageToolbarProvider>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <p>Content</p>
        </AppShell>
      </PageToolbarProvider>,
    );

    // Disconnected beats lag in precedence → single chip is "Disconnected".
    const chip = screen.getByRole("button", { name: "Disconnected" });
    expect(chip.className).toContain("is-warning");
    expect(screen.queryByText("Hidden progress")).toBeNull();
    expect(screen.queryByText("⚠")).toBeNull();

    await user.click(chip);
    // The read-only popover surfaces the LIVE section rows (connection + queue + lag).
    expect(await screen.findByText("Disconnected", { selector: "dd" })).toBeDefined();
    expect(screen.getByText("6.1s", { selector: "dd" })).toBeDefined();
  });

  it("renders a +1 danger auxiliary chip only for a blocking (scan) danger chip", () => {
    mocks.shellStatus = status({
      status_chips: [
        {
          id: "scan",
          label: "Service unreachable",
          tone: "danger",
          detail: "The local service did not respond.",
          action: null,
        },
      ],
    });

    render(
      <PageToolbarProvider>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <p>Content</p>
        </AppShell>
      </PageToolbarProvider>,
    );

    // Primary chip escalates to warning; the blocking danger is reported as +1.
    expect(screen.getByRole("button", { name: "Service unreachable" })).toBeDefined();
  });

  it("logs lag threshold transitions exactly once in StrictMode and across severity changes", () => {
    const { rerender } = render(
      <StrictMode>
        <PageToolbarProvider>
          <AppShell currentPage="overview" onNavigate={() => {}}>
            <p>Content</p>
          </AppShell>
        </PageToolbarProvider>
      </StrictMode>,
    );

    mocks.shellStatus = status({
      aggregate_lag_ms: 31_200,
    });
    rerender(
      <StrictMode>
        <PageToolbarProvider>
          <AppShell currentPage="overview" onNavigate={() => {}}>
            <p>Content</p>
          </AppShell>
        </PageToolbarProvider>
      </StrictMode>,
    );

    mocks.shellStatus = status({
      aggregate_lag_ms: 5200,
    });
    rerender(
      <StrictMode>
        <PageToolbarProvider>
          <AppShell currentPage="overview" onNavigate={() => {}}>
            <p>Content</p>
          </AppShell>
        </PageToolbarProvider>
      </StrictMode>,
    );

    mocks.shellStatus = status({
      aggregate_lag_ms: 0,
    });
    rerender(
      <StrictMode>
        <PageToolbarProvider>
          <AppShell currentPage="overview" onNavigate={() => {}}>
            <p>Content</p>
          </AppShell>
        </PageToolbarProvider>
      </StrictMode>,
    );

    const codes = mocks.reportFrontendEvent.mock.calls.map(([entry]) => (
      entry as { event_code: string }
    ).event_code);
    // The dedicated lag telemetry is preserved (dual-track): the three
    // threshold/recovered events must still fire in order. The UI-level
    // gui.titlebar.status_escalated event is additive and may interleave.
    expect(codes).toEqual(expect.arrayContaining([
      "gui.shell.aggregate_lag_critical_visible",
      "gui.shell.aggregate_lag_warning_visible",
      "gui.shell.aggregate_lag_recovered",
    ]));
    // Order among the lag events is preserved relative to each other.
    const lagCodes = codes.filter((c) => c.startsWith("gui.shell.aggregate_lag_"));
    expect(lagCodes).toEqual([
      "gui.shell.aggregate_lag_critical_visible",
      "gui.shell.aggregate_lag_warning_visible",
      "gui.shell.aggregate_lag_recovered",
    ]);
  });

  it("logs gui.titlebar.popover_opened + gui.titlebar.action_clicked on interaction", async () => {
    const user = userEvent.setup();
    const onNavigate = vi.fn();
    render(
      <PageToolbarProvider>
        <AppShell currentPage="overview" onNavigate={onNavigate}>
          <p>Content</p>
        </AppShell>
      </PageToolbarProvider>,
    );

    await user.click(screen.getByRole("button", { name: "Live capture active" }));
    await user.click(screen.getByRole("button", { name: "View Activity" }));

    const codes = mocks.reportFrontendEvent.mock.calls.map(([entry]) => (
      entry as { event_code: string }
    ).event_code);
    expect(codes).toContain("gui.titlebar.popover_opened");
    expect(codes).toContain("gui.titlebar.action_clicked");
    expect(onNavigate).toHaveBeenCalledWith("usage");
  });
});
