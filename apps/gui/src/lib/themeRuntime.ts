/**
 * Theme runtime for Busytok.
 *
 * Owns live application of the resolved color theme to `document.documentElement`.
 * No React state lives here — the runtime is a module-level singleton with an
 * explicit init/dispose contract so production bootstrap (via `main.tsx`) and
 * tests share one lifecycle.
 */

import {
  PREFERENCES_UPDATED_EVENT,
  PREFERENCES_STORAGE_KEY,
  defaultPreferences,
  loadPreferences,
  type ConsumerPreferences,
  type ThemePreference,
} from "./preferencesStorage";
import { reportFrontendEvent } from "../logging/reporter";

export type ResolvedTheme = "light" | "dark";

const DARK_MEDIA_QUERY = "(prefers-color-scheme: dark)";

type MediaQueryListener = (event: { matches: boolean }) => void;

interface MediaQueryListLike {
  matches: boolean;
  addEventListener(type: "change", listener: MediaQueryListener): void;
  removeEventListener(type: "change", listener: MediaQueryListener): void;
}

interface PreferencesUpdatedListener {
  (event: Event): void;
}

interface StorageEventListener {
  (event: StorageEvent): void;
}

/**
 * Parse a `storage` event payload (raw localStorage value written by another
 * window) and extract the theme preference, if any. Returns `null` for missing
 * or malformed payloads so callers can fall back silently.
 */
function readThemePreferenceFromStorageValue(rawValue: string | null): ThemePreference | null {
  if (rawValue == null) return null;
  try {
    const parsed = JSON.parse(rawValue) as Partial<ConsumerPreferences>;
    const theme = parsed?.themePreference;
    if (theme === "light" || theme === "dark" || theme === "system") {
      return theme;
    }
  } catch {
    // Malformed JSON — fall back silently.
  }
  return null;
}

export interface ThemeRuntime {
  /** Replace the active preference and re-apply the resolved theme immediately. */
  setPreference(preference: ThemePreference): void;
  /** Stop the runtime: remove DOM listeners, drop matchMedia subscription. */
  dispose(): void;
}

// ---------- Pure helpers ----------

/**
 * Resolve a user-facing preference to the concrete theme that should be applied.
 * `system` consults the OS via `matchMedia`; explicit choices pass through.
 */
export function resolveTheme(preference: ThemePreference): ResolvedTheme {
  if (prefersDarkSystem()) {
    return "dark";
  }
  return "light";

  function prefersDarkSystem(): boolean {
    if (preference !== "system") {
      return preference === "dark";
    }
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
      return false;
    }
    return window.matchMedia(DARK_MEDIA_QUERY).matches;
  }
}

/**
 * Apply a concrete theme to `document.documentElement` so CSS token overrides
 * and the UA-level color-scheme both reflect the choice.
 */
export function applyResolvedTheme(theme: ResolvedTheme): void {
  if (typeof document === "undefined") return;
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
}

// ---------- Runtime ----------

interface ThemeRuntimeState {
  preference: ThemePreference;
  resolved: ResolvedTheme;
  mql: MediaQueryListLike | null;
  onMediaChange: MediaQueryListener;
  onPreferencesUpdated: PreferencesUpdatedListener;
  onStorage: StorageEventListener;
}

function readInitialPreference(): ThemePreference {
  try {
    return loadPreferences().themePreference ?? defaultPreferences.themePreference;
  } catch {
    return defaultPreferences.themePreference;
  }
}

function getSystemMediaQueryList(): MediaQueryListLike | null {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    return null;
  }
  return window.matchMedia(DARK_MEDIA_QUERY) as MediaQueryListLike;
}

/**
 * Construct and start a standalone runtime instance. Tests use this directly
 * to inspect behavior without going through the module-level singleton.
 * Returns a handle with `setPreference` and `dispose` for explicit cleanup.
 */
export function startThemeRuntime(): ThemeRuntime {
  return startThemeRuntimeInternal();
}

