import type { UpdaterStatus } from "../api/UpdaterProvider";

/**
 * Presentation derived from an {@link UpdaterStatus} for the Settings → Updates
 * panel. Centralizing this keeps the JSX free of nested ternaries and makes
 * every state machine branch unit-testable.
 *
 * - `description`: the human-readable status line shown under "Software Update".
 * - `buttonLabel` / `buttonDisabled`: the primary action button.
 * - `appliesUpdate`: when true (state `available`) the button installs the
 *   update; otherwise it (re-)checks for one.
 */
export interface UpdatePanelView {
  description: string;
  buttonLabel: string;
  buttonDisabled: boolean;
  appliesUpdate: boolean;
}

/**
 * Map an update status to its panel view. Exhaustive over {@link UpdaterStatus}
 * — every state gets a distinct, intentional description (notably `up-to-date`,
 * which previously reused the idle placeholder and made the panel look unchanged
 * before/after an upgrade).
 */
export function updatePanelView(status: UpdaterStatus): UpdatePanelView {
  switch (status.state) {
    case "downloading":
      return {
        description: status.percent == null ? "Downloading update…" : `Downloading update… ${status.percent}%`,
        buttonLabel: "Check for updates",
        buttonDisabled: true,
        appliesUpdate: false,
      };
    case "installed-pending-restart":
      return {
        description: "Update installed — restarting…",
        buttonLabel: "Check for updates",
        buttonDisabled: true,
        appliesUpdate: false,
      };
    case "installed-needs-manual-restart":
      return {
        description: `Updated to v${status.version} — please restart Busytok manually.`,
        buttonLabel: "Check for updates",
        buttonDisabled: false,
        appliesUpdate: false,
      };
    case "error":
      return {
        description: `Update check failed: ${status.message}`,
        buttonLabel: "Retry",
        buttonDisabled: false,
        appliesUpdate: false,
      };
    case "available":
      return {
        description: `v${status.version} is available.`,
        buttonLabel: "Update now",
        buttonDisabled: false,
        appliesUpdate: true,
      };
    case "checking":
      return {
        description: "Checking for updates…",
        buttonLabel: "Checking…",
        buttonDisabled: true,
        appliesUpdate: false,
      };
    case "up-to-date":
      return {
        description: "You're on the latest version of Busytok.",
        buttonLabel: "Up to date",
        buttonDisabled: false,
        appliesUpdate: false,
      };
    case "idle":
      return {
        description: "Check for and install the latest version of Busytok.",
        buttonLabel: "Check for updates",
        buttonDisabled: false,
        appliesUpdate: false,
      };
  }
}
