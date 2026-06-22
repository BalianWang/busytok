import * as Popover from "@radix-ui/react-popover";
import type { StatusChipDto, StatusActionDto } from "@busytok/protocol-types";
import type { DesktopPage } from "../AppShell";

interface StatusChipProps {
  model: StatusChipDto;
  onAction?: (page: DesktopPage) => void;
}

/** Maps a StatusActionDto to a DesktopPage for navigation. */
function actionToPage(action: StatusActionDto): DesktopPage | undefined {
  switch (action) {
    case "open_activity":
      return "usage";
    case "open_settings":
      return "settings";
    default:
      return undefined;
  }
}

export function StatusChip({ model, onAction }: StatusChipProps) {
  const hasPopover = model.detail != null || model.action != null;

  if (!hasPopover) {
    return (
      <span className={`status-chip status-chip--${model.tone}`} aria-label={model.label}>
        <span className="status-chip__dot" aria-hidden="true" />
        <span>{model.label}</span>
      </span>
    );
  }

  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button
          type="button"
          className={`status-chip status-chip--${model.tone}`}
          aria-label={model.label}
        >
          <span className="status-chip__dot" aria-hidden="true" />
          <span>{model.label}</span>
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content className="status-popover" sideOffset={8} align="start">
          <p className="status-popover__title">{model.label}</p>
          {model.detail ? (
            <p className="status-popover__detail">{model.detail}</p>
          ) : null}
          {model.action && onAction ? (
            (() => {
              const targetPage = actionToPage(model.action);
              return targetPage ? (
                <button
                  type="button"
                  className="desktop-button desktop-button--small"
                  onClick={() => onAction(targetPage)}
                >
                  {model.action === "open_activity"
                    ? "View Activity"
                    : "Open Settings"}
                </button>
              ) : null;
            })()
          ) : null}
          <Popover.Arrow className="status-popover__arrow" />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
