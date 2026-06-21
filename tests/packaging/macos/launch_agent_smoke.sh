#!/usr/bin/env bash
set -euo pipefail

# LaunchAgent smoke test — verifies the SMAppService model for Busytok.
#
# Under SMAppService the service LaunchAgent plist is bundled into
# Busytok.app/Contents/Library/LaunchAgents/ and read from that bundle
# location at registration time. The app does NOT install a copy to
# ~/Library/LaunchAgents/.
#
# desktop-host login-start is registered through SMAppService.mainApp, so
# there must be no com.busytok.desktop-host.plist anywhere in the bundle.

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

echo "=== LaunchAgent Smoke Test (SMAppService model) ==="
echo "  app bundle: $APP_PATH"

if [ ! -d "$APP_PATH" ]; then
    echo "Busytok.app not found at $APP_PATH"
    echo "Build it first (ALLOW_UNSIGNED_DEV_BUILD=1 packaging/macos/scripts/package_app.sh)"
    echo "or set BUSYTOK_APP_PATH."
    exit 1
fi

echo ""
echo "--- Bundle-native service plist ---"
if [ -f "$BUNDLE_SERVICE_PLIST" ]; then
    echo "  bundle service plist: OK"
else
    fail "bundle service plist missing at $BUNDLE_SERVICE_PLIST"
fi

# Bundle plist is what SMAppService reads, so it must NOT also exist in
# user home as a handwritten artifact on the supported path.
if [ -f "$USER_SERVICE_PLIST" ]; then
    fail "user-home service plist must not exist on the supported path: $USER_SERVICE_PLIST"
else
    echo "  no user-home service plist: OK"
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
    echo "  service label not currently registered (run Busytok.app once to register via SMAppService)"
fi

echo ""
if [ "$FAILURES" -eq 0 ]; then
    echo "=== LaunchAgent smoke PASSED ==="
else
    echo "=== LaunchAgent smoke FAILED ($FAILURES failures) ==="
    exit 1
fi
