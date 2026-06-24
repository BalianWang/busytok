interface ToggleSwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  "aria-label": string;
  size?: "default" | "dense";
  disabled?: boolean;
}

/**
 * Canonical boolean toggle — the project's single toggle switch.
 * Replaces the page-private `toggle-label` / `toggle` / `toggle-track`
 * pattern previously in SettingsPage + pages.css.
 *
 * This is a PURE SWITCH CONTROL. It carries NO visible text — the
 * caller (typically SettingsRow) already owns label and description
 * on the left side. Accessibility is via `aria-label`.
 */
export function ToggleSwitch({
  checked,
  onChange,
  "aria-label": ariaLabel,
  size = "default",
  disabled,
}: ToggleSwitchProps) {
  return (
    <label className={`toggle-switch toggle-switch--${size}`}>
      <input
        type="checkbox"
        className="toggle-switch__input"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        disabled={disabled}
        aria-label={ariaLabel}
      />
      <span className="toggle-switch__track" aria-hidden="true" />
    </label>
  );
}
