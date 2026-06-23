//! AppShell — Surge desktop application shell.
//!
//! Renders the sidebar navigation, a titlebar with ONE calm status chip
//! derived from shell.status via the titlebar view-model, and a scrollable
//! content slot. Escalations (warning/danger) are projected into the single
//! chip in place; a +1 danger auxiliary appears only for blocking issues.

import { useEffect, useRef, type ReactNode } from "react";
import { useShellStatus } from "../api/useBusytokData";
import { useEventSubscription } from "../api/useEventSubscription";
import { safeReportEvent } from "../logging/reporter";
import { syncAggregateLagTelemetry } from "./desktop/aggregateLagStatus";
import { Sidebar } from "./desktop/Sidebar";
import { TitlebarStatusChip } from "./desktop/TitlebarStatusChip";
import {
  deriveTitlebarStatus,
  type TitlebarTone,
} from "./desktop/titlebarStatus";
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

export function AppShell({ currentPage, onNavigate, children }: AppShellProps) {
  const { data: shellStatus } = useShellStatus();
  const { connectionStatus } = useEventSubscription();
  const toolbarContext = usePageToolbar();

  const aggregateLagMs = shellStatus?.aggregate_lag_ms ?? null;

  const status = deriveTitlebarStatus({
    readiness: shellStatus?.readiness ?? "starting",
    statusChips: shellStatus?.status_chips?.filter((c) => c.id !== "scan_progress") ?? [],
    connection: connectionStatus,
    queueDepth: shellStatus?.writer_queue_depth ?? null,
    aggregateLagMs,
    generatedAtMs: shellStatus?.generated_at_ms ?? null,
  });

  // Dual-track observability: the dedicated lag telemetry (below) emits the
  // 5s/30s threshold + recovered events with full semantics; the coarser
  // UI-level escalation event (further down) only records tone transitions.
  useEffect(() => {
    syncAggregateLagTelemetry(aggregateLagMs);
  }, [aggregateLagMs]);

  const prevToneRef = useRef<TitlebarTone | null>(null);
  useEffect(() => {
    const prev = prevToneRef.current;
    if (prev != null && prev !== status.tone) {
      safeReportEvent(
        "gui.titlebar.status_escalated",
        "Titlebar status tone changed",
        { from: prev, to: status.tone, reason: status.reason },
      );
    }
    prevToneRef.current = status.tone;
  }, [status.tone, status.reason]);

  return (
    <div className="desktop-shell">
      <Sidebar currentPage={currentPage} onNavigate={onNavigate} />
      <main className="desktop-workspace">
        <div className="desktop-titlebar" data-tauri-drag-region>
          <div className="desktop-titlebar__status">
            <TitlebarStatusChip status={status} onAction={onNavigate} />
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

