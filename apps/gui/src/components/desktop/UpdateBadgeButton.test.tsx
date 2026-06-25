import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useUpdater } from "../../hooks/useUpdater";
import { UpdateBadgeButton } from "./UpdateBadgeButton";

vi.mock("../../hooks/useUpdater", () => ({ useUpdater: vi.fn() }));
const mocked = vi.mocked(useUpdater);

function setStatus(status: ReturnType<typeof useUpdater>["status"]) {
  mocked.mockReturnValue({ status, currentVersion: "0.0.2", checkNow: vi.fn(), applyNow: vi.fn() });
}

beforeEach(() => vi.clearAllMocks());
afterEach(() => cleanup());

describe("UpdateBadgeButton", () => {
  it("renders nothing when idle/up-to-date/checking/error", () => {
    setStatus({ state: "idle" });
    const { rerender } = render(<UpdateBadgeButton />);
    expect(screen.queryByRole("button")).toBeNull();
    setStatus({ state: "up-to-date" });
    rerender(<UpdateBadgeButton />);
    expect(screen.queryByRole("button")).toBeNull();
    setStatus({ state: "checking" });
    rerender(<UpdateBadgeButton />);
    expect(screen.queryByRole("button")).toBeNull();
    setStatus({ state: "error", message: "network timeout" });
    rerender(<UpdateBadgeButton />);
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("available: shows version in tooltip + applies on click", async () => {
    const applyNow = vi.fn();
    mocked.mockReturnValue({ status: { state: "available", version: "0.3.0", notes: "fixes", date: "d" }, currentVersion: "0.0.2", checkNow: vi.fn(), applyNow });
    render(<UpdateBadgeButton />);
    const btn = screen.getByRole("button");
    expect(btn.title).toContain("0.3.0");
    expect(btn.title).toContain("fixes");
    await userEvent.click(btn);
    expect(applyNow).toHaveBeenCalledTimes(1);
  });

  it("downloading: disabled, shows percent", () => {
    setStatus({ state: "downloading", percent: 42 });
    render(<UpdateBadgeButton />);
    const btn = screen.getByRole("button");
    expect(btn).toHaveProperty("disabled", true);
    expect(btn.textContent).toContain("42");
  });

  it("downloading with null percent: shows Updating…", () => {
    setStatus({ state: "downloading", percent: null });
    render(<UpdateBadgeButton />);
    expect(screen.getByRole("button").textContent).toMatch(/updating/i);
  });

  it("installed-needs-manual-restart: shows manual-restart text", () => {
    setStatus({ state: "installed-needs-manual-restart", version: "0.3.0" });
    render(<UpdateBadgeButton />);
    expect(screen.getByRole("status").textContent).toMatch(/restart.*manually/i);
  });

  it("installed-pending-restart: disabled Restarting… button", () => {
    setStatus({ state: "installed-pending-restart" });
    render(<UpdateBadgeButton />);
    const btn = screen.getByRole("button");
    expect(btn).toHaveProperty("disabled", true);
    expect(btn.textContent).toMatch(/restarting/i);
  });

  it("available with empty notes: tooltip shows fallback text", () => {
    mocked.mockReturnValue({
      status: { state: "available", version: "0.3.0", notes: "", date: "d" },
      currentVersion: "0.0.2",
      checkNow: vi.fn(),
      applyNow: vi.fn(),
    });
    render(<UpdateBadgeButton />);
    expect(screen.getByRole("button").title).toContain("(no release notes)");
  });
});
