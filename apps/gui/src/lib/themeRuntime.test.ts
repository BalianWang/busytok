import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  PREFERENCES_UPDATED_EVENT,
  PREFERENCES_STORAGE_KEY,
  defaultPreferences,
} from "./preferencesStorage";
import * as reporter from "../logging/reporter";

// ---------- localStorage helper (shared shape with other suites) ----------

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

// ---------- matchMedia mock ----------

type ChangeListener = (event: { matches: boolean }) => void;

interface StubMediaQueryList {
  matches: boolean;
  media: string;
  onchange: ChangeListener | null;
  addEventListener: (
    type: "change",
    listener: ChangeListener,
    options?: unknown,
  ) => void;
  removeEventListener: (
    type: "change",
    listener: ChangeListener,
    options?: unknown,
  ) => void;
  addListener: (listener: ChangeListener) => void;
  removeListener: (listener: ChangeListener) => void;
  dispatchChange: (event: { matches: boolean }) => void;
}

let currentMatches = false;
const mqlByMedia = new Map<string, StubMediaQueryList>();

function installMatchMedia(): void {
  currentMatches = false;
  mqlByMedia.clear();

  const factory = (media: string): StubMediaQueryList => {
    const existing = mqlByMedia.get(media);
    if (existing) return existing;

    const listeners = new Set<ChangeListener>();
    const mql: StubMediaQueryList = {
      matches: currentMatches,
      media,
      onchange: null,
      addEventListener: (type, listener) => {
        if (type === "change") listeners.add(listener);
      },
      removeEventListener: (type, listener) => {
        if (type === "change") listeners.delete(listener);
      },
      addListener: (listener) => listeners.add(listener),
      removeListener: (listener) => listeners.delete(listener),
      dispatchChange: (event) => {
        mql.matches = event.matches;
        for (const listener of listeners) listener(event);
        if (mql.onchange) mql.onchange(event);
      },
    };
    mqlByMedia.set(media, mql);
    return mql;
  };

  vi.stubGlobal("matchMedia", vi.fn(factory));
}

function mockMatchMedia(options: { matches: boolean }): void {
  currentMatches = options.matches;
  for (const mql of mqlByMedia.values()) {
    mql.matches = options.matches;
  }
}

function triggerMatchMedia(matches: boolean): void {
  // Dispatch on every registered MQL so the listener attached by the runtime
  // receives the change event regardless of which `matchMedia(query)` call
  // returned the list the runtime is subscribed to. When the runtime is on an
  // explicit preference it never queries matchMedia, so there is nothing to
  // dispatch — that's the correct no-op behavior.
  for (const mql of mqlByMedia.values()) {
    mql.dispatchChange({ matches });
  }
}

// ---------- Imports under test (after mocks are ready) ----------

import {
  applyResolvedTheme,
  disposeThemeRuntime,
  initThemeRuntime,
  resolveTheme,
  startThemeRuntime,
} from "./themeRuntime";

// ---------- Reset helpers ----------

function flushMicrotasks(): Promise<void> {
  // Two ticks to clear any queueMicrotask chains from savePreferences broadcast.
  return Promise.resolve().then(() => Promise.resolve());
}

function clearDocumentTheme(): void {
  delete document.documentElement.dataset.theme;
  document.documentElement.style.colorScheme = "";
}

