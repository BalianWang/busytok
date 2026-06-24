type SettingsValueTone = "default" | "muted" | "warning" | "danger";
type SettingsValueSize = "default" | "dense";

interface SettingsValueProps {
  value: string;
  tone?: SettingsValueTone;
  size?: SettingsValueSize;
}

/**
 * Canonical read-only value for SettingsPage control slots.
 * Replaces the page-private `diag-value` CSS class pattern.
 *
 * Tones:
 * - default: primary text, standard read-only result
 * - muted:   secondary information, supplementary context
 * - warning: cautionary text, non-blocking
 * - danger:  failure or attention-needed text
 */
export function SettingsValue({
  value,
  tone = "default",
  size = "default",
}: SettingsValueProps) {
  return (
    <span className={`settings-value settings-value--${tone} settings-value--${size}`}>
      {value}
    </span>
  );
}
