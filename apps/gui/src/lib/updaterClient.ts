import { check, type Update, type DownloadEvent } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

/** Bounded timeout (ms) for the update *check* HTTP request. */
export const CHECK_TIMEOUT_MS = 20_000;
/** Bounded timeout (ms) for the update *download*. */
export const DOWNLOAD_TIMEOUT_MS = 120_000;

export type CheckOutcome =
  | { kind: "up-to-date" }
  | { kind: "available"; version: string; notes: string; date: string; update: Update }
  | { kind: "error"; message: string };

export type ApplyOutcome =
  | { kind: "updated"; version: string }
  | { kind: "needs-manual-restart"; version: string }
  | { kind: "error"; message: string };

export interface DownloadProgress {
  chunkLength: number;
  contentLength?: number;
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Detect (only) whether an update is available. Never applies. */
export async function checkForUpdate(): Promise<CheckOutcome> {
  try {
    const update = await check({ timeout: CHECK_TIMEOUT_MS });
    if (!update) return { kind: "up-to-date" };
    return {
      kind: "available",
      version: update.version,
      notes: update.body ?? "",
      date: update.date ?? "",
      update,
    };
  } catch (e) {
    return { kind: "error", message: errorMessage(e) };
  }
}

/**
 * Download + install + relaunch the given update. Progress (chunk bytes +
 * total content-length, captured from the `Started` event) is forwarded to
 * `onProgress`. A rejected `relaunch()` is mapped to `needs-manual-restart`
 * so the UI never hangs on a spinner.
 */
export async function applyUpdate(
  update: Update,
  onProgress?: (p: DownloadProgress) => void,
): Promise<ApplyOutcome> {
  let contentLength: number | undefined;
  try {
    await update.downloadAndInstall((event: DownloadEvent) => {
      if (event.event === "Started") {
        contentLength = event.data.contentLength;
      } else if (event.event === "Progress" && onProgress) {
        onProgress({ chunkLength: event.data.chunkLength, contentLength });
      }
    }, { timeout: DOWNLOAD_TIMEOUT_MS });
  } catch (e) {
    return { kind: "error", message: errorMessage(e) };
  }
  try {
    await relaunch();
    return { kind: "updated", version: update.version };
  } catch {
    return { kind: "needs-manual-restart", version: update.version };
  }
}
