import { useQuery } from "@tanstack/react-query";
import { listAvailableVersions, type VersionHistoryEntry } from "../lib/versionCommands";

export type { VersionHistoryEntry };

/**
 * Version history for the manual-downgrade panel. Backed by the Rust
 * `list_available_versions` command (CORS-free; see versionCommands.ts).
 */
export function useVersionHistory() {
  return useQuery({
    queryKey: ["version-history"],
    queryFn: listAvailableVersions,
    staleTime: 60 * 60 * 1000, // 1h — the index rarely changes within a session
    retry: 1, // one fast retry — versions.json is a small cached endpoint; don't pay the global 3x exponential backoff
  });
}
