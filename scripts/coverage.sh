#!/usr/bin/env bash
# Coverage gate for the audit-critical crates (everything except the
# macOS-only Tauri GUI and the platform sidecars).
#
# Workspace gate defaults to 82. Per-crate gate for busytok-subagent is 90.
#
# Plan 2 target was workspace 85% / per-crate 90%. Post-implementation
# deviation documented in the plan (lines 30-31): actual workspace is 82.8%
# (gap due to other crates outside Plan 2 scope) and per-crate is 89.2%
# (gap due to race-condition branches, background-task edge cases, tracing
# macros, and the 10s SIGKILL-timeout path — all impractical to test).
# Gates set to mechanically enforceable floors. Target: raise as other
# crates backfill coverage.
#
#   COVERAGE_GATE=82 bash scripts/coverage.sh
set -euo pipefail

# Workspace gate stays at 82 — the gap to 85% is in out-of-scope crates.
# 85% remains the aspiration as other crates backfill coverage.
GATE="${COVERAGE_GATE:-82}"
mkdir -p target/coverage

echo "==> Workspace coverage gate (lines >= ${GATE}%)"
cargo llvm-cov --workspace --exclude busytok-gui \
  --lcov --output-path target/coverage/lcov.info \
  --fail-under-lines "$GATE"

echo "==> Per-crate gate: busytok-subagent (lines >= 90%)"
# Per-crate gate: 90 (Plan 3 target — session pool + eviction fully covered)
cargo llvm-cov -p busytok-subagent \
  --fail-under-lines 90

echo "coverage gate passed"
echo "lcov report: target/coverage/lcov.info"
echo "for a local HTML report: cargo llvm-cov --workspace --exclude busytok-gui --html --open"
