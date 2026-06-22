import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { usePreferences } from "./usePreferences";
import { PREFERENCES_UPDATED_EVENT, type ConsumerPreferences } from "../lib/preferencesStorage";

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

describe("usePreferences", () => {
  beforeEach(() => {
    storage.reset();
  });

  it("hydrates stored preferences", () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({
      defaultDashboardRange: "year",
      budgetUsd: 40,
      reminderEnabled: true,
      reminderThresholdPct: 80,
    }));

    const { result, unmount } = renderHook(() => usePreferences());

    expect(result.current.preferences.defaultDashboardRange).toBe("year");
    expect(result.current.preferences.budgetUsd).toBe(40);
    unmount();
  });

  it("persists once and broadcasts updates after the current stack", async () => {
    const dispatchSpy = vi.spyOn(window, "dispatchEvent");
    const first = renderHook(() => usePreferences());
    const second = renderHook(() => usePreferences());

    act(() => {
      first.result.current.updatePreference("defaultDashboardRange", "month");
      expect(dispatchSpy).not.toHaveBeenCalled();
    });

    expect(storage.setItem).toHaveBeenCalledTimes(1);
    expect(first.result.current.preferences.defaultDashboardRange).toBe("month");
    expect(second.result.current.preferences.defaultDashboardRange).toBe("day");

    await waitFor(() => {
      expect(dispatchSpy).toHaveBeenCalledTimes(1);
      expect(second.result.current.preferences.defaultDashboardRange).toBe("month");
    });

    first.unmount();
    second.unmount();
    dispatchSpy.mockRestore();
  });

  it("updates preferences and persists them", async () => {
    const { result, unmount } = renderHook(() => usePreferences());

    act(() => {
      result.current.updatePreference("defaultDashboardRange", "month");
    });

    expect(result.current.preferences.defaultDashboardRange).toBe("month");
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}").defaultDashboardRange).toBe("month");
    await waitFor(() => {
      expect(storage.setItem).toHaveBeenCalledTimes(1);
    });
    unmount();
  });

  it("falls back to loadPreferences when custom event has no detail", async () => {
    const first = renderHook(() => usePreferences());
    const second = renderHook(() => usePreferences());

    // Store a value so loadPreferences returns something different from default
    act(() => {
      first.result.current.updatePreference("defaultDashboardRange", "year");
    });

    await waitFor(() => {
      expect(first.result.current.preferences.defaultDashboardRange).toBe("year");
    });

    // Dispatch a custom event with no detail (null)
    act(() => {
      window.dispatchEvent(new CustomEvent(PREFERENCES_UPDATED_EVENT, { detail: undefined as unknown as ConsumerPreferences }));
    });

    // The second hook should fall back to loadPreferences() which returns the stored value
    await waitFor(() => {
      expect(second.result.current.preferences.defaultDashboardRange).toBe("year");
    });

    first.unmount();
    second.unmount();
  });
});
