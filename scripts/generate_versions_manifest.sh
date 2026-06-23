#!/usr/bin/env bash
# Emit versions.json (the version-history index) from a JSON array on stdin.
#
# Each stdin element: { "tag_name", "published_at", "body", "manifest_url" }
# Output: { "versions": [ { "version", "date", "notes", "manifest_url" } ... ] }
# limited to the first N entries (default 5).
#
# Hermetic: no network, no `date` — CI assembles the input array (git tags +
# per-tag latest.json metadata). Parallels generate_updater_manifest.sh.
#
# Requires: jq.
set -euo pipefail

N="${1:-5}"

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required" >&2
  exit 1
fi

jq --argjson n "$N" '{
  versions: [
    .[]
    | {
        version: .tag_name,
        date: (.published_at // ""),
        notes: (.body // ""),
        manifest_url: .manifest_url
      }
  ] | .[0:$n]
}'
