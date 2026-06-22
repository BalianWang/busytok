//! AppShell — Surge desktop application shell.
//!
//! Renders the sidebar navigation, a titlebar with global status chips
//! fetched from shell.status, and a scrollable content slot.
//!
//! When shell.status reports degraded, rebuilding, or elevated aggregate lag,
//! compact status chips are rendered in the titlebar. Queue depth remains a
//! lightweight text indicator when work is actively queued.

import { useEffect, type ReactNode } from "react";
import type { ReadinessStateDto, StatusChipDto } from "@busytok/protocol-types";
import { useShellStatus } from "../api/useBusytokData";
import { useEventSubscription } from "../api/useEventSubscription";
import {
  aggregateLagStatusChip,
  syncAggregateLagTelemetry,
} from "./desktop/aggregateLagStatus";
import { Sidebar } from "./desktop/Sidebar";
import { StatusChip } from "./desktop/StatusChip";
import { usePageToolbar } from "./desktop/PageToolbarContext";

/** All routes in the Surge desktop shell. */
export type DesktopPage =
  | "overview"
  | "usage"
  | "prompt_palette"
  | "settings";

interface AppShellProps {
  currentPage: DesktopPage;
  onNavigate: (page: DesktopPage) => void;
  children: ReactNode;
}

const VISUALLY_HIDDEN_STYLE = {
  position: "absolute" as const,
  width: "1px",
  height: "1px",
  padding: 0,
  margin: "-1px",
  overflow: "hidden",
  clip: "rect(0, 0, 0, 0)",
  whiteSpace: "nowrap" as const,
  border: 0,
};

function readinessChip(readiness: ReadinessStateDto): StatusChipDto | null {
  switch (readiness) {
    case "starting":
      return {
        id: "readiness_starting",
        label: "Starting",
        tone: "warning",
        detail: "Service is starting up. Recent data may be incomplete for a moment.",
        action: null,
      };
    case "rebuilding":
      return {
        id: "readiness_rebuilding",
        label: "Rebuilding",
        tone: "warning",
        detail: "Service is rebuilding aggregates. Data remains usable, but some totals may lag.",
        action: null,
      };
    case "ready_degraded":
      return {
        id: "readiness_degraded",
        label: "Degraded",
        tone: "warning",
        detail: "Service is running in degraded mode. Some data may be approximate.",
        action: null,
      };
    default:
      return null;
  }
}

export function AppShell({ currentPage, onNavigate, children }: AppShellProps) {
  const { data: shellStatus } = useShellStatus();
  const { connectionStatus } = useEventSubscription();
  const toolbarContext = usePageToolbar();

  // Derive display values from shell status
  const readiness: ReadinessStateDto = shellStatus?.readiness ?? "starting";
  const readinessStatusChip = readinessChip(readiness);
  const readinessAnnouncement = readinessStatusChip?.detail ?? null;
  const queueDepth = shellStatus?.writer_queue_depth ?? null;
  const aggregateLagMs = shellStatus?.aggregate_lag_ms ?? null;
  const aggregateLagChip = aggregateLagStatusChip(aggregateLagMs);

  useEffect(() => {
    syncAggregateLagTelemetry(aggregateLagMs);
  }, [aggregateLagMs]);

  return (
    <div className="desktop-shell">
      <Sidebar currentPage={currentPage} onNavigate={onNavigate} />
      <main className="desktop-workspace">
        <div className="desktop-titlebar" data-tauri-drag-region>
          <div className="desktop-titlebar__status">
            {readinessAnnouncement ? (
              <p role="status" aria-live="polite" aria-atomic="true" style={VISUALLY_HIDDEN_STYLE}>
                {readinessAnnouncement}
              </p>
            ) : null}
            {readinessStatusChip ? (
              <StatusChip model={readinessStatusChip} onAction={onNavigate} />
            ) : null}

            {/* Status chips from shell */}
            {shellStatus?.status_chips
              ?.filter((chip) => chip.id !== "scan_progress")
              .map((chip) => (
                <StatusChip key={chip.id} model={chip} onAction={onNavigate} />
              ))}

            {/* Connection status */}
            {connectionStatus !== "connected" && (
              <span className="desktop-titlebar__conn-status" title="Live updates paused">
                {connectionStatus === "reconnecting" ? "⟳" : "⚠"}
              </span>
            )}

            {/* Queue depth — show when non-null */}
            {queueDepth != null && queueDepth > 0 && (
              <span
                className="desktop-titlebar__queue-depth"
                title={`Writer queue depth: ${queueDepth}`}
              >
                Q:{queueDepth}
              </span>
            )}

            {aggregateLagChip ? (
              <StatusChip model={aggregateLagChip} onAction={onNavigate} />
            ) : null}
            {toolbarContext?.toolbar ? (
              <div className="desktop-titlebar__toolbar">{toolbarContext.toolbar}</div>
            ) : null}
          </div>
        </div>

        <section className="desktop-workspace__content">{children}</section>
      </main>
    </div>
  );
}
