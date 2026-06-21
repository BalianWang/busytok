import { useCallback, useEffect, useRef, useState } from "react";
import * as Popover from "@radix-ui/react-popover";
import { useSuggestTags } from "../api/useBusytokData";
import { nativeTextAssistDisabledProps } from "../lib/nativeTextAssist";

const DEBOUNCE_MS = 200;

interface TagFilterComboboxProps {
  appliedTag: string;
  onApplyTag: (tag: string) => void;
  onClear: () => void;
  placeholder?: string;
}

export function TagFilterCombobox({
  appliedTag,
  onApplyTag,
  onClear,
  placeholder = "Filter by tag",
}: TagFilterComboboxProps) {
  const [draftInput, setDraftInput] = useState(appliedTag);
  const [debouncedQuery, setDebouncedQuery] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [highlightIndex, setHighlightIndex] = useState<number | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout>>(undefined);

  // Sync draft input when appliedTag changes externally
  useEffect(() => {
    if (!open) {
      setDraftInput(appliedTag);
    }
  }, [appliedTag, open]);

  // Reset highlight when candidates change
  useEffect(() => {
    setHighlightIndex(null);
  }, [debouncedQuery]);

  // Debounce: update debouncedQuery 200ms after draftInput stops changing
  useEffect(() => {
    if (!open) return;
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      setDebouncedQuery(draftInput.trim());
    }, DEBOUNCE_MS);
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, [draftInput, open]);

  const { data } = useSuggestTags(open ? debouncedQuery : null);
  const candidates = data?.tags ?? [];

  const handleInputChange = useCallback((value: string) => {
    setDraftInput(value);
    setOpen(true);
  }, []);

  const handleSelect = useCallback(
    (tag: string) => {
      onApplyTag(tag);
      setDraftInput(tag);
      setOpen(false);
    },
    [onApplyTag],
  );

  const handleClear = useCallback(() => {
    setDraftInput("");
    onClear();
    setOpen(false);
    inputRef.current?.focus();
  }, [onClear]);

  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent) => {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        if (candidates.length === 0) return;
        setHighlightIndex((prev) => {
          if (prev === null) return 0;
          return (prev + 1) % candidates.length;
        });
      } else if (event.key === "ArrowUp") {
        event.preventDefault();
        if (candidates.length === 0) return;
        setHighlightIndex((prev) => {
          if (prev === null) return candidates.length - 1;
          return (prev - 1 + candidates.length) % candidates.length;
        });
      } else if (event.key === "Enter") {
        event.preventDefault();
        if (highlightIndex !== null && highlightIndex < candidates.length) {
          handleSelect(candidates[highlightIndex]);
        } else {
          setOpen(false);
        }
      } else if (event.key === "Escape") {
        event.preventDefault();
        setOpen(false);
      }
    },
    [candidates, highlightIndex, handleSelect],
  );

  const handleFocus = useCallback(() => {
    setOpen(true);
  }, []);

  // When no candidates are visible (popover not rendered), blur must
  // close `open` so the sync effect resets draftInput to appliedTag.
  const handleBlur = useCallback(() => {
    if (candidates.length === 0) {
      setOpen(false);
    }
  }, [candidates.length]);

  const isPopoverOpen = open && candidates.length > 0;

  return (
    <div className="tag-filter-combobox prompt-page__filter">
      <Popover.Root open={isPopoverOpen} onOpenChange={setOpen}>
        <Popover.Trigger asChild>
          <div className="tag-filter-combobox__input-wrap">
            <input
              ref={inputRef}
              {...nativeTextAssistDisabledProps}
              role="combobox"
              aria-label={placeholder}
              aria-expanded={isPopoverOpen}
              aria-autocomplete="list"
              value={draftInput}
              onChange={(e) => handleInputChange(e.target.value)}
              onFocus={handleFocus}
              onBlur={handleBlur}
              onKeyDown={handleKeyDown}
              placeholder={placeholder}
            />
            {appliedTag && (
              <button
                type="button"
                className="tag-filter-combobox__clear"
                aria-label="Clear tag filter"
                onClick={handleClear}
              >
                ×
              </button>
            )}
          </div>
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content
            className="app-select__content tag-filter-combobox__popover"
            sideOffset={4}
            align="start"
            onOpenAutoFocus={(e) => e.preventDefault()}
          >
              <div role="listbox" aria-label="Tag suggestions">
                {candidates.map((tag, index) => (
                  <div
                    key={tag}
                    role="option"
                    aria-selected={index === highlightIndex}
                    className={`app-select__item${index === highlightIndex ? " is-highlighted" : ""}`}
                    onClick={() => handleSelect(tag)}
                    onPointerEnter={() => setHighlightIndex(index)}
                  >
                    {tag}
                  </div>
                ))}
              </div>
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>
    </div>
  );
}
