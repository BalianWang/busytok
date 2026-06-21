import {
  getPromptPaletteAccessibilityStatus,
  pasteActiveApp,
  type PromptActionDeps,
  type PromptPalettePasteResult,
} from "./promptPaletteActions";

interface PromptPasteBridgeOptions {
  onWindowHidden?: () => void;
  getAccessibilityStatus?: () => Promise<PromptPalettePasteResult>;
  hideWindowForPaste?: () => Promise<PromptPalettePasteResult>;
  pasteIntoActiveApp?: () => Promise<PromptPalettePasteResult>;
  restoreWindow?: () => Promise<void>;
}

export function createPromptPasteBridge(
  options: PromptPasteBridgeOptions = {},
): Pick<PromptActionDeps, "beforePaste" | "pasteActiveApp"> {
  const getAccessibilityStatus =
    options.getAccessibilityStatus ?? getPromptPaletteAccessibilityStatus;
  const hideWindowForPaste = options.hideWindowForPaste ?? (async () => ({ ok: true, failure_reason: null }) as PromptPalettePasteResult);
  const pasteIntoActiveApp = options.pasteIntoActiveApp ?? pasteActiveApp;
  const restoreWindow = options.restoreWindow ?? (async () => {});
  let windowHidden = false;

  return {
    beforePaste: async () => {
      const status = await getAccessibilityStatus();
      if (!status.ok) {
        return status;
      }

      const hidden = await hideWindowForPaste();
      if (hidden.ok) {
        windowHidden = true;
        options.onWindowHidden?.();
      }
      return hidden;
    },
    pasteActiveApp: async () => {
      try {
        return await pasteIntoActiveApp();
      } finally {
        if (windowHidden) {
          windowHidden = false;
          await restoreWindow();
        }
      }
    },
  };
}
