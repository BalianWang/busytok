/** Time range presets for the Busytok dashboard. */
export type RangePreset = "day" | "week" | "month" | "year";

/** User-facing theme preference. `system` defers to the OS via matchMedia. */
export type ThemePreference = "system" | "light" | "dark";

export const PREFERENCES_STORAGE_KEY = "busytok.consumer.preferences.v1";
export const PREFERENCES_UPDATED_EVENT = "busytok:preferences-updated";

export interface ConsumerPreferences {
  defaultDashboardRange: RangePreset;
  budgetUsd: number | null;
  reminderEnabled: boolean;
  reminderThresholdPct: number;
  themePreference: ThemePreference;
}

export const defaultPreferences: ConsumerPreferences = {
  defaultDashboardRange: "day",
  budgetUsd: null,
  reminderEnabled: false,
  reminderThresholdPct: 80,
  themePreference: "system",
};

const THEME_PREFERENCE_VALUES: readonly ThemePreference[] = ["system", "light", "dark"];

function sanitizeThemePreference(input: unknown): ThemePreference {
  return THEME_PREFERENCE_VALUES.includes(input as ThemePreference)
    ? (input as ThemePreference)
    : defaultPreferences.themePreference;
}

export function loadPreferences(): ConsumerPreferences {
  const raw = localStorage.getItem(PREFERENCES_STORAGE_KEY);
  if (!raw) {
    return defaultPreferences;
  }

  try {
    const parsed = JSON.parse(raw) as Partial<ConsumerPreferences>;
    return sanitizePreferences(parsed);
  } catch {
    return defaultPreferences;
  }
}

export function persistPreferences(preferences: ConsumerPreferences): ConsumerPreferences {
  const sanitized = sanitizePreferences(preferences);
  localStorage.setItem(PREFERENCES_STORAGE_KEY, JSON.stringify(sanitized));
  return sanitized;
}

export function notifyPreferencesUpdated(preferences: ConsumerPreferences): void {
  const sanitized = sanitizePreferences(preferences);
  queuePreferenceBroadcast(() => {
    window.dispatchEvent(new CustomEvent(PREFERENCES_UPDATED_EVENT, { detail: sanitized }));
  });
}

export function savePreferences(preferences: ConsumerPreferences): void {
  const sanitized = persistPreferences(preferences);
  notifyPreferencesUpdated(sanitized);
}

function sanitizePreferences(input: Partial<ConsumerPreferences>): ConsumerPreferences {
  const range = ["day", "week", "month", "year"].includes(String(input.defaultDashboardRange))
    ? (input.defaultDashboardRange as RangePreset)
    : defaultPreferences.defaultDashboardRange;
  const budgetUsd = typeof input.budgetUsd === "number" && Number.isFinite(input.budgetUsd) && input.budgetUsd > 0
    ? input.budgetUsd
    : null;
  const reminderThresholdPct = typeof input.reminderThresholdPct === "number" && Number.isFinite(input.reminderThresholdPct)
    ? Math.min(100, Math.max(1, Math.round(input.reminderThresholdPct)))
    : defaultPreferences.reminderThresholdPct;

  return {
    defaultDashboardRange: range,
    budgetUsd,
    reminderEnabled: Boolean(input.reminderEnabled),
    reminderThresholdPct,
    themePreference: sanitizeThemePreference(input.themePreference),
  };
}

function queuePreferenceBroadcast(callback: () => void): void {
  if (typeof queueMicrotask === "function") {
    queueMicrotask(callback);
    return;
  }

  Promise.resolve().then(callback);
}
