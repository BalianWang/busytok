//! titlebarStatus — pure view-model that collapses shell.status into ONE
//! escalatable titlebar status. This is the spec's "adapter, not a parallel
//! health state machine": all inputs come from shell.status; this module only
//! projects them into a single chip + read-only popover + optional +1 danger.

import type {
  ReadinessStateDto,
  StatusActionDto,
  StatusChipDto,
} from "@busytok/protocol-types";
import { aggregateLagSeverity } from "./aggregateLagStatus";

export type TitlebarTone = "neutral" | "warning" | "danger";

export interface TitlebarStatusRow {
  label: string;
  value: string;
}

export interface TitlebarStatusSection {
  label: string;
  rows: TitlebarStatusRow[];
}

export interface TitlebarStatusAction {
  label: string;
  action: StatusActionDto;
}

export interface TitlebarAuxiliary {
  label: string;
  tone: "danger";
  detail: string | null;
}

export interface TitlebarStatus {
  tone: TitlebarTone;
  label: string;
  labelShort: string;
  dotToken: string;
  /** Read-only popover sections. */
  sections: TitlebarStatusSection[];
  /** Existing read-only nav actions only (open_activity / open_settings). */
  actions: TitlebarStatusAction[];
  /** Optional +1 danger entry — only for blocking danger chips. */
  auxiliary: TitlebarAuxiliary | undefined;
  /** Human reason for telemetry. */
  reason: string;
}

export interface TitlebarStatusInput {
  readiness: ReadinessStateDto;
  statusChips: StatusChipDto[];
  connection: "connected" | "reconnecting" | "disconnected";
  queueDepth: number | null;
  aggregateLagMs: number | null;
  generatedAtMs: number | null;
}

const READINESS_LABEL: Record<ReadinessStateDto, string | null> = {
  ready_exact: null,
  ready_degraded: "Degraded",
  rebuilding: "Rebuilding",
  starting: "Starting",
};

function hasWarningChip(chips: StatusChipDto[]): StatusChipDto | undefined {
  return chips.find((c) => c.tone === "warning");
}

// Blocking-danger chip IDs — only these warrant the +1 auxiliary entry.
// "scan" with danger tone = service offline, the one blocking condition the
// backend emits today (supervisor.rs:1397). Explicit allowlist by id so a
// future non-blocking danger chip does NOT silently trigger +1 (spec: only
// service-down / permission / must-decide get +1; perceivable-non-blocking
// issues stay a single warning). Extend this set only when the backend adds a
// new genuinely-blocking danger chip.
const BLOCKING_DANGER_CHIP_IDS = new Set(["scan"]);

function blockingDangerChip(chips: StatusChipDto[]): StatusChipDto | undefined {
  return chips.find((c) => c.tone === "danger" && BLOCKING_DANGER_CHIP_IDS.has(c.id));
}

function formatLag(ms: number | null): string {
  if (ms == null) return "—";
  if (ms >= 1000) return `${(ms / 1000).toFixed(ms >= 10_000 ? 0 : 1)}s`;
  return `${ms}ms`;
}

/**
 * Derive the single titlebar status. Pure: same input → same output, no I/O.
 * Escalation precedence: blocking-danger auxiliary is reported SEPARATELY
 * (auxiliary) while the primary status reflects the consolidated tone.
 */
export function deriveTitlebarStatus(input: TitlebarStatusInput): TitlebarStatus {
  const readinessLabel = READINESS_LABEL[input.readiness];
  const warningChip = hasWarningChip(input.statusChips);
  const dangerChip = blockingDangerChip(input.statusChips);

  const reasons: string[] = [];
  if (readinessLabel) reasons.push(`readiness:${input.readiness}`);
  if (input.connection !== "connected") reasons.push(`connection:${input.connection}`);
  if (input.queueDepth != null && input.queueDepth > 0) reasons.push(`queue:${input.queueDepth}`);
  if (warningChip) reasons.push(`chip:${warningChip.id}`);

  const lagSeverity = aggregateLagSeverity(input.aggregateLagMs);
  if (lagSeverity != null) reasons.push(`lag:${input.aggregateLagMs}ms`);
  if (dangerChip) reasons.push(`blocking-danger:${dangerChip.id}`);

  // A blocking-danger chip (service-down) elevates the consolidated primary
  // tone to warning AND surfaces separately as the +1 danger auxiliary. The
  // primary stays "warning" (consolidated), the auxiliary carries the danger.
  const isWarning =
    readinessLabel != null ||
    input.connection !== "connected" ||
    (input.queueDepth != null && input.queueDepth > 0) ||
    warningChip != null ||
    lagSeverity != null ||
    dangerChip != null;

  const tone: TitlebarTone = isWarning ? "warning" : "neutral";
  const label = isWarning
    ? readinessLabel ?? (input.connection === "reconnecting" ? "Reconnecting…"
        : input.connection === "disconnected" ? "Disconnected"
        : (input.queueDepth != null && input.queueDepth > 0) ? "Backlog"
        : warningChip?.label ?? (lagSeverity != null ? "Lag elevated" : "Degraded"))
    : "Live capture active";

  const sections: TitlebarStatusSection[] = [
    {
      label: "SERVICE",
      rows: [
        { label: "Readiness", value: readinessLabel ?? "Ready" },
      ],
    },
    {
      label: "LIVE",
      rows: [
        { label: "Connection", value: connectionLabel(input.connection) },
        { label: "Queue depth", value: input.queueDepth != null ? String(input.queueDepth) : "—" },
        { label: "Aggregate lag", value: formatLag(input.aggregateLagMs) },
      ],
    },
  ];

  const actions: TitlebarStatusAction[] = [
    { label: "View Activity", action: "open_activity" },
    { label: "Open Settings", action: "open_settings" },
  ];

  const auxiliary: TitlebarAuxiliary | undefined = dangerChip
    ? { label: dangerChip.label, tone: "danger", detail: dangerChip.detail }
    : undefined;

  return {
    tone,
    label,
    labelShort: isWarning ? label : "Capture active",
    dotToken: tone === "neutral" ? "var(--color-status-success)" : "var(--color-status-warning)",
    sections,
    actions,
    auxiliary,
    reason: reasons.length > 0 ? reasons.join(",") : "healthy",
  };
}

function connectionLabel(c: TitlebarStatusInput["connection"]): string {
  if (c === "connected") return "Connected";
  if (c === "reconnecting") return "Reconnecting";
  return "Disconnected";
}
