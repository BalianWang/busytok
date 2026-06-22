#!/usr/bin/env bash
# Generate the Tauri 2 updater manifest (latest.json) for a Busytok release.
#
# Usage:
#   generate_updater_manifest.sh <version> <sig-file> <download-url> [<notes>]
#
# Outputs JSON to stdout. The pub_date field is __PUB_DATE_PLACEHOLDER__;
# CI replaces it with the actual upload timestamp just before publishing.
# This keeps the script hermetic (no `date` call) and unit-testable.
#
# For universal binary, both darwin-aarch64 and darwin-x86_64 point to
# the same .app.tar.gz payload.
set -euo pipefail

if [ "$#" -lt 3 ]; then
    echo "Usage: $0 <version> <sig-file> <download-url> [<notes>]" >&2
    exit 1
fi

VERSION="$1"
SIG_FILE="$2"
URL="$3"
NOTES="${4:-Busytok $VERSION}"

if [ ! -f "$SIG_FILE" ]; then
    echo "sig-file not found: $SIG_FILE" >&2
    exit 1
fi

SIG="$(cat "$SIG_FILE")"

# Escape the notes for JSON (basic: backslash + double-quote).
NOTES_ESCAPED="${NOTES//\\/\\\\}"
NOTES_ESCAPED="${NOTES_ESCAPED//\"/\\\"}"

cat <<EOF
{
  "version": "$VERSION",
  "notes": "$NOTES_ESCAPED",
  "pub_date": "__PUB_DATE_PLACEHOLDER__",
  "platforms": {
    "darwin-aarch64": {
      "signature": "$SIG",
      "url": "$URL"
    },
    "darwin-x86_64": {
      "signature": "$SIG",
      "url": "$URL"
    }
  }
}
EOF
