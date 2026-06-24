import type {
  PromptActionDto,
  PromptEntryDto,
  PromptUseFailureReasonDto,
  PromptUseRequestDto,
  PromptUseResultDto,
  PromptUseSurfaceDto,
} from "@busytok/protocol-types";
import type { PaletteRuntime } from "./paletteRuntime";
import { reportFrontendEvent } from "../logging/reporter";

type PastePreparationResult =
  | { ok: true; failure_reason?: null }
  | { ok: false; failure_reason: PromptUseFailureReasonDto };

type PasteAttemptResult =
  | { ok: true; failure_reason?: null }
  | { ok: false; failure_reason: PromptUseFailureReasonDto };

export type PromptPalettePasteResult =
  | { ok: true; failure_reason: null }
  | { ok: false; failure_reason: PromptUseFailureReasonDto };

export type PromptPaletteReportEvent = (entry: {
  level: "INFO" | "WARN" | "ERROR";
  event_code: string;
  message: string;
  details?: Record<string, unknown>;
}) => void;

export interface PromptActionDeps {
  writeClipboard: (text: string) => Promise<void>;
  /** Read the current system clipboard; returns null when unsupported. */
  readClipboard?: () => Promise<string | null>;
  beforePaste?: () => Promise<PastePreparationResult>;
  pasteActiveApp?: () => Promise<PasteAttemptResult>;
  recordUse: (request: PromptUseRequestDto) => Promise<PromptUseResultDto>;
  reportEvent?: PromptPaletteReportEvent;
}

export interface PromptActionResult {
  request: PromptUseRequestDto;
  result: PromptUseResultDto;
  outcome: PromptUseRequestDto["outcome"];
  failure_reason: PromptUseFailureReasonDto | null;
}

export const PROMPT_ACTION_ERROR_MESSAGE = "Could not run prompt action. Try again.";

export function promptActionStatusMessage(
  result: Pick<PromptActionResult, "outcome" | "failure_reason">,
): string | null {
  if (result.outcome !== "paste_fell_back_to_copy") {
    return null;
  }
  if (result.failure_reason === "permission_missing") {
    return "Copied instead. Automatic paste was unavailable. Enable Accessibility permission in Settings.";
  }
  return "Copied instead. Automatic paste was unavailable.";
}

export async function writeSystemClipboard(text: string): Promise<void> {
  if (globalThis.navigator?.clipboard?.writeText) {
    await globalThis.navigator.clipboard.writeText(text);
    return;
  }
  throw new Error("Clipboard API is unavailable");
}

export async function readSystemClipboard(): Promise<string | null> {
  if (globalThis.navigator?.clipboard?.readText) {
    return await globalThis.navigator.clipboard.readText();
  }
  return null; // unsupported read → don't restore (null sentinel)
}

function unsupportedPlatformResult(): PromptPalettePasteResult {
  return { ok: false, failure_reason: "unsupported_platform" };
}

export async function pasteActiveApp(
  runtime?: PaletteRuntime,
): Promise<PromptPalettePasteResult> {
  try {
    if (runtime) {
      return await runtime.invoke("prompt_palette_paste_active_app") as PromptPalettePasteResult;
    }
    const { invoke } = await import("@tauri-apps/api/core");
    return await invoke<PromptPalettePasteResult>("prompt_palette_paste_active_app");
  } catch {
    return unsupportedPlatformResult();
  }
}

export async function getPromptPaletteAccessibilityStatus(
  runtime?: PaletteRuntime,
): Promise<PromptPalettePasteResult> {
  try {
    if (runtime) {
      return await runtime.invoke("prompt_palette_accessibility_status") as PromptPalettePasteResult;
    }
    const { invoke } = await import("@tauri-apps/api/core");
    return await invoke<PromptPalettePasteResult>("prompt_palette_accessibility_status");
  } catch {
    return unsupportedPlatformResult();
  }
}

export async function openPromptPaletteAccessibilitySettings(
  runtime?: PaletteRuntime,
): Promise<void> {
  try {
    if (runtime) {
      await runtime.invoke("prompt_palette_open_accessibility_settings");
      return;
    }
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("prompt_palette_open_accessibility_settings");
  } catch {
    globalThis.open?.(
      "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
      "_blank",
      "noopener,noreferrer",
    );
  }
}

function eventDetails(
  entry: PromptEntryDto,
  action: PromptActionDto,
  surface: PromptUseSurfaceDto,
  outcome: PromptUseRequestDto["outcome"],
  failure_reason: PromptUseFailureReasonDto | null,
): Record<string, unknown> {
  return {
    prompt_id: entry.id,
    action,
    surface,
    outcome,
    failure_reason,
  };
}

