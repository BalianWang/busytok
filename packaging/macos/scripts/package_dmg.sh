#!/usr/bin/env bash
# LOAD-BEARING: this script is the macOS packaging source of truth for
# BOTH local rehearsals (./scripts/verify_release.sh) AND CI releases
# (.github/workflows/release.yml). Do not assume it is local-only.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
cd "$PROJECT_ROOT"
source "$SCRIPT_DIR/_release_vars.sh"

APP_VERSION="${BUSYTOK_RELEASE_VERSION:-$(resolve_release_version)}"
if [ -z "$APP_VERSION" ]; then
    echo "Could not resolve release version from apps/gui/src-tauri/Cargo.toml"
    exit 1
fi

DMG_STAGING_DIR=""
# BUNDLE_DMG_DIR and BUNDLE_MACOS_DIR are sourced from _release_vars.sh

cleanup() {
    if [ -n "$DMG_STAGING_DIR" ] && [ -d "$DMG_STAGING_DIR" ]; then
        rm -rf "$DMG_STAGING_DIR"
    fi
}

trap cleanup EXIT

purge_stale_bundle_outputs() {
    mkdir -p "$BUNDLE_DMG_DIR" "$BUNDLE_MACOS_DIR"

    find "$BUNDLE_DMG_DIR" -maxdepth 1 -type f -name "*.dmg" -delete
    find "$BUNDLE_MACOS_DIR" -maxdepth 1 -type d -name "*.app" -prune -exec rm -rf {} +
}

echo "Cleaning previous bundle outputs..."
purge_stale_bundle_outputs

echo "Installing workspace dependencies..."
pnpm install --frozen-lockfile

echo "Building universal helper binaries via lipo..."
cargo build --release --target aarch64-apple-darwin -p busytok-service -p busytok
cargo build --release --target x86_64-apple-darwin -p busytok-service -p busytok
lipo -create -output target/release/busytok-service \
    target/aarch64-apple-darwin/release/busytok-service \
    target/x86_64-apple-darwin/release/busytok-service
lipo -create -output target/release/busytok \
    target/aarch64-apple-darwin/release/busytok \
    target/x86_64-apple-darwin/release/busytok

echo "Building universal macOS .app bundle via Tauri..."
pnpm --filter @busytok/gui exec tauri build --bundles app --target universal-apple-darwin

