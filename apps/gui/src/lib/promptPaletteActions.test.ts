import { describe, expect, it, vi } from "vitest";
import type {
  PromptEntryDto,
  PromptUseFailureReasonDto,
  PromptUseRequestDto,
} from "@busytok/protocol-types";
import {
  executePromptAction,
  getPromptPaletteAccessibilityStatus,
  openPromptPaletteAccessibilitySettings,
  pasteActiveApp,
  writeSystemClipboard,
  type PromptActionDeps,
} from "./promptPaletteActions";

const { mockInvoke } = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mockInvoke,
}));

function makePrompt(overrides: Partial<PromptEntryDto> = {}): PromptEntryDto {
  return {
    id: "prompt-1",
    content: "Sensitive prompt content should never be logged.",
    tags: ["release"],
    alias: "notes",
    is_pinned: false,
    usage_count: 0,
    last_used_at_ms: null,
    created_at_ms: 1715900000000,
    updated_at_ms: 1716000000000,
    ...overrides,
  };
}

function makeDeps(overrides: Partial<PromptActionDeps> = {}): PromptActionDeps {
  return {
    writeClipboard: vi.fn().mockResolvedValue(undefined),
    beforePaste: vi.fn().mockResolvedValue({ ok: true }),
    pasteActiveApp: vi.fn().mockResolvedValue({ ok: true }),
    recordUse: vi.fn().mockResolvedValue({ usage_count: 1, last_used_at_ms: 1716000000000 }),
    reportEvent: vi.fn(),
    ...overrides,
  };
}

describe("executePromptAction", () => {
  it("records use and reports copy event after clipboard succeeds", async () => {
    const entry = makePrompt();
    const calls: string[] = [];
    const deps = makeDeps({
      writeClipboard: vi.fn().mockImplementation(async () => {
        calls.push("clipboard");
      }),
      recordUse: vi.fn().mockImplementation(async (_request: PromptUseRequestDto) => {
        calls.push("record");
        return { usage_count: 0, last_used_at_ms: null };
      }),
    });
    const reportEvent = vi.mocked(deps.reportEvent!);

    const result = await executePromptAction(entry, "copy", "overlay", deps);

    expect(calls).toEqual(["clipboard", "record"]);
    expect(deps.writeClipboard).toHaveBeenCalledWith(entry.content);
    expect(deps.recordUse).toHaveBeenCalledWith({
      id: entry.id,
      action: "copy",
      surface: "overlay",
      outcome: "copy",
      failure_reason: null,
    });
    expect(reportEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        level: "INFO",
        event_code: "gui.prompt_palette.copy",
      }),
    );
    expect(result.request.outcome).toBe("copy");
    expect(result.outcome).toBe("copy");
    // Copy does not increment usage_count (backend guard), but the RPC still fires.
    expect(result.result.usage_count).toBe(0);
  });

  it("reports failure without recording use or prompt content when clipboard fails", async () => {
    const entry = makePrompt();
    const deps = makeDeps({
      writeClipboard: vi.fn().mockRejectedValue(new Error("denied")),
    });
    const reportEvent = vi.mocked(deps.reportEvent!);

    await expect(executePromptAction(entry, "copy", "overlay", deps)).rejects.toThrow("denied");

    expect(deps.recordUse).not.toHaveBeenCalled();
    expect(reportEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        level: "ERROR",
        event_code: "gui.prompt_palette.action_failed",
        details: expect.objectContaining({
          prompt_entry_id: entry.id,
          action: "copy",
          surface: "overlay",
          outcome: "failed",
          failure_reason: "clipboard_write_failed",
        }),
      }),
    );
    expect(JSON.stringify(reportEvent.mock.calls[0]?.[0].details)).not.toContain(entry.content);
  });

  it("records use and reports fallback event when paste bridge is unsupported", async () => {
    const deps = makeDeps({
      pasteActiveApp: vi.fn().mockResolvedValue({
        ok: false,
        failure_reason: "unsupported_platform" satisfies PromptUseFailureReasonDto,
      }),
    });
    const reportEvent = vi.mocked(deps.reportEvent!);

    const result = await executePromptAction(makePrompt(), "paste", "overlay", deps);

    expect(deps.recordUse).toHaveBeenCalledWith(
      expect.objectContaining({
        action: "paste",
        outcome: "paste_fell_back_to_copy",
        failure_reason: "unsupported_platform",
      }),
    );
    expect(reportEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        level: "WARN",
        event_code: "gui.prompt_palette.paste_fallback",
      }),
    );
    expect(result.outcome).toBe("paste_fell_back_to_copy");
    expect(result.failure_reason).toBe("unsupported_platform");
  });

  it("skips pasteActiveApp when beforePaste reports focus_lost", async () => {
    const deps = makeDeps({
      beforePaste: vi.fn().mockResolvedValue({ ok: false, failure_reason: "focus_lost" }),
    });

    await executePromptAction(makePrompt(), "paste", "overlay", deps);

    expect(deps.pasteActiveApp).not.toHaveBeenCalled();
    expect(deps.recordUse).toHaveBeenCalledWith(
      expect.objectContaining({
        outcome: "paste_fell_back_to_copy",
        failure_reason: "focus_lost",
      }),
    );
  });

  it("falls back when no paste bridge hooks are provided", async () => {
    const deps = makeDeps({
      beforePaste: undefined,
      pasteActiveApp: undefined,
    });

    const result = await executePromptAction(makePrompt(), "paste", "page", deps);

    expect(deps.recordUse).toHaveBeenCalledWith(
      expect.objectContaining({
        surface: "page",
        outcome: "paste_fell_back_to_copy",
        failure_reason: "unsupported_platform",
      }),
    );
    expect(result.failure_reason).toBe("unsupported_platform");
  });

  it("reports copy, paste attempted, and paste fallback events without prompt content", async () => {
    const entry = makePrompt();
    const reportEvent = vi.fn();

    await executePromptAction(entry, "copy", "page", makeDeps({ reportEvent }));
    await executePromptAction(entry, "paste", "overlay", makeDeps({ reportEvent }));
    await executePromptAction(
      entry,
      "paste",
      "overlay",
      makeDeps({
        reportEvent,
        pasteActiveApp: vi.fn().mockResolvedValue({
          ok: false,
          failure_reason: "unsupported_platform",
        }),
      }),
    );

    expect(reportEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        level: "INFO",
        event_code: "gui.prompt_palette.copy",
      }),
    );
    expect(reportEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        level: "INFO",
        event_code: "gui.prompt_palette.paste_attempted",
      }),
    );
    expect(reportEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        level: "WARN",
        event_code: "gui.prompt_palette.paste_fallback",
      }),
    );

    for (const call of reportEvent.mock.calls) {
      expect(JSON.stringify(call[0].details)).not.toContain(entry.content);
    }
  });
});

