#!/usr/bin/env bash
# Shared release variables + path helpers.
# Sourced by package_dmg.sh, verify_package.sh, etc.

# All bundle outputs live under the universal-apple-darwin target root,
# since release builds are universal (one DMG covers arm64 + x86_64).
BUNDLE_ROOT="target/universal-apple-darwin/release/bundle"
BUNDLE_DMG_DIR="$BUNDLE_ROOT/dmg"
BUNDLE_MACOS_DIR="$BUNDLE_ROOT/macos"

resolve_release_version() {
    sed -n 's/^version = "\(.*\)"/\1/p' "$PROJECT_ROOT/apps/gui/src-tauri/Cargo.toml" | head -1
}

signed_dmg_path() {
    local version="$1"
    printf '%s/Busytok_%s.dmg' "$BUNDLE_DMG_DIR" "$version"
}

unsigned_dmg_path() {
    local version="$1"
    printf '%s/Busytok_%s_unsigned.dmg' "$BUNDLE_DMG_DIR" "$version"
}