async function recordAndReport(
  entry: PromptEntryDto,
  request: PromptUseRequestDto,
  deps: PromptActionDeps,
): Promise<PromptActionResult> {
  const result = await deps.recordUse(request);
  const reportEvent = deps.reportEvent ?? reportFrontendEvent;

  if (request.outcome === "copy") {
    reportEvent({
      level: "INFO",
      event_code: "gui.prompt_palette.copy",
      message: "Prompt copied to clipboard",
      details: eventDetails(entry, request.action, request.surface, request.outcome, null),
    });
  } else if (request.outcome === "paste_attempted") {
    reportEvent({
      level: "INFO",
      event_code: "gui.prompt_palette.paste_attempted",
      message: "Prompt paste attempted",
      details: eventDetails(entry, request.action, request.surface, request.outcome, null),
    });
  } else {
    reportEvent({
      level: "WARN",
      event_code: "gui.prompt_palette.paste_fallback",
      message: "Prompt paste fell back to clipboard copy",
      details: eventDetails(
        entry,
        request.action,
        request.surface,
        request.outcome,
        request.failure_reason,
      ),
    });
  }

  return {
    request,
    result,
    outcome: request.outcome,
    failure_reason: request.failure_reason,
  };
}

/**
 * Shared paste flow (beforePaste → pasteActiveApp → recordUse).  Used by
 * both Copy&Paste and OnlyPaste — the clipboard write (and optional
 * save/restore) is handled by the caller.
 */
async function runPaste(
  entry: PromptEntryDto,
  action: PromptActionDto,
  surface: PromptUseSurfaceDto,
  deps: PromptActionDeps,
): Promise<PromptActionResult> {
  const fallback = (failure_reason: PromptUseFailureReasonDto) =>
    recordAndReport(
      entry,
      {
        id: entry.id,
        action,
        surface,
        outcome: "paste_fell_back_to_copy",
        failure_reason,
      },
      deps,
    );

  if (deps.beforePaste) {
    const prepared = await deps.beforePaste();
    if (!prepared.ok) {
      return fallback(prepared.failure_reason);
    }
  }

  if (!deps.pasteActiveApp) {
    return fallback("unsupported_platform");
  }

  const pasted = await deps.pasteActiveApp();
  if (!pasted.ok) {
    return fallback(pasted.failure_reason);
  }

  return recordAndReport(
    entry,
    {
      id: entry.id,
      action,
      surface,
      outcome: "paste_attempted",
      failure_reason: null,
    },
    deps,
  );
}

export async function executePromptAction(
  entry: PromptEntryDto,
  action: PromptActionDto,
  surface: PromptUseSurfaceDto,
  deps: PromptActionDeps,
): Promise<PromptActionResult> {
  // ── OnlyPaste: save old clipboard → write → paste → restore ──
  if (action === "OnlyPaste") {
    // Snapshot the old clipboard. null = "don't restore" — only a real
    // readClipboard success produces a non-null value, so a missing or
    // throwing readClipboard won't overwrite the user's clipboard with an
    // empty string (P0).
    let oldClipboard: string | null = null;
    if (deps.readClipboard) {
      try {
        oldClipboard = await deps.readClipboard();
      } catch {
        // clipboard read failure is non-fatal — oldClipboard stays null
      }
    }

    try {
      await deps.writeClipboard(entry.content);
    } catch (error) {
      const reportEvent = deps.reportEvent ?? reportFrontendEvent;
      reportEvent({
        level: "ERROR",
        event_code: "gui.prompt_palette.action_failed",
        message: "OnlyPaste clipboard write failed",
        details: {
          prompt_entry_id: entry.id,
          action,
          surface,
          outcome: "failed",
          failure_reason: "clipboard_write_failed",
          error_name: error instanceof Error ? error.name : typeof error,
        },
      });
      throw error;
    }

    // P1: wrap paste + restore in try/finally so the old clipboard is
    // always restored, even when runPaste (recordUse / beforePaste /
    // pasteActiveApp) throws or rejects.
    let result: PromptActionResult;
    try {
      result = await runPaste(entry, action, surface, deps);
    } finally {
      if (oldClipboard !== null) {
        try {
          await deps.writeClipboard(oldClipboard);
        } catch {
          const reportEvent = deps.reportEvent ?? reportFrontendEvent;
          reportEvent({
            level: "WARN",
            event_code: "gui.prompt_palette.only_paste_restore_failed",
            message: "OnlyPaste failed to restore the old clipboard after paste",
            details: { prompt_entry_id: entry.id },
          });
        }
      }
    }
    return result;
  }

  // ── OnlyCopy / Copy&Paste: write clipboard first ──
  try {
    await deps.writeClipboard(entry.content);
  } catch (error) {
    const reportEvent = deps.reportEvent ?? reportFrontendEvent;
    reportEvent({
      level: "ERROR",
      event_code: "gui.prompt_palette.action_failed",
      message: "Prompt action failed before usage could be recorded",
      details: {
        prompt_entry_id: entry.id,
        action,
        surface,
        outcome: "failed",
        failure_reason: "clipboard_write_failed",
        error_name: error instanceof Error ? error.name : typeof error,
      },
    });
    throw error;
  }

  if (action === "OnlyCopy") {
    const request: PromptUseRequestDto = {
      id: entry.id,
      action,
      surface,
      outcome: "copy",
      failure_reason: null,
    };
    return recordAndReport(entry, request, deps);
  }

  // Copy&Paste: clipboard already written, now paste.
  return runPaste(entry, action, surface, deps);
}
