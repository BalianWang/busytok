import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Search } from "lucide-react";
import type { PromptActionDto, PromptEntryDto } from "@busytok/protocol-types";
import { nativeTextAssistDisabledProps } from "../../lib/nativeTextAssist";
import {
  promptActionLabel,
  promptDisplayLabel,
  promptDisplayTitle,
} from "../../lib/promptPaletteText";
import { PROMPT_ACTION_ERROR_MESSAGE } from "../../lib/promptPaletteActions";

export interface PromptPaletteOverlayProps {
  open: boolean;
  entries: PromptEntryDto[];
  isLoading?: boolean;
  error?: string | null;
  query: string;
  onQueryChange: (query: string) => void;
  onClose: () => void;
  onExecute: (entry: PromptEntryDto, action: PromptActionDto) => void | Promise<void>;
  onOpenPage: () => void;
  onCreateNew: () => void;
  defaultAction: PromptActionDto;
  onEdit?: (entry: PromptEntryDto) => void;
  onTogglePin?: (entry: PromptEntryDto) => void;
  onDelete?: (entry: PromptEntryDto) => void;
  statusMessage?: string | null;
  presentation?: "overlay" | "window";
}

function focusableElements(root: HTMLElement) {
  return Array.from(
    root.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
    ),
  ).filter((element) => element.getAttribute("aria-disabled") !== "true");
}

function MenuItem({
  children,
  disabled,
  onSelect,
}: {
  children: ReactNode;
  disabled?: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      role="menuitem"
      className="prompt-overlay__menu-item"
      aria-disabled={disabled ? "true" : undefined}
      onClick={() => {
        if (!disabled) {
          onSelect();
        }
      }}
    >
      {children}
    </button>
  );
}

