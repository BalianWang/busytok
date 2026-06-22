import { useCallback, useEffect, useRef, useState } from "react";
import type {
  PromptCreateRequestDto,
  PromptEntryDto,
  PromptSortDto,
  PromptUpdateRequestDto,
} from "@busytok/protocol-types";
import * as Tooltip from "@radix-ui/react-tooltip";
import { Check, Copy, Pencil, Pin, PinOff, Trash2 } from "lucide-react";
import { AppSelect, AppSelectItem } from "../components/Select";
import {
  usePromptCreate,
  usePromptDelete,
  usePromptUse,
  usePromptsList,
  usePromptUpdate,
} from "../api/useBusytokData";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { PageState } from "../components/PageState";
import { TagFilterCombobox } from "../components/TagFilterCombobox";
import { PromptEntryDialog } from "../components/prompt-palette/PromptEntryDialog";
import { nativeTextAssistDisabledProps } from "../lib/nativeTextAssist";
import {
  promptDisplayHeadline,
  promptDisplayTitle,
  promptLastUsedLabel,
  promptUpdatedLabel,
  promptUseCountLabel,
} from "../lib/promptPaletteText";
import {
  executePromptAction,
  PROMPT_ACTION_ERROR_MESSAGE,
  promptActionStatusMessage,
  writeSystemClipboard,
} from "../lib/promptPaletteActions";
import { createPromptPasteBridge } from "../lib/promptPalettePasteBridge";
import type { PromptPalettePasteResult } from "../lib/promptPaletteActions";
import { useRefreshToolbar } from "../components/desktop/useRefreshToolbar";
import { reportFrontendEvent } from "../logging/reporter";

async function hideWindowForTauriPaste(): Promise<PromptPalettePasteResult> {
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().hide();
    return { ok: true, failure_reason: null };
  } catch {
    return { ok: false, failure_reason: "focus_lost" };
  }
}

async function restoreTauriWindowAfterPaste(): Promise<void> {
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const window = getCurrentWindow();
    await window.show();
    await window.setFocus();
  } catch {
    // Restoring the window is best-effort after paste.
  }
}

const SORT_OPTIONS: Array<{ value: PromptSortDto; label: string }> = [
  { value: "pinned_first", label: "Pinned first" },
  { value: "recently_updated", label: "Recently updated" },
  { value: "recently_used", label: "Recently used" },
  { value: "most_used", label: "Most used" },
  { value: "alphabetical", label: "Alphabetical" },
  { value: "smart", label: "Smart" },
];

function updatePayload(entry: PromptEntryDto, isPinned = entry.is_pinned): PromptUpdateRequestDto {
  return {
    id: entry.id,
    content: entry.content,
    alias: entry.alias,
    tags: entry.tags,
    is_pinned: isPinned,
  };
}

type CopyState = "idle" | "copying" | "copied" | "failed";

