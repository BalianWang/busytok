#!/usr/bin/env bash
set -euo pipefail

echo "=== Busytok Release Verification ==="

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# 1. Run acceptance gate first
echo "--- Acceptance Gate ---"
"$SCRIPT_DIR/verify_acceptance.sh"

# 2. Build the .app (dev: cargo build; release: Tauri + signing via package_dmg.sh)
echo "--- Building release artifacts ---"
if [ "${ALLOW_UNSIGNED_DEV_BUILD:-}" = "1" ]; then
    "$PROJECT_ROOT/packaging/macos/scripts/package_app.sh"
else
    echo "Building signed .app via package_dmg.sh..."
    "$PROJECT_ROOT/packaging/macos/scripts/package_dmg.sh"
fi

# 3. Verify package
echo "--- Verifying package ---"
"$PROJECT_ROOT/packaging/macos/scripts/verify_package.sh"

# 4. CLI smoke
echo "--- CLI smoke ---"
"$PROJECT_ROOT/tests/packaging/macos/cli_smoke.sh"

# 5. Codesign verification
APP_PATH="target/universal-apple-darwin/release/bundle/macos/Busytok.app"
if [ "${ALLOW_UNSIGNED_DEV_BUILD:-}" = "1" ]; then
    echo "--- Codesign Verification ---"
    echo "Skipped (unsigned dev build)"
else
    echo "--- Codesign Verification ---"
    echo "Verifying app bundle..."

    if ! codesign --verify --deep --strict "$APP_PATH"; then
        echo "  codesign --verify --deep --strict FAILED"
        echo "  Helper binaries copied after the Tauri build may not be signed."
        echo "  Run the full release packaging pipeline (package_dmg.sh) for signed builds."
        exit 1
    fi
    codesign_info="$(codesign -dv --verbose=4 "$APP_PATH" 2>&1)"
    printf '%s\n' "$codesign_info" | sed -n '1,5p'

    for helper in busytok-service busytok; do
        HELPER_PATH="$APP_PATH/Contents/MacOS/$helper"
        if [ -f "$HELPER_PATH" ]; then
            echo "Verifying $helper..."
            if ! codesign -dv --verbose=4 "$HELPER_PATH" 2>&1; then
                echo "  $helper is not signed"
                exit 1
            fi
        else
            echo "  $helper not found in app bundle"
            exit 1
        fi
    done
fi

# 6. Installed app smoke — mandatory when the .app exists.
#    In release mode, the .app MUST exist.
if [ -d "$APP_PATH" ] && [ -x "$APP_PATH/Contents/MacOS/busytok-gui" ]; then
    echo "--- Installed App Smoke ---"
    BUSYTOK_APP_PATH="$APP_PATH" "$PROJECT_ROOT/tests/packaging/macos/installed_app_smoke.sh"
else
    if [ "${ALLOW_UNSIGNED_DEV_BUILD:-}" = "1" ]; then
        case "$(uname -s)" in
            Darwin)
                echo "--- Installed App Smoke ---"
                echo "  App bundle not found at $APP_PATH after package_app.sh"
                exit 1
                ;;
            *)
                echo "--- Installed App Smoke ---"
                echo "Skipped on non-macOS host"
                ;;
        esac
    else
        echo "--- Installed App Smoke ---"
        echo "  App bundle not found at $APP_PATH (required for release)"
        exit 1
    fi
fi

echo "=== Release verification PASSED ==="
