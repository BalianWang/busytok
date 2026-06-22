#!/usr/bin/env bash
set -euo pipefail

echo "=== Busytok Acceptance Gate ==="

echo "--- cargo fmt ---"
cargo fmt --all --check

echo "--- cargo clippy ---"
cargo clippy --workspace --all-targets -- -D warnings

echo "--- cargo test ---"
cargo test --workspace

echo "--- pnpm install ---"
pnpm install --frozen-lockfile

echo "--- pnpm typecheck ---"
pnpm typecheck

echo "--- pnpm test ---"
pnpm -r test

echo "--- Release workflow smoke ---"
bash tests/workflows/release_workflow_test.sh

echo "--- CLI smoke ---"
if [ ! -f target/release/busytok ]; then
    echo "Building busytok..."
    cargo build --release -p busytok
fi
tests/packaging/macos/cli_smoke.sh

APP_PATH="target/release/bundle/macos/Busytok.app"
if [ "${ALLOW_UNSIGNED_DEV_BUILD:-}" = "1" ]; then
    case "$(uname -s)" in
        Darwin)
            echo "--- Building unsigned app bundle for verification ---"
            ALLOW_UNSIGNED_DEV_BUILD=1 packaging/macos/scripts/package_app.sh
            ;;
    esac
fi

echo "--- Package verification ---"
if [ "${ALLOW_UNSIGNED_DEV_BUILD:-}" = "1" ]; then
    packaging/macos/scripts/verify_package.sh
else
    echo "Skipped (set ALLOW_UNSIGNED_DEV_BUILD=1 for dev verification)"
fi

echo "--- Installed app smoke ---"
if [ -d "$APP_PATH" ] && [ -x "$APP_PATH/Contents/MacOS/busytok-gui" ]; then
    BUSYTOK_APP_PATH="$APP_PATH" tests/packaging/macos/installed_app_smoke.sh
else
    case "$(uname -s)" in
        Darwin)
            echo "  .app not available after macOS build attempt"
            exit 1
            ;;
        *)
            echo "  installed-app smoke not available on non-macOS host"
            ;;
    esac
fi

echo "--- LaunchAgent smoke ---"
if [ -f "$HOME/Library/LaunchAgents/com.busytok.service.plist" ]; then
    tests/packaging/macos/launch_agent_smoke.sh
else
    echo "Skipped (LaunchAgent not installed — run installed-app smoke first)"
fi

echo "=== Acceptance gate PASSED ==="
