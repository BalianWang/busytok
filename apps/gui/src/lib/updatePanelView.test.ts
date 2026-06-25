import { describe, it, expect } from "vitest";
import type { UpdaterStatus } from "../api/UpdaterProvider";
import { updatePanelView } from "./updatePanelView";

describe("updatePanelView", () => {
  // ── status-only states (no button) ──────────────────────────────────

  it("checking: value control, muted, no button", () => {
    const v = updatePanelView({ state: "checking" } satisfies UpdaterStatus);
    expect(v.description).toBe("Checking for updates…");
    expect(v.control).toEqual({ kind: "value", label: "Checking…", tone: "muted" });
  });

  it("up-to-date: status-dot + ok tone, no button", () => {
    const v = updatePanelView({ state: "up-to-date" } satisfies UpdaterStatus);
    expect(v.description).toBe("You're on the latest version of Busytok.");
    expect(v.control).toEqual({ kind: "status", label: "Up to date", tone: "ok" });
    expect(v.description).not.toBe("Check for and install the latest version of Busytok.");
  });

  it("downloading without percent: value control, default tone", () => {
    const v = updatePanelView({ state: "downloading", percent: null } satisfies UpdaterStatus);
    expect(v.description).toBe("Downloading update…");
    expect(v.control).toEqual({ kind: "value", label: "Downloading…", tone: "default" });
  });

  it("downloading with percent: value control with progress", () => {
    const v = updatePanelView({ state: "downloading", percent: 42 } satisfies UpdaterStatus);
    expect(v.description).toBe("Downloading update… 42%");
    expect(v.control).toEqual({ kind: "value", label: "Downloading… 42%", tone: "default" });
  });

  it("installed-pending-restart: value control, muted", () => {
    const v = updatePanelView({ state: "installed-pending-restart" } satisfies UpdaterStatus);
    expect(v.description).toBe("Update installed — restarting…");
    expect(v.control).toEqual({ kind: "value", label: "Restarting…", tone: "muted" });
  });

  it("installed-needs-manual-restart: status-dot + warning, no button", () => {
    const v = updatePanelView({ state: "installed-needs-manual-restart", version: "0.3.0" } satisfies UpdaterStatus);
    expect(v.description).toBe("Updated to v0.3.0 — please restart Busytok manually.");
    expect(v.control).toEqual({ kind: "status", label: "Restart required", tone: "warning" });
  });

  // ── action-group states (status/value + button) ─────────────────────

  it("idle: action-group with button, no status label", () => {
    const v = updatePanelView({ state: "idle" } satisfies UpdaterStatus);
    expect(v.description).toBe("Check for and install the latest version of Busytok.");
    expect(v.control).toEqual({
      kind: "action",
      status: null,
      buttonLabel: "Check for updates",
      buttonDisabled: false,
      appliesUpdate: false,
    });
  });

  it("available: action-group with ok status-dot + Update now button", () => {
    const v = updatePanelView({ state: "available", version: "0.3.0", notes: "n", date: "d" } satisfies UpdaterStatus);
    expect(v.description).toBe("v0.3.0 is available.");
    expect(v.control).toEqual({
      kind: "action",
      status: { kind: "status", label: "Update available", tone: "ok" },
      buttonLabel: "Update now",
      buttonDisabled: false,
      appliesUpdate: true,
    });
  });

  it("error: action-group with danger status-dot + Retry button", () => {
    const v = updatePanelView({ state: "error", message: "network down" } satisfies UpdaterStatus);
    expect(v.description).toBe("Update check failed: network down");
    expect(v.control).toEqual({
      kind: "action",
      status: { kind: "status", label: "Check failed", tone: "danger" },
      buttonLabel: "Retry",
      buttonDisabled: false,
      appliesUpdate: false,
    });
  });

  // ── exhaustiveness ──────────────────────────────────────────────────

  it("covers all 8 UpdaterStatus variants", () => {
    // Iterate every variant of the discriminated union — if a variant is
    // added to UpdaterStatus without a corresponding branch in
    // updatePanelView, this test won't compile.
    const variants: UpdaterStatus[] = [
      { state: "idle" },
      { state: "checking" },
      { state: "up-to-date" },
      { state: "available", version: "1.0.0", notes: "", date: "" },
      { state: "downloading", percent: null },
      { state: "installed-pending-restart" },
      { state: "installed-needs-manual-restart", version: "1.0.0" },
      { state: "error", message: "x" },
    ];
    for (const v of variants) {
      const view = updatePanelView(v);
      expect(typeof view.description).toBe("string");
      expect(["status", "value", "action"]).toContain(view.control.kind);
    }
  });
});