export function PromptPalettePage() {
  const [query, setQuery] = useState("");
  const [tag, setTag] = useState("");
  const [sort, setSort] = useState<PromptSortDto>("pinned_first");
  const [dialogEntry, setDialogEntry] = useState<PromptEntryDto | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [actionMessage, setActionMessage] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<PromptEntryDto | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  // Per-entry copy state: "idle" | "copying" | "copied" | "failed"
  const [copyStates, setCopyStates] = useState<Record<string, CopyState>>({});
  const [copyTooltipOpen, setCopyTooltipOpen] = useState<Record<string, boolean>>({});
  const [pinningStates, setPinningStates] = useState<Record<string, boolean>>({});
  const copyTimers = useRef<Record<string, ReturnType<typeof setTimeout>>>({});

  const listRequest = {
    query: query || null,
    tag: tag || null,
    sort: sort ?? "pinned_first",
    limit: 100,
  };
  const { data, isLoading, isError, isFetching, refetch } = usePromptsList(listRequest);
  const createPrompt = usePromptCreate();
  const updatePrompt = usePromptUpdate();
  const deletePrompt = usePromptDelete();
  const usePrompt = usePromptUse();

  const entries = data?.data.entries ?? [];

  useRefreshToolbar({
    surface: "prompt_palette",
    onRefresh: refetch,
    isFetching,
  });

  useEffect(() => {
    const timers = copyTimers.current;
    return () => {
      for (const id in timers) clearTimeout(timers[id]);
    };
  }, []);

  function openNewDialog() {
    setDialogEntry(null);
    setDialogOpen(true);
  }

  function openEditDialog(entry: PromptEntryDto) {
    if (pinningStates[entry.id]) {
      return;
    }
    setDialogEntry(entry);
    setDialogOpen(true);
  }

  async function handleSave(payload: PromptCreateRequestDto) {
    if (dialogEntry) {
      await updatePrompt.mutateAsync({
        id: dialogEntry.id,
        ...payload,
        is_pinned: dialogEntry.is_pinned,
      });
    } else {
      await createPrompt.mutateAsync(payload);
    }
    setDialogOpen(false);
    setDialogEntry(null);
  }

  async function handleTogglePinned(entry: PromptEntryDto) {
    if (pinningStates[entry.id]) {
      return;
    }

    setPinningStates((prev) => ({ ...prev, [entry.id]: true }));
    try {
      await updatePrompt.mutateAsync(updatePayload(entry, !entry.is_pinned));
    } finally {
      setPinningStates((prev) => {
        if (!prev[entry.id]) return prev;
        const next = { ...prev };
        delete next[entry.id];
        return next;
      });
    }
  }

  function requestDelete(entry: PromptEntryDto) {
    if (pinningStates[entry.id]) {
      return;
    }
    setDeleteTarget(entry);
    setDeleteError(null);
    reportFrontendEvent({
      level: "INFO",
      event_code: "gui.prompt.delete_confirm_opened",
      message: "Delete confirmation dialog opened",
      details: { entry_id: entry.id, entry_alias: entry.alias ?? null, surface: "prompt_palette" },
    });
  }

  function cancelDelete() {
    reportFrontendEvent({
      level: "INFO",
      event_code: "gui.prompt.delete_confirm_cancelled",
      message: "Delete confirmation cancelled",
      details: { entry_id: deleteTarget?.id ?? null, surface: "prompt_palette" },
    });
    setDeleteTarget(null);
    setDeleteError(null);
  }

  async function confirmDelete() {
    if (!deleteTarget) return;
    reportFrontendEvent({
      level: "INFO",
      event_code: "gui.prompt.delete_confirmed",
      message: "Delete confirmed",
      details: { entry_id: deleteTarget.id, entry_alias: deleteTarget.alias ?? null, surface: "prompt_palette" },
    });
    try {
      await deletePrompt.mutateAsync({ id: deleteTarget.id });
      setDeleteTarget(null);
      setDeleteError(null);
    } catch (err) {
      const message = err instanceof Error ? err.message : "Delete failed";
      setDeleteError(message);
      reportFrontendEvent({
        level: "ERROR",
        event_code: "gui.prompt.delete_failed",
        message: "Delete failed",
        details: { entry_id: deleteTarget.id, error: message, surface: "prompt_palette" },
      });
    }
  }

  const clearCopyFeedback = useCallback((entryId: string) => {
    setCopyStates((prev) => {
      if (prev[entryId] !== "copied" && prev[entryId] !== "failed") return prev;
      const next = { ...prev };
      delete next[entryId];
      return next;
    });
    setCopyTooltipOpen((prev) => {
      if (!prev[entryId]) return prev;
      const next = { ...prev };
      delete next[entryId];
      return next;
    });
    delete copyTimers.current[entryId];
  }, []);

  async function handleCopy(entry: PromptEntryDto) {
    setActionMessage(null);
    setCopyStates((prev) => ({ ...prev, [entry.id]: "copying" }));
    try {
      const result = await executePromptAction(entry, "copy", "page", {
        writeClipboard: writeSystemClipboard,
        ...createPromptPasteBridge({
          hideWindowForPaste: hideWindowForTauriPaste,
          restoreWindow: restoreTauriWindowAfterPaste,
        }),
        recordUse: (request) => usePrompt.mutateAsync(request),
      });
      setCopyStates((prev) => ({ ...prev, [entry.id]: "copied" }));
      // Keep "Copied" visible for 1.5s, then revert.
      if (copyTimers.current[entry.id]) clearTimeout(copyTimers.current[entry.id]);
      copyTimers.current[entry.id] = setTimeout(() => clearCopyFeedback(entry.id), 1500);
    } catch {
      setCopyStates((prev) => ({ ...prev, [entry.id]: "failed" }));
      setActionMessage(PROMPT_ACTION_ERROR_MESSAGE);
      if (copyTimers.current[entry.id]) clearTimeout(copyTimers.current[entry.id]);
      copyTimers.current[entry.id] = setTimeout(() => clearCopyFeedback(entry.id), 1500);
    }
  }

  function copyTooltipLabel(entryId: string): string {
    const state = copyStates[entryId];
    if (state === "copied") return "Copied";
    if (state === "failed") return "Copy failed";
    return "Copy";
  }

  function copyButtonIcon(entryId: string) {
    return copyStates[entryId] === "copied" ? <Check size={15} /> : <Copy size={15} />;
  }

  return (
    <div className="prompt-page">
      <header className="prompt-page__header">
        <p>Manage reusable prompts for fast copy and paste from the palette.</p>
        <button type="button" className="btn btn--primary" onClick={openNewDialog}>
          New Prompt
        </button>
      </header>

      <section className="prompt-page__toolbar page-surface" aria-label="Prompt filters">
        <label className="prompt-page__search">
          <span>Search</span>
          <input
            {...nativeTextAssistDisabledProps}
            type="search"
            aria-label="Search prompts"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search shortcut, content, or tags"
          />
        </label>

        <TagFilterCombobox
          appliedTag={tag}
          onApplyTag={setTag}
          onClear={() => setTag("")}
          placeholder="Filter by tag"
        />

        <div className="prompt-page__filter">
          <AppSelect
            label="Sort"
            aria-label="Sort prompts"
            value={sort}
            onValueChange={(v) => setSort(v as PromptSortDto)}
          >
            {SORT_OPTIONS.map((option) => (
              <AppSelectItem key={option.value} value={option.value}>
                {option.label}
              </AppSelectItem>
            ))}
          </AppSelect>
        </div>

      </section>

      {actionMessage ? (
        <p className="prompt-page__action-status" role="status">
          {actionMessage}
        </p>
      ) : null}

      {isLoading && !data ? (
        <PageState
          kind="loading"
          title="Loading prompts"
          message="Fetching saved prompt entries..."
        />
      ) : isError && !data ? (
        <PageState
          kind="error"
          title="Prompt Palette unavailable"
          message="Could not load saved prompts."
          actionLabel="Retry"
          onAction={() => refetch()}
        />
      ) : entries.length === 0 ? (
        <PageState
          kind="empty"
          title="No prompts"
          message="Create a prompt to make it available in the palette."
        />
      ) : (
        <Tooltip.Provider delayDuration={300}>
          <section className="prompt-page__list" aria-label="Prompt entries">
            {entries.map((entry) => {
              const copyState = copyStates[entry.id] ?? "idle";
              const isCopying = copyState === "copying";
              const isPinning = pinningStates[entry.id] === true;
              return (
              <article className="prompt-row page-surface" key={entry.id}>
                <div className="prompt-row__main">
                  <h3 className="prompt-row__title">{promptDisplayHeadline(entry.alias, entry.content)}</h3>
                  <div className="prompt-row__meta">
                    <span>{promptUseCountLabel(entry.usage_count)}</span>
                    <span>{promptLastUsedLabel(entry.last_used_at_ms)}</span>
                    <span>{promptUpdatedLabel(entry.updated_at_ms)}</span>
                    {entry.tags.slice(0, 3).map((t) => (
                      <span key={t} className="prompt-tag">{t}</span>
                    ))}
                    {entry.tags.length > 3 && (
                      <span className="prompt-row__tag-overflow">+{entry.tags.length - 3}</span>
                    )}
                  </div>
                </div>

                <div className="prompt-row__actions prompt-row__actions--recessed">
                  <Tooltip.Root
                    open={copyState === "copied" || copyState === "failed"
                      ? true
                      : (copyTooltipOpen[entry.id] ?? false)}
                    onOpenChange={(value: boolean) => {
                      if (copyState === "copied" || copyState === "failed") return;
                      setCopyTooltipOpen((prev) => ({ ...prev, [entry.id]: value }));
                    }}
                  >
                    <Tooltip.Trigger asChild>
                      <button
                        type="button"
                        className={`prompt-row__icon-btn${copyState === "copied" ? " prompt-row__icon-btn--success" : ""}`}
                        aria-label="Copy"
                        onClick={() => handleCopy(entry)}
                        disabled={isCopying}
                      >
                        {copyButtonIcon(entry.id)}
                      </button>
                    </Tooltip.Trigger>
                    <Tooltip.Portal>
                      <Tooltip.Content className="prompt-row__tooltip">
                        {copyTooltipLabel(entry.id)}
                        <Tooltip.Arrow className="prompt-row__tooltip-arrow" />
                      </Tooltip.Content>
                    </Tooltip.Portal>
                  </Tooltip.Root>

                  <Tooltip.Root>
                    <Tooltip.Trigger asChild>
                      <button
                        type="button"
                        className="prompt-row__icon-btn"
                        aria-label="Edit"
                        aria-disabled={isPinning}
                        onClick={() => openEditDialog(entry)}
                      >
                        <Pencil size={15} />
                      </button>
                    </Tooltip.Trigger>
                    <Tooltip.Portal>
                      <Tooltip.Content className="prompt-row__tooltip">
                        Edit
                        <Tooltip.Arrow className="prompt-row__tooltip-arrow" />
                      </Tooltip.Content>
                    </Tooltip.Portal>
                  </Tooltip.Root>

                  <Tooltip.Root>
                    <Tooltip.Trigger asChild>
                      <button
                        type="button"
                        className={`prompt-row__icon-btn${entry.is_pinned ? " prompt-row__icon-btn--active" : ""}`}
                        aria-label={entry.is_pinned ? "Unpin" : "Pin"}
                        onClick={() => handleTogglePinned(entry)}
                        aria-disabled={isPinning}
                      >
                        {entry.is_pinned ? <PinOff size={15} /> : <Pin size={15} />}
                      </button>
                    </Tooltip.Trigger>
                    <Tooltip.Portal>
                      <Tooltip.Content className="prompt-row__tooltip">
                        {entry.is_pinned ? "Unpin" : "Pin"}
                        <Tooltip.Arrow className="prompt-row__tooltip-arrow" />
                      </Tooltip.Content>
                    </Tooltip.Portal>
                  </Tooltip.Root>

                  <Tooltip.Root>
                    <Tooltip.Trigger asChild>
                      <button
                        type="button"
                        className="prompt-row__icon-btn prompt-row__icon-btn--danger"
                        aria-label="Delete"
                        aria-disabled={isPinning}
                        onClick={() => requestDelete(entry)}
                      >
                        <Trash2 size={15} />
                      </button>
                    </Tooltip.Trigger>
                    <Tooltip.Portal>
                      <Tooltip.Content className="prompt-row__tooltip">
                        Delete
                        <Tooltip.Arrow className="prompt-row__tooltip-arrow" />
                      </Tooltip.Content>
                    </Tooltip.Portal>
                  </Tooltip.Root>
                </div>
              </article>
              );
            })}
          </section>
        </Tooltip.Provider>
      )}

      <PromptEntryDialog
        open={dialogOpen}
        entry={dialogEntry}
        isSubmitting={createPrompt.isPending || updatePrompt.isPending}
        onClose={() => {
          setDialogOpen(false);
          setDialogEntry(null);
        }}
        onSubmit={handleSave}
      />

      <ConfirmDialog
        open={deleteTarget !== null}
        title="Delete Prompt?"
        body="This will permanently remove this prompt from your library."
        detail={deleteTarget ? promptDisplayTitle(deleteTarget.alias, deleteTarget.content) : undefined}
        confirmLabel="Delete Prompt"
        loading={deletePrompt.isPending}
        error={deleteError}
        onConfirm={confirmDelete}
        onCancel={cancelDelete}
      />
    </div>
  );
}
