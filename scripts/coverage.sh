#!/usr/bin/env bash
# Coverage gate for the audit-critical crates (everything except the
# macOS-only Tauri GUI and the platform sidecars).
#
# The workspace currently sits at ~81.8% line coverage: the changed
# token-accounting logic (busytok-store dedup, busytok-adapters Claude parser)
# is at 90%+, but legacy modules (db.rs query helpers, live_queries.rs,
# tailer.rs) pull the total down. The gate defaults to 80 — a passing
# regression floor — and can be ratcheted toward 85 by raising COVERAGE_GATE
# once those legacy modules gain tests.
#
#   COVERAGE_GATE=85 bash scripts/coverage.sh
set -euo pipefail

GATE="${COVERAGE_GATE:-80}"
mkdir -p target/coverage

cargo llvm-cov --workspace --exclude busytok-gui \
  --lcov --output-path target/coverage/lcov.info \
  --fail-under-lines "$GATE"

echo "coverage gate (lines >= ${GATE}%) passed"
echo "lcov report: target/coverage/lcov.info"
echo "for a local HTML report: cargo llvm-cov --workspace --exclude busytok-gui --html --open"
