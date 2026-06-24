import type { ReactNode } from "react";

interface SettingsActionGroupProps {
  children: ReactNode;
  direction?: "col" | "row";
}

/**
 * Canonical composite control container — value + action, status + action,
 * or read-only value + link/button. Replaces the page-private
 * `manual-root-controls` div pattern.
 *
 * Layout: `col` (stacked, for error+retry patterns) or `row` (inline
 * value+action). Default: `col`.
 */
export function SettingsActionGroup({
  children,
  direction = "col",
}: SettingsActionGroupProps) {
  return (
    <div className={`settings-action-group settings-action-group--${direction}`}>
      {children}
    </div>
  );
}
