import { useQuery } from "@tanstack/react-query";
import { VERSIONS_MANIFEST_URL, type VersionHistoryEntry } from "../lib/versionCommands";

interface VersionsManifest {
  versions: VersionHistoryEntry[];
}

async function fetchVersionHistory(): Promise<VersionsManifest> {
  const res = await fetch(VERSIONS_MANIFEST_URL);
  if (!res.ok) throw new Error(`versions.json HTTP ${res.status}`);
  const json = (await res.json()) as Partial<VersionsManifest>;
  return { versions: Array.isArray(json.versions) ? json.versions : [] };
}

export function useVersionHistory() {
  return useQuery({
    queryKey: ["version-history"],
    queryFn: fetchVersionHistory,
    staleTime: 60 * 60 * 1000, // 1h — the index rarely changes within a session
  });
}
