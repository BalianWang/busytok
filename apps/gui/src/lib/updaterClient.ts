import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type UpdaterResult =
  | { kind: "up-to-date" }
  | { kind: "updated"; version: string }
  | { kind: "error"; message: string };

export async function checkAndApplyUpdate(): Promise<UpdaterResult> {
  console.log("[updater] checking for updates...");
  try {
    const update = await check();
    if (!update) {
      console.log("[updater] up-to-date");
      return { kind: "up-to-date" };
    }
    console.log(`[updater] update available: ${update.version}`);
    console.log("[updater] downloading + installing...");
    await update.downloadAndInstall();
    console.log("[updater] downloaded; restarting");
    await relaunch();
    return { kind: "updated", version: update.version };
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    console.error("[updater] failed:", message);
    return { kind: "error", message };
  }
}

// Module-level latch survives StrictMode double-mount (parallels initThemeRuntime)
let autoCheckFired = false;

export function initUpdaterAutoCheck(): void {
  if (autoCheckFired) return;
  autoCheckFired = true;
  void checkAndApplyUpdate();
}

export function _testOnlyResetLatch(): void {
  autoCheckFired = false;
}