describe("writeSystemClipboard", () => {
  it("uses navigator.clipboard.writeText to copy text", async () => {
    const originalClipboard = navigator.clipboard;
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });

    try {
      await writeSystemClipboard("copy through browser API");
    } finally {
      Object.defineProperty(navigator, "clipboard", {
        value: originalClipboard,
        configurable: true,
      });
    }

    expect(writeText).toHaveBeenCalledWith("copy through browser API");
  });

  it("throws when browser clipboard API is unavailable", async () => {
    const originalClipboard = navigator.clipboard;
    Object.defineProperty(navigator, "clipboard", {
      value: undefined,
      configurable: true,
    });

    try {
      await expect(writeSystemClipboard("copy")).rejects.toThrow();
    } finally {
      Object.defineProperty(navigator, "clipboard", {
        value: originalClipboard,
        configurable: true,
      });
    }
  });
});

describe("pasteActiveApp", () => {
  it("invokes the native prompt palette paste command", async () => {
    mockInvoke.mockResolvedValueOnce({ ok: true, failure_reason: null });

    const result = await pasteActiveApp();

    expect(mockInvoke).toHaveBeenCalledWith("prompt_palette_paste_active_app");
    expect(result).toEqual({ ok: true, failure_reason: null });
  });

  it("falls back to unsupported_platform when the native paste command rejects", async () => {
    mockInvoke.mockRejectedValueOnce(new Error("ipc unavailable"));

    await expect(pasteActiveApp()).resolves.toEqual({
      ok: false,
      failure_reason: "unsupported_platform",
    });
  });
});

describe("getPromptPaletteAccessibilityStatus", () => {
  it("invokes the native accessibility status command", async () => {
    mockInvoke.mockResolvedValueOnce({ ok: false, failure_reason: "permission_missing" });

    const result = await getPromptPaletteAccessibilityStatus();

    expect(mockInvoke).toHaveBeenCalledWith("prompt_palette_accessibility_status");
    expect(result).toEqual({ ok: false, failure_reason: "permission_missing" });
  });

  it("falls back to unsupported_platform when the accessibility command rejects", async () => {
    mockInvoke.mockRejectedValueOnce(new Error("ipc unavailable"));

    await expect(getPromptPaletteAccessibilityStatus()).resolves.toEqual({
      ok: false,
      failure_reason: "unsupported_platform",
    });
  });
});

describe("openPromptPaletteAccessibilitySettings", () => {
  it("invokes the native accessibility settings command", async () => {
    mockInvoke.mockResolvedValueOnce(undefined);

    await openPromptPaletteAccessibilitySettings();

    expect(mockInvoke).toHaveBeenCalledWith("prompt_palette_open_accessibility_settings");
  });

  it("opens the macOS accessibility pane when the native command rejects", async () => {
    const originalOpen = globalThis.open;
    const open = vi.fn();
    Object.defineProperty(globalThis, "open", {
      value: open,
      configurable: true,
    });
    mockInvoke.mockRejectedValueOnce(new Error("ipc unavailable"));

    try {
      await openPromptPaletteAccessibilitySettings();
    } finally {
      Object.defineProperty(globalThis, "open", {
        value: originalOpen,
        configurable: true,
      });
    }

    expect(open).toHaveBeenCalledWith(
      "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
      "_blank",
      "noopener,noreferrer",
    );
  });
});
