#!/usr/bin/env bash
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
CLI="${PROJECT_ROOT}/target/release/busytok"
if [ ! -x "$CLI" ]; then
    echo "busytok not found at $CLI"
    exit 1
fi
out="$("$CLI" --help)"
if ! echo "$out" | grep -q "busytok"; then
    echo "FAIL: --help output does not mention busytok"
    exit 1
fi
echo "PASS: busytok --help OK"
