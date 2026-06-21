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
  it.each([
    [
      "starting",
      "Starting",
      "Service is starting up. Recent data may be incomplete for a moment.",
    ],
    [
      "rebuilding",
      "Rebuilding",
      "Service is rebuilding aggregates. Data remains usable, but some totals may lag.",
    ],
    [
      "ready_degraded",
      "Degraded",
      "Service is running in degraded mode. Some data may be approximate.",
    ],
  ] as const)("renders %s readiness as a compact status chip", async (readiness, label, detail) => {
    const user = userEvent.setup();
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
    expect(screen.getByRole("status").textContent).toContain(detail);
    expect(document.querySelector(".desktop-progress-banner")).toBeNull();

    await user.click(chip);
    expect(await screen.findByText(detail, { selector: ".status-popover__detail" })).toBeDefined();
  });

  it("renders toolbar, queue depth, hides healthy lag, and shows reconnecting status", () => {
    mocks.connectionStatus = "reconnecting";
    mocks.shellStatus = status({
      writer_queue_depth: 7,
      aggregate_lag_ms: 900,
    });

    render(
      <PageToolbarProvider>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <ToolbarRegistrant />
        </AppShell>
      </PageToolbarProvider>,
    );

    expect(screen.getByText("Refresh now")).toBeDefined();
    expect(screen.getByText("Q:7")).toBeDefined();
    expect(screen.queryByText(/Lag/i)).toBeNull();
    expect(screen.getByText("⟳")).toBeDefined();
  });

  it("renders warning lag as a status chip and filters non-progress status chips", async () => {
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
        {
          id: "settings_alert",
          label: "Settings alert",
          tone: "warning",
          detail: "Check your settings for issues.",
          action: "open_settings",
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

    const lagChip = screen.getByRole("button", { name: "Lag 6.1s" });
    expect(lagChip).toBeDefined();
    expect(screen.getByText("⚠")).toBeDefined();
    expect(screen.queryByText("Hidden progress")).toBeNull();
    expect(screen.getByRole("button", { name: "Settings alert" })).toBeDefined();

    await user.click(lagChip);
    expect(
      await screen.findByText(
        "Processing delay is elevated. Recent totals may take a moment to catch up.",
        { selector: ".status-popover__detail" },
      ),
    ).toBeDefined();
  });

  it("renders critical lag as a danger status chip", async () => {
    const user = userEvent.setup();
    mocks.shellStatus = status({
      aggregate_lag_ms: 31_200,
    });

    render(
      <PageToolbarProvider>
        <AppShell currentPage="overview" onNavigate={() => {}}>
          <p>Content</p>
        </AppShell>
      </PageToolbarProvider>,
    );

    const lagChip = screen.getByRole("button", { name: "Lag 31.2s" });
    expect(lagChip.className).toContain("status-chip--danger");

    await user.click(lagChip);
    expect(
      await screen.findByText(
        "Processing delay is severely elevated. Recent totals may be noticeably behind.",
        { selector: ".status-popover__detail" },
      ),
    ).toBeDefined();
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

    expect(
      mocks.reportFrontendEvent.mock.calls.map(([entry]) => (
        entry as { event_code: string }
      ).event_code),
    ).toEqual([
      "gui.shell.aggregate_lag_critical_visible",
      "gui.shell.aggregate_lag_warning_visible",
      "gui.shell.aggregate_lag_recovered",
    ]);
  });
});
