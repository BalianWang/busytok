import type { UpdaterStatus } from "../api/UpdaterProvider";

/**
 * Status-dot control (SettingsStatus): dot + label with an ok/warning/danger
 * or muted (no-dot) tone. Used for read-only update states: up-to-date,
 * installed-needs-manual-restart.
 */
interface StatusControl {
  kind: "status";
  label: string;
  tone: "ok" | "warning" | "danger" | "muted";
}

/**
 * Plain-value control (SettingsValue): text-only readout with a default,
 * muted, warning, or danger tone. Used for in-progress states: checking,
 * downloading, installed-pending-restart.
 */
interface ValueControl {
  kind: "value";
  label: string;
  tone: "default" | "muted" | "warning" | "danger";
}

/**
 * Action-group control (SettingsActionGroup): an optional status or value
 * label paired with a small secondary button. Used for states that offer a
 * real user action: idle (check), available (install), error (retry).
 */
interface ActionControl {
  kind: "action";
  status: StatusControl | ValueControl | null;
  buttonLabel: string;
  buttonDisabled: boolean;
  appliesUpdate: boolean;
}

/**
 * Presentation derived from an {@link UpdaterStatus} for the Settings →
 * Updates panel. Centralizing this keeps the JSX terse and makes every
 * state machine branch unit-testable.
 *
 * The `control` field is a discriminated union — SettingsPage switches on
 * `kind` with full exhaustiveness checking and zero `as` casts. This
 * eliminates the risk of returning an invalid tone for a given component
 * (e.g. `controlHasDot + tone=default`), which the previous flat-object
 * interface could not prevent.
 */
export interface UpdatePanelView {
  description: string;
  control: StatusControl | ValueControl | ActionControl;
}

/**
 * Map an update status to its panel view. Exhaustive over
 * {@link UpdaterStatus}.
 *
 * Design rationale (per gui-control-system-convergence spec §Settings
 * controls): the Settings page's control slot conveys state or action —
 * never a faux button pretending to be a read-out. States that offer no
 * real user action render `SettingsStatus` or `SettingsValue`. States
 * with a clickable action render `SettingsActionGroup` with the canonical
 * small-secondary-button pattern.
 */
export function updatePanelView(status: UpdaterStatus): UpdatePanelView {
  const ctrl = controlFor(status);
  return { description: descriptionFor(status), control: ctrl };
}

// ─── per-field helpers (each pure + testable via the public function) ──────

function descriptionFor(s: UpdaterStatus): string {
  switch (s.state) {
    case "downloading":
      return s.percent == null ? "Downloading update…" : `Downloading update… ${s.percent}%`;
    case "installed-pending-restart":
      return "Update installed — restarting…";
    case "installed-needs-manual-restart":
      return `Updated to v${s.version} — please restart Busytok manually.`;
    case "error":
      return `Update check failed: ${s.message}`;
    case "available":
      return `v${s.version} is available.`;
    case "checking":
      return "Checking for updates…";
    case "up-to-date":
      return "You're on the latest version of Busytok.";
    case "idle":
      return "Check for and install the latest version of Busytok.";
  }
}

function controlFor(s: UpdaterStatus): UpdatePanelView["control"] {
  switch (s.state) {
    case "downloading": {
      const label = s.percent == null ? "Downloading…" : `Downloading… ${s.percent}%`;
      return { kind: "value", label, tone: "default" } satisfies ValueControl;
    }
    case "installed-pending-restart":
      return { kind: "value", label: "Restarting…", tone: "muted" } satisfies ValueControl;
    case "installed-needs-manual-restart":
      return { kind: "status", label: "Restart required", tone: "warning" } satisfies StatusControl;
    case "error":
      return {
        kind: "action",
        status: { kind: "status", label: "Check failed", tone: "danger" } satisfies StatusControl,
        buttonLabel: "Retry",
        buttonDisabled: false,
        appliesUpdate: false,
      } satisfies ActionControl;
    case "available":
      return {
        kind: "action",
        status: { kind: "status", label: "Update available", tone: "ok" } satisfies StatusControl,
        buttonLabel: "Update now",
        buttonDisabled: false,
        appliesUpdate: true,
      } satisfies ActionControl;
    case "checking":
      return { kind: "value", label: "Checking…", tone: "muted" } satisfies ValueControl;
    case "up-to-date":
      return { kind: "status", label: "Up to date", tone: "ok" } satisfies StatusControl;
    case "idle":
      return {
        kind: "action",
        status: null,
        buttonLabel: "Check for updates",
        buttonDisabled: false,
        appliesUpdate: false,
      } satisfies ActionControl;
  }
}
