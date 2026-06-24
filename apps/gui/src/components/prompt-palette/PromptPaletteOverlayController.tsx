import { useEffect, useState } from "react";
import type { PromptActionDto, PromptEntryDto } from "@busytok/protocol-types";
import { usePromptsList, usePromptUse } from "../../api/useBusytokData";
import { useEventSubscription } from "../../api/useEventSubscription";
import {
  executePromptAction,
  getPromptPaletteAccessibilityStatus,
  PROMPT_ACTION_ERROR_MESSAGE,
  promptActionStatusMessage,
  pasteActiveApp,
  readSystemClipboard,
  writeSystemClipboard,
} from "../../lib/promptPaletteActions";
import { createPromptPasteBridge } from "../../lib/promptPalettePasteBridge";
import { getPanelRuntime, requestPanelClose } from "../../lib/panelWindowOps";
import { PromptPaletteOverlay } from "./PromptPaletteOverlay";
import { reportFrontendEvent } from "../../logging/reporter";

interface PromptPaletteOverlayControllerProps {
  open: boolean;
  onClose: () => void;
  onOpenPage: () => void;
  onCreateNew: () => void;
  presentation?: "overlay" | "window";
  defaultAction?: PromptActionDto;
}

export function PromptPaletteOverlayController({
  open,
  onClose,
  onOpenPage,
  onCreateNew,
  presentation = "overlay",
  defaultAction = "Copy&Paste",
}: PromptPaletteOverlayControllerProps) {
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const { serviceStatus } = useEventSubscription();
  const serviceReady = serviceStatus === "ready";

  const promptList = usePromptsList({
    query: query || null,
    tag: null,
    sort: "smart",
    limit: 50,
  });
  const { refetch } = promptList;
  const promptUse = usePromptUse();
  const entries = promptList.data?.data.entries ?? [];

  useEffect(() => {
    if (open) {
      void refetch();
    }
  }, [open, refetch]);

  useEffect(() => {
    if (presentation !== "window") {
      return;
    }

    const handleFocus = () => {
      void refetch();
    };

    window.addEventListener("focus", handleFocus);
    return () => {
      window.removeEventListener("focus", handleFocus);
    };
  }, [presentation, refetch]);

  async function execute(entry: PromptEntryDto, action: PromptActionDto) {
    if (!serviceReady) {
      reportFrontendEvent({
        level: "WARN",
        event_code: "gui.prompt_palette.action_blocked_service_unavailable",
        message: `Prompt action "${action}" blocked: service status is "${serviceStatus}"`,
        details: { entry_id: entry.id, action, service_status: serviceStatus },
      });
      setStatus("Service unavailable. Please wait.");
      return;
    }
    const runtime = presentation === "window" ? getPanelRuntime() : undefined;
    setStatus(null);
    try {
      const result = await executePromptAction(entry, action, "overlay", {
        writeClipboard: writeSystemClipboard,
        readClipboard: readSystemClipboard,
        ...createPromptPasteBridge({
          // In window mode, hideWindowForPaste handles the actual close.
          // In overlay mode, onWindowHidden notifies the parent to close.
          onWindowHidden: runtime ? undefined : onClose,
          ...(runtime
            ? {
                getAccessibilityStatus: () =>
                  getPromptPaletteAccessibilityStatus(runtime),
                hideWindowForPaste: async () => {
                  await requestPanelClose();
                  await new Promise((resolve) => setTimeout(resolve, 200));
                  return { ok: true as const, failure_reason: null };
                },
                pasteIntoActiveApp: () => pasteActiveApp(runtime),
              }
            : {}),
        }),
        recordUse: (request) => promptUse.mutateAsync(request),
      });
      setStatus(promptActionStatusMessage(result));
      if (presentation === "window" && result.outcome !== "paste_attempted") {
        onClose?.();
      }
    } catch {
      setStatus(PROMPT_ACTION_ERROR_MESSAGE);
      if (presentation === "window") {
        onClose?.();
      }
    }
  }

  return (
    <>
      {!open && status ? (
        <p className="prompt-app-status" role="status">
          {status}
        </p>
      ) : null}
      <PromptPaletteOverlay
        open={open}
        entries={entries}
        isLoading={promptList.isLoading}
        error={promptList.isError ? "Could not load saved prompts." : null}
        query={query}
        defaultAction={defaultAction}
        onQueryChange={(nextQuery) => {
          setQuery(nextQuery);
          setStatus(null);
        }}
        onClose={onClose}
        onExecute={execute}
        onOpenPage={onOpenPage}
        onCreateNew={onCreateNew}
        statusMessage={status}
        presentation={presentation}
      />
    </>
  );
}