describe("themeRuntime", () => {
  beforeEach(() => {
    storage.reset();
    clearDocumentTheme();
    installMatchMedia();
    vi.spyOn(reporter, "reportFrontendEvent").mockImplementation(() => {});
  });

  afterEach(() => {
    disposeThemeRuntime();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  describe("resolveTheme", () => {
    it("resolves explicit light preference", () => {
      expect(resolveTheme("light")).toBe("light");
    });

    it("resolves explicit dark preference", () => {
      expect(resolveTheme("dark")).toBe("dark");
    });

    it("resolves system preference to dark when matchMedia matches", () => {
      mockMatchMedia({ matches: true });
      expect(resolveTheme("system")).toBe("dark");
    });

    it("resolves system preference to light when matchMedia does not match", () => {
      mockMatchMedia({ matches: false });
      expect(resolveTheme("system")).toBe("light");
    });
  });

  describe("applyResolvedTheme", () => {
    it("applies theme to documentElement and colorScheme", () => {
      applyResolvedTheme("light");
      expect(document.documentElement.dataset.theme).toBe("light");
      expect(document.documentElement.style.colorScheme).toBe("light");
    });

    it("updates dataset.theme and style.colorScheme on dark", () => {
      applyResolvedTheme("dark");
      expect(document.documentElement.dataset.theme).toBe("dark");
      expect(document.documentElement.style.colorScheme).toBe("dark");
    });
  });

  describe("startThemeRuntime", () => {
    it("defaults to system and applies the resolved theme at start", () => {
      mockMatchMedia({ matches: true });
      const runtime = startThemeRuntime();
      try {
        expect(document.documentElement.dataset.theme).toBe("dark");
      } finally {
        runtime.dispose();
      }
    });

    it("switches light -> dark -> system at runtime without reload", async () => {
      mockMatchMedia({ matches: false });
      const runtime = startThemeRuntime();
      try {
        runtime.setPreference("dark");
        expect(document.documentElement.dataset.theme).toBe("dark");

        runtime.setPreference("system");
        // system with matches:false resolves to light
        expect(document.documentElement.dataset.theme).toBe("light");

        triggerMatchMedia(true);
        expect(document.documentElement.dataset.theme).toBe("dark");
      } finally {
        runtime.dispose();
      }
    });

    it("unsubscribes from matchMedia when switching away from system", async () => {
      mockMatchMedia({ matches: true });
      const runtime = startThemeRuntime();
      try {
        // While on system, a matchMedia change must be observed.
        triggerMatchMedia(false);
        expect(document.documentElement.dataset.theme).toBe("light");

        // Switching to an explicit preference must detach the listener.
        runtime.setPreference("dark");
        expect(document.documentElement.dataset.theme).toBe("dark");

        // Subsequent matchMedia changes must NOT mutate the applied theme.
        triggerMatchMedia(true);
        expect(document.documentElement.dataset.theme).toBe("dark");
      } finally {
        runtime.dispose();
      }
    });

    it("re-subscribes to matchMedia when returning to system", async () => {
      mockMatchMedia({ matches: false });
      const runtime = startThemeRuntime();
      try {
        runtime.setPreference("dark");
        expect(document.documentElement.dataset.theme).toBe("dark");

        runtime.setPreference("system");
        // system with matches:false -> light
        expect(document.documentElement.dataset.theme).toBe("light");

        triggerMatchMedia(true);
        expect(document.documentElement.dataset.theme).toBe("dark");
      } finally {
        runtime.dispose();
      }
    });

    it("applies persisted preference at start", () => {
      localStorage.setItem(
        PREFERENCES_STORAGE_KEY,
        JSON.stringify({ ...defaultPreferences, themePreference: "dark" }),
      );
      mockMatchMedia({ matches: false });

      const runtime = startThemeRuntime();
      try {
        expect(document.documentElement.dataset.theme).toBe("dark");
      } finally {
        runtime.dispose();
      }
    });

    it("applies live preference updates from PREFERENCES_UPDATED_EVENT", async () => {
      mockMatchMedia({ matches: false });
      const runtime = startThemeRuntime();
      try {
        expect(document.documentElement.dataset.theme).toBe("light");

        window.dispatchEvent(
          new CustomEvent(PREFERENCES_UPDATED_EVENT, {
            detail: { ...defaultPreferences, themePreference: "dark" },
          }),
        );
        await flushMicrotasks();

        expect(document.documentElement.dataset.theme).toBe("dark");
      } finally {
        runtime.dispose();
      }
    });

    it("applies cross-window preference updates from storage events", () => {
      mockMatchMedia({ matches: false });
      const runtime = startThemeRuntime();
      try {
        expect(document.documentElement.dataset.theme).toBe("light");

        // Simulate another window writing a new themePreference to localStorage.
        // The `storage` event fires only in OTHER windows in browsers; in tests
        // we dispatch it directly to mirror the cross-window contract.
        window.dispatchEvent(
          new StorageEvent("storage", {
            key: PREFERENCES_STORAGE_KEY,
            newValue: JSON.stringify({ ...defaultPreferences, themePreference: "dark" }),
          }),
        );

        expect(document.documentElement.dataset.theme).toBe("dark");
      } finally {
        runtime.dispose();
      }
    });

    it("ignores storage events for unrelated keys", () => {
      mockMatchMedia({ matches: false });
      const runtime = startThemeRuntime();
      try {
        window.dispatchEvent(
          new StorageEvent("storage", {
            key: "some-other-storage-key",
            newValue: JSON.stringify({ ...defaultPreferences, themePreference: "dark" }),
          }),
        );

        // Unrelated key — current theme is preserved.
        expect(document.documentElement.dataset.theme).toBe("light");
      } finally {
        runtime.dispose();
      }
    });

    it("falls back silently when storage event clears the value (newValue: null)", () => {
      // Documented behavior: cleared storage does not change the live preference.
      // The runtime keeps the current applied theme; the next reload will pick up
      // the empty storage and re-default to `system`.
      mockMatchMedia({ matches: false });
      const runtime = startThemeRuntime();
      try {
        runtime.setPreference("dark");
        expect(document.documentElement.dataset.theme).toBe("dark");

        expect(() => {
          window.dispatchEvent(
            new StorageEvent("storage", {
              key: PREFERENCES_STORAGE_KEY,
              newValue: null,
            }),
          );
        }).not.toThrow();

        // Preference unchanged — stays at dark.
        expect(document.documentElement.dataset.theme).toBe("dark");
      } finally {
        runtime.dispose();
      }
    });

    it("falls back silently on malformed JSON in storage event payload", () => {
      mockMatchMedia({ matches: false });
      const runtime = startThemeRuntime();
      try {
        expect(document.documentElement.dataset.theme).toBe("light");

        expect(() => {
          window.dispatchEvent(
            new StorageEvent("storage", {
              key: PREFERENCES_STORAGE_KEY,
              newValue: "{ not valid json",
            }),
          );
        }).not.toThrow();

        // Malformed payload must not mutate the applied theme.
        expect(document.documentElement.dataset.theme).toBe("light");
      } finally {
        runtime.dispose();
      }
    });

    it("emits gui.theme.system_changed when the resolved theme changes via matchMedia", async () => {
      const spy = vi.mocked(reporter.reportFrontendEvent);
      mockMatchMedia({ matches: false });
      const runtime = startThemeRuntime();
      spy.mockClear();
      try {
        triggerMatchMedia(true);
        expect(spy).toHaveBeenCalledWith(
          expect.objectContaining({
            event_code: "gui.theme.system_changed",
          }),
        );
      } finally {
        runtime.dispose();
      }
    });

    it("emits gui.theme.resolved_changed (not system_changed) when setPreference shifts the resolved theme", () => {
      const spy = vi.mocked(reporter.reportFrontendEvent);
      mockMatchMedia({ matches: false });
      const runtime = startThemeRuntime();
      spy.mockClear();
      try {
        // system -> light is the starting resolved theme; switching to dark
        // is a user-driven resolved-theme shift and must emit resolved_changed.
        runtime.setPreference("dark");
        expect(spy).toHaveBeenCalledWith(
          expect.objectContaining({
            event_code: "gui.theme.resolved_changed",
          }),
        );
        // And must NOT emit system_changed for a preference change.
        const systemChangedCalls = spy.mock.calls.filter(
          (call) =>
            (call[0] as { event_code?: string }).event_code ===
            "gui.theme.system_changed",
        );
        expect(systemChangedCalls).toHaveLength(0);
      } finally {
        runtime.dispose();
      }
    });

    it("does not re-apply or log when setPreference receives the same value", () => {
      mockMatchMedia({ matches: false });
      const runtime = startThemeRuntime();
      const spy = vi.mocked(reporter.reportFrontendEvent);
      spy.mockClear();
      try {
        runtime.setPreference("system");
        // Still system, no resolved change — should be a no-op.
        expect(spy).not.toHaveBeenCalled();
        expect(document.documentElement.dataset.theme).toBe("light");
      } finally {
        runtime.dispose();
      }
    });

    it("does not emit a system_changed event when matchMedia change keeps the resolved theme the same", () => {
      // Start on an explicit dark preference so matchMedia changes are inert.
      localStorage.setItem(
        PREFERENCES_STORAGE_KEY,
        JSON.stringify({ ...defaultPreferences, themePreference: "dark" }),
      );
      mockMatchMedia({ matches: false });
      const runtime = startThemeRuntime();
      const spy = vi.mocked(reporter.reportFrontendEvent);
      spy.mockClear();
      try {
        triggerMatchMedia(true);
        expect(spy).not.toHaveBeenCalled();
        expect(document.documentElement.dataset.theme).toBe("dark");
      } finally {
        runtime.dispose();
      }
    });
  });

  describe("initThemeRuntime singleton", () => {
    it("starts the singleton exactly once across multiple calls", () => {
      mockMatchMedia({ matches: true });
      initThemeRuntime();
      initThemeRuntime();
      initThemeRuntime();

      expect(document.documentElement.dataset.theme).toBe("dark");
    });

    it("disposeThemeRuntime is idempotent when called multiple times", () => {
      disposeThemeRuntime();
      disposeThemeRuntime();
      // no throw is the contract
      expect(true).toBe(true);
    });

    it("initThemeRuntime can re-init after dispose", () => {
      mockMatchMedia({ matches: false });
      initThemeRuntime();
      expect(document.documentElement.dataset.theme).toBe("light");
      disposeThemeRuntime();

      clearDocumentTheme();
      mockMatchMedia({ matches: true });

      initThemeRuntime();
      expect(document.documentElement.dataset.theme).toBe("dark");
      disposeThemeRuntime();
    });

    it("initThemeRuntime emits a bootstrap event", () => {
      const spy = vi.mocked(reporter.reportFrontendEvent);
      spy.mockClear();
      initThemeRuntime();
      try {
        expect(spy).toHaveBeenCalledWith(
          expect.objectContaining({
            event_code: "gui.theme.bootstrap",
          }),
        );
      } finally {
        disposeThemeRuntime();
      }
    });
  });
});
