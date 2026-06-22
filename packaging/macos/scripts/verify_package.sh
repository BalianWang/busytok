#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$PROJECT_ROOT"

MISSING=0

check_artifact() {
    local path="$1"
    local label="$2"
    if [ -f "$path" ] && [ -s "$path" ] && [ -x "$path" ]; then
        echo "  $label: $path"
    elif [ -f "$path" ] && [ -s "$path" ]; then
        echo "  $label: $path (exists but not executable)"
        MISSING=1
    else
        echo "  $label: $path (MISSING or empty)"
        MISSING=1
    fi
}

echo "=== Busytok Package Verification ==="

if [ "${ALLOW_UNSIGNED_DEV_BUILD:-}" = "1" ]; then
    echo "Mode: dev unsigned"
    check_artifact "target/release/busytok-service" "busytok-service"
    check_artifact "target/release/busytok" "busytok"
    # GUI binary — in dev mode, check the cargo build output
    if [ -f "target/release/busytok-gui" ]; then
        echo "  busytok-gui: target/release/busytok-gui"
    else
        echo "  busytok-gui: not found (Tauri GUI may need separate build)"
        # Not fatal in dev mode — Tauri build requires Xcode/macOS SDK
    fi
else
    echo "Mode: release packaged"
    check_artifact "target/release/busytok-service" "busytok-service"
    check_artifact "target/release/busytok" "busytok"

    # In release mode, GUI is mandatory
    if [ -f "target/release/busytok-gui" ]; then
        echo "  busytok-gui: target/release/busytok-gui"
    elif [ -d "target/universal-apple-darwin/release/bundle/macos" ]; then
        echo "  busytok-gui bundle: target/universal-apple-darwin/release/bundle/macos/"
    else
        echo "  busytok-gui: MISSING (required for release)"
        MISSING=1
    fi
fi

# Verify helpers and bundle-native service plist inside the .app bundle.
# desktop-host uses SMAppService.mainApp and must NOT ship as a LaunchAgent.
APP_PATH="target/universal-apple-darwin/release/bundle/macos/Busytok.app"
if [ -d "$APP_PATH" ]; then
    echo ""
    echo "--- App Bundle Helper Verification ---"
    check_artifact "$APP_PATH/Contents/MacOS/busytok-service" "bundle busytok-service"
    check_artifact "$APP_PATH/Contents/MacOS/busytok" "bundle busytok"
    check_artifact "$APP_PATH/Contents/MacOS/busytok-gui" "bundle busytok-gui"

    echo ""
    echo "--- App Bundle LaunchAgent Verification ---"
    check_artifact "$APP_PATH/Contents/Library/LaunchAgents/com.busytok.service.plist" "bundle service plist"
    if [ -e "$APP_PATH/Contents/Library/LaunchAgents/com.busytok.desktop-host.plist" ]; then
        echo "  FAIL: com.busytok.desktop-host.plist must NOT exist — desktop-host uses SMAppService.mainApp"
        MISSING=1
    else
        echo "  no bundled com.busytok.desktop-host.plist: OK (desktop-host is SMAppService.mainApp)"
    fi
fi

echo ""
if [ "$MISSING" -eq 0 ]; then
    echo "=== Package verification PASSED ==="
else
    echo "=== Package verification FAILED ($MISSING missing artifacts) ==="
    exit 1
fi
