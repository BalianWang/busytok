import type { ReactElement } from "react";

/**
 * Reusable degraded-state banner. Renders a warning ribbon when the page's
 * data envelope is stale or approximate. Unifies the previously duplicated
 * `.overview-console__degraded-ribbon` (OverviewPage) and `.degraded-ribbon`
 * (SubagentsPage) markup into a single component (reviewer P2-3).
 */
export function DegradedRibbon({
  show,
  reason,
  isStale,
}: {
  show: boolean;
  reason: string | null;
  isStale: boolean;
}): ReactElement | null {
  if (!show) return null;
  const message =
    reason ??
    (isStale
      ? "Showing stale data — refresh in progress"
      : "Data is approximate — exact aggregates not yet available");
  return (
    <div className="degraded-ribbon" role="status">
      <span className="degraded-ribbon__dot" aria-hidden="true" />
      <span>{message}</span>
    </div>
  );
}
