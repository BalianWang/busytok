#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORKFLOW="$PROJECT_ROOT/.github/workflows/release.yml"

echo "=== release.yml structural smoke ==="

test -f "$WORKFLOW" || { echo "release.yml missing"; exit 1; }
echo "file exists: OK"

python3 - <<'PY' "$WORKFLOW"
import sys
import yaml

workflow = yaml.safe_load(open(sys.argv[1]))
on_block = workflow.get("on", workflow.get(True))

assert on_block is not None, "missing on trigger block"
assert "push" in on_block, "missing push trigger"
assert "tags" in on_block["push"], "missing tag trigger"
assert any("v*" in pattern for pattern in on_block["push"]["tags"]), "missing v* tag pattern"
assert workflow["permissions"]["contents"] == "write", "missing contents: write"
assert workflow["concurrency"]["cancel-in-progress"] is False, "cancel-in-progress must be false"

print("yaml + trigger + permissions + concurrency: OK")
PY

grep -q "package_dmg.sh" "$WORKFLOW" || { echo "missing package_dmg.sh invocation"; exit 1; }
echo "package_dmg.sh invocation: OK"

grep -q "xcrun notarytool submit" "$WORKFLOW" || { echo "missing notarytool submit"; exit 1; }
echo "notarytool submit: OK"

grep -q "generate_updater_manifest.sh" "$WORKFLOW" || { echo "missing generate_updater_manifest.sh"; exit 1; }
echo "manifest generation: OK"

if grep -q "tauri-apps/tauri-action" "$WORKFLOW"; then
    echo "unexpected tauri-action usage"
    exit 1
fi
echo "no tauri-action: OK"

if ! rg -q "uses: pnpm/action-setup@v4" "$WORKFLOW"; then
    echo "missing pnpm/action-setup"
    exit 1
fi

if rg -q "version:\\s*[\"']?10[\"']?" "$WORKFLOW"; then
    echo "workflow still hardcodes pnpm version; should rely on packageManager"
    exit 1
fi
echo "pnpm version source is single-origin: OK"

if ! rg -q "rustup target add aarch64-apple-darwin x86_64-apple-darwin" "$WORKFLOW"; then
    echo "missing explicit universal Apple target install"
    exit 1
fi
echo "explicit universal target install: OK"

echo "=== release.yml structural smoke PASSED ==="
