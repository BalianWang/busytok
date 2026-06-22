#!/usr/bin/env bash
set -euo pipefail
# Build busytok-service and busytok binaries for both macOS arches,
# then copy them into apps/gui/src-tauri/binaries/ so Tauri's externalBin
# mechanism can find them at dev/build time.
#
# CI does not call this script — it builds + lipo's the universal binary
# directly in the release workflow. This script is for local dev only.

cd "$(dirname "$0")/.."

BINARIES_DIR="apps/gui/src-tauri/binaries"
mkdir -p "$BINARIES_DIR"

for triple in aarch64-apple-darwin x86_64-apple-darwin; do
    echo "Building for $triple..."
    cargo build --release --target "$triple" -p busytok-service -p busytok

    cp "target/$triple/release/busytok-service" "$BINARIES_DIR/busytok-service-$triple"
    cp "target/$triple/release/busytok"         "$BINARIES_DIR/busytok-$triple"
    echo "  copied $triple binaries into $BINARIES_DIR"
done

echo ""
echo "Done. Built:"
ls -1 "$BINARIES_DIR/"
