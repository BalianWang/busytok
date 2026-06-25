import { describe, it, expect } from "vitest";
import type { UpdaterStatus } from "../api/UpdaterProvider";
import { updatePanelView } from "./updatePanelView";

// updatePanelView derives the Updates-panel presentation (description text,
// button label, disabled flag, and whether the primary action applies vs
// re-checks) from the UpdaterStatus state machine. Centralizing it keeps the
// SettingsPage JSX free of nested ternaries and makes every state assertable.

describe("updatePanelView", () => {
  it("idle: prompts to check, enabled, re-checks", () => {
    expect(updatePanelView({ state: "idle" } satisfies UpdaterStatus)).toEqual({
      description: "Check for and install the latest version of Busytok.",
      buttonLabel: "Check for updates",
      buttonDisabled: false,
      appliesUpdate: false,
    });
  });

  it("checking: in-progress copy, disabled", () => {
    expect(updatePanelView({ state: "checking" } satisfies UpdaterStatus)).toEqual({
      description: "Checking for updates…",
      buttonLabel: "Checking…",
      buttonDisabled: true,
      appliesUpdate: false,
    });
  });

  it("up-to-date: reassuring copy that is DISTINCT from idle, re-check enabled", () => {
    const view = updatePanelView({ state: "up-to-date" } satisfies UpdaterStatus);
    expect(view.description).toBe("You're on the latest version of Busytok.");
    expect(view.buttonLabel).toBe("Up to date");
    expect(view.buttonDisabled).toBe(false);
    expect(view.appliesUpdate).toBe(false);
    // Regression guard for the original bug: the up-to-date state must not
    // collapse onto the idle placeholder description (which made the panel look
    // identical before/after an upgrade).
    expect(view.description).not.toBe("Check for and install the latest version of Busytok.");
  });

  it("available: names the new version; button applies the update", () => {
    expect(
      updatePanelView({ state: "available", version: "0.3.0", notes: "n", date: "d" } satisfies UpdaterStatus),
    ).toEqual({
      description: "v0.3.0 is available.",
      buttonLabel: "Update now",
      buttonDisabled: false,
      appliesUpdate: true,
    });
  });

  it("downloading without percent", () => {
    const view = updatePanelView({ state: "downloading", percent: null } satisfies UpdaterStatus);
    expect(view.description).toBe("Downloading update…");
    expect(view.buttonDisabled).toBe(true);
  });

  it("downloading with percent", () => {
    const view = updatePanelView({ state: "downloading", percent: 42 } satisfies UpdaterStatus);
    expect(view.description).toBe("Downloading update… 42%");
    expect(view.buttonDisabled).toBe(true);
  });

  it("installed-pending-restart: restarting copy, disabled", () => {
    const view = updatePanelView({ state: "installed-pending-restart" } satisfies UpdaterStatus);
    expect(view.description).toBe("Update installed — restarting…");
    expect(view.buttonDisabled).toBe(true);
  });

  it("installed-needs-manual-restart: names version, not disabled", () => {
    const view = updatePanelView({ state: "installed-needs-manual-restart", version: "0.3.0" } satisfies UpdaterStatus);
    expect(view.description).toBe("Updated to v0.3.0 — please restart Busytok manually.");
    expect(view.buttonDisabled).toBe(false);
    expect(view.appliesUpdate).toBe(false);
  });

  it("error: surfaces the failure message; Retry re-checks", () => {
    const view = updatePanelView({ state: "error", message: "network down" } satisfies UpdaterStatus);
    expect(view.description).toBe("Update check failed: network down");
    expect(view.buttonLabel).toBe("Retry");
    expect(view.appliesUpdate).toBe(false);
  });
});
