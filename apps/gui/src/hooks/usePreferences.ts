import { useCallback, useEffect, useRef, useState } from "react";
import {
  defaultPreferences,
  loadPreferences,
  PREFERENCES_UPDATED_EVENT,
  persistPreferences,
  notifyPreferencesUpdated,
  type ConsumerPreferences,
} from "../lib/preferencesStorage";

export function usePreferences() {
  const [preferences, setPreferences] = useState<ConsumerPreferences>(() => {
    try {
      return loadPreferences();
    } catch {
      return defaultPreferences;
    }
  });

  const preferencesRef = useRef(preferences);

  useEffect(() => {
    preferencesRef.current = preferences;
  }, [preferences]);

  useEffect(() => {
    const syncFromStorage = () => setPreferences(loadPreferences());
    const syncFromCustomEvent = (event: Event) => {
      const detail = (event as CustomEvent<ConsumerPreferences>).detail;
      setPreferences(detail ?? loadPreferences());
    };

    window.addEventListener("storage", syncFromStorage);
    window.addEventListener(PREFERENCES_UPDATED_EVENT, syncFromCustomEvent as EventListener);
    return () => {
      window.removeEventListener("storage", syncFromStorage);
      window.removeEventListener(PREFERENCES_UPDATED_EVENT, syncFromCustomEvent as EventListener);
    };
  }, []);

  const updatePreference = useCallback(<K extends keyof ConsumerPreferences>(key: K, value: ConsumerPreferences[K]) => {
    const next = persistPreferences({
      ...preferencesRef.current,
      [key]: value,
    });

    preferencesRef.current = next;
    setPreferences(next);
    notifyPreferencesUpdated(next);
  }, []);

  return { preferences, updatePreference };
}
