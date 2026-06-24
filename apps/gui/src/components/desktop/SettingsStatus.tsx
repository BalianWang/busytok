type SettingsStatusTone = "ok" | "warning" | "danger" | "muted";
type SettingsStatusSize = "default" | "dense";

interface SettingsStatusProps {
  label: string;
  tone?: SettingsStatusTone;
  size?: SettingsStatusSize;
}

/**
 * Canonical status display for Settings control slots.
 * Distinct from StatusPill (which is for table/list pill badges).
 *
 * Visual rule: text + lightweight status dot for ok/warning/danger;
 * muted suppresses the dot. This is NOT a capsule/pill — if a pill is
 * needed, compose with StatusPill instead.
 */
export function SettingsStatus({
  label,
  tone = "ok",
  size = "default",
}: SettingsStatusProps) {
  return (
    <span className={`settings-status settings-status--${tone} settings-status--${size}`}>
      {tone !== "muted" ? (
        <span className="settings-status__dot" aria-hidden="true" />
      ) : null}
      <span>{label}</span>
    </span>
  );
}
