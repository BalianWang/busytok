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

echo ""
echo "=== Release script smoke PASSED ==="