function startThemeRuntimeInternal(): ThemeRuntime {
  const preference = readInitialPreference();
  const resolved = resolveTheme(preference);
  applyResolvedTheme(resolved);

  const state: ThemeRuntimeState = {
    preference,
    resolved,
    mql: null,
    onMediaChange: () => {
      // OS preference flip — emit gui.theme.system_changed.
      applyResolvedIfChanged(state, "system");
    },
    onPreferencesUpdated: (event) => {
      const detail = (event as CustomEvent<ConsumerPreferences>).detail;
      if (!detail) return;
      setPreferenceImpl(state, detail.themePreference ?? defaultPreferences.themePreference);
    },
    onStorage: (event) => {
      // Cross-window sync: another window (e.g. main app Settings) wrote a
      // new preference to localStorage. The in-window PREFERENCES_UPDATED_EVENT
      // is not visible here, so mirror it from the storage event payload.
      if (event.key !== PREFERENCES_STORAGE_KEY) return;
      const next = readThemePreferenceFromStorageValue(event.newValue);
      if (next) setPreferenceImpl(state, next);
    },
  };

  reconfigureSystemWatcher(state, preference);

  window.addEventListener(PREFERENCES_UPDATED_EVENT, state.onPreferencesUpdated);
  window.addEventListener("storage", state.onStorage);

  return {
    setPreference: (next) => setPreferenceImpl(state, next),
    dispose: () => disposeState(state),
  };
}

function setPreferenceImpl(state: ThemeRuntimeState, preference: ThemePreference): void {
  if (state.preference === preference) {
    return;
  }
  state.preference = preference;
  reconfigureSystemWatcher(state, preference);
  // User-driven resolved theme shift — emit gui.theme.resolved_changed.
  applyResolvedIfChanged(state, "resolved");
}

/**
 * Resolve the active preference to a concrete theme, apply it if it differs
 * from the current applied theme, and emit a change log. The event code is
 * chosen by the caller so OS-driven (`system`) and user-driven (`resolved`)
 * changes are distinguishable in log analysis.
 */
function applyResolvedIfChanged(
  state: ThemeRuntimeState,
  trigger: "system" | "resolved",
): void {
  const next = resolveTheme(state.preference);
  if (next === state.resolved) {
    return;
  }
  state.resolved = next;
  applyResolvedTheme(next);
  reportFrontendEvent({
    level: "INFO",
    event_code:
      trigger === "system"
        ? "gui.theme.system_changed"
        : "gui.theme.resolved_changed",
    message: "Resolved theme changed",
    details: {
      preference: state.preference,
      resolved: next,
    },
  });
}

function reconfigureSystemWatcher(
  state: ThemeRuntimeState,
  preference: ThemePreference,
): void {
  // Detach any previous subscription first.
  if (state.mql) {
    state.mql.removeEventListener("change", state.onMediaChange);
    state.mql = null;
  }
  if (preference !== "system") {
    return;
  }
  const mql = getSystemMediaQueryList();
  if (!mql) return;
  state.mql = mql;
  mql.addEventListener("change", state.onMediaChange);
}

function disposeState(state: ThemeRuntimeState): void {
  if (state.mql) {
    state.mql.removeEventListener("change", state.onMediaChange);
    state.mql = null;
  }
  if (typeof window !== "undefined") {
    window.removeEventListener(PREFERENCES_UPDATED_EVENT, state.onPreferencesUpdated);
    window.removeEventListener("storage", state.onStorage);
  }
}

// ---------- Module-level singleton ----------

let singleton: ThemeRuntime | null = null;

/**
 * Start the singleton runtime exactly once. Safe to call from `main.tsx`
 * before the first React render; StrictMode remounts will not double-start.
 */
export function initThemeRuntime(): void {
  if (singleton) return;
  singleton = startThemeRuntimeInternal();
  reportFrontendEvent({
    level: "INFO",
    event_code: "gui.theme.bootstrap",
    message: "Theme runtime started",
    details: {
      preference: readInitialPreference(),
    },
  });
}

/**
 * Stop and clear the singleton runtime. Idempotent — calling repeatedly is a
 * no-op. Reserved for tests and explicit non-React shutdown paths.
 */
export function disposeThemeRuntime(): void {
  if (!singleton) return;
  singleton.dispose();
  singleton = null;
}
