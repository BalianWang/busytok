#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo build --release -p busytok-service
echo "Built: target/release/busytok-service"
