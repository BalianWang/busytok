//! OverviewHeatmapPanel — token activity calendar heatmap powered by
//! useOverviewHeatmap.
//!
//! Owns the useOverviewHeatmap hook and renders the existing
//! OverviewTokenHeatmap component.  Always uses a 12-month window
//! independent of the selected range.

import { useEffect, useMemo, useRef } from "react";
import { useOverviewHeatmap } from "../../api/useBusytokData";
import { OverviewTokenHeatmap } from "./OverviewTokenHeatmap";
import {
  buildOverviewHeatmapModel,
  diagnosticsSignature,
  summarizeHeatmapForDiagnostics,
} from "../../lib/heatmap";
import { safeReportEvent } from "../../logging/reporter";
import type { RangePresetDto } from "@busytok/protocol-types";

interface OverviewHeatmapPanelProps {
  range: RangePresetDto;
}

export function OverviewHeatmapPanel({ range }: OverviewHeatmapPanelProps) {
  const { data: envelope, isLoading, isError } = useOverviewHeatmap(range);

  // Hook must be called before any early return so the call order is stable.
  const heatmapDays = envelope?.data.heatmap;
  const heatmapModel = useMemo(
    () => (heatmapDays ? buildOverviewHeatmapModel(heatmapDays) : null),
    [heatmapDays],
  );

  // Observability: emit the model's intensity distribution so "user can't see
  // activity" reports can be triaged (sparse vs all-zero vs fully binned).
  // Guarded by a signature so an identical distribution from a refetch (window
  // focus / reconnect hand back new object refs) does not re-emit. The ref
  // resets on unmount, so returning to the Overview surface always emits at
  // least once per visit. Fire-and-forget; never blocks rendering.
  const lastDiagnosticsSig = useRef<string | null>(null);
  useEffect(() => {
    if (!heatmapModel) return;
    const diag = summarizeHeatmapForDiagnostics(heatmapModel);
    const sig = diagnosticsSignature(diag);
    if (sig === lastDiagnosticsSig.current) return;
    lastDiagnosticsSig.current = sig;
    safeReportEvent(
      "gui.overview.heatmap_model_built",
      "Heatmap model built",
      diag,
    );
  }, [heatmapModel]);

  // ── Error ────────────────────────────────────────────────────────────
  if (isError) {
    return (
      <section className="overview-panel overview-panel--error" aria-label="Token activity heatmap">
        <p className="overview-panel__error">Heatmap unavailable</p>
      </section>
    );
  }

  // ── Loading ──────────────────────────────────────────────────────────
  if (isLoading || !envelope) {
    return (
      <section className="overview-panel overview-panel--loading" aria-label="Token activity heatmap">
        <div className="overview-panel__placeholder">
          <span className="overview-panel__spinner" />
          <p>Loading heatmap...</p>
        </div>
      </section>
    );
  }

  if (!heatmapModel) {
    return (
      <section className="overview-panel overview-panel--empty" aria-label="Token activity heatmap">
        <p>No heatmap data available.</p>
      </section>
    );
  }

  return (
    <section className="overview-console__heatmap" aria-label="Token activity heatmap">
      <OverviewTokenHeatmap model={heatmapModel} />
    </section>
  );
}
