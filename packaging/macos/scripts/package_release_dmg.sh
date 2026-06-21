#!/usr/bin/env bash
# Convenience wrapper around package_dmg.sh for release builds.
# Sets DEVELOPER_ID_APPLICATION from environment and delegates to package_dmg.sh.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
exec "$SCRIPT_DIR/package_dmg.sh" "$@"
