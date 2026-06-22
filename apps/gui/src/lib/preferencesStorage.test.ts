import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  loadPreferences,
  notifyPreferencesUpdated,
  savePreferences,
  defaultPreferences,
} from "./preferencesStorage";

const STORAGE_KEY = "busytok.consumer.preferences.v1";

const storage = (() => {
  let store: Record<string, string> = {};
  return {
    getItem: vi.fn((key: string) => (key in store ? store[key] : null)),
    setItem: vi.fn((key: string, value: string) => {
      store[key] = value;
    }),
    removeItem: vi.fn((key: string) => {
      delete store[key];
    }),
    clear: vi.fn(() => {
      store = {};
    }),
    reset() {
      store = {};
      this.getItem.mockClear();
      this.setItem.mockClear();
      this.removeItem.mockClear();
      this.clear.mockClear();
    },
  };
})();

Object.defineProperty(globalThis, "localStorage", {
  value: storage,
  configurable: true,
});

afterEach(() => {
  storage.reset();
});

describe("preferencesStorage", () => {
  beforeEach(() => {
    storage.reset();
  });

  it("returns defaults when no preferences are stored", () => {
    expect(loadPreferences()).toEqual(defaultPreferences);
  });

  it("persists and reloads preferences", () => {
    savePreferences({ ...defaultPreferences, defaultDashboardRange: "month", budgetUsd: 25, reminderEnabled: true });

    expect(loadPreferences()).toMatchObject({
      defaultDashboardRange: "month",
      budgetUsd: 25,
      reminderEnabled: true,
    });
  });

  it("broadcasts saved preferences asynchronously once", async () => {
    const dispatchSpy = vi.spyOn(window, "dispatchEvent");

    savePreferences({ ...defaultPreferences, defaultDashboardRange: "week" });

    expect(dispatchSpy).not.toHaveBeenCalled();
    await Promise.resolve();
    expect(dispatchSpy).toHaveBeenCalledTimes(1);

    dispatchSpy.mockRestore();
  });

  it("falls back safely when stored data is malformed", () => {
    localStorage.setItem(STORAGE_KEY, "{bad json");

    expect(loadPreferences()).toEqual(defaultPreferences);
  });

  it("sanitizes invalid stored preference values", () => {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        defaultDashboardRange: "forever",
        budgetUsd: -10,
        reminderEnabled: 1,
        reminderThresholdPct: 150.4,
      }),
    );

    expect(loadPreferences()).toEqual({
      ...defaultPreferences,
      reminderEnabled: true,
      reminderThresholdPct: 100,
    });

    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        defaultDashboardRange: "year",
        budgetUsd: Number.POSITIVE_INFINITY,
        reminderThresholdPct: -2,
      }),
    );

    expect(loadPreferences()).toMatchObject({
      defaultDashboardRange: "year",
      budgetUsd: null,
      reminderThresholdPct: 1,
    });
  });

  it("persists and reloads theme preference", () => {
    savePreferences({
      ...defaultPreferences,
      themePreference: "dark",
    });

    expect(loadPreferences().themePreference).toBe("dark");
  });

  it("falls back to system when stored themePreference is missing or invalid", () => {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({ themePreference: "sepia" }),
    );

    expect(loadPreferences().themePreference).toBe("system");
  });

  it("falls back to a resolved promise when queueMicrotask is unavailable", async () => {
    const originalQueueMicrotask = globalThis.queueMicrotask;
    const dispatchSpy = vi.spyOn(window, "dispatchEvent");
    Object.defineProperty(globalThis, "queueMicrotask", {
      value: undefined,
      configurable: true,
    });

    try {
      notifyPreferencesUpdated(defaultPreferences);
      expect(dispatchSpy).not.toHaveBeenCalled();
      await Promise.resolve();
      expect(dispatchSpy).toHaveBeenCalledTimes(1);
    } finally {
      Object.defineProperty(globalThis, "queueMicrotask", {
        value: originalQueueMicrotask,
        configurable: true,
      });
      dispatchSpy.mockRestore();
    }
  });
});
