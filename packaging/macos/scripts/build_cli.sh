#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo build --release -p busytok
echo "Built: target/release/busytok"
