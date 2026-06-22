import { useEffect, useRef, useState } from "react";
import type {
  PromptCreateRequestDto,
  PromptEntryDto,
} from "@busytok/protocol-types";
import { nativeTextAssistDisabledProps } from "../../lib/nativeTextAssist";
import { parsePromptTags } from "../../lib/promptPaletteText";

interface PromptEntryDialogProps {
  open: boolean;
  entry?: PromptEntryDto | null;
  isSubmitting?: boolean;
  onClose: () => void;
  onSubmit: (payload: PromptCreateRequestDto) => void | Promise<void>;
}

interface ValidationErrors {
  content?: string;
  alias?: string;
}

const ALIAS_FORBIDDEN_RE = /[\s"'`​-‍⁠﻿]/;

export function PromptEntryDialog({
  open,
  entry = null,
  isSubmitting = false,
  onClose,
  onSubmit,
}: PromptEntryDialogProps) {
  const [content, setContent] = useState("");
  const [alias, setAlias] = useState("");
  const [tags, setTags] = useState("");
  const [errors, setErrors] = useState<ValidationErrors>({});
  const dialogRef = useRef<HTMLElement | null>(null);
  const contentRef = useRef<HTMLTextAreaElement | null>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open) {
      return;
    }

    setContent(entry?.content ?? "");
    setAlias(entry?.alias ?? "");
    setTags(entry?.tags.join(", ") ?? "");
    setErrors({});
  }, [entry, open]);

  useEffect(() => {
    if (!open) {
      return;
    }

    restoreFocusRef.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    contentRef.current?.focus();

    return () => {
      const restoreTarget = restoreFocusRef.current;
      restoreFocusRef.current = null;
      if (restoreTarget?.isConnected) {
        restoreTarget.focus();
      }
    };
  }, [open]);

  if (!open) {
    return null;
  }

  function getDialogFocusables(): HTMLElement[] {
    return Array.from(
      dialogRef.current?.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
      ) ?? [],
    ).filter((element) => !element.hasAttribute("aria-hidden"));
  }

  function handleDialogKeyDown(event: React.KeyboardEvent<HTMLElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }

    if (event.key !== "Tab") {
      return;
    }

    const focusableElements = getDialogFocusables();
    if (focusableElements.length === 0) {
      event.preventDefault();
      return;
    }

    const firstElement = focusableElements[0];
    const lastElement = focusableElements[focusableElements.length - 1];

    if (event.shiftKey && document.activeElement === firstElement) {
      event.preventDefault();
      lastElement.focus();
    } else if (!event.shiftKey && document.activeElement === lastElement) {
      event.preventDefault();
      firstElement.focus();
    }
  }

  async function handleSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();

    const nextErrors: ValidationErrors = {};
    const trimmedAlias = alias.trim();

    if (!content.trim()) {
      nextErrors.content = "Content is required.";
    } else if ([...content].length > 65536) {
      nextErrors.content = "Content must be at most 65536 characters.";
    }

    if (trimmedAlias.length > 80) {
      nextErrors.alias = "Alias must be at most 80 characters.";
    } else if (ALIAS_FORBIDDEN_RE.test(trimmedAlias)) {
      nextErrors.alias = "Alias must not contain whitespace, quotes, or backticks.";
    }

    setErrors(nextErrors);
    if (Object.keys(nextErrors).length > 0) {
      return;
    }

    try {
      await onSubmit({
        content,
        alias: trimmedAlias || null,
        tags: parsePromptTags(tags),
      });
    } catch (err) {
      setErrors((prev) => ({
        ...prev,
        alias: (err as Error).message || "An error occurred.",
      }));
    }
  }

  return (
    <div className="prompt-dialog__overlay" role="presentation">
      <section
        ref={dialogRef}
        className="prompt-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="prompt-dialog-title"
        onKeyDown={handleDialogKeyDown}
      >
        <header className="prompt-dialog__header">
          <h2 id="prompt-dialog-title">{entry ? "Edit Prompt" : "New Prompt"}</h2>
          <button
            type="button"
            className="desktop-icon-button"
            aria-label="Close"
            onClick={onClose}
          >
            x
          </button>
        </header>

        <form className="prompt-dialog__form" onSubmit={handleSubmit}>
          <label className="prompt-dialog__field">
            <span>Content</span>
            <textarea
              ref={contentRef}
              value={content}
              onChange={(event) => setContent(event.target.value)}
              rows={8}
              aria-invalid={errors.content ? true : undefined}
            />
            {errors.content ? <small>{errors.content}</small> : null}
          </label>

          <label className="prompt-dialog__field">
            <span>Alias</span>
            <input
              value={alias}
              onChange={(event) => setAlias(event.target.value)}
              placeholder="Optional shortcut"
              aria-invalid={errors.alias ? true : undefined}
            />
            {errors.alias ? <small>{errors.alias}</small> : null}
          </label>

          <label className="prompt-dialog__field">
            <span>Tags</span>
            <input
              {...nativeTextAssistDisabledProps}
              value={tags}
              onChange={(event) => setTags(event.target.value)}
              placeholder="review, testing, release"
            />
          </label>

          <footer className="prompt-dialog__actions">
            <button type="button" className="btn btn--secondary" onClick={onClose}>
              Cancel
            </button>
            <button type="submit" className="btn btn--primary" disabled={isSubmitting}>
              Save
            </button>
          </footer>
        </form>
      </section>
    </div>
  );
}
