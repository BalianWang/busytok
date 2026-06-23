import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TitlebarStatusChip } from "./TitlebarStatusChip";
import type { TitlebarStatus } from "./titlebarStatus";

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

afterEach(cleanup);

describe("TitlebarStatusChip", () => {
  it("renders the single calm chip with a success dot when healthy", () => {
    render(<TitlebarStatusChip status={healthy} onAction={() => {}} />);
    expect(screen.getByRole("button", { name: /Live capture active/ })).toBeDefined();
    expect(screen.queryByText(/Q:/)).toBeNull(); // no queue capsule
  });

  it("renders the auxiliary danger entry beside the primary when present", () => {
    const s: TitlebarStatus = { ...healthy, auxiliary: { label: "Service unreachable", tone: "danger", detail: null } };
    render(<TitlebarStatusChip status={s} onAction={() => {}} />);
    expect(screen.getByRole("button", { name: /Service unreachable/ })).toBeDefined();
  });
});
