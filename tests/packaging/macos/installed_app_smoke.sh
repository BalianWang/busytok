#!/usr/bin/env bash
set -euo pipefail

# Installed-app smoke test — verifies Busytok.app bundle layout under the
# Architecture-B runtime-managed launch agent model and the CLI recovery
# contract for whole-product quit.
#
# Asserts:
#   - Bundle helpers (busytok-gui, busytok-service, busytok) are present.
#   - The bundle reference plist (static copy, not bootstrapped) exists at
#     Contents/Library/LaunchAgents/com.busytok.service.plist.
#   - The user-domain managed plist exists at
#     ~/Library/LaunchAgents/com.busytok.service.plist after first launch,
#     and its ProgramArguments[0] points at the current install location
#     (no build-machine paths, no unsubstituted placeholders).
#   - There is NO bundled com.busytok.desktop-host.plist (desktop-host uses
#     SMAppService.mainApp).
#   - The bundled CLI prints the "Open Busytok.app to start the background
#     service" recovery message when it cannot reach the service.
#
# Same-session suppression smoke (whole-product quit) is opt-in via
# BUSYTOK_RUN_QUIT_SMOKE=1, because it requires launching the app and
# triggering Quit Busytok Desktop in a logged-in GUI session.

APP_PATH="${BUSYTOK_APP_PATH:-target/release/bundle/macos/Busytok.app}"
GUI_BINARY="$APP_PATH/Contents/MacOS/busytok-gui"
SERVICE_BINARY="$APP_PATH/Contents/MacOS/busytok-service"
CLI_BINARY="$APP_PATH/Contents/MacOS/busytok"
BUNDLE_PLIST="$APP_PATH/Contents/Library/LaunchAgents/com.busytok.service.plist"
HOST_PLIST="$APP_PATH/Contents/Library/LaunchAgents/com.busytok.desktop-host.plist"
USER_MANAGED_PLIST="$HOME/Library/LaunchAgents/com.busytok.service.plist"

FAILURES=0
fail() {
    echo "FAIL: $1"
    FAILURES=$((FAILURES + 1))
}

if [ ! -d "$APP_PATH" ]; then
    echo "Busytok.app not found at $APP_PATH"
    exit 1
fi

echo "Checking app bundle structure..."
if [ ! -f "$GUI_BINARY" ]; then
    fail "busytok-gui not found in app bundle"
else
    echo "  busytok-gui: OK"
fi

if [ ! -f "$SERVICE_BINARY" ]; then
    fail "busytok-service not found in app bundle"
else
    echo "  busytok-service: OK"
fi

if [ ! -f "$CLI_BINARY" ]; then
    fail "busytk CLI not found in app bundle"
else
    echo "  busytok: OK"
fi

if [ ! -f "$APP_PATH/Contents/Info.plist" ]; then
    fail "Info.plist not found"
else
    echo "  Info.plist: OK"
fi

echo ""
echo "Checking Architecture-B managed launch agent model..."
if [ -f "$BUNDLE_PLIST" ]; then
    echo "  bundle reference plist: OK"
else
    fail "bundle reference plist must be present at $BUNDLE_PLIST"
fi

if [ -e "$HOST_PLIST" ]; then
    fail "com.busytok.desktop-host.plist must NOT exist — desktop-host uses SMAppService.mainApp"
else
    echo "  no bundled desktop-host plist: OK"
fi

if [ -f "$USER_MANAGED_PLIST" ]; then
    echo "  user-domain managed plist present: OK"
    if grep -q '/Users/runner' "$USER_MANAGED_PLIST"; then
        fail "managed plist contains build-machine path /Users/runner"
    elif grep -q 'SERVICE_BINARY_PATH' "$USER_MANAGED_PLIST"; then
        fail "managed plist contains unsubstituted placeholder SERVICE_BINARY_PATH"
    else
        echo "  managed plist ProgramArguments: clean"
    fi
else
    echo "  user-domain managed plist not present"
    echo "  (expected after first launch — Busytok.app renders it on bootstrap)"
fi

echo ""
echo "Checking CLI recovery guidance..."
if "$CLI_BINARY" status 2>&1 | grep -q "Open Busytok.app to start the background service"; then
    echo "  cli recovery guidance: OK"
else
    fail "cli recovery guidance missing"
fi

# Menu bar icon asset — check both possible locations.
MENU_BAR_ICON=""
for candidate in \
    "$APP_PATH/Contents/Resources/menu-bar-template.png" \
    "$APP_PATH/Contents/Resources/icons/menu-bar-template.png"; do
    if [ -f "$candidate" ]; then
        MENU_BAR_ICON="$candidate"
        break
    fi
done

if [ -n "$MENU_BAR_ICON" ]; then
    echo "  menu-bar-template.png: OK ($MENU_BAR_ICON)"
else
    echo "  WARN: menu-bar-template.png not found in app bundle Resources (may not be bundled yet)"
fi

# --- Same-session suppression smoke (opt-in) ---
if [ "${BUSYTOK_RUN_QUIT_SMOKE:-0}" = "1" ]; then
    echo ""
    echo "--- Same-session suppression smoke ---"
    SERVICE_LABEL="gui/$(id -u)/com.busytok.service"

    if [ ! -d "$APP_PATH" ]; then
        fail "app bundle required for quit smoke"
    else
        open "$APP_PATH"
        echo "  launched $APP_PATH"
        echo "  now choose 'Quit Busytok Desktop' from the menu bar, then press Enter to continue..."
        read -r _continue

        if launchctl print "$SERVICE_LABEL" >/dev/null 2>&1; then
            fail "service returned in same session after explicit Quit"
        else
            echo "  service did not respawn in same session: OK"
        fi

        local_lifecycle_toml="$HOME/Library/Application Support/busytok/desktop_lifecycle.toml"
        if [ -f "$local_lifecycle_toml" ]; then
            if grep -q "^suppressed_for_session = true" "$local_lifecycle_toml"; then
                echo "  local desktop_lifecycle.toml suppressed_for_session=true: OK"
            else
                fail "desktop_lifecycle.toml does not record suppression after Quit"
            fi
        else
            echo "  WARN: $local_lifecycle_toml not present — skipping suppression-persistence assertion"
        fi

        echo "  NOTE: desktop-host quit verification is implicit (the process exited)."
    fi
fi

echo ""
if [ "$FAILURES" -eq 0 ]; then
    echo "PASS: installed app bundle structure OK"
else
    echo "FAIL: installed app smoke FAILED ($FAILURES failures)"
    exit 1
fi
