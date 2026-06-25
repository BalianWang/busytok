import {
  createContext,
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import {
  applyUpdate,
  checkForUpdate,
  type ApplyOutcome,
  type CheckOutcome,
  type DownloadProgress,
} from "../lib/updaterClient";
import type { Update } from "@tauri-apps/plugin-updater";
import { reportFrontendEventSafely } from "../logging/safeReporter";

/** Re-check interval while the app is open. */
const POLL_INTERVAL_MS = 12 * 60 * 60 * 1000; // 12h
/** Re-check on focus only if the last check was older than this. */
const FOCUS_RECHECK_MS = 60 * 60 * 1000; // 1h

export type UpdaterStatus =
  | { state: "idle" }
  | { state: "checking" }
  | { state: "up-to-date" }
  | { state: "available"; version: string; notes: string; date: string }
  | { state: "downloading"; percent: number | null }
  | { state: "installed-pending-restart" }
  | { state: "installed-needs-manual-restart"; version: string }
  | { state: "error"; message: string };

export interface UpdaterContextValue {
  status: UpdaterStatus;
  /** Running app version (semver), or null while loading/unknown. */
  currentVersion: string | null;
  checkNow: () => Promise<void>;
  applyNow: () => Promise<void>;
}

// Default value (idle) so consumers rendered without a provider — e.g. AppShell
// in isolation tests — read a safe idle status instead of throwing. Mirrors
// EventSubscriptionProvider.
const DEFAULT_UPDATER_VALUE: UpdaterContextValue = {
  status: { state: "idle" },
  currentVersion: null,
  checkNow: async () => {},
  applyNow: async () => {},
};
export const UpdaterContext = createContext<UpdaterContextValue>(DEFAULT_UPDATER_VALUE);

export function UpdaterProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<UpdaterStatus>({ state: "idle" });
  const [currentVersion, setCurrentVersion] = useState<string | null>(null);
  // Synchronous mirror of currentVersion for reads inside async runCheck
  // (avoids a stale-closure / race where the check telemetry would capture a
  // not-yet-loaded version). Written by the loader effect below.
  const appVersionRef = useRef<string | null>(null);

  // The live Tauri Update is a server-side resource (D8): hold it in a ref,
  // never React state; close before swap + on unmount.
  const updateRef = useRef<Update | null>(null);
  const lastCheckAtRef = useRef<number>(0);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const downloadedBytesRef = useRef<number>(0);
  const didMountCheckRef = useRef(false);
  // True while applyNow's download is in flight. Guards runCheck so a
  // 12h-interval or focus re-check can't closeHeld() the in-use Update or
  // swap updateRef.current mid-download (download errors + stale metadata).
  const downloadingRef = useRef(false);

  const closeHeld = useCallback(() => {
    const u = updateRef.current;
    if (u) {
      updateRef.current = null;
      void u.close().catch(() => { /* best-effort */ });
    }
  }, []);

  const runCheck = useCallback(async () => {
    // Don't check mid-download: closeHeld()/ref-swap would break the in-flight
    // download and surface wrong metadata (mount, interval, focus, manual).
    if (downloadingRef.current) return;
    setStatus({ state: "checking" });
    const outcome: CheckOutcome = await checkForUpdate();
    lastCheckAtRef.current = Date.now();
    const appVersion = appVersionRef.current;
    if (outcome.kind === "up-to-date") {
      closeHeld();
      reportFrontendEventSafely({
        level: "INFO",
        event_code: "gui.update.checked",
        message: "Up to date",
        details: { currentVersion: appVersion },
      });
      setStatus({ state: "up-to-date" });
    } else if (outcome.kind === "available") {
      closeHeld();
      updateRef.current = outcome.update;
      reportFrontendEventSafely({
        level: "INFO",
        event_code: "gui.update.checked",
        message: "Update available",
        details: { currentVersion: appVersion, latestVersion: outcome.version },
      });
      setStatus({ state: "available", version: outcome.version, notes: outcome.notes, date: outcome.date });
    } else {
      reportFrontendEventSafely({
        level: "WARN",
        event_code: "gui.update.check_failed",
        message: "Update check failed",
        details: { message: outcome.message },
      });
      setStatus({ state: "error", message: outcome.message });
    }
  }, [closeHeld]);

  const checkNow = useCallback(async () => {
    await runCheck();
    // D11: a manual check resets the poll interval.
    if (intervalRef.current) clearInterval(intervalRef.current);
    intervalRef.current = setInterval(() => { void runCheck(); }, POLL_INTERVAL_MS);
  }, [runCheck]);

  const applyNow = useCallback(async () => {
    const update = updateRef.current;
    if (!update) return;
    downloadingRef.current = true;
    downloadedBytesRef.current = 0;
    setStatus({ state: "downloading", percent: null });
    const onProgress = (p: DownloadProgress) => {
      downloadedBytesRef.current += p.chunkLength;
      const percent = p.contentLength ? Math.min(100, Math.round((downloadedBytesRef.current / p.contentLength) * 100)) : null;
      setStatus({ state: "downloading", percent });
    };
    try {
      const outcome: ApplyOutcome = await applyUpdate(update, onProgress);
      if (outcome.kind === "updated") {
        reportFrontendEventSafely({ level: "INFO", event_code: "gui.update.applied", message: "Update applied", details: { version: outcome.version } });
        setStatus({ state: "installed-pending-restart" });
      } else if (outcome.kind === "needs-manual-restart") {
        reportFrontendEventSafely({ level: "WARN", event_code: "gui.update.relaunch_failed", message: "Relaunch failed; manual restart needed", details: { version: outcome.version } });
        setStatus({ state: "installed-needs-manual-restart", version: outcome.version });
      } else {
        // Download/install error: Update is still valid → return to available for retry.
        // Re-read the current Update (defense-in-depth: with the downloadingRef guard
        // updateRef.current is unchanged here, but don't rely on the captured local).
        const current = updateRef.current;
        reportFrontendEventSafely({ level: "ERROR", event_code: "gui.update.download_failed", message: "Update download/install failed", details: { message: outcome.message } });
        setStatus(current
          ? { state: "available", version: current.version, notes: current.body ?? "", date: current.date ?? "" }
          : { state: "error", message: outcome.message });
      }
    } finally {
      downloadingRef.current = false;
    }
    // Deps []: applyNow reads only refs (updateRef, downloadedBytesRef), the
    // module-level applyUpdate/reportFrontendEventSafely imports, and setStatus —
    // all stable, so [] is correct and avoids a stale closure.
  }, []);

  // Load the running app version once. Declared before the mount-check effect.
  // runCheck() awaits checkForUpdate() (Tauri IPC + network), so getVersion()
  // resolves first in practice, making currentVersion available for telemetry.
  useEffect(() => {
    let cancelled = false;
    void getVersion()
      .then((v) => {
        if (cancelled) return;
        appVersionRef.current = v;
        setCurrentVersion(v);
      })
      .catch(() => {
        // getVersion is a local Tauri call that essentially never fails; on
        // failure leave currentVersion null (UI shows the loading placeholder).
        if (!cancelled) setCurrentVersion(null);
      });
    return () => { cancelled = true; };
  }, []);

  // Mount: one check (StrictMode-safe), the 12h interval, and a focus listener.
  useEffect(() => {
    if (!didMountCheckRef.current) {
      didMountCheckRef.current = true;
      void runCheck();
    }
    intervalRef.current = setInterval(() => { void runCheck(); }, POLL_INTERVAL_MS);
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void getCurrentWindow().onFocusChanged((event) => {
      if (event.payload && Date.now() - lastCheckAtRef.current > FOCUS_RECHECK_MS) {
        void runCheck();
      }
    }).then((fn) => { if (cancelled) fn(); else unlisten = fn; });
    return () => {
      cancelled = true;
      if (intervalRef.current) clearInterval(intervalRef.current);
      unlisten?.();
      closeHeld();
    };
  }, [runCheck, closeHeld]);

  return (
    <UpdaterContext.Provider value={{ status, currentVersion, checkNow, applyNow }}>
      {children}
    </UpdaterContext.Provider>
  );
}
