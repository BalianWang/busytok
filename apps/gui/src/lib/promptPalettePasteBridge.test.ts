import { describe, expect, it, vi } from "vitest";
import { createPromptPasteBridge } from "./promptPalettePasteBridge";

describe("createPromptPasteBridge", () => {
  it("hides first, notifies when hidden, and restores the window after a successful paste attempt", async () => {
    const onWindowHidden = vi.fn();
    const restoreWindow = vi.fn().mockResolvedValue(undefined);
    const bridge = createPromptPasteBridge({
      getAccessibilityStatus: vi.fn().mockResolvedValue({ ok: true, failure_reason: null }),
      hideWindowForPaste: vi.fn().mockResolvedValue({ ok: true, failure_reason: null }),
      pasteIntoActiveApp: vi.fn().mockResolvedValue({ ok: true, failure_reason: null }),
      restoreWindow,
      onWindowHidden,
    });

    await expect(bridge.beforePaste?.()).resolves.toEqual({ ok: true, failure_reason: null });
    await expect(bridge.pasteActiveApp?.()).resolves.toEqual({ ok: true, failure_reason: null });

    expect(onWindowHidden).toHaveBeenCalledOnce();
    expect(restoreWindow).toHaveBeenCalledOnce();
  });

  it("does not restore when accessibility permission is missing before the window is hidden", async () => {
    const restoreWindow = vi.fn().mockResolvedValue(undefined);
    const bridge = createPromptPasteBridge({
      getAccessibilityStatus: vi
        .fn()
        .mockResolvedValue({ ok: false, failure_reason: "permission_missing" }),
      hideWindowForPaste: vi.fn(),
      pasteIntoActiveApp: vi.fn(),
      restoreWindow,
    });

    await expect(bridge.beforePaste?.()).resolves.toEqual({
      ok: false,
      failure_reason: "permission_missing",
    });

    expect(restoreWindow).not.toHaveBeenCalled();
  });
});
