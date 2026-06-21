export type PromptPaletteShortcutStatus =
  | { state: "idle" }
  | { state: "registered" }
  | { state: "failed"; message: string };

type PromptPaletteShortcutStatusListener = (status: PromptPaletteShortcutStatus) => void;

let currentStatus: PromptPaletteShortcutStatus = { state: "idle" };
const listeners = new Set<PromptPaletteShortcutStatusListener>();

export function getPromptPaletteShortcutStatus(): PromptPaletteShortcutStatus {
  return currentStatus;
}

export function setPromptPaletteShortcutStatus(status: PromptPaletteShortcutStatus) {
  currentStatus = status;
  for (const listener of listeners) {
    listener(currentStatus);
  }
}

export function subscribePromptPaletteShortcutStatus(
  listener: PromptPaletteShortcutStatusListener,
): () => void {
  listeners.add(listener);
  listener(currentStatus);
  return () => {
    listeners.delete(listener);
  };
}
