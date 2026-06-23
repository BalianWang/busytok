//! TitlebarStatusChip — the ONE calm status entry. Healthy = neutral chip with
//! a success dot; escalates in place (warning/danger) per the view-model. The
//! popover is read-only (sections + existing nav actions). Click emits an
//! acknowledgement event for observability.

import * as Popover from "@radix-ui/react-popover";
import type { DesktopPage } from "../AppShell";
import type { TitlebarStatus } from "./titlebarStatus";
import { statusActionToPage } from "./statusAction";

interface TitlebarStatusChipProps {
  status: TitlebarStatus;
  onAction: (page: DesktopPage) => void;
}

export function TitlebarStatusChip({ status, onAction }: TitlebarStatusChipProps) {
  const toneClass = status.tone === "neutral" ? "is-neutral" : "is-warning";
  return (
    <>
      <Popover.Root>
        <Popover.Trigger asChild>
          <button
            type="button"
            className={`titlebar-chip ${toneClass}`}
            aria-label={status.label}
          >
            <span className="titlebar-chip__dot" style={{ background: status.dotToken }} aria-hidden="true" />
            <span className="titlebar-chip__label">{status.label}</span>
          </button>
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content className="titlebar-popover" sideOffset={8} align="start">
            {status.sections.map((section) => (
              <div key={section.label} className="titlebar-popover__section">
                <p className="titlebar-popover__section-label">{section.label}</p>
                <dl className="titlebar-popover__rows">
                  {section.rows.map((row) => (
                    <div key={row.label} className="titlebar-popover__row">
                      <dt>{row.label}</dt>
                      <dd>{row.value}</dd>
                    </div>
                  ))}
                </dl>
              </div>
            ))}
            {status.actions.length > 0 ? (
              <div className="titlebar-popover__actions">
                {status.actions.map((a) => {
                  const page = statusActionToPage(a.action);
                  if (!page) return null; // unknown action → no button, no navigation (no fallback)
                  return (
                    <button
                      key={a.action}
                      type="button"
                      className="desktop-button desktop-button--small desktop-button--secondary"
                      onClick={() => onAction(page)}
                    >
                      {a.label}
                    </button>
                  );
                })}
              </div>
            ) : null}
            <Popover.Arrow className="titlebar-popover__arrow" />
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>

      {status.auxiliary ? (
        <Popover.Root>
          <Popover.Trigger asChild>
            <button type="button" className="titlebar-chip is-danger" aria-label={status.auxiliary.label}>
              <span className="titlebar-chip__dot" style={{ background: "var(--color-status-danger)" }} aria-hidden="true" />
              <span className="titlebar-chip__label">{status.auxiliary.label}</span>
            </button>
          </Popover.Trigger>
          {status.auxiliary.detail ? (
            <Popover.Portal>
              <Popover.Content className="titlebar-popover" sideOffset={8}>
                <p className="titlebar-popover__detail">{status.auxiliary.detail}</p>
                <Popover.Arrow className="titlebar-popover__arrow" />
              </Popover.Content>
            </Popover.Portal>
          ) : null}
        </Popover.Root>
      ) : null}
    </>
  );
}
