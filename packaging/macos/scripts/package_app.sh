#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
cd "$PROJECT_ROOT"

echo "Building busytok-service..."
cargo build --release -p busytok-service

echo "Building busytok..."
cargo build --release -p busytok

echo "Building Tauri GUI..."
if [ "${ALLOW_UNSIGNED_DEV_BUILD:-}" = "1" ]; then
    echo "Building unsigned dev .app bundle..."
    if ! command -v pnpm >/dev/null 2>&1; then
        echo "pnpm is required to build the macOS app bundle."
        exit 1
    fi
    pnpm install --frozen-lockfile
    pnpm --filter @busytok/gui exec tauri build --bundles app --target universal-apple-darwin
else
    echo "Release packaging requires Developer ID credentials."
    echo "Set ALLOW_UNSIGNED_DEV_BUILD=1 for unsigned dev builds."
fi

# Copy helpers and the bundle-native service LaunchAgent plist into the
# Tauri-built app bundle. SMAppService reads the plist from
# Contents/Library/LaunchAgents/; the app never installs it to ~/Library/LaunchAgents/.
source "$SCRIPT_DIR/_bundle_helpers.sh"
APP_BUNDLE="target/universal-apple-darwin/release/bundle/macos/Busytok.app"
if [ -d "$APP_BUNDLE" ]; then
    bundle_helpers_into_app "$APP_BUNDLE"
    bundle_service_plist_into_app "$APP_BUNDLE"
    echo "Unsigned dev app bundle ready at $APP_BUNDLE"
else
    if [ "${ALLOW_UNSIGNED_DEV_BUILD:-}" = "1" ]; then
        echo "App bundle not found at $APP_BUNDLE after Tauri build."
        exit 1
    fi
fi
