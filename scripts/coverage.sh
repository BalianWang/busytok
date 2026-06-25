#!/usr/bin/env bash
# Coverage gate for the audit-critical crates (everything except the
# macOS-only Tauri GUI and the platform sidecars).
#
# Workspace gate defaults to 85 (CI floor). Per-crate gate for
# busytok-subagent is 90 (Plan 2 requirement).
#
#   COVERAGE_GATE=85 bash scripts/coverage.sh
set -euo pipefail

GATE="${COVERAGE_GATE:-85}"
mkdir -p target/coverage

echo "==> Workspace coverage gate (lines >= ${GATE}%)"
cargo llvm-cov --workspace --exclude busytok-gui \
  --lcov --output-path target/coverage/lcov.info \
  --fail-under-lines "$GATE"

echo "==> Per-crate gate: busytok-subagent (lines >= 90%)"
cargo llvm-cov -p busytok-subagent \
  --fail-under-lines 90

echo "coverage gate passed"
echo "lcov report: target/coverage/lcov.info"
echo "for a local HTML report: cargo llvm-cov --workspace --exclude busytok-gui --html --open"
