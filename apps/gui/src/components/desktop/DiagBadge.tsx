type DiagBadgeTone = "ok" | "error";

interface DiagBadgeProps {
  tone: DiagBadgeTone;
  label: string;
}

/**
 * Canonical diagnostics badge — a capsule pill for binary status readouts
 * (healthy/unhealthy, skewed/synced). Used by SettingsPage diagnostics.
 *
 * Distinct from StatusPill (table/list pill) and SettingsStatus
 * (settings-row status with dot). Renders the `.diag-badge` CSS family
 * migrated from pages.css to components.css.
 */
export function DiagBadge({ tone, label }: DiagBadgeProps) {
  return <span className={`diag-badge diag-badge--${tone}`}>{label}</span>;
}
