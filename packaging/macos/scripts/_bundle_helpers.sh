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

# Copy the (static, reference-only) service LaunchAgent plist into
# Busytok.app/Contents/Library/LaunchAgents/com.busytok.service.plist.
#
# IMPORTANT: production launchd lifecycle does NOT consume this file.
# The GUI renders the real, install-location-correct plist into
# ~/Library/LaunchAgents/ at runtime — see
# apps/gui/src-tauri/src/service_lifecycle/managed_launch_agent.rs.
#
# This bundle copy is shipped for documentation / future SMAppService
# migration only. It is copied verbatim (NO substitution): an earlier
# build substituted SERVICE_BINARY_PATH / BUSYTOK_LOGS_DIR /
# BUSYTOK_APP_SUPPORT_DIR here, which baked /Users/runner/... absolute
# paths into the shipped plist and broke launchd registration on
# end-user machines. That substitution has been removed.
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
    # Verbatim copy — no path substitution.
    cp "$template" "$plist_path"

    echo "  bundled service plist (reference only) -> $plist_path"
}
