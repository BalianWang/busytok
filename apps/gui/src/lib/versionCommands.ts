import { invoke } from "@tauri-apps/api/core";

export const VERSIONS_MANIFEST_URL =
  "https://github.com/BalianWang/busytok/releases/latest/download/versions.json";

export interface VersionHistoryEntry {
  version: string;
  date: string;
  notes: string;
  manifest_url: string;
}

/** Raw shape returned by the Rust install_version command. */
export type InstallVersionOutcome =
  | { kind: "installed"; version: string }
  | { kind: "failed"; message: string };

export type InstallVersionResult = InstallVersionOutcome;

export async function installVersion(manifestUrl: string): Promise<InstallVersionResult> {
  try {
    const outcome = await invoke<InstallVersionOutcome>("install_version", { manifestUrl });
    return outcome;
  } catch (e) {
    return { kind: "failed", message: e instanceof Error ? e.message : String(e) };
  }
}
