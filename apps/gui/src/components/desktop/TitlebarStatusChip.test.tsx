import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TitlebarStatusChip } from "./TitlebarStatusChip";
import type { TitlebarStatus } from "./titlebarStatus";

const mocks = vi.hoisted(() => ({
  reportFrontendEvent: vi.fn(),
}));

vi.mock("../../logging/reporter", () => ({
  reportFrontendEvent: (...args: unknown[]) => mocks.reportFrontendEvent(...args),
  safeReportEvent: (...args: unknown[]) => {
    const [event_code, message, details] = args as [string, string, Record<string, unknown>?];
    try {
      mocks.reportFrontendEvent({ level: "INFO", event_code, message, details });
    } catch {
      // Observability must not break the user action path.
    }
  },
}));

interface ReportEntry {
  level: string;
  event_code: string;
  message: string;
  details?: Record<string, unknown>;
}

function findCalls(code: string): ReportEntry[] {
  return mocks.reportFrontendEvent.mock.calls
    .map((call) => call[0] as ReportEntry)
    .filter((entry) => entry.event_code === code);
}

const healthy: TitlebarStatus = {
  tone: "neutral",
  label: "Live capture active",
  labelShort: "Capture active",
  dotToken: "var(--color-status-success)",
  sections: [
    { label: "SERVICE", rows: [{ label: "Readiness", value: "Ready" }] },
    { label: "LIVE", rows: [{ label: "Connection", value: "Connected" }] },
  ],
  actions: [{ label: "View Activity", action: "open_activity" }],
  auxiliary: undefined,
  reason: "healthy",
};

beforeEach(() => {
  mocks.reportFrontendEvent.mockReset();
});

afterEach(cleanup);

describe("TitlebarStatusChip", () => {
  it("renders the single calm chip with a success dot when healthy", () => {
    render(<TitlebarStatusChip status={healthy} onAction={() => {}} />);
    expect(screen.getByRole("button", { name: /Live capture active/ })).toBeDefined();
    expect(screen.queryByText(/Q:/)).toBeNull(); // no queue capsule
  });

  it("renders both the long and short label spans", () => {
    render(<TitlebarStatusChip status={healthy} onAction={() => {}} />);
    expect(screen.getByText("Live capture active")).toBeDefined();
    expect(screen.getByText("Capture active")).toBeDefined();
  });

  it("renders the auxiliary danger entry beside the primary when present", () => {
    const s: TitlebarStatus = { ...healthy, auxiliary: { label: "Service unreachable", tone: "danger", detail: null } };
    render(<TitlebarStatusChip status={s} onAction={() => {}} />);
    expect(screen.getByRole("button", { name: /Service unreachable/ })).toBeDefined();
  });

  it("logs gui.titlebar.popover_opened when the primary popover opens", async () => {
    const user = userEvent.setup();
    render(<TitlebarStatusChip status={healthy} onAction={() => {}} />);
    await user.click(screen.getByRole("button", { name: /Live capture active/ }));

    const opened = findCalls("gui.titlebar.popover_opened");
    expect(opened).toHaveLength(1);
    expect(opened[0].details?.tone).toBe("neutral");
  });

  it("logs gui.titlebar.popover_opened when the auxiliary popover opens", async () => {
    const user = userEvent.setup();
    const s: TitlebarStatus = { ...healthy, auxiliary: { label: "Service unreachable", tone: "danger", detail: null } };
    render(<TitlebarStatusChip status={s} onAction={() => {}} />);
    await user.click(screen.getByRole("button", { name: /Service unreachable/ }));

    const opened = findCalls("gui.titlebar.popover_opened");
    expect(opened.length).toBeGreaterThanOrEqual(1);
    expect(opened.some((e) => e.details?.tone === "danger")).toBe(true);
  });

  it("logs gui.titlebar.action_clicked before navigating", async () => {
    const user = userEvent.setup();
    const onAction = vi.fn();
    render(<TitlebarStatusChip status={healthy} onAction={onAction} />);
    await user.click(screen.getByRole("button", { name: /Live capture active/ }));
    await user.click(screen.getByRole("button", { name: "View Activity" }));

    const clicked = findCalls("gui.titlebar.action_clicked");
    expect(clicked).toHaveLength(1);
    expect(clicked[0].details).toEqual({ action: "open_activity", page: "usage" });
    expect(onAction).toHaveBeenCalledWith("usage");
  });
});
