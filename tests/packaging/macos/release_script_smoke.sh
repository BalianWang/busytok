#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "=== Release Script Smoke ==="

echo "  Checking that packaging scripts exist..."
for script in \
    "$PROJECT_ROOT/packaging/macos/scripts/package_app.sh" \
    "$PROJECT_ROOT/packaging/macos/scripts/package_dmg.sh" \
    "$PROJECT_ROOT/packaging/macos/scripts/_bundle_helpers.sh" \
    "$PROJECT_ROOT/packaging/macos/scripts/verify_package.sh"; do
    if [ -f "$script" ] && [ -x "$script" ]; then
        echo "    $(basename "$script"): OK"
    else
        echo "    $(basename "$script"): MISSING or not executable"
        exit 1
    fi
done

echo "  Checking launchd plist template..."
PLIST_TEMPLATE="$PROJECT_ROOT/packaging/macos/launchd/com.busytok.service.plist.template"
if [ -f "$PLIST_TEMPLATE" ]; then
    echo "    com.busytok.service.plist.template: OK"
else
    echo "    com.busytok.service.plist.template: MISSING"
    exit 1
fi

echo "  Checking that package_dmg.sh uses the standard --app-drop-link"
echo "  and does not reintroduce the custom Applications alias / resource-fork"
echo "  workaround (regression guard for macOS 26 Finder/IconServices pollution)."
PKG_DMG="$PROJECT_ROOT/packaging/macos/scripts/package_dmg.sh"
if ! grep -q -- '--app-drop-link' "$PKG_DMG"; then
    echo "    FAILED: package_dmg.sh is missing --app-drop-link (must be present)"
    exit 1
fi
for token in 'make new alias file' 'Rez -append' 'SetFile -a C'; do
    if grep -q "$token" "$PKG_DMG"; then
        echo "    FAILED: package_dmg.sh contains forbidden workaround: $token"
        exit 1
    fi
done
echo "    package_dmg.sh DMG layout contract: OK"

echo "  Checking pi-sidecar bundling contract in package_dmg.sh..."
PKG_DMG="$PROJECT_ROOT/packaging/macos/scripts/package_dmg.sh"
HELPER="$PROJECT_ROOT/packaging/macos/scripts/_bundle_sidecar.sh"

# Check 1: package_dmg.sh sources + calls the sidecar bundler.
if ! grep -q '_bundle_sidecar.sh' "$PKG_DMG"; then
    echo "    FAILED: package_dmg.sh does not source _bundle_sidecar.sh"
    exit 1
fi
if ! grep -q 'bundle_sidecar_resources_into_app' "$PKG_DMG"; then
    echo "    FAILED: package_dmg.sh does not call bundle_sidecar_resources_into_app"
    exit 1
fi
echo "    package_dmg.sh invokes sidecar bundler: OK"

# Check 2: the sidecar bundler script exists + is executable.
if [ ! -x "$HELPER" ]; then
    echo "    FAILED: _bundle_sidecar.sh missing or not executable"
    exit 1
fi
echo "    _bundle_sidecar.sh exists + executable: OK"

# Check 3: the bundler generates manifest.json (grep for the generator).
if ! grep -q 'generate_sidecar_manifest' "$HELPER"; then
    echo "    FAILED: _bundle_sidecar.sh does not call generate_sidecar_manifest"
    exit 1
fi
echo "    manifest.json generation: OK"

# Check 4: the bundler downloads + places node binaries for both arches.
if ! grep -q 'download_node_binary' "$HELPER"; then
    echo "    FAILED: _bundle_sidecar.sh does not call download_node_binary"
    exit 1
fi
if ! grep -q 'aarch64' "$HELPER" || ! grep -q 'x86_64' "$HELPER"; then
    echo "    FAILED: _bundle_sidecar.sh does not handle both arches"
    exit 1
fi
echo "    dual-arch node binary download: OK"

# Check 5: the sidecar-node entitlements plist exists (allow-jit only).
NODE_ENT="$PROJECT_ROOT/packaging/macos/entitlements/sidecar-node.plist"
if [ ! -f "$NODE_ENT" ]; then
    echo "    FAILED: sidecar-node.plist entitlements missing"
    exit 1
fi
if ! grep -q 'allow-jit' "$NODE_ENT"; then
    echo "    FAILED: sidecar-node.plist missing allow-jit"
    exit 1
fi
if grep -q 'allow-unsigned-executable-memory' "$NODE_ENT"; then
    echo "    FAILED: sidecar-node.plist must NOT contain allow-unsigned-executable-memory (spec §383: verify empirically before broadening)"
    exit 1
fi
echo "    sidecar-node.plist (allow-jit only): OK"

# Check 6: package_dmg.sh signs the node binaries before re-seal.
if ! grep -q 'sign_sidecar_node_binaries' "$PKG_DMG"; then
    echo "    FAILED: package_dmg.sh does not call sign_sidecar_node_binaries"
    exit 1
fi
echo "    node binary signing: OK"

echo ""
echo "=== Release script smoke PASSED ==="
