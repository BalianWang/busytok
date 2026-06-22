import { afterEach, describe, expect, it } from "vitest";
import {
  getPromptPaletteShortcutStatus,
  setPromptPaletteShortcutStatus,
  subscribePromptPaletteShortcutStatus,
} from "./promptPaletteShortcutState";

describe("promptPaletteShortcutState", () => {
  afterEach(() => {
    setPromptPaletteShortcutStatus({ state: "idle" });
  });

  it("immediately notifies subscribers with the current status", () => {
    setPromptPaletteShortcutStatus({ state: "registered" });
    const received: Array<ReturnType<typeof getPromptPaletteShortcutStatus>> = [];

    subscribePromptPaletteShortcutStatus((status) => {
      received.push(status);
    });

    expect(received).toEqual([{ state: "registered" }]);
  });

  it("notifies subscribers of registered and failed updates", () => {
    const received: Array<ReturnType<typeof getPromptPaletteShortcutStatus>> = [];

    subscribePromptPaletteShortcutStatus((status) => {
      received.push(status);
    });
    setPromptPaletteShortcutStatus({ state: "registered" });
    setPromptPaletteShortcutStatus({ state: "failed", message: "Already registered" });

    expect(received).toEqual([
      { state: "idle" },
      { state: "registered" },
      { state: "failed", message: "Already registered" },
    ]);
  });

  it("stops notifying after unsubscribe", () => {
    const received: Array<ReturnType<typeof getPromptPaletteShortcutStatus>> = [];
    const unsubscribe = subscribePromptPaletteShortcutStatus((status) => {
      received.push(status);
    });

    unsubscribe();
    setPromptPaletteShortcutStatus({ state: "registered" });

    expect(received).toEqual([{ state: "idle" }]);
  });
});