export function PromptPaletteOverlay({
  open,
  entries,
  isLoading = false,
  error = null,
  query,
  onQueryChange,
  onClose,
  onExecute,
  onOpenPage,
  onCreateNew,
  defaultAction,
  onEdit,
  onTogglePin,
  onDelete,
  statusMessage,
  presentation = "overlay",
}: PromptPaletteOverlayProps) {
  const searchRef = useRef<HTMLInputElement>(null);
  const surfaceRef = useRef<HTMLElement>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [actionsOpen, setActionsOpen] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  // TanStack Query keeps stale `data` on refetch failure, so entries can be
  // non-empty while `error` is also set. The hint footer targets the visible
  // list — hide it whenever the list is replaced by an error or loading state.
  const hasResults = entries.length > 0 && !error && !isLoading;

  useEffect(() => {
    if (open) {
      const activeElement = document.activeElement;
      restoreFocusRef.current =
        activeElement instanceof HTMLElement ? activeElement : null;
      setSelectedIndex(0);
      setActionsOpen(false);
      setActionError(null);
      const timer = window.setTimeout(() => searchRef.current?.focus(), 0);
      return () => {
        window.clearTimeout(timer);
        const restoreTarget = restoreFocusRef.current;
        restoreFocusRef.current = null;
        if (restoreTarget && document.contains(restoreTarget)) {
          restoreTarget.focus();
        }
      };
    }
    return undefined;
  }, [open]);

  useEffect(() => {
    setSelectedIndex((current) => {
      if (entries.length === 0) {
        return 0;
      }
      return Math.min(current, entries.length - 1);
    });
  }, [entries.length]);

  useEffect(() => {
    setSelectedIndex(0);
    setActionsOpen(false);
  }, [entries, query]);

  const selectedEntry = entries[selectedIndex] ?? null;
  const visibleStatusMessage = statusMessage ?? actionError;

  const emptyTitle = useMemo(() => {
    if (query.trim().length > 0) {
      return "No matches";
    }
    return "No saved prompts";
  }, [query]);

  if (!open) {
    return null;
  }

  function executeEntry(entry: PromptEntryDto, action: PromptActionDto) {
    setActionError(null);
    setActionsOpen(false);
    // Controller contract: onExecute never rejects. Catch is defensive against contract drift.
    void Promise.resolve(onExecute(entry, action)).catch(() => {
      setActionError(PROMPT_ACTION_ERROR_MESSAGE);
    });
  }

  function executeSelected(action: PromptActionDto) {
    if (selectedEntry) {
      executeEntry(selectedEntry, action);
    }
  }

  function trapFocus(event: React.KeyboardEvent) {
    const surface = surfaceRef.current;
    if (!surface) {
      return;
    }

    const focusables = focusableElements(surface);
    if (focusables.length === 0) {
      event.preventDefault();
      surface.focus();
      return;
    }

    const first = focusables[0];
    const last = focusables[focusables.length - 1];
    const active = document.activeElement;

    if (event.shiftKey && active === first) {
      event.preventDefault();
      last.focus();
      return;
    }

    if (!event.shiftKey && active === last) {
      event.preventDefault();
      first.focus();
      return;
    }

    if (!surface.contains(active)) {
      event.preventDefault();
      first.focus();
    }
  }

  function handleKeyDown(event: React.KeyboardEvent) {
    const isModifierPressed = event.metaKey || event.ctrlKey;

    if (event.key === "Tab") {
      trapFocus(event);
      return;
    }

    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }

    if (isModifierPressed && event.key.toLowerCase() === "n") {
      event.preventDefault();
      onCreateNew();
      return;
    }

    if (!selectedEntry) {
      return;
    }

    if (event.key === "ArrowDown") {
      event.preventDefault();
      setSelectedIndex((current) => Math.min(current + 1, entries.length - 1));
      setActionsOpen(false);
      return;
    }

    if (event.key === "ArrowUp") {
      event.preventDefault();
      setSelectedIndex((current) => Math.max(current - 1, 0));
      setActionsOpen(false);
      return;
    }

    if (event.key === "Enter" && isModifierPressed) {
      event.preventDefault();
      executeSelected("paste");
      return;
    }

    if (event.key === "Enter") {
      event.preventDefault();
      executeSelected(defaultAction);
      return;
    }

    if (isModifierPressed && event.key.toLowerCase() === "c") {
      event.preventDefault();
      executeSelected("copy");
      return;
    }

    if (isModifierPressed && event.key.toLowerCase() === "k") {
      event.preventDefault();
      setActionsOpen(true);
    }
  }

  const surface = (
    <section
      ref={surfaceRef}
      className={`prompt-overlay__surface${
        presentation === "window" ? " prompt-overlay__surface--window" : ""
      }`}
      role="dialog"
      aria-modal="true"
      aria-label="Prompt Palette"
      tabIndex={-1}
    >
        <header className="prompt-overlay__header">
          <label className="prompt-overlay__search">
            <Search aria-hidden="true" className="prompt-overlay__search-icon" size={22} />
            <input
              ref={searchRef}
              {...nativeTextAssistDisabledProps}
              type="search"
              aria-label="Prompt Palette search"
              value={query}
              onChange={(event) => onQueryChange(event.target.value)}
              placeholder="Search prompts"
            />
          </label>
          <button
            type="button"
            className="prompt-overlay__close prompt-overlay__keycap"
            aria-label="Close Prompt Palette"
            onClick={onClose}
          >
            <span className="prompt-overlay__keycap-label" aria-hidden="true">
              Esc
            </span>
          </button>
        </header>
        <div className="prompt-overlay__divider" aria-hidden="true" />

        {visibleStatusMessage ? (
          <p className="prompt-overlay__status" role="status">
            {visibleStatusMessage}
          </p>
        ) : null}

        {error ? (
          <p className="prompt-overlay__state prompt-overlay__state--error">{error}</p>
        ) : isLoading ? (
          <p className="prompt-overlay__state">Loading prompts...</p>
        ) : entries.length === 0 ? (
          <div className="prompt-overlay__empty">
            <h3>{emptyTitle}</h3>
            <p>
              {query.trim().length > 0
                ? "Try a different search or adjust your tags."
                : "Saved prompts will appear here."}
            </p>
          </div>
        ) : (
          <div className="prompt-overlay__list" role="listbox" aria-label="Prompt results">
            {entries.map((entry, index) => {
              const selected = index === selectedIndex;
              return (
                <button
                  type="button"
                  key={entry.id}
                  role="option"
                  aria-selected={selected}
                  className={`prompt-overlay__row${selected ? " is-selected" : ""}`}
                  onFocus={() => setSelectedIndex(index)}
                  onPointerEnter={() => setSelectedIndex(index)}
                  onClick={() => executeEntry(entry, defaultAction)}
                >
                  <span className="prompt-overlay__row-main">
                    <span className="prompt-overlay__title">
                      {promptDisplayTitle(entry.alias, entry.content, 80)}
                    </span>
                  </span>
                  <span className="prompt-overlay__accessory">
                    {entry.is_pinned ? (
                      <span className="prompt-overlay__pin">Pinned</span>
                    ) : null}
                    {entry.tags.length > 0 ? (
                      <span className="prompt-overlay__tags">
                        {entry.tags.slice(0, 3).join(" • ")}
                        {entry.tags.length > 3 ? ` +${entry.tags.length - 3}` : ""}
                      </span>
                    ) : null}
                  </span>
                </button>
              );
            })}
          </div>
        )}

        {hasResults ? (
          <footer
            className="prompt-overlay__hints"
            role="region"
            aria-label="Keyboard shortcuts"
          >
            <span className="prompt-overlay__hint">
              <kbd className="prompt-overlay__keycap">
                <span className="prompt-overlay__keycap-label">↵</span>
              </kbd>
              <span className="prompt-overlay__hint-label">{promptActionLabel(defaultAction)}</span>
            </span>
            <span className="prompt-overlay__hint">
              <kbd className="prompt-overlay__keycap">
                <span className="prompt-overlay__keycap-label">⌘↵</span>
              </kbd>
              <span className="prompt-overlay__hint-label">Paste</span>
            </span>
            <span className="prompt-overlay__hint">
              <kbd className="prompt-overlay__keycap">
                <span className="prompt-overlay__keycap-label">⌘K</span>
              </kbd>
              <span className="prompt-overlay__hint-label">Actions</span>
            </span>
            <span className="prompt-overlay__hint">
              <kbd className="prompt-overlay__keycap">
                <span className="prompt-overlay__keycap-label">⌘N</span>
              </kbd>
              <span className="prompt-overlay__hint-label">New</span>
            </span>
          </footer>
        ) : null}

        {actionsOpen && selectedEntry ? (
          <div
            className="prompt-overlay__menu"
            role="menu"
            aria-label={`Actions for ${promptDisplayLabel(selectedEntry.alias, selectedEntry.content).slice(0, 120)}`}
          >
            <MenuItem onSelect={() => executeEntry(selectedEntry, "copy")}>
              {promptActionLabel("copy")}
            </MenuItem>
            <MenuItem onSelect={() => executeEntry(selectedEntry, "paste")}>
              {promptActionLabel("paste")}
            </MenuItem>
            <MenuItem
              disabled={!onEdit}
              onSelect={() => onEdit?.(selectedEntry)}
            >
              Edit
            </MenuItem>
            <MenuItem
              disabled={!onTogglePin}
              onSelect={() => onTogglePin?.(selectedEntry)}
            >
              {selectedEntry.is_pinned ? "Unpin" : "Pin"}
            </MenuItem>
            <MenuItem
              disabled={!onDelete}
              onSelect={() => onDelete?.(selectedEntry)}
            >
              Delete
            </MenuItem>
            <MenuItem onSelect={onOpenPage}>Open in Prompt Palette Page</MenuItem>
          </div>
        ) : null}
    </section>
  );

  if (presentation === "window") {
    return <div className="prompt-overlay__window-shell" onKeyDown={handleKeyDown}>{surface}</div>;
  }

  return createPortal(
    <div className="prompt-overlay" onKeyDown={handleKeyDown}>
      <div className="prompt-overlay__backdrop" onMouseDown={onClose} />
      {surface}
    </div>,
    document.body,
  );
}
