#!/usr/bin/env bash
# Shared helpers: copies busytok-service, busytok, and the bundle-native
# service LaunchAgent plist into Busytok.app.
#
# Source this from package scripts. Expects PROJECT_ROOT to be set.
#
# Usage:
#   bundle_helpers_into_app APP_BUNDLE_PATH
#   bundle_service_plist_into_app APP_BUNDLE_PATH

_absolute_app_bundle_path() {
    local app_bundle="$1"
    local app_parent
    local app_name

    app_parent="$(cd "$(dirname "$app_bundle")" && pwd -P)"
    app_name="$(basename "$app_bundle")"
    printf '%s/%s\n' "$app_parent" "$app_name"
}

# Copy helper binaries into Busytok.app/Contents/MacOS/.
bundle_helpers_into_app() {
    local app_bundle
    app_bundle="$(_absolute_app_bundle_path "$1")"
    local macos_dir="$app_bundle/Contents/MacOS"

    echo "Bundling helpers into $app_bundle..."
    for helper in busytok-service busytok; do
        if [ -f "target/release/$helper" ]; then
            cp "target/release/$helper" "$macos_dir/$helper"
            chmod +x "$macos_dir/$helper"
            echo "  bundled $helper -> $macos_dir/$helper"
        else
            echo "  $helper not found at target/release/$helper"
            return 1
        fi
    done
    echo "Helper bundling complete."
}

# Render the bundle-native service LaunchAgent plist into
# Busytok.app/Contents/Library/LaunchAgents/com.busytok.service.plist.
#
# launchctl bootstraps the plist from this bundle location.
# The app does NOT write a copy to ~/Library/LaunchAgents/.
#
# Substitution tokens (see com.busytok.service.plist.template):
#   SERVICE_BINARY_PATH      -> absolute path to the bundled busytok-service
#   BUSYTOK_LOGS_DIR         -> per-user log directory
#   BUSYTOK_APP_SUPPORT_DIR  -> per-user app data directory
bundle_service_plist_into_app() {
    local app_bundle
    app_bundle="$(_absolute_app_bundle_path "$1")"

    local project_root="${PROJECT_ROOT:?PROJECT_ROOT must be set}"
    local template="$project_root/packaging/macos/launchd/com.busytok.service.plist.template"
    local launchagents_dir="$app_bundle/Contents/Library/LaunchAgents"
    local plist_name="com.busytok.service.plist"
    local plist_path="$launchagents_dir/$plist_name"

    if [ ! -f "$template" ]; then
        echo "service plist template not found at $template"
        return 1
    fi

    mkdir -p "$launchagents_dir"

    local service_binary="$app_bundle/Contents/MacOS/busytok-service"
    if [ ! -f "$service_binary" ]; then
        echo "bundled busytok-service not found at $service_binary"
        return 1
    fi

    # Per-user paths. launchd evaluates these in the user's Aqua session,
    # so user-relative paths are correct here.
    local logs_dir="${HOME}/Library/Logs/Busytok"
    local app_support_dir="${HOME}/Library/Application Support/Busytok"

    sed -e "s|SERVICE_BINARY_PATH|$service_binary|g" \
        -e "s|BUSYTOK_LOGS_DIR|$logs_dir|g" \
        -e "s|BUSYTOK_APP_SUPPORT_DIR|$app_support_dir|g" \
        "$template" > "$plist_path"

    echo "  bundled service plist -> $plist_path"
}
