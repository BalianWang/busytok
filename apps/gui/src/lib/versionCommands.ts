import { invoke } from "@tauri-apps/api/core";

export interface VersionHistoryEntry {
  version: string;
  date: string;
  notes: string;
  manifest_url: string;
}

/**
 * Fetch the published versions manifest (versions.json) via the Rust
 * `list_available_versions` command. Routed through Rust (not a browser
 * `fetch`) so it is not subject to CORS — GitHub's release-asset CDN serves no
 * `Access-Control-Allow-Origin` header, which is what made the webview fetch
 * fail with "Unavailable". Rejects on any failure so the useVersionHistory
 * query surfaces `isError`.
 */
export async function listAvailableVersions(): Promise<VersionHistoryEntry[]> {
  return invoke<VersionHistoryEntry[]>("list_available_versions");
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
