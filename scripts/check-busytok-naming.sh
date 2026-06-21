#!/usr/bin/env bash
set -euo pipefail

forbidden='autoken|Autoken|AUTO_TOKEN|@autoken'

if rg -n "$forbidden" \
  --glob '!dist/**' \
  --glob '!docs/**' \
  --glob '!.git/**' \
  --glob '!**/*test*' \
  --glob '!**/tests/**' \
  --glob '!scripts/check-busytok-naming.sh' \
  --glob '!scripts/check-busytok-gui-surfaces.sh' \
  --glob '!**/paths.rs' \
  --glob '!README.md'; then
  echo "Found old Autoken naming outside allowed reference docs"
  exit 1
fi

# Check user-visible surfaces for stale autoken references.
# README.md is excluded until PR4 polishes it (Task 36 of the
# macOS public launch plan); the badge URLs will be updated then.
stale=$(rg -n 'BalianWang/autoken|/autoken/' \
    --glob '!README.md' \
    --glob '!node_modules' \
    --glob '!target' \
    --glob '!dist' \
    .github/ 2>/dev/null || true)
if [ -n "$stale" ]; then
    echo "FAIL: stale autoken reference in user-visible surfaces:"
    echo "$stale"
    exit 1
fi
echo "  naming check PASSED"
