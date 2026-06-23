#!/usr/bin/env bash
set -euo pipefail
SCRIPT="$(dirname "$0")/../../scripts/generate_versions_manifest.sh"

FIXTURE='[{"tag_name":"v0.1.0-rc.5","published_at":"2026-06-23T00:00:00Z","body":"fifth","manifest_url":"https://x/v0.1.0-rc.5/latest.json"},{"tag_name":"v0.1.0-rc.4","published_at":"2026-06-20T00:00:00Z","body":"fourth","manifest_url":"https://x/v0.1.0-rc.4/latest.json"}]'

OUT="$(printf '%s' "$FIXTURE" | bash "$SCRIPT" 5)"

echo "$OUT" | jq -e '.versions | length == 2' >/dev/null
echo "$OUT" | jq -e '.versions[0].version == "v0.1.0-rc.5"' >/dev/null
echo "$OUT" | jq -e '.versions[0].manifest_url == "https://x/v0.1.0-rc.5/latest.json"' >/dev/null
echo "$OUT" | jq -e '.versions[0].notes == "fifth"' >/dev/null
echo "$OUT" | jq -e '.versions[0].date == "2026-06-23T00:00:00Z"' >/dev/null

# N limit
OUT2="$(printf '%s' "$FIXTURE" | bash "$SCRIPT" 1)"
echo "$OUT2" | jq -e '.versions | length == 1' >/dev/null

echo "generate_versions_manifest_test: OK"