# Copy frontend dist into the bundle Resources so the native WKWebView
# palette panel can load index.html?window=prompt-palette via file:// URL.
# Tauri 2 embeds assets into the binary; standalone WKWebView needs them
# on the filesystem.
APP_PATH="$BUNDLE_MACOS_DIR/Busytok.app"
FRONTEND_DIST="apps/gui/dist"
if [ -d "$APP_PATH" ] && [ -d "$FRONTEND_DIST" ]; then
    echo "Copying frontend dist into app bundle for native palette panel..."
    cp -R "$FRONTEND_DIST"/* "$APP_PATH/Contents/Resources/"
fi

# Copy helpers and the bundle-native service LaunchAgent plist into the .app
# bundle alongside busytok-gui. The plist must be in place before the bundle
# is re-sealed so the seal covers Contents/Library/LaunchAgents/.
source "$SCRIPT_DIR/_bundle_helpers.sh"
ENTITLEMENTS_DIR="$SCRIPT_DIR/../entitlements"
if [ -d "$APP_PATH" ]; then
    bundle_helpers_into_app "$APP_PATH"
    bundle_service_plist_into_app "$APP_PATH"

    # Sign the helpers with service-specific entitlements before the
    # enclosing bundle is re-sealed.  The final bundle seal uses the app
    # entitlements so the app/helper split is explicit:
    #
    # Signing model:
    #   busytok-gui    → final bundle re-seal below (app.plist when present)
    #   helpers        → codesign below (service.plist)
    #   bundle seal    → codesign below (no --deep, preserves helper entitlements)
    if [ -n "${DEVELOPER_ID_APPLICATION:-}" ]; then
	# Apple timestamp server is unreachable from many CI runners.
	# BUSYTOK_SKIP_TIMESTAMP=1 skips codesign-level timestamping;
	# the notarization staple provides the official timestamp.
	TIMESTAMP_FLAG="--timestamp"
	if [ "${BUSYTOK_SKIP_TIMESTAMP:-}" = "1" ]; then
	    TIMESTAMP_FLAG="--timestamp=none"
	fi

        echo "Signing helper binaries with $DEVELOPER_ID_APPLICATION..."
        for helper in busytok-service busytok; do
            HELPER_PATH="$APP_PATH/Contents/MacOS/$helper"
            entitlements="$ENTITLEMENTS_DIR/service.plist"
            if [ -f "$entitlements" ]; then
                codesign --sign "$DEVELOPER_ID_APPLICATION" \
                    --entitlements "$entitlements" \
                    --options runtime \
                    $TIMESTAMP_FLAG \
                    --force \
                    "$HELPER_PATH"
            else
                codesign --sign "$DEVELOPER_ID_APPLICATION" \
                    --options runtime \
                    $TIMESTAMP_FLAG \
                    --force \
                    "$HELPER_PATH"
            fi
            echo "  signed $helper"
        done

        # Re-seal the app bundle so its _CodeSignature includes the
        # newly-added-and-signed helpers.  --deep is deliberately omitted:
        # it would re-sign the helpers and overwrite their service.plist
        # entitlements.
        echo "Re-sealing app bundle..."
        app_entitlements="$ENTITLEMENTS_DIR/app.plist"
        if [ -f "$app_entitlements" ]; then
            codesign --sign "$DEVELOPER_ID_APPLICATION" \
                --entitlements "$app_entitlements" \
                --options runtime \
                $TIMESTAMP_FLAG \
                --force \
                "$APP_PATH"
        else
            codesign --sign "$DEVELOPER_ID_APPLICATION" \
                --options runtime \
                $TIMESTAMP_FLAG \
                --force \
                "$APP_PATH"
        fi
        echo "  bundle re-sealed"
    else
        echo "DEVELOPER_ID_APPLICATION not set — helpers are unsigned."
        echo "Run with DEVELOPER_ID_APPLICATION set for signed release builds."
    fi
else
    echo "  App bundle not found at $APP_PATH"
    exit 1
fi

# Create a drag-to-install DMG with a custom Finder layout (background,
# window size, icon positions, Applications link) via create-dmg.
# create-dmg handles the .DS_Store / .background/ plumbing that the old
# bare hdiutil create -srcfolder did not. Codesign happens separately
# (below) so $TIMESTAMP_FLAG is respected.
echo "Creating DMG (create-dmg)..."
if [ -n "${DEVELOPER_ID_APPLICATION:-}" ]; then
    DMG_PATH="$(signed_dmg_path "$APP_VERSION")"
else
    DMG_PATH="$(unsigned_dmg_path "$APP_VERSION")"
fi
mkdir -p "$(dirname "$DMG_PATH")"

DMG_STAGING_DIR="$(mktemp -d "${TMPDIR:-/tmp}/busytok-dmg-staging.XXXXXX")"
cp -R "$APP_PATH" "$DMG_STAGING_DIR/Busytok.app"

DMG_WINDOW_WIDTH=768
DMG_WINDOW_HEIGHT=512
DMG_BACKGROUND_SOURCE="$PROJECT_ROOT/packaging/macos/assets/dmg-background.png"

create-dmg \
    --volname "Busytok" \
    --background "$DMG_BACKGROUND_SOURCE" \
    --window-size "$DMG_WINDOW_WIDTH" "$DMG_WINDOW_HEIGHT" \
    --text-size 12 \
    --icon-size 112 \
    --icon "Busytok.app" 194 188 \
    --hide-extension "Busytok.app" \
    --app-drop-link 566 188 \
    --format UDZO \
    --hdiutil-quiet \
    "$DMG_PATH" \
    "$DMG_STAGING_DIR"

if [ -n "${DEVELOPER_ID_APPLICATION:-}" ]; then
    echo "Signing DMG container..."
    codesign --sign "$DEVELOPER_ID_APPLICATION" \
        $TIMESTAMP_FLAG \
        --force \
        "$DMG_PATH"
    echo "  signed $DMG_PATH"
else
    echo "DEVELOPER_ID_APPLICATION not set — DMG container is unsigned."
fi

echo ""
echo "Bundle outputs:"
find "$BUNDLE_ROOT" -maxdepth 2 -type f \
  \( -name "*.app" -o -name "*.dmg" -o -name "*.sig" -o -name "*.tar.gz" \) \
  | sort
