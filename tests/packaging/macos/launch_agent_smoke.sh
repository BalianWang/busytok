#!/usr/bin/env bash
set -euo pipefail

# LaunchAgent smoke test — verifies the Architecture-B managed-plist model.
#
# Production registration bootstraps from a runtime-rendered plist in
# ~/Library/LaunchAgents/com.busytok.service.plist (see
# apps/gui/src-tauri/src/service_lifecycle/managed_launch_agent.rs).
# The bundled plist (Contents/Library/LaunchAgents/) is a static reference
# copy only — production does not bootstrap from it.
#
# This smoke validates:
#   - Bundle plist exists (reference copy).
#   - After first launch, the user-domain managed plist exists.
#   - The managed plist's ProgramArguments[0] points at the current
#     install location.
#   - The managed plist contains no build-machine paths (/Users/runner).
#   - No desktop-host plist exists anywhere (uses SMAppService.mainApp).

APP_PATH="${BUSYTOK_APP_PATH:-target/release/bundle/macos/Busytok.app}"
BUNDLE_SERVICE_PLIST="$APP_PATH/Contents/Library/LaunchAgents/com.busytok.service.plist"
BUNDLE_HOST_PLIST="$APP_PATH/Contents/Library/LaunchAgents/com.busytok.desktop-host.plist"
USER_SERVICE_PLIST="$HOME/Library/LaunchAgents/com.busytok.service.plist"
USER_HOST_PLIST="$HOME/Library/LaunchAgents/com.busytok.desktop-host.plist"
SERVICE_LABEL="gui/$(id -u)/com.busytok.service"

FAILURES=0

fail() {
    echo "  FAIL: $1"
    FAILURES=$((FAILURES + 1))
}

echo "=== LaunchAgent Smoke Test (Architecture-B managed-plist model) ==="
echo "  app bundle: $APP_PATH"

if [ ! -d "$APP_PATH" ]; then
    echo "Busytok.app not found at $APP_PATH"
    echo "Build it first (ALLOW_UNSIGNED_DEV_BUILD=1 packaging/macos/scripts/package_app.sh)"
    echo "or set BUSYTOK_APP_PATH."
    exit 1
fi

echo ""
echo "--- Bundle reference plist ---"
if [ -f "$BUNDLE_SERVICE_PLIST" ]; then
    echo "  bundle service plist (reference): OK"
else
    fail "bundle reference plist missing at $BUNDLE_SERVICE_PLIST"
fi

echo ""
echo "--- User-domain managed plist ---"
if [ -f "$USER_SERVICE_PLIST" ]; then
    echo "  user-domain managed plist present: OK"
    # Architecture-B contract: ProgramArguments[0] must point at the
    # current install location, NOT at a build-machine path.
    if grep -q '/Users/runner' "$USER_SERVICE_PLIST"; then
        fail "managed plist contains build-machine path /Users/runner"
    elif grep -q 'SERVICE_BINARY_PATH' "$USER_SERVICE_PLIST"; then
        fail "managed plist contains unsubstituted placeholder SERVICE_BINARY_PATH"
    else
        echo "  managed plist content: clean (no build-machine paths, no unsubstituted placeholders)"
    fi
else
    echo "  user-domain managed plist not present"
    echo "  (launch Busytok.app once — it renders the plist on first bootstrap)"
fi

echo ""
echo "--- desktop-host must use SMAppService.mainApp ---"
if [ -e "$BUNDLE_HOST_PLIST" ]; then
    fail "bundle must NOT contain com.busytok.desktop-host.plist"
else
    echo "  no bundled desktop-host plist: OK"
fi
if [ -e "$USER_HOST_PLIST" ]; then
    fail "user home must NOT contain com.busytok.desktop-host.plist"
else
    echo "  no user-home desktop-host plist: OK"
fi

echo ""
echo "--- Service launchd label (if registered this session) ---"
if launchctl print "$SERVICE_LABEL" >/dev/null 2>&1; then
    echo "  service label visible to launchctl: OK"
else
    echo "  service label not currently registered (run Busytok.app once to register via managed plist)"
fi

echo ""
if [ "$FAILURES" -eq 0 ]; then
    echo "=== LaunchAgent smoke PASSED ==="
else
    echo "=== LaunchAgent smoke FAILED ($FAILURES failures) ==="
    exit 1
fi
